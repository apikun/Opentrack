//! Project-wide frame cache keyed by `(MediaId, frame)` with a byte budget
//! and LRU eviction (RFC-0007). The per-decoder caches inside
//! [`crate::video`] keep single-clip scrubbing hot; this one is shared by the
//! compositor so several video tracks' decoders draw from one budget.

use std::collections::{HashMap, VecDeque};

use opentrack_core::MediaId;

/// Default cache budget: 256 MiB of RGBA pixels.
pub const DEFAULT_BUDGET_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone)]
struct Entry {
    frame: std::sync::Arc<crate::video::RgbaFrame>,
    bytes: usize,
}

/// LRU frame cache: `get` refreshes recency, `put` evicts oldest-first when
/// the budget is exceeded.
pub struct FrameCache {
    map: HashMap<(MediaId, u64), Entry>,
    order: VecDeque<(MediaId, u64)>,
    bytes: usize,
    budget: usize,
    // stats
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl Default for FrameCache {
    fn default() -> Self {
        Self::new(DEFAULT_BUDGET_BYTES)
    }
}

impl FrameCache {
    /// A cache with a byte budget (0 = unbounded, used by tests).
    pub fn new(budget: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            budget,
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }

    /// Current resident byte count.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Decoded-hit / miss counters.
    pub fn stats(&self) -> (u64, u64, u64) {
        (self.hits, self.misses, self.evictions)
    }

    /// Looks a frame up, refreshing its LRU position.
    pub fn get(
        &mut self,
        media: MediaId,
        frame: u64,
    ) -> Option<std::sync::Arc<crate::video::RgbaFrame>> {
        let key = (media, frame);
        if let Some(e) = self.map.get(&key) {
            let f = e.frame.clone();
            self.hits += 1;
            self.touch(key);
            return Some(f);
        }
        self.misses += 1;
        None
    }

    /// Inserts a frame, evicting oldest entries until the budget holds.
    pub fn put(&mut self, media: MediaId, frame: u64, f: std::sync::Arc<crate::video::RgbaFrame>) {
        let key = (media, frame);
        let bytes = f.rgba.len();
        if let Some(old) = self.map.insert(key, Entry { frame: f, bytes }) {
            self.bytes = self.bytes.saturating_sub(old.bytes);
        } else {
            self.order.push_back(key);
        }
        self.bytes += bytes;
        while self.bytes > self.budget {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(old) = self.map.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(old.bytes);
                self.evictions += 1;
            }
        }
    }

    fn touch(&mut self, key: (MediaId, u64)) {
        if let Some(pos) = self.order.iter().position(|k| *k == key) {
            self.order.remove(pos);
            self.order.push_back(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::RgbaFrame;

    fn frame(px: usize) -> std::sync::Arc<RgbaFrame> {
        std::sync::Arc::new(RgbaFrame {
            width: px as u32,
            height: 1,
            rgba: vec![0; px * 4],
            pts_secs: 0.0,
            key: true,
        })
    }

    #[test]
    fn eviction_respects_budget_and_lru_order() {
        let mut c = FrameCache::new(32); // 32 bytes: two 4-px frames
        let m = MediaId(1);
        c.put(m, 0, frame(4)); // 16 bytes
        c.put(m, 1, frame(4)); // 16 bytes — budget exactly reached
        c.get(m, 0); // refresh 0 → 1 becomes LRU
        c.put(m, 2, frame(4)); // must evict frame 1, not 0
        assert!(c.get(m, 0).is_some(), "refreshed entry survives");
        assert!(c.get(m, 1).is_none(), "LRU tail evicted");
        assert!(c.get(m, 2).is_some());
        let (hits, misses, evictions) = c.stats();
        assert_eq!((hits, misses, evictions), (3, 1, 1));
    }

    #[test]
    fn keys_are_per_media() {
        let mut c = FrameCache::new(1024);
        c.put(MediaId(1), 0, frame(2));
        c.put(MediaId(2), 0, frame(2));
        assert!(c.get(MediaId(1), 0).is_some());
        assert!(c.get(MediaId(2), 0).is_some());
        assert!(c.get(MediaId(3), 0).is_none());
    }
}
