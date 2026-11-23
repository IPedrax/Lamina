//! A printer on a USB cable: the G-code streamed to it line by line while it prints, from Marlin's public G-code documentation and
//! RepRap's checksum rule. After `M110 N0` sets the printer's count to zero, each line goes out as `N<number> <line>*<checksum>`, the
//! checksum being the XOR of every byte before the `*`. The printer answers `ok` for each line it takes; `Resend: N` (or `rs N`)
//! when a line came in garbled, after which everything from line N goes again; `busy:` while a long move or a heat-up runs.
//!
//! At most [`Pace::window`] lines wait for their `ok`, so the printer's command queue never overflows. A printer that says nothing
//! for [`Pace::quiet`] while lines wait is taken to have lost an `ok`, once; quiet for as long again, it is given up on. A printer that
//! reports an error, restarts, or keeps asking for the same line stops the stream, and so does the window's Cancel
//! (`lamina_core::cancel`); each time the heaters are turned off.
//!
//! The protocol is written over any `Read + Write`, so it is tested against a printer in memory; [`send`] opens the real port with the
//! `serial2` crate (BSD-2-Clause or Apache-2.0). Opening a port resets many boards, so the port is opened only to print, and
//! [`status`] says what the stream in this program is doing rather than asking the printer.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use lamina_core::cancel;

use crate::{Sent, Status};

/// The speed when none is given: what most Marlin printers use (some take 250000).
pub const BAUD: u32 = 115_200;

/// The longest line Marlin reads, with its number and checksum (its `MAX_CMD_SIZE`, less the end).
const LONGEST: usize = 95;

/// Turns the heaters off when a stream stops early. Plain lines, which a printer takes without a number.
const HEATERS_OFF: &[u8] = b"M104 S0\nM140 S0\n";

/// How a stream is paced.
#[derive(Clone, Copy, Debug)]
pub struct Pace {
    /// Lines in flight at most: Marlin's command queue holds four.
    pub window: usize,
    /// How long a printer may say nothing while lines wait. Marlin says `busy:` every two seconds through a long command, and
    /// reports the temperatures while it heats.
    pub quiet: Duration,
    /// How long to wait for `start` before the first line: opening the port restarts many boards, which then miss what comes first.
    pub boot: Duration,
}

pub const PACE: Pace = Pace { window: 4, quiet: Duration::from_secs(60), boot: Duration::from_secs(2) };

/// The G-code's lines as they go out: comments and blank lines dropped, and each line cut at a `*`, which would end it for the printer.
fn lines(gcode: &[u8]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for (i, raw) in String::from_utf8_lossy(gcode).lines().enumerate() {
        let line = raw.split([';', '*']).next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if numbered(out.len() as u64 + 1, line).len() > LONGEST + 1 {
            return Err(format!("line {} of the G-code is longer than the {LONGEST} characters a printer reads", i + 1));
        }
        out.push(line.to_string());
    }
    Ok(out)
}

/// `line` as it goes out: `N<n> <line>*<checksum>` and a line feed.
fn numbered(n: u64, line: &str) -> String {
    let body = format!("N{n} {line}");
    let sum = body.bytes().fold(0u8, |a, b| a ^ b);
    format!("{body}*{sum}\n")
}

