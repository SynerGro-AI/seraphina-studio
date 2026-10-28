//! Safe pure MaterialRegistry (named material store).
//!
//! Holds materials by name. Empty and duplicate names are
//! rejected. No unwraps.
//!
//! Differential: add + get round-trips.

use std::collections::HashMap;

use crate::material::{Material, MaterialError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MaterialRegistryError {
    EmptyName,
    Duplicate,
    Missing,
}

impl From<MaterialError> for MaterialRegistryError {
    fn from(_: MaterialError) -> Self {
        MaterialRegistryError::EmptyName
    }
}

#[derive(Clone, Debug, Default)]
pub struct MaterialRegistry {
    materials: HashMap<String, Material>,
}

impl MaterialRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, name: &str) -> Result<(), MaterialRegistryError> {
        if name.is_empty() {
            return Err(MaterialRegistryError::EmptyName);
        }
        if self.materials.contains_key(name) {
            return Err(MaterialRegistryError::Duplicate);
        }
        let m = Material::new(name)?;
        self.materials.insert(name.to_string(), m);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<&Material, MaterialRegistryError> {
        self.materials
            .get(name)
            .ok_or(MaterialRegistryError::Missing)
    }

    pub fn get_mut(&mut self, name: &str) -> Result<&mut Material, MaterialRegistryError> {
        self.materials
            .get_mut(name)
            .ok_or(MaterialRegistryError::Missing)
    }

    pub fn remove(&mut self, name: &str) -> bool {
        self.materials.remove(name).is_some()
    }

    pub fn len(&self) -> usize {
        self.materials.len()
    }

    pub fn is_empty(&self) -> bool {
        self.materials.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_get() {
        let mut r = MaterialRegistry::new();
        r.add("chrome").unwrap();
        assert_eq!(r.get("chrome").unwrap().name(), "chrome");
    }

    #[test]
    fn duplicate_and_empty() {
        let mut r = MaterialRegistry::new();
        r.add("a").unwrap();
        assert!(matches!(r.add("a"), Err(MaterialRegistryError::Duplicate)));
        assert!(matches!(r.add(""), Err(MaterialRegistryError::EmptyName)));
    }

    #[test]
    fn hand_checked_values() {
        let mut r = MaterialRegistry::new();
        r.add("paint").unwrap();
        r.get_mut("paint").unwrap().set_roughness(0.1);
        assert_eq!(r.get("paint").unwrap().roughness(), 0.1);
        assert_eq!(r.len(), 1);
    }
}
