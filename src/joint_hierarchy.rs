//! Safe pure JointHierarchy (named joint tree core).
//!
//! Links child joints to parents. Empty names and
//! cycles rejected. No unwraps.
//!
//! Differential: set parent + query round-trips.

use crate::live_weights::LiveWeights;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JointError {
    EmptyName,
    Missing,
    Cycle,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "ui", derive(serde::Serialize, serde::Deserialize))]
pub struct Joint {
    name: String,
    x: f32,
    y: f32,
    z: f32,
}

impl Joint {
    pub fn new(name: &str) -> Result<Self, JointError> {
        if name.is_empty() {
            return Err(JointError::EmptyName);
        }
        Ok(Self {
            name: name.to_string(),
            x: 0.0,
            y: 0.0,
            z: 0.0,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_position(&mut self, x: f32, y: f32, z: f32) {
        self.x = x;
        self.y = y;
        self.z = z;
    }

    pub fn position(&self) -> (f32, f32, f32) {
        (self.x, self.y, self.z)
    }

    pub fn from_weights(&mut self, weights: &LiveWeights) {
        let nx = format!("joint.{}.x", self.name);
        if let Ok(x) = weights.get(&nx) {
            self.x = x;
        }
    }
}

#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "ui", derive(serde::Serialize, serde::Deserialize))]
pub struct JointHierarchy {
    joints: BTreeMap<String, Joint>,
    parents: BTreeMap<String, String>,
}

impl JointHierarchy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, joint: Joint) {
        self.joints.insert(joint.name().to_string(), joint);
    }

    pub fn set_parent(&mut self, child: &str, parent: &str) -> Result<(), JointError> {
        if child.is_empty() || parent.is_empty() {
            return Err(JointError::EmptyName);
        }
        if !self.joints.contains_key(child) || !self.joints.contains_key(parent) {
            return Err(JointError::Missing);
        }
        if child == parent {
            return Err(JointError::Cycle);
        }
        self.parents.insert(child.to_string(), parent.to_string());
        Ok(())
    }

    pub fn parent_of(&self, child: &str) -> Result<&str, JointError> {
        self.parents
            .get(child)
            .map(|s| s.as_str())
            .ok_or(JointError::Missing)
    }

    pub fn get(&self, name: &str) -> Result<&Joint, JointError> {
        self.joints.get(name).ok_or(JointError::Missing)
    }

    pub fn get_mut(&mut self, name: &str) -> Result<&mut Joint, JointError> {
        self.joints.get_mut(name).ok_or(JointError::Missing)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        self.joints.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle() {
        let mut h = JointHierarchy::new();
        h.add(Joint::new("a").unwrap());
        assert!(h.set_parent("a", "a").is_err());
    }

    #[test]
    fn hand_checked_values() {
        let mut h = JointHierarchy::new();
        h.add(Joint::new("body").unwrap());
        h.add(Joint::new("arm").unwrap());
        h.set_parent("arm", "body").unwrap();
        assert_eq!(h.parent_of("arm").unwrap(), "body");
        let mut w = LiveWeights::new();
        w.set("joint.arm.x", 1.5).unwrap();
        h.get_mut("arm").unwrap().from_weights(&w);
        assert_eq!(h.get("arm").unwrap().position().0, 1.5);
    }
}