/// The line a `Resend: N` or `rs N` asks for.
fn resend(reply: &str) -> Option<u64> {
    let lower = reply.to_ascii_lowercase();
    let rest = lower.strip_prefix("resend:").or_else(|| lower.strip_prefix("rs"))?;
    let digits: String = rest.trim_start_matches([' ', ':', 'n', 'N']).chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Streams `gcode` to the printer at the other end of `port`; `progress` hears how many lines of how many it has taken. Whatever
/// stops it early turns the heaters off.
pub fn stream(port: &mut (impl Read + Write), gcode: &[u8], pace: Pace, progress: impl FnMut(u64, u64)) -> Result<(), String> {
    let lines = lines(gcode)?;
    let out = run(port, &lines, pace, progress);
    if out.is_err() {
        let _ = port.write_all(HEATERS_OFF).and_then(|()| port.flush());
    }
    out
}

fn run(port: &mut (impl Read + Write), lines: &[String], pace: Pace, mut progress: impl FnMut(u64, u64)) -> Result<(), String> {
    let total = lines.len() as u64;
    let window = pace.window.max(1);
    let opened = Instant::now();
    // The next line to send (0 is `M110 N0`), the lines waiting for their `ok`, and whether the printer has taken `M110`.
    let (mut next, mut in_flight, mut started, mut booted) = (0u64, 0usize, false, false);
    // After a resend from line N: how many more asks for N are echoes of lines sent before it, and how many `ok`s go with them.
    let (mut echoes, mut skip_ok) = (0u64, 0usize);
    // The line last asked for again, and how many times in a row.
    let (mut asked, mut again) = (u64::MAX, 0);
    let (mut heard, mut poked, mut taken) = (Instant::now(), false, 0u64);
    let mut buf = Vec::new();
    loop {
        cancel::check()?;
        let ready = started || (next == 0 && (booted || opened.elapsed() >= pace.boot));
        while ready && in_flight < window && next <= total && (started || next == 0) {
            let text = if next == 0 { "M110 N0" } else { lines.get(next as usize - 1).map_or("", String::as_str) };
            port.write_all(numbered(next, text).as_bytes()).map_err(|e| format!("the printer's port: {e}"))?;
            if in_flight == 0 {
                heard = Instant::now(); // the wait for an answer starts now
            }
            (next, in_flight) = (next + 1, in_flight + 1);
        }
        port.flush().map_err(|e| format!("the printer's port: {e}"))?;
        if started && next > total && in_flight == 0 {
            return Ok(());
        }
        let mut chunk = [0u8; 512];
        match port.read(&mut chunk) {
            Ok(0) => return Err("the printer's port closed: is the cable still in?".into()),
            Ok(n) => buf.extend_from_slice(&chunk[..n.min(chunk.len())]),
            Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
            Err(e) => return Err(format!("the printer's port: {e}")),
        }
        while let Some(end) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=end).collect();
            let reply = String::from_utf8_lossy(&line);
            let reply = reply.trim();
            let lower = reply.to_ascii_lowercase();
            (heard, poked) = (Instant::now(), false);
            if lower.starts_with("ok") {
                if skip_ok > 0 {
                    skip_ok -= 1;
                } else if in_flight > 0 {
                    // (An `ok` with nothing in flight is a stray, from before the port was opened.)
                    (in_flight, started) = (in_flight - 1, true);
                    taken = taken.max(next.saturating_sub(1 + in_flight as u64).min(total));
                    progress(taken, total);
                }
            } else if let Some(n) = resend(reply) {
                if n >= next {
                    return Err(format!("the printer asked again for line {n}, which was never sent: is something else talking to it?"));
                }
                if echoes > 0 && n == asked {
                    // A line sent before the resend, refused in its turn: its `ok` is not for a line in flight.
                    (echoes, skip_ok) = (echoes - 1, skip_ok + 1);
                    continue;
                }
                again = if n == asked { again + 1 } else { 1 };
                if again > 10 {
                    return Err(format!("the printer keeps asking for line {n} again: check the cable and the baud rate"));
                }
                // Lines after `n` come back refused, each with its own ask; only the `ok` after this one is for a line in flight.
                (asked, echoes, skip_ok, in_flight, next) = (n, next - 1 - n, 0, 1, n);
            } else if lower == "start" {
                if started {
                    return Err("the printer restarted part way (it said start)".into());
                }
                // It restarted as the port opened and missed `M110`: from the top.
                (next, in_flight, echoes, skip_ok, booted) = (0, 0, 0, 0, true);
            } else if lower.starts_with("error:") || lower.starts_with("!!") {
                let garbled = ["checksum", "line number", "last line", "expected line"].iter().any(|w| lower.contains(w));
                if !garbled {
                    return Err(format!(
                        "the printer stopped: {}",
                        reply.chars().filter(|c| !c.is_control()).take(200).collect::<String>()
                    ));
                }
            }
        }
        if buf.len() > 4096 {
            buf.clear(); // no printer sends a line this long: noise, or the wrong baud rate
        }
        if in_flight > 0 && heard.elapsed() >= pace.quiet {
            if poked {
                return Err(format!("the printer said nothing for {} s", 2 * pace.quiet.as_secs()));
            }
            // An `ok` may have come garbled: count the oldest line as taken, once, and go on (or send `M110` again).
            (in_flight, poked, heard) = (in_flight - 1, true, Instant::now());
            if !started {
                (next, in_flight) = (0, 0);
            }
        }
    }
}

/// What the streams in this program are doing, by port.
static PRINTING: Mutex<BTreeMap<String, Status>> = Mutex::new(BTreeMap::new());

