//! Hold encoder packets for a fixed delay before they go out (after OBS's obs-output-delay.c).
//!
//! The stream delay pushes each packet with the time it arrived and pops it
//! once `delay` has passed, in arrival order, so viewers see the stream
//! exactly `delay` late and nothing is reordered or dropped on the way.

use crate::encoder_packet::EncoderPacket;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct OutputDelayQueue {
    delay: Duration,
    queue: VecDeque<(Instant, EncoderPacket)>,
}

impl OutputDelayQueue {
    pub fn new(delay: Duration) -> Self {
        Self {
            delay,
            queue: VecDeque::new(),
        }
    }

    /// Queue a packet that arrived at `now`.
    pub fn push(&mut self, packet: EncoderPacket, now: Instant) {
        self.queue.push_back((now, packet));
    }

    /// The oldest packet if it has waited the full delay by `now`.
    pub fn pop_due(&mut self, now: Instant) -> Option<EncoderPacket> {
        let (t, _) = self.queue.front()?;
        if now.saturating_duration_since(*t) >= self.delay {
            self.queue.pop_front().map(|(_, p)| p)
        } else {
            None
        }
    }

    /// When the oldest packet becomes due (`None` when empty).
    pub fn next_due(&self) -> Option<Instant> {
        self.queue.front().map(|(t, _)| *t + self.delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt(dts_ms: u32) -> EncoderPacket {
        EncoderPacket {
            dts_ms,
            bytes: vec![dts_ms as u8],
        }
    }

    #[test]
    fn releases_each_packet_only_after_the_delay_in_arrival_order() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut q = OutputDelayQueue::new(ms(2000));
        assert_eq!(q.next_due(), None);
        q.push(pkt(0), t0);
        q.push(pkt(33), t0 + ms(33));
        q.push(pkt(66), t0 + ms(66));
        assert_eq!(q.next_due(), Some(t0 + ms(2000)));
        assert_eq!(q.pop_due(t0 + ms(1999)), None);
        assert_eq!(q.pop_due(t0 + ms(2000)), Some(pkt(0)));
        assert_eq!(q.pop_due(t0 + ms(2000)), None);
        assert_eq!(q.next_due(), Some(t0 + ms(2033)));
        assert_eq!(q.pop_due(t0 + ms(5000)), Some(pkt(33)));
        assert_eq!(q.pop_due(t0 + ms(5000)), Some(pkt(66)));
        assert_eq!(q.pop_due(t0 + ms(5000)), None);
    }

    #[test]
    fn zero_delay_releases_at_once() {
        let t0 = Instant::now();
        let mut q = OutputDelayQueue::new(Duration::ZERO);
        q.push(pkt(1), t0);
        assert_eq!(q.pop_due(t0), Some(pkt(1)));
    }
}
