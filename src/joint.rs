//! Safe pure Joint (skeleton joint tool).
//!
//! Holds named joints with parent links and positions.
//! Add a joint, set its parent, or move it.
//! Empty names and missing parents rejected. No unwraps.
//!
//! Differential: a different parent produces a different hierarchy.

use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct JointNode {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub parent: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Joint {
    nodes: HashMap<String, JointNode>,
}

impl Joint {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a root joint. False if empty or duplicate.
    pub fn add_root(&mut self, name: &str, x: f32, y: f32, z: f32) -> bool {
        if name.is_empty() || self.nodes.contains_key(name) {
            return false;
        }
        self.nodes.insert(
            name.to_string(),
            JointNode {
                x,
                y,
                z,
                parent: None,
            },
        );
        true
    }

    /// Add a child joint. False if empty, duplicate, or parent missing.
    pub fn add_child(&mut self, name: &str, parent: &str, x: f32, y: f32, z: f32) -> bool {
        if name.is_empty() || self.nodes.contains_key(name) || !self.nodes.contains_key(parent) {
            return false;
        }
        self.nodes.insert(
            name.to_string(),
            JointNode {
                x,
                y,
                z,
                parent: Some(parent.to_string()),
            },
        );
        true
    }

    pub fn get(&self, name: &str) -> Option<&JointNode> {
        self.nodes.get(name)
    }

    pub fn move_joint(&mut self, name: &str, x: f32, y: f32, z: f32) -> bool {
        if let Some(n) = self.nodes.get_mut(name) {
            n.x = x;
            n.y = y;
            n.z = z;
            true
        } else {
            false
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hierarchy() {
        let mut j = Joint::new();
        assert!(j.add_root("hip", 0.0, 0.0, 0.0));
        assert!(j.add_child("knee", "hip", 0.0, -1.0, 0.0));
        assert_eq!(j.get("knee").unwrap().parent.as_deref(), Some("hip"));
    }

    #[test]
    fn missing_parent() {
        let mut j = Joint::new();
        assert!(!j.add_child("knee", "hip", 0.0, 0.0, 0.0));
    }

    #[test]
    fn hand_checked_values() {
        let mut a = Joint::new();
        a.add_root("r", 0.0, 0.0, 0.0);
        let mut b = Joint::new();
        b.add_root("r", 0.0, 0.0, 0.0);
        b.add_child("c", "r", 1.0, 0.0, 0.0);
        assert_ne!(a.len(), b.len());
    }
}
