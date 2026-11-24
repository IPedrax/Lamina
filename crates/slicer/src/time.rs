//! Print time as a printer's planner would take it: each move speeds up and slows down at `max_acceleration` (a trapezoid, or a
//! triangle when the move is too short to reach its speed), and passes a corner no faster than the junction deviation allows (the
//! formula Marlin documents: `v^2 = a * jd * sin(t/2) / (1 - sin(t/2))`, `t` the angle the path turns through; Klipper's square
//! corner velocity `scv` is the same with `jd = scv^2 * (sqrt 2 - 1) / a`). A backward pass makes every move able to stop in time, a
//! forward pass keeps every speed up reachable. Clean-room, from the firmwares' documentation.

/// One move of the head, as the planner sees it.
#[derive(Clone, Copy, Debug)]
struct Move {
    length: f64,
    /// The speed asked for, mm/s.
    feed: f64,
    /// Its direction in the plane, a unit vector; none for a move that is only z or only filament, which starts and ends at rest.
    dir: Option<[f64; 2]>,
}

#[derive(Debug)]
pub struct Planner {
    accel: f64,
    deviation: f64,
    moves: Vec<Move>,
    /// Time spent standing still (waiting for a nozzle to heat), seconds.
    waits: f64,
}

impl Planner {
    pub fn new(accel: f64, deviation: f64) -> Planner {
        Planner { accel: accel.max(1.0), deviation: deviation.max(0.0), moves: Vec::new(), waits: 0.0 }
    }

    /// A move in the plane from `from` to `to` at `feed` mm/s.
    pub fn xy(&mut self, from: [f64; 2], to: [f64; 2], feed: f64) {
        let d = [to[0] - from[0], to[1] - from[1]];
        let length = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if length > 1e-9 && feed > 0.0 {
            self.moves.push(Move { length, feed, dir: Some([d[0] / length, d[1] / length]) });
        }
    }

    /// A move that is not in the plane (z, or filament alone): from rest to rest.
    pub fn other(&mut self, length: f64, feed: f64) {
        if length > 1e-9 && feed > 0.0 {
            self.moves.push(Move { length, feed, dir: None });
        }
    }

    /// The head standing still for `seconds` (a nozzle heating); what comes after starts from rest, as a tool change's moves do.
    pub fn wait(&mut self, seconds: f64) {
        if seconds.is_finite() && seconds > 0.0 {
            self.waits += seconds;
        }
    }

    /// The fastest the head may pass from move `a` into move `b`.
    fn junction(&self, a: &Move, b: &Move) -> f64 {
        let (Some(u), Some(v)) = (a.dir, b.dir) else { return 0.0 };
        let cos = -(u[0] * v[0] + u[1] * v[1]);
        let sin_half = (0.5 * (1.0 - cos)).max(0.0).sqrt();
        let limit = a.feed.min(b.feed);
        if sin_half >= 1.0 - 1e-9 {
            return limit; // straight on
        }
        (self.accel * self.deviation * sin_half / (1.0 - sin_half)).sqrt().min(limit)
    }

    /// The time of every move, seconds.
    pub fn seconds(&self) -> f64 {
        let n = self.moves.len();
        if n == 0 {
            return self.waits;
        }
        let a = self.accel;
        // The speed at each boundary: 0 before the first and after the last, else what the corner allows.
        let mut v: Vec<f64> = std::iter::once(0.0)
            .chain((0..n - 1).map(|i| self.junction(&self.moves[i], &self.moves[i + 1])))
            .chain(std::iter::once(0.0))
            .collect();
        // Backward: every move must be able to slow to its exit speed; forward: to reach its exit speed from its entry speed.
        for i in (0..n).rev() {
            v[i] = v[i].min((v[i + 1] * v[i + 1] + 2.0 * a * self.moves[i].length).sqrt());
        }
        for i in 0..n {
            v[i + 1] = v[i + 1].min((v[i] * v[i] + 2.0 * a * self.moves[i].length).sqrt());
        }
        self.waits + self.moves.iter().enumerate().map(|(i, m)| trapezoid(m.length, v[i], v[i + 1], m.feed, a)).sum::<f64>()
    }
}

/// The time to cover `d` mm entering at `vi`, leaving at `vo`, cruising at no more than `vm`, speeding up and slowing at `a`.
fn trapezoid(d: f64, vi: f64, vo: f64, vm: f64, a: f64) -> f64 {
    let vm = vm.max(vi).max(vo);
    let (up, down) = ((vm * vm - vi * vi) / (2.0 * a), (vm * vm - vo * vo) / (2.0 * a));
    if up + down <= d {
        (vm - vi) / a + (vm - vo) / a + (d - up - down) / vm
    } else {
        let peak = ((2.0 * a * d + vi * vi + vo * vo) / 2.0).sqrt();
        (peak - vi).max(0.0) / a + (peak - vo).max(0.0) / a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_straight_move_takes_its_length_over_speed_plus_the_ramps() {
        let mut p = Planner::new(1000.0, 0.013);
        p.xy([0.0, 0.0], [200.0, 0.0], 100.0);
        // Up from rest and down to rest at 1000 mm/s2: 0.1 s each way covering 5 mm each, 190 mm at 100 mm/s.
        assert!((p.seconds() - (0.1 + 0.1 + 1.9)).abs() < 1e-9, "{}", p.seconds());
    }

    #[test]
    fn a_short_move_never_reaches_its_speed_and_corners_slow_the_head() {
        let mut p = Planner::new(1000.0, 0.013);
        p.xy([0.0, 0.0], [1.0, 0.0], 100.0);
        // A triangle: to 31.6 mm/s over half a millimetre and back.
        assert!((p.seconds() - 2.0 * (1000.0f64).sqrt() / 1000.0).abs() < 1e-9, "{}", p.seconds());
        // The same path as one straight move, and as a zigzag of right angles: the zigzag stops nearly dead at each corner.
        let (mut straight, mut zigzag) = (Planner::new(1500.0, 0.013), Planner::new(1500.0, 0.013));
        straight.xy([0.0, 0.0], [40.0, 0.0], 80.0);
        for i in 0..20 {
            let x = f64::from(i) * 2.0;
            zigzag.xy([x, 0.0], [x + 1.0, 1.0], 80.0);
            zigzag.xy([x + 1.0, 1.0], [x + 2.0, 0.0], 80.0);
        }
        assert!(zigzag.seconds() > straight.seconds() * 1.5, "{} {}", zigzag.seconds(), straight.seconds());
        // Going straight on through a split point costs nothing.
        let mut split = Planner::new(1500.0, 0.013);
        split.xy([0.0, 0.0], [20.0, 0.0], 80.0);
        split.xy([20.0, 0.0], [40.0, 0.0], 80.0);
        assert!((split.seconds() - straight.seconds()).abs() < 1e-9);
    }

    #[test]
    fn moves_off_the_plane_start_and_end_at_rest() {
        let mut p = Planner::new(1000.0, 0.013);
        p.other(0.8, 35.0);
        p.xy([0.0, 0.0], [100.0, 0.0], 100.0);
        assert!(p.seconds() > 0.8 / 35.0 + 1.0);
        assert_eq!(Planner::new(1000.0, 0.013).seconds(), 0.0);
    }
}
