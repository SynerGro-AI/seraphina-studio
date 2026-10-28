//! Safe pure LiveWeights (runtime parameter store).
//!
//! Holds named float weights that can change between frames.
//! Empty names rejected. No unwraps.
//!
//! Differential: set + get + update round-trips.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WeightError {
    EmptyName,
    Missing,
}

#[derive(Clone, Debug, Default)]
pub struct LiveWeights {
    values: BTreeMap<String, f32>,
    capacity: usize,
}

impl LiveWeights {
    pub fn new() -> Self {
        Self {
            capacity: 1024,
            ..Self::default()
        }
    }

    /// Set the maximum number of weights.
    /// Existing keys are never removed by this call.
    pub fn set_capacity(&mut self, n: usize) {
        self.capacity = n.max(1);
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn set(&mut self, name: &str, value: f32) -> Result<(), WeightError> {
        if name.is_empty() {
            return Err(WeightError::EmptyName);
        }
        self.values.insert(name.to_string(), value);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<f32, WeightError> {
        self.values.get(name).copied().ok_or(WeightError::Missing)
    }

    /// Add delta to an existing weight. Missing is an error.
    pub fn update(&mut self, name: &str, delta: f32) -> Result<f32, WeightError> {
        let v = self.get(name)?;
        let next = v + delta;
        self.values.insert(name.to_string(), next);
        Ok(next)
    }

    pub fn remove(&mut self, name: &str) -> bool {
        self.values.remove(name).is_some()
    }

    /// Merge lessons into the existing weights.
    /// Only inserts keys that are not already present.
    /// Stops adding when capacity is reached.
    /// Existing values are never overwritten.
    /// Empty names are skipped. Returns the number of new keys added.
    pub fn merge_lessons(&mut self, lessons: &[(&str, f32)]) -> usize {
        let mut added = 0;
        for (name, value) in lessons {
            if name.is_empty() {
                continue;
            }
            if self.values.contains_key(*name) {
                continue;
            }
            if self.values.len() >= self.capacity {
                break;
            }
            self.values.insert((*name).to_string(), *value);
            added += 1;
        }
        added
    }

    /// True if the name already exists.
    pub fn contains(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.values.keys().map(|s| s.as_str()).collect()
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty() {
        let mut w = LiveWeights::new();
        assert!(w.set("", 1.0).is_err());
        assert!(w.get("missing").is_err());
    }

    #[test]
    fn capacity_limits_new() {
        let mut w = LiveWeights::new();
        w.set_capacity(2);
        w.set("keep", 1.0).unwrap();
        let added = w.merge_lessons(&[("a", 0.1), ("b", 0.2), ("c", 0.3)]);
        assert_eq!(added, 1);
        assert_eq!(w.get("keep").unwrap(), 1.0);
        assert!(w.contains("a"));
        assert!(!w.contains("c"));
    }

    #[test]
    fn merge_adds_only_new() {
        let mut w = LiveWeights::new();
        w.set("glow", 0.5).unwrap();
        let added = w.merge_lessons(&[("glow", 0.9), ("blur", 0.3), ("", 1.0)]);
        assert_eq!(added, 1);
        assert_eq!(w.get("glow").unwrap(), 0.5);
        assert_eq!(w.get("blur").unwrap(), 0.3);
        assert!(!w.contains(""));
    }

    #[test]
    fn hand_checked_values() {
        let mut w = LiveWeights::new();
        w.set("glow", 0.5).unwrap();
        assert_eq!(w.get("glow").unwrap(), 0.5);
        let next = w.update("glow", 0.2).unwrap();
        assert_eq!(next, 0.7);
        assert!(w.remove("glow"));
        assert!(w.is_empty());
    }
}
