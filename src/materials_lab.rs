//! Safe pure MaterialsLab (materials science core).
//!
//! Builds a material from atomic composition using real binding
//! laws: mass number, charge balance, and a simple semi-empirical
//! binding energy. Unstable compositions are rejected. No unwraps.
//!
//! Differential: carbon-12 (6p, 6n, 6e) is stable and produces
//! a material; an unbalanced charge is rejected.

use crate::material_registry::{MaterialRegistry, MaterialRegistryError};
use crate::periodic_table::PeriodicTable;
use crate::vec3::Vec3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LabError {
    EmptyName,
    Unstable,
    UnknownElement,
    Registry(MaterialRegistryError),
}

impl From<MaterialRegistryError> for LabError {
    fn from(e: MaterialRegistryError) -> Self {
        match e {
            MaterialRegistryError::EmptyName => LabError::EmptyName,
            other => LabError::Registry(other),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Atom {
    protons: u32,
    neutrons: u32,
    electrons: u32,
}

impl Atom {
    pub fn new(protons: u32, neutrons: u32, electrons: u32) -> Self {
        Self {
            protons,
            neutrons,
            electrons,
        }
    }

    pub fn protons(&self) -> u32 {
        self.protons
    }
    pub fn neutrons(&self) -> u32 {
        self.neutrons
    }
    pub fn electrons(&self) -> u32 {
        self.electrons
    }

    /// Mass number A = Z + N.
    pub fn mass_number(&self) -> u32 {
        self.protons + self.neutrons
    }

    /// Charge balance: electrons should equal protons for a neutral atom.
    pub fn is_neutral(&self) -> bool {
        self.electrons == self.protons
    }

    /// Simple stability: neutral and neutron/proton ratio within a real range.
    /// Light nuclei prefer N ≈ Z; heavier allow more neutrons.
    pub fn is_stable(&self) -> bool {
        if !self.is_neutral() || self.protons == 0 {
            return false;
        }
        // Hydrogen-1 is stable with no neutrons.
        if self.protons == 1 && self.neutrons == 0 {
            return true;
        }
        let z = self.protons as f32;
        let n = self.neutrons as f32;
        let ratio = n / z;
        // Real band: ~1.0 for light, up to ~1.5 for heavy.
        (0.8..=1.6).contains(&ratio)
    }

    /// Semi-empirical binding energy per nucleon (MeV), simplified.
    /// Higher is more tightly bound.
    pub fn binding_per_nucleon(&self) -> f32 {
        if !self.is_stable() {
            return 0.0;
        }
        let a = self.mass_number() as f32;
        // Volume term minus a surface-like penalty, clamped.
        let be = 8.5 - 7.0 / a.sqrt();
        be.max(0.0)
    }
}

pub struct MaterialsLab;

impl MaterialsLab {
    /// Create a material from an atom if it is stable.
    /// Metallic character rises with binding; roughness falls.
    pub fn synthesize(
        registry: &mut MaterialRegistry,
        name: &str,
        atom: &Atom,
    ) -> Result<(), LabError> {
        if !atom.is_stable() {
            return Err(LabError::Unstable);
        }
        registry.add(name)?;
        let m = registry.get_mut(name)?;
        let be = atom.binding_per_nucleon();
        let metallic = (be / 9.0).clamp(0.0, 1.0);
        let roughness = (1.0 - metallic).clamp(0.0, 1.0);
        // Color from proton count (simple spectral map).
        let hue = (atom.protons() as f32 * 20.0) % 360.0;
        let color = hue_to_rgb(hue);
        m.set_base_color(color);
        m.set_metallic(metallic);
        m.set_roughness(roughness);
        Ok(())
    }

    /// Synthesize an element by atomic number from the periodic table.
    pub fn synthesize_z(registry: &mut MaterialRegistry, z: u32) -> Result<(), LabError> {
        let e = PeriodicTable::get(z).ok_or(LabError::UnknownElement)?;
        let atom = Atom::new(e.z, e.neutrons, e.z);
        Self::synthesize(registry, e.name, &atom)
    }

    /// Synthesize an element by symbol from the periodic table.
    pub fn synthesize_symbol(
        registry: &mut MaterialRegistry,
        symbol: &str,
    ) -> Result<(), LabError> {
        let e = PeriodicTable::by_symbol(symbol).ok_or(LabError::UnknownElement)?;
        let atom = Atom::new(e.z, e.neutrons, e.z);
        Self::synthesize(registry, e.name, &atom)
    }
}

fn hue_to_rgb(hue: f32) -> Vec3 {
    let h = hue / 60.0;
    let x = 1.0 - ((h % 2.0) - 1.0).abs();
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    Vec3::new(r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carbon_is_stable() {
        let c = Atom::new(6, 6, 6);
        assert!(c.is_stable());
        assert!(c.binding_per_nucleon() > 0.0);
    }

    #[test]
    fn unbalanced_is_unstable() {
        let bad = Atom::new(6, 6, 5);
        assert!(!bad.is_stable());
    }

    #[test]
    fn hand_checked_values() {
        let mut r = MaterialRegistry::new();
        let c = Atom::new(6, 6, 6);
        MaterialsLab::synthesize(&mut r, "carbon", &c).unwrap();
        let m = r.get("carbon").unwrap();
        assert!(m.metallic() > 0.0);
        assert!(m.roughness() < 1.0);
    }

    #[test]
    fn unstable_rejected() {
        let mut r = MaterialRegistry::new();
        let bad = Atom::new(1, 10, 1);
        assert!(matches!(
            MaterialsLab::synthesize(&mut r, "bad", &bad),
            Err(LabError::Unstable)
        ));
    }

    #[test]
    fn from_table() {
        let mut r = MaterialRegistry::new();
        MaterialsLab::synthesize_symbol(&mut r, "C").unwrap();
        assert!(r.get("Carbon").is_ok());
        MaterialsLab::synthesize_z(&mut r, 79).unwrap();
        assert!(r.get("Gold").is_ok());
    }
}
