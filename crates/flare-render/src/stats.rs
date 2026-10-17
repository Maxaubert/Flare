//! Frame-time statistics over a rolling window of samples.

use std::collections::VecDeque;

/// A rolling window of frame times in milliseconds. Pushing past `cap` drops the oldest sample.
#[derive(Debug, Clone)]
pub struct FrameStats {
    samples: VecDeque<f64>,
    cap: usize,
}

impl FrameStats {
    pub fn new(cap: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(cap),
            cap,
        }
    }

    pub fn push_ms(&mut self, ms: f64) {
        self.samples.push_back(ms);
        while self.samples.len() > self.cap {
            self.samples.pop_front();
        }
    }

    pub fn last(&self) -> Option<f64> {
        self.samples.back().copied()
    }

    pub fn avg(&self) -> Option<f64> {
        if self.samples.is_empty() {
            return None;
        }
        Some(self.samples.iter().sum::<f64>() / self.samples.len() as f64)
    }

    pub fn max(&self) -> Option<f64> {
        self.samples.iter().copied().reduce(f64::max)
    }

    /// Nearest-rank 95th percentile over a sorted copy of the window.
    pub fn p95(&self) -> Option<f64> {
        let n = self.samples.len();
        if n == 0 {
            return None;
        }
        let mut sorted: Vec<f64> = self.samples.iter().copied().collect();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let idx = (95 * n).div_ceil(100) - 1;
        Some(sorted[idx])
    }

    /// One line, for logs and the overlay: `last 1.20 ms, avg 1.10 ms, p95 1.90 ms, max 2.00 ms (n=20)`,
    /// or `no frames` when the window is empty.
    pub fn summary(&self) -> String {
        match (self.last(), self.avg(), self.p95(), self.max()) {
            (Some(last), Some(avg), Some(p95), Some(max)) => format!(
                "last {last:.2} ms, avg {avg:.2} ms, p95 {p95:.2} ms, max {max:.2} ms (n={})",
                self.samples.len()
            ),
            _ => "no frames".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_none() {
        let s = FrameStats::new(10);
        assert_eq!(s.last(), None);
        assert_eq!(s.avg(), None);
        assert_eq!(s.max(), None);
        assert_eq!(s.p95(), None);
        assert_eq!(s.summary(), "no frames");
    }

    #[test]
    fn cap_drops_oldest() {
        let mut s = FrameStats::new(3);
        for v in [1.0, 2.0, 3.0, 4.0] {
            s.push_ms(v);
        }
        assert_eq!(s.avg(), Some(3.0));
        assert_eq!(s.last(), Some(4.0));
    }

    #[test]
    fn avg_and_max() {
        let mut s = FrameStats::new(10);
        s.push_ms(1.0);
        s.push_ms(5.0);
        s.push_ms(3.0);
        assert_eq!(s.avg(), Some(3.0));
        assert_eq!(s.max(), Some(5.0));
    }

    #[test]
    fn p95_nearest_rank() {
        let mut s = FrameStats::new(100);
        for i in 1..=20 {
            s.push_ms(f64::from(i));
        }
        assert_eq!(s.p95(), Some(19.0));

        let mut one = FrameStats::new(100);
        one.push_ms(5.0);
        assert_eq!(one.p95(), Some(5.0));
    }

    #[test]
    fn summary_format() {
        let mut s = FrameStats::new(10);
        s.push_ms(1.0);
        s.push_ms(2.0);
        assert_eq!(
            s.summary(),
            "last 2.00 ms, avg 1.50 ms, p95 2.00 ms, max 2.00 ms (n=2)"
        );
    }
}