fn tell(port: &str, status: Option<Status>) {
    if let Ok(mut all) = PRINTING.lock() {
        match status {
            Some(s) => all.insert(port.to_string(), s),
            None => all.remove(port),
        };
    }
}

/// What the stream to `port` is doing, if Lamina is printing on it.
pub fn status(port: &str) -> Status {
    let now = PRINTING.lock().ok().and_then(|all| all.get(port.trim()).cloned());
    now.unwrap_or_else(|| Status { state: "not printing from Lamina".into(), ..Status::default() })
}

/// Whether `port` names a serial port: `/dev/...` on Linux and macOS, `COM3` or `\\.\COM10` on Windows.
fn port_ok(port: &str) -> bool {
    let named =
        if cfg!(windows) { port.to_ascii_uppercase().starts_with("COM") || port.starts_with(r"\\.\") } else { port.starts_with("/dev/") };
    named && port.len() <= 200 && !port.contains("..") && !port.chars().any(char::is_control)
}

/// Prints `gcode` on the printer at serial `port`, at `baud` (default [`BAUD`]); returns when its last line is taken.
pub fn send(port: &str, baud: Option<u32>, name: &str, gcode: &[u8], print: bool) -> Result<Sent, String> {
    if !print {
        return Err(
            "a printer on a USB cable prints as Lamina streams the G-code to it, so it can only Send and print (print: true)".into()
        );
    }
    let port = port.trim();
    if !port_ok(port) {
        return Err(format!("`{port}` is not a serial port: give one such as /dev/ttyUSB0, /dev/ttyACM0 or COM3"));
    }
    let baud = baud.unwrap_or(BAUD);
    if !(1200..=4_000_000).contains(&baud) {
        return Err(format!("{baud} is not a baud rate printers use: most take 115200 or 250000"));
    }
    let mut link = serial2::SerialPort::open(port, baud).map_err(|e| format!("the port {port}: {e}"))?;
    link.set_read_timeout(Duration::from_millis(100)).map_err(|e| format!("the port {port}: {e}"))?;
    let watch = |done: u64, total: u64| {
        let progress = Some(if total == 0 { 1.0 } else { done as f64 / total as f64 });
        tell(port, Some(Status { state: "printing".into(), file: Some(name.to_string()), progress, ..Status::default() }));
    };
    watch(0, 1);
    let out = stream(&mut link, gcode, PACE, watch);
    tell(port, None);
    out.map(|()| Sent { name: name.to_string(), printing: true })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A Marlin printer in memory: it checks each line's number and checksum, answers as Marlin does, and keeps what it took.
    #[derive(Default)]
    struct Marlin {
        input: Vec<u8>,
        out: VecDeque<u8>,
        last: u64,
        /// The G-code lines it took, in order, and the plain lines (no number) it got.
        took: Vec<String>,
        plain: Vec<String>,
        /// Line numbers that arrive garbled the first time.
        garble: Vec<u64>,
        /// Say `rs N` rather than `Resend: N`.
        rs: bool,
        /// Lose the first `M110` and say `start`, as a board does that restarts when the port opens.
        restart: bool,
        /// Stop answering after this many lines.
        mute_after: Option<usize>,
        /// Say this instead of `ok` to a line holding the text.
        fail: Option<(&'static str, &'static str)>,
        /// Set when this line is taken (the user pressing Cancel).
        cancel_at: Option<(u64, Arc<AtomicBool>)>,
        lines_in: usize,
        /// Whether each `ok` not yet read is for a command it took (rather than one it refused).
        oks: VecDeque<bool>,
        /// Commands it took whose `ok` the host has not read: what its command queue holds. The most there ever were.
        queued: usize,
        most_in_flight: usize,
    }

    impl Marlin {
        fn say(&mut self, text: &str) {
            self.out.extend(text.as_bytes());
        }

        /// `ok` for a command taken into the queue, or for one refused.
        fn ok(&mut self, taken: bool, text: &str) {
            if taken {
                self.queued += 1;
                self.most_in_flight = self.most_in_flight.max(self.queued);
            }
            self.oks.push_back(taken);
            self.say(text);
        }

        fn line(&mut self, line: &str) {
            self.lines_in += 1;
            if self.mute_after.is_some_and(|m| self.lines_in > m) {
                return;
            }
            let Some(rest) = line.strip_prefix('N') else {
                self.plain.push(line.to_string());
                return self.ok(true, "ok\n");
            };
            let (body, sum) = line.rsplit_once('*').unwrap();
            let n: u64 = rest.split(' ').next().unwrap().parse().unwrap();
            let command = body.split_once(' ').unwrap().1.to_string();
            let mut good = body.bytes().fold(0u8, |a, b| a ^ b).to_string() == sum;
            if let Some(i) = self.garble.iter().position(|&g| g == n) {
                self.garble.remove(i);
                good = false;
            }
            if self.restart && command.starts_with("M110") {
                self.restart = false;
                return self.say("start\necho:Marlin 2.1\n");
            }
            let resend = |m: &mut Marlin, why: &str| {
                let ask = if m.rs { format!("rs {}", m.last + 1) } else { format!("Resend: {}", m.last + 1) };
                m.say(&format!("Error:{why}, Last Line: {}\n{ask}\n", m.last));
                m.ok(false, "ok\n");
            };
            if !good {
                return resend(self, "checksum mismatch");
            }
            if command.starts_with("M110") {
                self.last = n;
                return self.ok(true, "ok\n");
            }
            if n != self.last + 1 {
                return resend(self, "Line Number is not Last Line Number+1");
            }
            self.last = n;
            if let Some((_, flag)) = self.cancel_at.as_ref().filter(|(at, _)| *at == n) {
                flag.store(true, Ordering::Relaxed);
            }
            if let Some((_, said)) = self.fail.filter(|(text, _)| command.contains(text)) {
                return self.say(&format!("{said}\n"));
            }
            if command == "G28" {
                self.say("echo:busy: processing\necho:busy: processing\n");
            }
            self.took.push(command);
            self.ok(true, "ok T:210.0 /210.0 B:60.0 /60.0\n");
        }
    }

    impl Write for Marlin {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.input.extend_from_slice(data);
            while let Some(end) = self.input.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.input.drain(..=end).collect();
                self.line(String::from_utf8(line).unwrap().trim());
            }
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// One answer line at a time, so the host reads them as they would come.
    impl Read for Marlin {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let Some(end) = self.out.iter().position(|&b| b == b'\n') else {
                return Err(ErrorKind::TimedOut.into());
            };
            let line: Vec<u8> = self.out.drain(..=end).collect();
            if line.starts_with(b"ok") && self.oks.pop_front().unwrap() {
                self.queued -= 1;
            }
            buf[..line.len()].copy_from_slice(&line);
            Ok(line.len())
        }
    }

    const GCODE: &[u8] = b"; a comment\nG28 ; home\nG1 X10 Y10 F3000\n\nG1 X20 E1.5\nM117 2*2\nG1 X30\nG1 X40\nG1 X50\nM84\n";
    const TOOK: [&str; 8] = ["G28", "G1 X10 Y10 F3000", "G1 X20 E1.5", "M117 2", "G1 X30", "G1 X40", "G1 X50", "M84"];
    const FAST: Pace = Pace { window: 4, quiet: Duration::from_millis(30), boot: Duration::ZERO };

    #[test]
    fn a_line_goes_out_numbered_with_its_checksum() {
        assert_eq!(numbered(123, "M105"), "N123 M105*39\n");
        assert_eq!(resend("Resend: 17"), Some(17));
        assert_eq!(resend("rs N17"), Some(17));
        assert_eq!((resend("rs"), resend("Resend: lots")), (None, None));
        let long = format!("M117 {}", "x".repeat(100));
        assert!(lines(long.as_bytes()).unwrap_err().contains("line 1"));
    }

    #[test]
    fn every_line_is_taken_once_in_order_with_a_few_in_flight() {
        for window in [1, 4] {
            let mut m = Marlin::default();
            let mut heard = Vec::new();
            stream(&mut m, GCODE, Pace { window, ..FAST }, |done, total| heard.push((done, total))).unwrap();
            assert_eq!(m.took, TOOK);
            assert!(m.most_in_flight <= window, "{} in flight", m.most_in_flight);
            assert_eq!(heard.last(), Some(&(8, 8)));
            assert!(m.plain.is_empty(), "the heaters stay as the G-code leaves them");
        }
    }

    #[test]
    fn a_garbled_line_is_sent_again_and_the_echoes_are_not_counted() {
        for (window, rs) in [(1, false), (4, false), (4, true)] {
            let mut m = Marlin { garble: vec![3, 6], rs, ..Marlin::default() };
            stream(&mut m, GCODE, Pace { window, ..FAST }, |_, _| {}).unwrap();
            assert_eq!(m.took, TOOK, "window {window}, rs {rs}");
            assert!(m.most_in_flight <= window, "{} in flight", m.most_in_flight);
        }
    }

    #[test]
    fn a_board_that_restarts_as_the_port_opens_is_started_again() {
        let mut m = Marlin { restart: true, ..Marlin::default() };
        stream(&mut m, GCODE, FAST, |_, _| {}).unwrap();
        assert_eq!(m.took, TOOK);
    }

    #[test]
    fn an_error_a_silence_or_a_cancel_stops_it_and_turns_the_heaters_off() {
        let mut m = Marlin { fail: Some(("X30", "Error:Printer halted. kill() called!")), ..Marlin::default() };
        let e = stream(&mut m, GCODE, FAST, |_, _| {}).unwrap_err();
        assert!(e.contains("halted"), "{e}");
        assert_eq!(m.plain, ["M104 S0", "M140 S0"]);
        let mut m = Marlin { mute_after: Some(3), ..Marlin::default() };
        assert!(stream(&mut m, GCODE, FAST, |_, _| {}).unwrap_err().contains("said nothing"));
        let flag = Arc::new(AtomicBool::new(false));
        let mut m = Marlin { cancel_at: Some((2, Arc::clone(&flag))), ..Marlin::default() };
        let e = cancel::watch(flag, || stream(&mut m, GCODE, FAST, |_, _| {})).unwrap_err();
        assert_eq!(e, cancel::CANCELLED);
        assert!(m.took.len() < TOOK.len(), "stopped part way: {:?}", m.took);
        assert_eq!(m.plain, ["M104 S0", "M140 S0"]);
        // Asked for the same line over and over (a bad cable, the wrong speed): given up on.
        let mut m = Marlin { garble: vec![2; 20], ..Marlin::default() };
        assert!(stream(&mut m, GCODE, FAST, |_, _| {}).unwrap_err().contains("keeps asking"));
    }

    #[test]
    fn the_port_is_checked_and_upload_only_is_refused() {
        assert!(send("/dev/ttyUSB0", None, "a.gcode", GCODE, false).unwrap_err().contains("Send and print"));
        for bad in ["/etc/passwd", "/dev/../etc/passwd", "", "printer.local"] {
            if !cfg!(windows) {
                assert!(send(bad, None, "a.gcode", GCODE, true).unwrap_err().contains("not a serial port"), "{bad}");
            }
        }
        assert!(send(if cfg!(windows) { "COM250" } else { "/dev/null" }, Some(7), "a.gcode", GCODE, true).unwrap_err().contains("baud"));
        assert_eq!(status("/dev/ttyUSB9").state, "not printing from Lamina");
        tell("/dev/ttyUSB9", Some(Status { state: "printing".into(), progress: Some(0.5), ..Status::default() }));
        assert_eq!(status("/dev/ttyUSB9").line(), "printing, 50 %");
        tell("/dev/ttyUSB9", None);
    }

    #[test]
    fn a_hostile_printer_never_panics_the_stream() {
        let mut g = lamina_core::fuzz::Rng::new(0x5e71a1);
        let good: &[&[u8]] =
            &[b"ok\n", b"Resend: 2\nok\n", b"rs 1\n", b"start\n", b"echo:busy: processing\n", b"Error:checksum mismatch\n"];
        for _ in 0..300 {
            /// Answers every line with something random, then falls silent.
            struct Noise(lamina_core::fuzz::Rng, Vec<Vec<u8>>, VecDeque<u8>, usize);
            impl Write for Noise {
                fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                    for _ in data.iter().filter(|&&b| b == b'\n') {
                        self.3 += 1;
                        if self.3 < 60 {
                            let good: Vec<&[u8]> = self.1.iter().map(Vec::as_slice).collect();
                            let said = self.0.input(&good);
                            self.2.extend(said);
                        }
                    }
                    Ok(data.len())
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            impl Read for Noise {
                fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                    let n = self.2.len().min(buf.len()).min(self.0.below(64) + 1);
                    for b in buf.iter_mut().take(n) {
                        *b = self.2.pop_front().unwrap();
                    }
                    if n == 0 { Err(ErrorKind::TimedOut.into()) } else { Ok(n) }
                }
            }
            let seed = g.next_u64();
            let mut noise = Noise(lamina_core::fuzz::Rng::new(seed), good.iter().map(|b| b.to_vec()).collect(), VecDeque::new(), 0);
            let fast = Pace { window: 1 + g.below(5), quiet: Duration::from_millis(2), boot: Duration::ZERO };
            let _ = stream(&mut noise, GCODE, fast, |done, total| assert!(done <= total));
        }
    }
}
