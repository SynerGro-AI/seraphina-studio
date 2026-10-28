//! Safe pure Material (surface core).
//!
//! A material owns a name, a base color, a roughness, and a
//! metallic value. Empty names are rejected. No unwraps.
//!
//! Differential: set roughness + get round-trips.

use crate::vec3::Vec3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MaterialError {
    EmptyName,
}

#[derive(Clone, Debug)]
pub struct Material {
    name: String,
    base_color: Vec3,
    roughness: f32,
    metallic: f32,
    ior: f32,
    density: f32,
}

impl Material {
    pub fn new(name: &str) -> Result<Self, MaterialError> {
        if name.is_empty() {
            return Err(MaterialError::EmptyName);
        }
        Ok(Self {
            name: name.to_string(),
            base_color: Vec3::new(0.8, 0.8, 0.8),
            roughness: 0.5,
            metallic: 0.0,
            ior: 1.5,
            density: 1000.0,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_base_color(&mut self, c: Vec3) {
        self.base_color = c;
    }

    pub fn base_color(&self) -> Vec3 {
        self.base_color
    }

    pub fn set_roughness(&mut self, r: f32) {
        self.roughness = r.clamp(0.0, 1.0);
    }

    pub fn roughness(&self) -> f32 {
        self.roughness
    }

    pub fn set_metallic(&mut self, m: f32) {
        self.metallic = m.clamp(0.0, 1.0);
    }

    pub fn metallic(&self) -> f32 {
        self.metallic
    }

    /// Index of refraction (dielectric Fresnel F0 = ((n-1)/(n+1))²).
    pub fn set_ior(&mut self, n: f32) {
        self.ior = n.clamp(1.0, 4.0);
    }

    pub fn ior(&self) -> f32 {
        self.ior
    }

    /// Density in kg/m³.
    pub fn set_density(&mut self, d: f32) {
        self.density = d.max(0.0);
    }

    pub fn density(&self) -> f32 {
        self.density
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create() {
        let m = Material::new("chrome").unwrap();
        assert_eq!(m.name(), "chrome");
        assert_eq!(m.roughness(), 0.5);
    }

    #[test]
    fn empty_name() {
        assert!(Material::new("").is_err());
    }

    #[test]
    fn hand_checked_values() {
        let mut m = Material::new("paint").unwrap();
        m.set_roughness(0.2);
        m.set_metallic(1.0);
        m.set_base_color(Vec3::new(0.1, 0.2, 0.8));
        assert_eq!(m.roughness(), 0.2);
        assert_eq!(m.metallic(), 1.0);
        assert_eq!(m.base_color().z, 0.8);
    }
}
