//! Booleans in a child process. csgrs recurses, and a recursion without end would overflow the stack and abort the whole app rather
//! than return an error. So an app that can (the desktop, the CLI) runs each boolean in a copy of itself started with [`CHILD_ARG`]:
//! the two meshes go in on its standard input, the result comes back on its standard output, and a child that aborts, hangs past
//! [`CHILD_TIME`] or says something that does not read is an error the app carries on from. The pattern, and `run_child`, are ported
//! from BIMCraft's `crates/dwg` (same author, MIT OR Apache-2.0), which reads DWG files the same way.
//!
//! What crosses the pipe: `LMB1`, an operation byte, then each mesh as its vertex count and triangle count (little-endian `u32`) and
//! the arrays (`f32` vertices, `u32` triangles), which is lossless. The answer is `LMB1` and one mesh; an error is the child's
//! message on its error stream and a failing exit.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use lamina_core::Mesh;
use lamina_core::mesh::MAX_TRIANGLES;

use crate::{MAX_INPUT_TRIANGLES, Op, boolean};

/// The argument that starts an app as the child that runs one boolean.
pub const CHILD_ARG: &str = "--boolean-child";
/// The longest a child may take over one boolean.
pub const CHILD_TIME: Duration = Duration::from_secs(120);
const MAGIC: &[u8; 4] = b"LMB1";
/// The most a child says on its error stream, and the most it may answer, bytes.
const MAX_SAID: u64 = 64 << 10;
const MAX_ANSWER: u64 = 512 << 20;

/// The program the booleans of this process run in, when the app has said so.
static CHILD: OnceLock<PathBuf> = OnceLock::new();

/// Makes every boolean of this process run in `exe` started as a child ([`child_main`]). An app calls it once at start with its own
/// program; without it (in tests, say) booleans run in the process.
pub fn run_in_child(exe: PathBuf) {
    let _ = CHILD.set(exe);
}

/// `a` combined with `b`: in a child when the app has asked for one, else here.
pub fn isolated(op: Op, a: &Mesh, b: &Mesh) -> Result<Mesh, String> {
    match CHILD.get() {
        Some(exe) => in_child(exe, op, a, b, CHILD_TIME),
        None => boolean(op, a, b),
    }
}

fn put(out: &mut Vec<u8>, m: &Mesh) {
    out.extend_from_slice(&(m.vertices.len() as u32).to_le_bytes());
    out.extend_from_slice(&(m.triangles.len() as u32).to_le_bytes());
    out.extend(m.vertices.iter().flatten().flat_map(|c| c.to_le_bytes()));
    out.extend(m.triangles.iter().flatten().flat_map(|i| i.to_le_bytes()));
}

/// A mesh from `bytes` at `*at`, moving `at` past it.
fn take(bytes: &[u8], at: &mut usize) -> Result<Mesh, String> {
    let word = |at: usize| -> Result<u32, String> {
        bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| "the mesh is cut short".to_string())
    };
    let (nv, nt) = (word(*at)? as usize, word(*at + 4)? as usize);
    if nt > MAX_TRIANGLES || nv > 3 * MAX_TRIANGLES {
        return Err("the mesh is larger than a mesh may be".into());
    }
    *at += 8;
    let mut vertices = Vec::with_capacity(nv);
    for _ in 0..nv {
        vertices.push([word(*at)?, word(*at + 4)?, word(*at + 8)?].map(f32::from_bits));
        *at += 12;
    }
    let mut triangles = Vec::with_capacity(nt);
    for _ in 0..nt {
        triangles.push([word(*at)?, word(*at + 4)?, word(*at + 8)?]);
        *at += 12;
    }
    Mesh::new(vertices, triangles)
}

fn op_byte(op: Op) -> u8 {
    match op {
        Op::Union => 0,
        Op::Difference => 1,
        Op::Intersect => 2,
    }
}

/// What an app does when it is started as the child that runs a boolean (`args`, its arguments after its own name, are just
/// [`CHILD_ARG`]): reads the meshes on its standard input, writes the result on its standard output, or says why not on its error
/// stream. `None` when `args` are not a child's, and the app goes on as itself.
pub fn child_main(args: &[String]) -> Option<ExitCode> {
    if args.len() != 1 || args[0] != CHILD_ARG {
        return None;
    }
    let mut input = Vec::new();
    let mut err = std::io::stderr();
    // Two meshes of at most the triangles one boolean takes, with room to spare.
    let cap = (MAX_INPUT_TRIANGLES as u64 + 64) * 80;
    if let Err(e) = std::io::stdin().lock().take(cap).read_to_end(&mut input) {
        let _ = writeln!(err, "could not read the meshes: {e}");
        return Some(ExitCode::FAILURE);
    }
    let result = (|| {
        if input.get(..4) != Some(MAGIC.as_slice()) {
            return Err("the child was not given meshes".to_string());
        }
        let op = match input.get(4) {
            Some(0) => Op::Union,
            Some(1) => Op::Difference,
            Some(2) => Op::Intersect,
            _ => return Err("the child was given no operation".to_string()),
        };
        let mut at = 5;
        let (a, b) = (take(&input, &mut at)?, take(&input, &mut at)?);
        boolean(op, &a, &b)
    })();
    match result {
        Ok(m) => {
            let mut out = MAGIC.to_vec();
            put(&mut out, &m);
            let mut stdout = std::io::stdout().lock();
            match stdout.write_all(&out).and_then(|()| stdout.flush()) {
                Ok(()) => Some(ExitCode::SUCCESS),
                Err(e) => {
                    let _ = writeln!(err, "{e}");
                    Some(ExitCode::FAILURE)
                }
            }
        }
        Err(e) => {
            let _ = writeln!(err, "{}", e.replace(['\r', '\n'], " "));
            Some(ExitCode::FAILURE)
        }
    }
}

