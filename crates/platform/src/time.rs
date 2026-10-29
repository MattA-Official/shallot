//! Frame timing.

use std::time::{Duration, Instant};

/// Measures time between frames.
///
/// Mirrors the engine's per-frame `time` resource: `delta` is the time since the previous
/// [`Clock::tick`], and `elapsed` the time since the clock was created.
#[derive(Debug, Clone)]
pub struct Clock {
    start: Instant,
    last: Instant,
    frame: u64,
}

/// One reading from [`Clock::tick`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameTime {
    /// Seconds since the previous tick.
    pub delta: f64,
    /// Seconds since the clock started.
    pub elapsed: f64,
    /// Number of ticks before this one.
    pub frame: u64,
}

impl Clock {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            start: now,
            last: now,
            frame: 0,
        }
    }

    /// Advances the clock to now and reports the frame's timing.
    pub fn tick(&mut self) -> FrameTime {
        let now = Instant::now();
        let time = FrameTime {
            delta: (now - self.last).as_secs_f64(),
            elapsed: (now - self.start).as_secs_f64(),
            frame: self.frame,
        };
        self.last = now;
        self.frame += 1;
        time
    }

    /// Time since the clock started, without advancing it.
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_advance_frames_and_time() {
        let mut clock = Clock::new();
        let first = clock.tick();
        std::thread::sleep(Duration::from_millis(2));
        let second = clock.tick();
        assert_eq!(first.frame, 0);
        assert_eq!(second.frame, 1);
        assert!(second.delta > 0.0);
        assert!(second.elapsed >= second.delta);
    }
}
