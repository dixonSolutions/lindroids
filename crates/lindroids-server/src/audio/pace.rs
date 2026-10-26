//! Real-time pacing of one stream.
//!
//! The server has no sound card to clock it, so it plays "in its head": once a stream runs, it
//! counts frames off a monotonic clock as if a device were consuming them, and lets the app hold
//! a fixed lead ahead of that point. The lead absorbs scheduling jitter on both sides of the link
//! and the time Android takes to write into its device buffer; anything more would only add
//! latency. The app's credits bound the burst size separately (see the stream code), so a slow
//! or absent app cannot make the server run ahead of real time either.

use std::time::Instant;

pub struct Pacer {
    rate: u64,
    /// Frames the app may hold beyond what real time has played.
    lead: u64,
    /// Frames taken from the client so far.
    sent: u64,
    /// Frames played when the clock last (re)started.
    base: u64,
    since: Option<Instant>,
}

impl Pacer {
    pub fn new(rate: u32, lead_ms: u32) -> Pacer {
        let rate = rate.max(1) as u64;
        Pacer { rate, lead: rate * lead_ms as u64 / 1000, sent: 0, base: 0, since: None }
    }

    fn clock(&self, now: Instant) -> u64 {
        let ran = self.since.map_or(0, |t| now.saturating_duration_since(t).as_micros() as u64 * self.rate / 1_000_000);
        self.base + ran
    }

    /// Frames played by now: never more than were sent.
    pub fn played(&self, now: Instant) -> u64 {
        self.clock(now).min(self.sent)
    }

    /// Frames sent and not played yet, i.e. what the app should still be holding.
    pub fn buffered(&self, now: Instant) -> u64 {
        self.sent - self.played(now)
    }

    pub fn sent(&self) -> u64 {
        self.sent
    }

    pub fn running(&self) -> bool {
        self.since.is_some()
    }

    pub fn start(&mut self, now: Instant) {
        if self.since.is_none() {
            self.since = Some(now);
        }
    }

    pub fn stop(&mut self, now: Instant) {
        self.base = self.played(now);
        self.since = None;
    }

    /// Frames that may be sent now.
    pub fn allowance(&self, now: Instant) -> u64 {
        if self.since.is_none() {
            return 0;
        }
        (self.clock(now) + self.lead).saturating_sub(self.sent)
    }

    pub fn take(&mut self, frames: u64) {
        self.sent += frames;
    }

    /// Real time has played everything that was sent.
    pub fn starved(&self, now: Instant) -> bool {
        self.since.is_some() && self.clock(now) >= self.sent
    }

    /// Restart the clock from what was sent, after a stall that was not the client's fault
    /// (the app held back credit), so the stall does not turn into a burst afterwards.
    pub fn resync(&mut self, now: Instant) {
        if self.since.is_some() && self.clock(now) > self.sent {
            self.base = self.sent;
            self.since = Some(now);
        }
    }

    pub fn set_rate(&mut self, now: Instant, rate: u32) {
        let running = self.running();
        let lead_ms = self.lead * 1000 / self.rate;
        self.stop(now);
        self.rate = rate.max(1) as u64;
        self.lead = self.rate * lead_ms / 1000;
        if running {
            self.start(now);
        }
    }

    pub fn frames_to_usec(&self, frames: u64) -> u64 {
        frames * 1_000_000 / self.rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn nothing_moves_until_started() {
        let t0 = Instant::now();
        let p = Pacer::new(48000, 100);
        assert_eq!(p.allowance(t0 + 500 * MS), 0);
        assert!(!p.starved(t0 + 500 * MS));
    }

    #[test]
    fn lead_then_real_time() {
        let t0 = Instant::now();
        let mut p = Pacer::new(48000, 100);
        p.start(t0);
        assert_eq!(p.allowance(t0), 4800, "the app may hold 100 ms straight away");
        p.take(4800);
        assert_eq!(p.allowance(t0), 0);
        assert_eq!(p.allowance(t0 + 20 * MS), 960, "then 20 ms of frames per 20 ms");
        assert_eq!(p.buffered(t0 + 20 * MS), 4800 - 960);
        p.take(960);
        // A second at real time sends a second's worth of frames.
        let mut sent = 0;
        for ms in (30..=1020).step_by(10) {
            let n = p.allowance(t0 + ms * MS);
            p.take(n);
            sent += n;
        }
        assert_eq!(sent, 48000);
        assert_eq!(p.buffered(t0 + 1020 * MS), 4800);
    }

    #[test]
    fn starves_when_the_lead_has_played_out() {
        let t0 = Instant::now();
        let mut p = Pacer::new(1000, 100);
        p.start(t0);
        p.take(100);
        assert!(!p.starved(t0 + 99 * MS));
        assert!(p.starved(t0 + 100 * MS));
        assert_eq!(p.played(t0 + 5000 * MS), 100, "never plays more than it was given");
        assert_eq!(p.buffered(t0 + 5000 * MS), 0);
    }

    #[test]
    fn stopping_keeps_what_the_app_holds() {
        let t0 = Instant::now();
        let mut p = Pacer::new(1000, 100);
        p.start(t0);
        p.take(100);
        p.stop(t0 + 40 * MS);
        assert_eq!(p.buffered(t0 + 1000 * MS), 60, "corked: the app pauses with 60 ms queued");
        p.start(t0 + 1000 * MS);
        assert_eq!(p.buffered(t0 + 1030 * MS), 30);
        assert_eq!(p.allowance(t0 + 1030 * MS), 70);
    }

    #[test]
    fn resync_after_a_credit_stall() {
        let t0 = Instant::now();
        let mut p = Pacer::new(1000, 100);
        p.start(t0);
        p.take(100);
        // The app took no more for a second: no burst of a second's worth afterwards.
        p.resync(t0 + 1100 * MS);
        assert_eq!(p.allowance(t0 + 1100 * MS), 100);
    }

    #[test]
    fn rate_changes_keep_position() {
        let t0 = Instant::now();
        let mut p = Pacer::new(1000, 100);
        p.start(t0);
        p.take(100);
        p.set_rate(t0 + 50 * MS, 2000);
        assert_eq!(p.buffered(t0 + 50 * MS), 50);
        assert_eq!(p.buffered(t0 + 60 * MS), 30);
        assert_eq!(p.frames_to_usec(2000), 1_000_000);
    }
}