/// `a` combined with `b` by `exe` started as a child ([`child_main`]), within `time`. A child that aborts, runs past `time` or answers
/// what does not read is stopped, and the reply says so.
pub fn in_child(exe: &Path, op: Op, a: &Mesh, b: &Mesh, time: Duration) -> Result<Mesh, String> {
    let mut input = MAGIC.to_vec();
    input.push(op_byte(op));
    put(&mut input, a);
    put(&mut input, b);
    let ran =
        run_child(exe, &[CHILD_ARG.as_ref()], Some(input), MAX_ANSWER, time).map_err(|e| format!("the boolean could not start: {e}"))?;
    let said = String::from_utf8_lossy(&ran.said).trim().to_string();
    match ran.status {
        None => Err(format!("the boolean took longer than {} s and was stopped; try fewer segments", time.as_secs_f64().ceil())),
        Some(s) if s.success() => {
            if ran.out.get(..4) != Some(MAGIC.as_slice()) {
                return Err("the boolean's answer did not read".into());
            }
            let mut at = 4;
            take(&ran.out, &mut at).map_err(|e| format!("the boolean's answer did not read: {e}"))
        }
        Some(_) if !said.is_empty() => Err(said),
        Some(s) => Err(format!(
            "the boolean engine stopped on these shapes ({s}); nothing was changed. Change a size or a position by a hundredth of a millimetre, or use fewer segments"
        )),
    }
}

/// What a child did: how it ended (none: it was stopped), what it wrote, and what it said on its error stream.
struct Ran {
    status: Option<std::process::ExitStatus>,
    out: Vec<u8>,
    said: Vec<u8>,
}

/// `exe args` run as a child: `input` on its standard input, what it writes taken up to `cap` bytes, for at most `time`. Both streams
/// are read while the child runs, so neither fills and stalls it. (BIMCraft's `run_child`, without its stop callback.)
fn run_child(exe: &Path, args: &[&std::ffi::OsStr], input: Option<Vec<u8>>, cap: u64, time: Duration) -> std::io::Result<Ran> {
    let mut child = Command::new(exe)
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Fed on a thread of its own: a child that writes before it has read all it is given would otherwise wait on us.
    let feed = child.stdin.take().zip(input).map(|(mut stdin, bytes)| {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&bytes);
        })
    });
    let out = child.stdout.take().map(|s| std::thread::spawn(move || drain(s, cap)));
    let said = child.stderr.take().map(|s| std::thread::spawn(move || drain(s, MAX_SAID)));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if start.elapsed() < time => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    if let Some(f) = feed {
        let _ = f.join();
    }
    let out = out.and_then(|t| t.join().ok()).unwrap_or_default();
    let said = said.and_then(|t| t.join().ok()).unwrap_or_default();
    Ok(Ran { status, out, said })
}

/// What `stream` gives, up to `cap` bytes.
fn drain(stream: impl Read, cap: u64) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = stream.take(cap).read_to_end(&mut buf);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meshes_cross_the_pipe_unchanged_and_bad_bytes_are_errors() {
        let (a, b) = (Mesh::cuboid([10.0, 10.0, 10.0]), Mesh::sphere(3.0, 24));
        let mut bytes = Vec::new();
        put(&mut bytes, &a);
        put(&mut bytes, &b);
        let mut at = 0;
        assert_eq!((take(&bytes, &mut at).unwrap(), take(&bytes, &mut at).unwrap()), (a, b));
        assert_eq!(at, bytes.len());
        for n in [0, 3, 8, 20, bytes.len() / 2] {
            assert!(take(&bytes[..n], &mut 0).is_err() || n >= 8 + 8 * 12 + 12 * 12, "{n}");
        }
        let mut lie = vec![0u8; 8];
        lie[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(take(&lie, &mut 0).is_err());
    }

    #[test]
    fn a_program_that_is_not_a_child_is_an_error_and_nothing_hangs() {
        // The test program itself, given the child's argument, runs no tests and prints nothing that reads as an answer.
        let exe = std::env::current_exe().unwrap();
        let (a, b) = (Mesh::cuboid([10.0, 10.0, 10.0]), Mesh::cuboid([4.0, 4.0, 14.0]));
        assert!(in_child(&exe, Op::Difference, &a, &b, Duration::from_secs(60)).is_err());
        assert!(
            in_child(Path::new("/nonexistent/lamina"), Op::Union, &a, &b, Duration::from_secs(5)).unwrap_err().contains("could not start")
        );
        // Without a child, the boolean runs here.
        assert!(isolated(Op::Difference, &a, &b).unwrap().is_watertight());
    }
}
