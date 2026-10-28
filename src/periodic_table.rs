//! Safe pure PeriodicTable (element lookup core).
//!
//! Maps atomic number to name, symbol, and a typical neutron
//! count for the most common isotope. No unwraps.
//!
//! Differential: Z=6 returns carbon with 6 neutrons.

#[derive(Clone, Copy, Debug)]
pub struct Element {
    pub z: u32,
    pub symbol: &'static str,
    pub name: &'static str,
    pub neutrons: u32,
}

pub struct PeriodicTable;

impl PeriodicTable {
    /// Look up an element by atomic number. Returns None if not in the table.
    pub fn get(z: u32) -> Option<Element> {
        ELEMENTS.iter().find(|e| e.z == z).map(|e| Element {
            z: e.z,
            symbol: e.symbol,
            name: e.name,
            neutrons: e.neutrons,
        })
    }

    /// Look up by symbol (case-sensitive).
    pub fn by_symbol(sym: &str) -> Option<Element> {
        ELEMENTS.iter().find(|e| e.symbol == sym).map(|e| Element {
            z: e.z,
            symbol: e.symbol,
            name: e.name,
            neutrons: e.neutrons,
        })
    }

    pub fn len() -> usize {
        ELEMENTS.len()
    }
}

const ELEMENTS: &[Element] = &[
    Element {
        z: 1,
        symbol: "H",
        name: "Hydrogen",
        neutrons: 0,
    },
    Element {
        z: 2,
        symbol: "He",
        name: "Helium",
        neutrons: 2,
    },
    Element {
        z: 3,
        symbol: "Li",
        name: "Lithium",
        neutrons: 4,
    },
    Element {
        z: 4,
        symbol: "Be",
        name: "Beryllium",
        neutrons: 5,
    },
    Element {
        z: 5,
        symbol: "B",
        name: "Boron",
        neutrons: 6,
    },
    Element {
        z: 6,
        symbol: "C",
        name: "Carbon",
        neutrons: 6,
    },
    Element {
        z: 7,
        symbol: "N",
        name: "Nitrogen",
        neutrons: 7,
    },
    Element {
        z: 8,
        symbol: "O",
        name: "Oxygen",
        neutrons: 8,
    },
    Element {
        z: 9,
        symbol: "F",
        name: "Fluorine",
        neutrons: 10,
    },
    Element {
        z: 10,
        symbol: "Ne",
        name: "Neon",
        neutrons: 10,
    },
    Element {
        z: 11,
        symbol: "Na",
        name: "Sodium",
        neutrons: 12,
    },
    Element {
        z: 12,
        symbol: "Mg",
        name: "Magnesium",
        neutrons: 12,
    },
    Element {
        z: 13,
        symbol: "Al",
        name: "Aluminum",
        neutrons: 14,
    },
    Element {
        z: 14,
        symbol: "Si",
        name: "Silicon",
        neutrons: 14,
    },
    Element {
        z: 15,
        symbol: "P",
        name: "Phosphorus",
        neutrons: 16,
    },
    Element {
        z: 16,
        symbol: "S",
        name: "Sulfur",
        neutrons: 16,
    },
    Element {
        z: 17,
        symbol: "Cl",
        name: "Chlorine",
        neutrons: 18,
    },
    Element {
        z: 18,
        symbol: "Ar",
        name: "Argon",
        neutrons: 22,
    },
    Element {
        z: 19,
        symbol: "K",
        name: "Potassium",
        neutrons: 20,
    },
    Element {
        z: 20,
        symbol: "Ca",
        name: "Calcium",
        neutrons: 20,
    },
    Element {
        z: 21,
        symbol: "Sc",
        name: "Scandium",
        neutrons: 24,
    },
    Element {
        z: 22,
        symbol: "Ti",
        name: "Titanium",
        neutrons: 26,
    },
    Element {
        z: 23,
        symbol: "V",
        name: "Vanadium",
        neutrons: 28,
    },
    Element {
        z: 24,
        symbol: "Cr",
        name: "Chromium",
        neutrons: 28,
    },
    Element {
        z: 25,
        symbol: "Mn",
        name: "Manganese",
        neutrons: 30,
    },
    Element {
        z: 26,
        symbol: "Fe",
        name: "Iron",
        neutrons: 30,
    },
    Element {
        z: 27,
        symbol: "Co",
        name: "Cobalt",
        neutrons: 32,
    },
    Element {
        z: 28,
        symbol: "Ni",
        name: "Nickel",
        neutrons: 31,
    },
    Element {
        z: 29,
        symbol: "Cu",
        name: "Copper",
        neutrons: 35,
    },
    Element {
        z: 30,
        symbol: "Zn",
        name: "Zinc",
        neutrons: 35,
    },
    Element {
        z: 31,
        symbol: "Ga",
        name: "Gallium",
        neutrons: 39,
    },
    Element {
        z: 32,
        symbol: "Ge",
        name: "Germanium",
        neutrons: 41,
    },
    Element {
        z: 33,
        symbol: "As",
        name: "Arsenic",
        neutrons: 42,
    },
    Element {
        z: 34,
        symbol: "Se",
        name: "Selenium",
        neutrons: 45,
    },
    Element {
        z: 35,
        symbol: "Br",
        name: "Bromine",
        neutrons: 45,
    },
    Element {
        z: 36,
        symbol: "Kr",
        name: "Krypton",
        neutrons: 48,
    },
    Element {
        z: 37,
        symbol: "Rb",
        name: "Rubidium",
        neutrons: 48,
    },
    Element {
        z: 38,
        symbol: "Sr",
        name: "Strontium",
        neutrons: 50,
    },
    Element {
        z: 39,
        symbol: "Y",
        name: "Yttrium",
        neutrons: 50,
    },
    Element {
        z: 40,
        symbol: "Zr",
        name: "Zirconium",
        neutrons: 51,
    },
    Element {
        z: 41,
        symbol: "Nb",
        name: "Niobium",
        neutrons: 52,
    },
    Element {
        z: 42,
        symbol: "Mo",
        name: "Molybdenum",
        neutrons: 54,
    },
    Element {
        z: 43,
        symbol: "Tc",
        name: "Technetium",
        neutrons: 55,
    },
    Element {
        z: 44,
        symbol: "Ru",
        name: "Ruthenium",
        neutrons: 57,
    },
    Element {
        z: 45,
        symbol: "Rh",
        name: "Rhodium",
        neutrons: 58,
    },
    Element {
        z: 46,
        symbol: "Pd",
        name: "Palladium",
        neutrons: 60,
    },
    Element {
        z: 47,
        symbol: "Ag",
        name: "Silver",
        neutrons: 61,
    },
    Element {
        z: 48,
        symbol: "Cd",
        name: "Cadmium",
        neutrons: 64,
    },
    Element {
        z: 49,
        symbol: "In",
        name: "Indium",
        neutrons: 66,
    },
    Element {
        z: 50,
        symbol: "Sn",
        name: "Tin",
        neutrons: 69,
    },
    Element {
        z: 51,
        symbol: "Sb",
        name: "Antimony",
        neutrons: 71,
    },
    Element {
        z: 52,
        symbol: "Te",
        name: "Tellurium",
        neutrons: 76,
    },
    Element {
        z: 53,
        symbol: "I",
        name: "Iodine",
        neutrons: 74,
    },
    Element {
        z: 54,
        symbol: "Xe",
        name: "Xenon",
        neutrons: 77,
    },
    Element {
        z: 55,
        symbol: "Cs",
        name: "Cesium",
        neutrons: 78,
    },
    Element {
        z: 56,
        symbol: "Ba",
        name: "Barium",
        neutrons: 81,
    },
    Element {
        z: 57,
        symbol: "La",
        name: "Lanthanum",
        neutrons: 82,
    },
    Element {
        z: 58,
        symbol: "Ce",
        name: "Cerium",
        neutrons: 82,
    },
    Element {
        z: 59,
        symbol: "Pr",
        name: "Praseodymium",
        neutrons: 82,
    },
    Element {
        z: 60,
        symbol: "Nd",
        name: "Neodymium",
        neutrons: 84,
    },
    Element {
        z: 61,
        symbol: "Pm",
        name: "Promethium",
        neutrons: 84,
    },
    Element {
        z: 62,
        symbol: "Sm",
        name: "Samarium",
        neutrons: 90,
    },
    Element {
        z: 63,
        symbol: "Eu",
        name: "Europium",
        neutrons: 89,
    },
    Element {
        z: 64,
        symbol: "Gd",
        name: "Gadolinium",
        neutrons: 93,
    },
    Element {
        z: 65,
        symbol: "Tb",
        name: "Terbium",
        neutrons: 94,
    },
    Element {
        z: 66,
        symbol: "Dy",
        name: "Dysprosium",
        neutrons: 97,
    },
    Element {
        z: 67,
        symbol: "Ho",
        name: "Holmium",
        neutrons: 98,
    },
    Element {
        z: 68,
        symbol: "Er",
        name: "Erbium",
        neutrons: 99,
    },
    Element {
        z: 69,
        symbol: "Tm",
        name: "Thulium",
        neutrons: 100,
    },
    Element {
        z: 70,
        symbol: "Yb",
        name: "Ytterbium",
        neutrons: 103,
    },
    Element {
        z: 71,
        symbol: "Lu",
        name: "Lutetium",
        neutrons: 104,
    },
    Element {
        z: 72,
        symbol: "Hf",
        name: "Hafnium",
        neutrons: 106,
    },
    Element {
        z: 73,
        symbol: "Ta",
        name: "Tantalum",
        neutrons: 108,
    },
    Element {
        z: 74,
        symbol: "W",
        name: "Tungsten",
        neutrons: 110,
    },
    Element {
        z: 75,
        symbol: "Re",
        name: "Rhenium",
        neutrons: 111,
    },
    Element {
        z: 76,
        symbol: "Os",
        name: "Osmium",
        neutrons: 114,
    },
    Element {
        z: 77,
        symbol: "Ir",
        name: "Iridium",
        neutrons: 115,
    },
    Element {
        z: 78,
        symbol: "Pt",
        name: "Platinum",
        neutrons: 117,
    },
    Element {
        z: 79,
        symbol: "Au",
        name: "Gold",
        neutrons: 118,
    },
    Element {
        z: 80,
        symbol: "Hg",
        name: "Mercury",
        neutrons: 121,
    },
    Element {
        z: 81,
        symbol: "Tl",
        name: "Thallium",
        neutrons: 124,
    },
    Element {
        z: 82,
        symbol: "Pb",
        name: "Lead",
        neutrons: 126,
    },
    Element {
        z: 83,
        symbol: "Bi",
        name: "Bismuth",
        neutrons: 126,
    },
    Element {
        z: 84,
        symbol: "Po",
        name: "Polonium",
        neutrons: 125,
    },
    Element {
        z: 85,
        symbol: "At",
        name: "Astatine",
        neutrons: 125,
    },
    Element {
        z: 86,
        symbol: "Rn",
        name: "Radon",
        neutrons: 136,
    },
    Element {
        z: 87,
        symbol: "Fr",
        name: "Francium",
        neutrons: 136,
    },
    Element {
        z: 88,
        symbol: "Ra",
        name: "Radium",
        neutrons: 138,
    },
    Element {
        z: 89,
        symbol: "Ac",
        name: "Actinium",
        neutrons: 138,
    },
    Element {
        z: 90,
        symbol: "Th",
        name: "Thorium",
        neutrons: 142,
    },
    Element {
        z: 91,
        symbol: "Pa",
        name: "Protactinium",
        neutrons: 140,
    },
    Element {
        z: 92,
        symbol: "U",
        name: "Uranium",
        neutrons: 146,
    },
    Element {
        z: 93,
        symbol: "Np",
        name: "Neptunium",
        neutrons: 144,
    },
    Element {
        z: 94,
        symbol: "Pu",
        name: "Plutonium",
        neutrons: 150,
    },
    Element {
        z: 95,
        symbol: "Am",
        name: "Americium",
        neutrons: 148,
    },
    Element {
        z: 96,
        symbol: "Cm",
        name: "Curium",
        neutrons: 151,
    },
    Element {
        z: 97,
        symbol: "Bk",
        name: "Berkelium",
        neutrons: 150,
    },
    Element {
        z: 98,
        symbol: "Cf",
        name: "Californium",
        neutrons: 153,
    },
    Element {
        z: 99,
        symbol: "Es",
        name: "Einsteinium",
        neutrons: 153,
    },
    Element {
        z: 100,
        symbol: "Fm",
        name: "Fermium",
        neutrons: 157,
    },
    Element {
        z: 101,
        symbol: "Md",
        name: "Mendelevium",
        neutrons: 157,
    },
    Element {
        z: 102,
        symbol: "No",
        name: "Nobelium",
        neutrons: 157,
    },
    Element {
        z: 103,
        symbol: "Lr",
        name: "Lawrencium",
        neutrons: 159,
    },
    Element {
        z: 104,
        symbol: "Rf",
        name: "Rutherfordium",
        neutrons: 157,
    },
    Element {
        z: 105,
        symbol: "Db",
        name: "Dubnium",
        neutrons: 157,
    },
    Element {
        z: 106,
        symbol: "Sg",
        name: "Seaborgium",
        neutrons: 157,
    },
    Element {
        z: 107,
        symbol: "Bh",
        name: "Bohrium",
        neutrons: 155,
    },
    Element {
        z: 108,
        symbol: "Hs",
        name: "Hassium",
        neutrons: 157,
    },
    Element {
        z: 109,
        symbol: "Mt",
        name: "Meitnerium",
        neutrons: 159,
    },
    Element {
        z: 110,
        symbol: "Ds",
        name: "Darmstadtium",
        neutrons: 159,
    },
    Element {
        z: 111,
        symbol: "Rg",
        name: "Roentgenium",
        neutrons: 161,
    },
    Element {
        z: 112,
        symbol: "Cn",
        name: "Copernicium",
        neutrons: 165,
    },
    Element {
        z: 113,
        symbol: "Nh",
        name: "Nihonium",
        neutrons: 170,
    },
    Element {
        z: 114,
        symbol: "Fl",
        name: "Flerovium",
        neutrons: 175,
    },
    Element {
        z: 115,
        symbol: "Mc",
        name: "Moscovium",
        neutrons: 173,
    },
    Element {
        z: 116,
        symbol: "Lv",
        name: "Livermorium",
        neutrons: 177,
    },
    Element {
        z: 117,
        symbol: "Ts",
        name: "Tennessine",
        neutrons: 177,
    },
    Element {
        z: 118,
        symbol: "Og",
        name: "Oganesson",
        neutrons: 176,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carbon() {
        let e = PeriodicTable::get(6).unwrap();
        assert_eq!(e.name, "Carbon");
        assert_eq!(e.neutrons, 6);
    }

    #[test]
    fn by_symbol() {
        let e = PeriodicTable::by_symbol("Au").unwrap();
        assert_eq!(e.z, 79);
        assert_eq!(e.name, "Gold");
    }

    #[test]
    fn hand_checked_values() {
        let e = PeriodicTable::get(1).unwrap();
        assert_eq!(e.symbol, "H");
        assert_eq!(e.neutrons, 0);
        assert!(PeriodicTable::get(200).is_none());
        assert_eq!(PeriodicTable::len(), 118);
        assert_eq!(PeriodicTable::get(118).unwrap().symbol, "Og");
    }
}
