//! Chemistry: thermochemistry and kinetics of real reactions.
//!
//! **Data.** Standard-state (298.15 K, 1 bar) enthalpy of formation ΔHf°,
//! entropy S° and heat capacity Cp° per species. The main sources are the
//! CODATA Key Values for Thermodynamics (Cox, Wagman & Medvedev, 1989), which
//! cover H₂O, CO₂, NH₃, HCl and the elements; the NIST Chemistry WebBook
//! (NIST-JANAF tables) for gases and simple solids; and the CRC Handbook of
//! Chemistry and Physics (5-4, "Standard thermodynamic properties of chemical
//! substances") for the rest. See each row's `source`.
//!
//! **What is exact and what is approximate.**
//! * Balancing (rational null space) is exact. So are ΔH°, ΔS° and ΔG° at
//!   298.15 K, to the precision of the tabulated data.
//! * Away from 298 K, gases with known vibrational frequencies use the
//!   rigid-rotor / harmonic-oscillator Cp(T) (statistical mechanics, exact
//!   for an ideal harmonic molecule; it ignores anharmonicity, centrifugal
//!   stretching and electronic states, so it is ~1–3 % low near 3000 K).
//!   Condensed phases keep their 298 K Cp (Kirchhoff with constant Cp).
//!   H(T) and S(T) are the analytic integrals of that Cp.
//! * Flame temperatures ignore dissociation (CO₂ → CO + ½O₂, H₂O → H + OH
//!   …), so they come out ~100 K above measured adiabatic flames.
//! * The stable phase of a species with several entries (H₂O l/g,
//!   C graphite/diamond) is the one with the lowest G(T) = H(T) − T·S(T).
//! * K = exp(−ΔG/RT) uses gas activities p/p° with p° = 1 bar; condensed
//!   phases have activity 1. Ideal gases throughout.
//! * Kinetics: `k = A·exp(−Ea/RT)`. Published global rate laws are used
//!   where they exist: Westbrook & Dryer (1981) single-step
//!   hydrocarbon/alcohol oxidation, Marinov et al. (1996) for H₂, and the
//!   classic first-order N₂O₅ decomposition. Their concentration exponents
//!   are empirical fits, valid near flame conditions. Elementary
//!   equilibria such as N₂O₄ ⇌ 2NO₂ use mass action. Reactions without data
//!   get a generic Ea = 100 kJ/mol and A = 1e10 s⁻¹, flagged `estimated`.
//! * The net rate is `r_f·(1 − Q/K)`, so every reactor relaxes to the
//!   thermodynamic equilibrium even when the forward law is empirical.

use crate::element_data;
use crate::material_pbr::parse_formula;

/// J/(mol·K), CODATA 2018 (exact since the 2019 SI redefinition).
pub const R: f64 = 8.314_462_618;
pub const T0: f64 = 298.15;
/// Standard pressure, Pa.
pub const P0: f64 = 100_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    Gas,
    Liquid,
    Solid,
    Aqueous,
}

impl State {
    pub fn tag(self) -> &'static str {
        match self {
            State::Gas => "g",
            State::Liquid => "l",
            State::Solid => "s",
            State::Aqueous => "aq",
        }
    }
    fn parse(t: &str) -> Option<Self> {
        Some(match t {
            "g" => State::Gas,
            "l" => State::Liquid,
            "s" | "cr" => State::Solid,
            "aq" => State::Aqueous,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Species {
    pub formula: &'static str,
    pub name: &'static str,
    pub state: State,
    /// kJ/mol
    pub dhf: f64,
    /// J/(mol·K)
    pub s: f64,
    /// J/(mol·K)
    pub cp: f64,
    pub source: &'static str,
}

/// Second radiation constant hc/k, m·K (CODATA 2018, exact).
pub const C2: f64 = 1.438_776_877e-2;

/// Fundamental vibrational wavenumbers (cm⁻¹, degeneracy) and linearity of
/// the gas-phase molecules, from NIST CCCBDB / Herzberg. Used for the
/// rigid-rotor + harmonic-oscillator heat capacity
/// `Cp = Cp_rigid + Σ g·R·x²eˣ/(eˣ−1)²`, `x = hcν̃/kT`, with
/// `Cp_rigid = 7/2 R` (linear) or `4 R` (non-linear).
pub fn vib_modes(formula: &str) -> Option<(bool, &'static [(f64, u32)])> {
    Some(match formula {
        "H2" => (true, &[(4401.2, 1)]),
        "O2" => (true, &[(1580.2, 1)]),
        "N2" => (true, &[(2358.6, 1)]),
        "Cl2" => (true, &[(559.7, 1)]),
        "CO" => (true, &[(2169.8, 1)]),
        "NO" => (true, &[(1904.2, 1)]),
        "HCl" => (true, &[(2990.9, 1)]),
        "CO2" => (true, &[(1333.0, 1), (667.4, 2), (2349.2, 1)]),
        "C2H2" => (
            true,
            &[
                (3374.0, 1),
                (1974.0, 1),
                (3289.0, 1),
                (612.0, 2),
                (730.0, 2),
            ],
        ),
        "H2O" => (false, &[(3657.0, 1), (1594.7, 1), (3755.9, 1)]),
        "NO2" => (false, &[(1318.0, 1), (749.7, 1), (1617.8, 1)]),
        "SO2" => (false, &[(1151.4, 1), (517.7, 1), (1361.8, 1)]),
        "NH3" => (false, &[(3337.0, 1), (950.0, 1), (3444.0, 2), (1627.0, 2)]),
        "CH4" => (false, &[(2917.0, 1), (1534.0, 2), (3019.0, 3), (1306.0, 3)]),
        _ => return None,
    })
}

fn rrho(formula: &str) -> Option<(f64, Vec<(f64, f64)>)> {
    let (linear, modes) = vib_modes(formula)?;
    let rigid = if linear { 3.5 * R } else { 4.0 * R };
    Some((
        rigid,
        modes
            .iter()
            .map(|(nu, g)| (C2 * nu * 100.0, *g as f64))
            .collect(),
    ))
}

fn einstein_h(theta: f64, t: f64) -> f64 {
    R * theta / ((theta / t).exp() - 1.0)
}
fn einstein_s(theta: f64, t: f64) -> f64 {
    let x = theta / t;
    R * (x / (x.exp() - 1.0) - (-(-x).exp()).ln_1p())
}
fn einstein_cp(theta: f64, t: f64) -> f64 {
    let x = theta / t;
    let e = x.exp();
    R * x * x * e / ((e - 1.0) * (e - 1.0))
}

impl Species {
    /// Heat capacity at T, J/(mol·K): RRHO model for gases with known
    /// vibrations, the tabulated 298 K value otherwise.
    pub fn cp_at(&self, t: f64) -> f64 {
        let t = t.max(1.0);
        match (self.state, rrho(self.formula)) {
            (State::Gas, Some((rigid, modes))) => {
                rigid
                    + modes
                        .iter()
                        .map(|(th, g)| g * einstein_cp(*th, t))
                        .sum::<f64>()
            }
            _ => self.cp,
        }
    }
    /// Molar enthalpy relative to the elements at 298.15 K, J/mol.
    pub fn h(&self, t: f64) -> f64 {
        let t = t.max(1.0);
        match (self.state, rrho(self.formula)) {
            (State::Gas, Some((rigid, modes))) => {
                self.dhf * 1e3
                    + rigid * (t - T0)
                    + modes
                        .iter()
                        .map(|(th, g)| g * (einstein_h(*th, t) - einstein_h(*th, T0)))
                        .sum::<f64>()
            }
            _ => self.dhf * 1e3 + self.cp * (t - T0),
        }
    }
    /// Molar entropy, J/(mol·K).
    pub fn s_at(&self, t: f64) -> f64 {
        let t = t.max(1.0);
        match (self.state, rrho(self.formula)) {
            (State::Gas, Some((rigid, modes))) => {
                self.s
                    + rigid * (t / T0).ln()
                    + modes
                        .iter()
                        .map(|(th, g)| g * (einstein_s(*th, t) - einstein_s(*th, T0)))
                        .sum::<f64>()
            }
            _ => self.s + self.cp * (t / T0).ln(),
        }
    }
    /// Molar Gibbs energy H − T·S, J/mol.
    pub fn g(&self, t: f64) -> f64 {
        self.h(t) - t * self.s_at(t)
    }
    pub fn label(&self) -> String {
        format!("{}({})", self.formula, self.state.tag())
    }
    pub fn molar_mass(&self) -> f64 {
        parse_formula(self.formula)
            .map(|c| c.molar_mass() as f64)
            .unwrap_or(0.0)
    }
}

const CODATA: &str = "CODATA Key Values 1989";
const JANAF: &str = "NIST-JANAF / WebBook";
const CRC: &str = "CRC Handbook 5-4";

macro_rules! sp {
    ($f:expr, $n:expr, $st:ident, $h:expr, $s:expr, $cp:expr, $src:expr) => {
        Species {
            formula: $f,
            name: $n,
            state: State::$st,
            dhf: $h,
            s: $s,
            cp: $cp,
            source: $src,
        }
    };
}

/// The species table (298.15 K, 1 bar).
pub static SPECIES: &[Species] = &[
    sp!("H2", "hydrogen", Gas, 0.0, 130.680, 28.836, CODATA),
    sp!("O2", "oxygen", Gas, 0.0, 205.152, 29.378, CODATA),
    sp!("N2", "nitrogen", Gas, 0.0, 191.609, 29.124, CODATA),
    sp!("Cl2", "chlorine", Gas, 0.0, 223.081, 33.949, CODATA),
    sp!("H2O", "water", Liquid, -285.830, 69.95, 75.351, CODATA),
    sp!("H2O", "steam", Gas, -241.826, 188.835, 33.609, CODATA),
    sp!(
        "H2O2",
        "hydrogen peroxide",
        Liquid,
        -187.78,
        109.6,
        89.1,
        CRC
    ),
    sp!(
        "CO2",
        "carbon dioxide",
        Gas,
        -393.51,
        213.785,
        37.135,
        CODATA
    ),
    sp!(
        "CO",
        "carbon monoxide",
        Gas,
        -110.53,
        197.660,
        29.141,
        CODATA
    ),
    sp!("C", "graphite", Solid, 0.0, 5.74, 8.517, CODATA),
    sp!("C", "diamond", Solid, 1.895, 2.377, 6.113, CRC),
    sp!("CH4", "methane", Gas, -74.87, 186.25, 35.69, JANAF),
    sp!("C2H6", "ethane", Gas, -84.0, 229.2, 52.49, CRC),
    sp!("C3H8", "propane", Gas, -104.7, 270.3, 73.6, CRC),
    sp!("C2H2", "acetylene", Gas, 227.4, 200.9, 44.0, CRC),
    sp!("CH3OH", "methanol", Liquid, -239.2, 126.8, 81.1, CRC),
    sp!("C2H5OH", "ethanol", Liquid, -277.6, 160.7, 112.3, CRC),
    sp!("C6H12O6", "glucose", Solid, -1273.3, 209.2, 218.2, CRC),
    sp!("NH3", "ammonia", Gas, -45.94, 192.77, 35.06, CODATA),
    sp!("NO", "nitric oxide", Gas, 91.29, 210.76, 29.86, JANAF),
    sp!("NO2", "nitrogen dioxide", Gas, 33.10, 240.04, 37.2, JANAF),
    sp!(
        "N2O4",
        "dinitrogen tetroxide",
        Gas,
        9.16,
        304.29,
        77.28,
        CRC
    ),
    sp!("N2O5", "dinitrogen pentoxide", Gas, 13.3, 355.7, 95.3, CRC),
    sp!(
        "HCl",
        "hydrogen chloride",
        Gas,
        -92.31,
        186.902,
        29.136,
        CODATA
    ),
    sp!("Na", "sodium", Solid, 0.0, 51.30, 28.23, CODATA),
    sp!("NaCl", "sodium chloride", Solid, -411.12, 72.11, 50.50, CRC),
    sp!("Fe", "iron", Solid, 0.0, 27.32, 25.10, CODATA),
    sp!("Fe2O3", "haematite", Solid, -824.2, 87.40, 103.85, CRC),
    sp!("Al", "aluminium", Solid, 0.0, 28.30, 24.20, CODATA),
    sp!("Al2O3", "corundum", Solid, -1675.7, 50.92, 79.04, CRC),
    sp!("Cu", "copper", Solid, 0.0, 33.15, 24.44, CODATA),
    sp!("CuO", "tenorite", Solid, -157.3, 42.63, 42.30, CRC),
    sp!("Mg", "magnesium", Solid, 0.0, 32.67, 24.87, CODATA),
    sp!("MgO", "periclase", Solid, -601.6, 26.95, 37.2, CRC),
    sp!("S", "sulfur (rhombic)", Solid, 0.0, 32.054, 22.64, CODATA),
    sp!(
        "SO2",
        "sulfur dioxide",
        Gas,
        -296.81,
        248.223,
        39.87,
        CODATA
    ),
    sp!("CaCO3", "calcite", Solid, -1207.6, 91.7, 83.5, CRC),
    sp!("CaO", "lime", Solid, -634.92, 38.1, 42.0, CRC),
    sp!("Si", "silicon", Solid, 0.0, 18.81, 20.00, CODATA),
    sp!("SiO2", "quartz", Solid, -910.7, 41.46, 44.43, CRC),
];

/// Every table entry for a formula (several when it has several phases).
pub fn entries(formula: &str) -> Vec<&'static Species> {
    SPECIES.iter().filter(|s| s.formula == formula).collect()
}

/// The stable entry at temperature `t` (lowest G), or the tagged one.
pub fn species(formula: &str, state: Option<State>, t: f64) -> Option<&'static Species> {
    let all = entries(formula);
    if let Some(st) = state {
        return all.into_iter().find(|s| s.state == st);
    }
    all.into_iter()
        .min_by(|a, b| a.g(t).partial_cmp(&b.g(t)).unwrap())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Term {
    pub coef: f64,
    pub formula: String,
    pub state: Option<State>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Equation {
    pub reactants: Vec<Term>,
    pub products: Vec<Term>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChemError {
    Syntax(String),
    Formula(String),
    Unbalanced(String),
    Unknown(String),
}

impl std::fmt::Display for ChemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChemError::Syntax(s) => write!(f, "syntax: {}", s),
            ChemError::Formula(s) => write!(f, "formula: {}", s),
            ChemError::Unbalanced(s) => write!(f, "cannot balance: {}", s),
            ChemError::Unknown(s) => write!(f, "no thermochemical data for {}", s),
        }
    }
}

fn parse_side(side: &str) -> Result<Vec<Term>, ChemError> {
    let mut out = Vec::new();
    for raw in side.split(" + ").flat_map(|p| p.split('+')) {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        // optional leading coefficient (integer or decimal or n/m)
        let split = t
            .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '/' || c == ' '))
            .unwrap_or(t.len());
        let (num, rest) = t.split_at(split);
        let num = num.trim();
        let coef = if num.is_empty() {
            f64::NAN
        } else if let Some((a, b)) = num.split_once('/') {
            a.trim()
                .parse::<f64>()
                .map_err(|_| ChemError::Syntax(t.into()))?
                / b.trim()
                    .parse::<f64>()
                    .map_err(|_| ChemError::Syntax(t.into()))?
        } else {
            num.parse::<f64>()
                .map_err(|_| ChemError::Syntax(t.into()))?
        };
        let mut f = rest.trim().to_string();
        let mut state = None;
        if let Some(open) = f.rfind('(') {
            if let Some(st) = f[open + 1..].strip_suffix(')').and_then(State::parse) {
                state = Some(st);
                f.truncate(open);
            }
        }
        let f = f.trim().to_string();
        if f.is_empty() {
            return Err(ChemError::Syntax(format!("missing formula in '{}'", t)));
        }
        parse_formula(&f).map_err(|e| ChemError::Formula(format!("{}: {:?}", f, e)))?;
        out.push(Term {
            coef,
            formula: f,
            state,
        });
    }
    if out.is_empty() {
        return Err(ChemError::Syntax("empty side".into()));
    }
    Ok(out)
}

/// Parse "CH4 + 2 O2 -> CO2 + 2 H2O(l)" (also `=`, `→`, `⇌`, `<=>`).
/// Without coefficients the equation is balanced automatically.
pub fn parse_equation(src: &str) -> Result<Equation, ChemError> {
    let s = src
        .replace(['→', '⇌'], "->")
        .replace("<=>", "->")
        .replace("<->", "->")
        .replace("=>", "->");
    let (l, r) = s
        .split_once("->")
        .or_else(|| s.split_once('='))
        .ok_or_else(|| ChemError::Syntax("use -> between reactants and products".into()))?;
    let mut eq = Equation {
        reactants: parse_side(l)?,
        products: parse_side(r)?,
    };
    let any_given = eq
        .reactants
        .iter()
        .chain(&eq.products)
        .any(|t| !t.coef.is_nan());
    if any_given {
        for t in eq.reactants.iter_mut().chain(eq.products.iter_mut()) {
            if t.coef.is_nan() {
                t.coef = 1.0;
            }
        }
        if let Some(e) = eq.imbalance() {
            return Err(ChemError::Unbalanced(e));
        }
    } else {
        eq.balance()?;
    }
    Ok(eq)
}

fn gcd(a: i128, b: i128) -> i128 {
    if b == 0 {
        a.abs()
    } else {
        gcd(b, a % b)
    }
}

impl Equation {
    fn elements(&self) -> Vec<u32> {
        let mut z: Vec<u32> = self
            .reactants
            .iter()
            .chain(&self.products)
            .flat_map(|t| {
                parse_formula(&t.formula)
                    .map(|c| c.0.iter().map(|(z, _)| *z).collect::<Vec<_>>())
                    .unwrap_or_default()
            })
            .collect();
        z.sort();
        z.dedup();
        z
    }

    fn count(formula: &str, z: u32) -> i128 {
        parse_formula(formula)
            .map(|c| {
                c.0.iter()
                    .filter(|(e, _)| *e == z)
                    .map(|(_, n)| *n as i128)
                    .sum()
            })
            .unwrap_or(0)
    }

    /// `Some(description)` when atoms are not conserved.
    pub fn imbalance(&self) -> Option<String> {
        let mut bad = Vec::new();
        for z in self.elements() {
            let l: f64 = self
                .reactants
                .iter()
                .map(|t| t.coef * Self::count(&t.formula, z) as f64)
                .sum();
            let r: f64 = self
                .products
                .iter()
                .map(|t| t.coef * Self::count(&t.formula, z) as f64)
                .sum();
            if (l - r).abs() > 1e-9 {
                let sym = element_data::get(z).map(|e| e.symbol).unwrap_or("?");
                bad.push(format!("{} {} vs {}", sym, l, r));
            }
        }
        (!bad.is_empty()).then(|| bad.join(", "))
    }

    /// Smallest positive integer coefficients conserving every element
    /// (one-dimensional null space of the element matrix, exact rationals).
    pub fn balance(&mut self) -> Result<(), ChemError> {
        let zs = self.elements();
        let terms: Vec<(String, i128)> = self
            .reactants
            .iter()
            .map(|t| (t.formula.clone(), 1))
            .chain(self.products.iter().map(|t| (t.formula.clone(), -1)))
            .collect();
        let n = terms.len();
        // rows: elements; rational matrix as (num, den)
        let mut m: Vec<Vec<(i128, i128)>> = zs
            .iter()
            .map(|&z| {
                terms
                    .iter()
                    .map(|(f, sgn)| (sgn * Self::count(f, z), 1))
                    .collect()
            })
            .collect();
        let norm = |(a, b): (i128, i128)| {
            let g = gcd(a, b).max(1);
            let (a, b) = (a / g, b / g);
            if b < 0 {
                (-a, -b)
            } else {
                (a, b)
            }
        };
        let sub = |x: (i128, i128), y: (i128, i128)| norm((x.0 * y.1 - y.0 * x.1, x.1 * y.1));
        let mul = |x: (i128, i128), y: (i128, i128)| norm((x.0 * y.0, x.1 * y.1));
        let div = |x: (i128, i128), y: (i128, i128)| norm((x.0 * y.1, x.1 * y.0));
        // reduced row echelon form
        let mut pivots = Vec::new();
        let mut row = 0;
        for col in 0..n {
            let Some(pr) = (row..m.len()).find(|&r| m[r][col].0 != 0) else {
                continue;
            };
            m.swap(row, pr);
            let pv = m[row][col];
            for v in m[row].iter_mut().take(n) {
                *v = div(*v, pv);
            }
            let pivot = m[row].clone();
            for (r, line) in m.iter_mut().enumerate() {
                if r != row && line[col].0 != 0 {
                    let f = line[col];
                    for (v, p) in line.iter_mut().zip(&pivot).take(n) {
                        *v = sub(*v, mul(f, *p));
                    }
                }
            }
            pivots.push(col);
            row += 1;
            if row == m.len() {
                break;
            }
        }
        let free: Vec<usize> = (0..n).filter(|c| !pivots.contains(c)).collect();
        if free.len() != 1 {
            return Err(ChemError::Unbalanced(if free.is_empty() {
                "no solution with these species".into()
            } else {
                "several independent reactions; give coefficients".into()
            }));
        }
        let fc = free[0];
        let mut x = vec![(0i128, 1i128); n];
        x[fc] = (1, 1);
        for (r, &pc) in pivots.iter().enumerate() {
            x[pc] = norm((-m[r][fc].0, m[r][fc].1));
        }
        let lcm = x.iter().fold(1i128, |l, (_, d)| l / gcd(l, *d) * d);
        let mut ints: Vec<i128> = x.iter().map(|(a, d)| a * (lcm / d)).collect();
        if ints.iter().all(|v| *v <= 0) {
            ints.iter_mut().for_each(|v| *v = -*v);
        }
        if ints.iter().any(|v| *v <= 0) {
            return Err(ChemError::Unbalanced(
                "a species would need a negative or zero coefficient".into(),
            ));
        }
        let g = ints.iter().fold(0, |g, v| gcd(g, *v));
        for (t, v) in self
            .reactants
            .iter_mut()
            .chain(self.products.iter_mut())
            .zip(ints)
        {
            t.coef = (v / g) as f64;
        }
        Ok(())
    }

    pub fn display(&self) -> String {
        let side = |ts: &Vec<Term>| {
            ts.iter()
                .map(|t| {
                    format!(
                        "{}{}{}",
                        if (t.coef - 1.0).abs() < 1e-12 {
                            String::new()
                        } else {
                            format!("{} ", fmt_coef(t.coef))
                        },
                        t.formula,
                        t.state
                            .map(|s| format!("({})", s.tag()))
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join(" + ")
        };
        format!("{} → {}", side(&self.reactants), side(&self.products))
    }
}

fn fmt_coef(c: f64) -> String {
    if (c - c.round()).abs() < 1e-9 {
        format!("{}", c.round() as i64)
    } else {
        format!("{}", c)
    }
}

/// Kinetic law for a reaction.
#[derive(Clone, Debug, PartialEq)]
pub struct RateLaw {
    /// Pre-exponential factor in (mol/cm³)^(1−Σorders)/s (Westbrook–Dryer
    /// convention), or s⁻¹ for first-order laws.
    pub a: f64,
    /// J/mol
    pub ea: f64,
    /// (formula, order) for the concentration terms.
    pub orders: Vec<(String, f64)>,
    /// rate is per mole of this species consumed (its coefficient normalises).
    pub per: String,
    pub source: &'static str,
    pub estimated: bool,
}

impl RateLaw {
    pub fn k(&self, t: f64) -> f64 {
        self.a * (-self.ea / (R * t)).exp()
    }
}

/// Published rate laws, keyed by fuel / reactant.
pub fn rate_law(eq: &Equation) -> RateLaw {
    let has = |f: &str| eq.reactants.iter().any(|t| t.formula == f);
    let cal = 4.184;
    let wd = |fuel: &str, a: f64, ea_kcal: f64, af: f64, bo: f64| RateLaw {
        a,
        ea: ea_kcal * 1000.0 * cal,
        orders: vec![(fuel.into(), af), ("O2".into(), bo)],
        per: fuel.into(),
        source: "Westbrook & Dryer 1981, Comb. Sci. Tech. 27:31, Table 2",
        estimated: false,
    };
    if has("O2") {
        if has("CH4") {
            return wd("CH4", 8.3e5, 30.0, -0.3, 1.3);
        }
        if has("C3H8") {
            return wd("C3H8", 8.6e11, 30.0, 0.1, 1.65);
        }
        if has("C2H6") {
            return wd("C2H6", 1.1e12, 30.0, 0.1, 1.65);
        }
        if has("CH3OH") {
            return wd("CH3OH", 3.2e12, 30.0, 0.25, 1.5);
        }
        if has("C2H5OH") {
            return wd("C2H5OH", 1.5e12, 30.0, 0.15, 1.6);
        }
        if has("H2") {
            return RateLaw {
                a: 1.8e13,
                ea: 35.0 * 1000.0 * cal,
                orders: vec![("H2".into(), 1.0), ("O2".into(), 0.5)],
                per: "H2".into(),
                source: "Marinov, Westbrook & Pitz 1996 (global H2/air)",
                estimated: false,
            };
        }
    }
    if has("N2O5") && eq.reactants.len() == 1 {
        return RateLaw {
            a: 4.94e13,
            ea: 103.4e3,
            orders: vec![("N2O5".into(), 1.0)],
            per: "N2O5".into(),
            source: "first-order N2O5 decomposition (Daniels & Johnston 1921; Ea 103 kJ/mol)",
            estimated: false,
        };
    }
    if has("N2O4") && eq.reactants.len() == 1 {
        // fast elementary dissociation; equilibrium is reached in microseconds
        return RateLaw {
            a: 1.0e16,
            ea: 57.0e3,
            orders: vec![("N2O4".into(), 1.0)],
            per: "N2O4".into(),
            source: "N2O4 dissociation, order-of-magnitude rate (equilibrium is what matters)",
            estimated: true,
        };
    }
    let first = eq.reactants[0].formula.clone();
    RateLaw {
        a: 1.0e10,
        ea: 100.0e3,
        orders: vec![(first.clone(), 1.0)],
        per: first,
        source: "no published rate law in the table; generic estimate",
        estimated: true,
    }
}

/// Thermochemistry of a balanced equation at temperature `t`.
#[derive(Clone, Debug, PartialEq)]
pub struct Thermo {
    pub t: f64,
    /// J/mol of reaction (as written)
    pub dh: f64,
    /// J/(mol·K)
    pub ds: f64,
    /// J/mol
    pub dg: f64,
    /// Equilibrium constant (gas activities p/1 bar)
    pub k: f64,
    /// Δn of gas moles
    pub dn_gas: f64,
    pub dcp: f64,
    /// The phase picked for each term, in order (reactants then products).
    pub picked: Vec<&'static Species>,
}

impl Thermo {
    pub fn exothermic(&self) -> bool {
        self.dh < 0.0
    }
    pub fn spontaneous(&self) -> bool {
        self.dg < 0.0
    }
}

/// Resolve each term to a table entry at `t`.
pub fn resolve(eq: &Equation, t: f64) -> Result<Vec<&'static Species>, ChemError> {
    eq.reactants
        .iter()
        .chain(&eq.products)
        .map(|term| {
            species(&term.formula, term.state, t).ok_or_else(|| {
                ChemError::Unknown(format!(
                    "{}{}",
                    term.formula,
                    term.state
                        .map(|s| format!("({})", s.tag()))
                        .unwrap_or_default()
                ))
            })
        })
        .collect()
}

pub fn thermo(eq: &Equation, t: f64) -> Result<Thermo, ChemError> {
    let picked = resolve(eq, t)?;
    let nr = eq.reactants.len();
    let (mut dh, mut ds, mut dn, mut dcp) = (0.0, 0.0, 0.0, 0.0);
    for (i, (term, sp)) in eq
        .reactants
        .iter()
        .chain(&eq.products)
        .zip(&picked)
        .enumerate()
    {
        let nu = if i < nr { -term.coef } else { term.coef };
        dh += nu * sp.h(t);
        ds += nu * sp.s_at(t);
        dcp += nu * sp.cp_at(t);
        if sp.state == State::Gas {
            dn += nu;
        }
    }
    let dg = dh - t * ds;
    Ok(Thermo {
        t,
        dh,
        ds,
        dg,
        k: (-dg / (R * t)).exp(),
        dn_gas: dn,
        dcp,
        picked,
    })
}

/// A well-mixed, constant-pressure, adiabatic reactor (or with a fixed
/// wall heat-transfer term). State: moles of every species and T.
#[derive(Clone, Debug)]
pub struct Reactor {
    pub eq: Equation,
    pub species: Vec<&'static Species>,
    /// mol, aligned with `species`
    pub n: Vec<f64>,
    /// signed stoichiometry ν (−reactant, +product), aligned with `species`
    pub nu: Vec<f64>,
    pub t: f64,
    pub p: f64,
    pub law: RateLaw,
    pub time: f64,
    /// extent of reaction, mol
    pub extent: f64,
    /// W/K to surroundings at `t_wall` (0 = adiabatic)
    pub ua: f64,
    pub t_wall: f64,
    /// J lost through the wall
    pub q_out: f64,
    /// Inert diluent (N₂ for "in air"), mol
    pub inert: f64,
}

impl Reactor {
    /// Stoichiometric charge of the reactants (`scale` mol of reaction),
    /// plus `inert` mol of N₂ (3.76 per O₂ for air).
    pub fn new(eq: &Equation, t: f64, p: f64, scale: f64, air: bool) -> Result<Self, ChemError> {
        let mut species: Vec<&'static Species> = Vec::new();
        let mut nu: Vec<f64> = Vec::new();
        let mut n: Vec<f64> = Vec::new();
        let picked = resolve(eq, t)?;
        let nr = eq.reactants.len();
        for (i, (term, sp)) in eq
            .reactants
            .iter()
            .chain(&eq.products)
            .zip(picked)
            .enumerate()
        {
            let v = if i < nr { -term.coef } else { term.coef };
            species.push(sp);
            nu.push(v);
            n.push(if i < nr { term.coef * scale } else { 0.0 });
        }
        let o2: f64 = eq
            .reactants
            .iter()
            .filter(|t| t.formula == "O2")
            .map(|t| t.coef * scale)
            .sum();
        let inert = if air { 3.76 * o2 } else { 0.0 };
        Ok(Self {
            eq: eq.clone(),
            species,
            n,
            nu,
            t,
            p,
            law: rate_law(eq),
            time: 0.0,
            extent: 0.0,
            ua: 0.0,
            t_wall: t,
            q_out: 0.0,
            inert,
        })
    }

    fn n2(&self) -> Option<&'static Species> {
        species("N2", Some(State::Gas), self.t)
    }

    /// Total enthalpy (J) of the mixture at its temperature.
    pub fn enthalpy(&self) -> f64 {
        let h: f64 = self
            .species
            .iter()
            .zip(&self.n)
            .map(|(s, n)| n * h_with_latent(s, self.t))
            .sum();
        h + self.inert * self.n2().map(|s| s.h(self.t)).unwrap_or(0.0)
    }

    pub fn heat_capacity(&self) -> f64 {
        let c: f64 = self
            .species
            .iter()
            .zip(&self.n)
            .map(|(s, n)| n * (s.cp_at(self.t) + latent(s, self.t).1))
            .sum();
        c + self.inert * self.n2().map(|s| s.cp_at(self.t)).unwrap_or(29.124)
    }

    pub fn gas_moles(&self) -> f64 {
        self.species
            .iter()
            .zip(&self.n)
            .filter(|(s, _)| s.state == State::Gas)
            .map(|(_, n)| *n)
            .sum::<f64>()
            + self.inert
    }

    /// Ideal-gas volume, m³.
    pub fn volume(&self) -> f64 {
        self.gas_moles().max(1e-12) * R * self.t / self.p
    }

    /// Reaction quotient Q with gas activities p_i/p°.
    pub fn quotient(&self) -> f64 {
        let ng = self.gas_moles().max(1e-300);
        let mut lnq = 0.0;
        for ((s, n), v) in self.species.iter().zip(&self.n).zip(&self.nu) {
            if s.state == State::Gas {
                let a = (n / ng * self.p / P0).max(1e-300);
                lnq += v * a.ln();
            }
        }
        lnq.exp()
    }

    /// Net rate of reaction, mol of reaction per second.
    pub fn rate(&self) -> f64 {
        let v = self.volume();
        let vcm3 = v * 1e6;
        let mut r = self.law.k(self.t);
        for (f, ord) in &self.law.orders {
            let i = self.species.iter().position(|s| s.formula == f.as_str());
            let n = i.map(|i| self.n[i]).unwrap_or(0.0);
            if n <= 0.0 {
                return 0.0;
            }
            // W–D laws use mol/cm³; first-order laws are unit-free
            r *= (n / vcm3).powf(*ord);
        }
        // mol/cm³/s × cm³ = mol/s of the reference species
        let mut rate = r * vcm3;
        let coef = self
            .eq
            .reactants
            .iter()
            .find(|t| t.formula == self.law.per)
            .map(|t| t.coef)
            .unwrap_or(1.0);
        rate /= coef;
        // detailed balance: the net rate vanishes at equilibrium
        let k_eq = thermo(&self.eq, self.t)
            .map(|th| th.k)
            .unwrap_or(f64::INFINITY);
        let q = self.quotient();
        rate * (1.0 - (q / k_eq).min(1e6))
    }

    /// Move the reaction by `dxi` mol over `h` s: moles change by ν·dξ and
    /// T follows from enthalpy conservation, with an implicit (backward
    /// Euler) wall loss so even a huge UA (isothermal wall) is stable.
    fn apply(&mut self, dxi: f64, h: f64) {
        let h_before = self.enthalpy();
        for (n, v) in self.n.iter_mut().zip(&self.nu) {
            *n = (*n + v * dxi).max(0.0);
        }
        self.extent += dxi;
        // adiabatic temperature first: H(T') = H_before (Newton on Cp(T),
        // bisection if Newton has not converged, e.g. at a plateau corner)
        let tol = 1e-9 * h_before.abs().max(1.0);
        for _ in 0..8 {
            let c = self.heat_capacity().max(1e-12);
            self.t = (self.t + (h_before - self.enthalpy()) / c).max(1.0);
        }
        if (self.enthalpy() - h_before).abs() > tol {
            let (mut lo, mut hi) = (1.0, 60_000.0);
            for _ in 0..100 {
                self.t = 0.5 * (lo + hi);
                if self.enthalpy() > h_before {
                    hi = self.t
                } else {
                    lo = self.t
                }
                if hi - lo < 1e-9 {
                    break;
                }
            }
        }
        if self.ua > 0.0 {
            let c = self.heat_capacity().max(1e-12);
            let f = self.ua * h / c;
            let t_new = (self.t + f * self.t_wall) / (1.0 + f);
            self.q_out += c * (self.t - t_new);
            self.t = t_new;
        }
        self.time += h;
    }

    /// Advance by `dt` seconds with adaptive sub-steps: each moves at most
    /// 2 % of the limiting species and ~5 K, and a step that would carry
    /// the mixture past equilibrium is cut back to it by bisection.
    pub fn advance(&mut self, dt: f64) {
        let mut left = dt;
        let mut guard = 0;
        while left > 1e-15 && guard < 100_000 {
            guard += 1;
            let r = self.rate();
            if r == 0.0 || !r.is_finite() {
                self.apply(0.0, left);
                break;
            }
            let limit = self
                .species
                .iter()
                .zip(&self.n)
                .zip(&self.nu)
                .filter(|((_, _), v)| (**v < 0.0) == (r > 0.0) && **v != 0.0)
                .map(|((_, n), v)| n / v.abs())
                .fold(f64::INFINITY, f64::min);
            // the limiting reactant is effectively gone: finish it in one
            // step (negative orders would otherwise make the steps Zeno-like)
            if limit.is_finite() && limit <= 1e-9 * (self.extent.abs() + limit) {
                self.apply(limit.copysign(r), left);
                break;
            }
            let mut h = left;
            if limit.is_finite() {
                h = h.min((0.02 * limit.max(1e-15) / r.abs()).max(1e-15));
            }
            if let Ok(th) = thermo(&self.eq, self.t) {
                let dtemp_rate = (th.dh * r).abs() / self.heat_capacity().max(1e-12);
                if dtemp_rate > 0.0 {
                    h = h.min((5.0 / dtemp_rate).max(1e-15));
                }
            }
            let dxi = (r * h).clamp(-limit, limit);
            let mut trial = self.clone();
            trial.apply(dxi, h);
            let r1 = trial.rate();
            if r1 * r < 0.0 {
                // overshot equilibrium: bisect the fraction of the step
                let (mut lo, mut hi) = (0.0, 1.0);
                for _ in 0..50 {
                    let mid = 0.5 * (lo + hi);
                    let mut t2 = self.clone();
                    t2.apply(dxi * mid, h * mid);
                    if t2.rate() * r > 0.0 {
                        lo = mid
                    } else {
                        hi = mid
                    }
                }
                let mut t2 = self.clone();
                t2.apply(dxi * lo, h * lo);
                trial = t2;
                let used = h * lo;
                *self = trial;
                left -= used;
                // the bisection brackets the sign change of the net rate, so
                // this is equilibrium: hold it for the rest of the step (only
                // the wall can move it; the next call re-evaluates)
                self.apply(0.0, left);
                break;
            }
            *self = trial;
            left -= h;
        }
    }

    /// Fraction of the limiting reactant consumed.
    pub fn conversion(&self) -> f64 {
        let mut best: f64 = 0.0;
        for ((_, n), v) in self.species.iter().zip(&self.n).zip(&self.nu) {
            if *v < 0.0 {
                let n0 = n + self.extent * v.abs();
                if n0 > 0.0 {
                    best = best.max(1.0 - n / n0);
                }
            }
        }
        best
    }
}

/// (formula, T_melt K, ΔH_fus kJ/mol, Some((T_boil K, ΔH_vap kJ/mol))).
pub type Transition = (&'static str, f64, f64, Option<(f64, f64)>);

/// Phase transitions of the condensed species: (formula, T_melt K,
/// ΔH_fus kJ/mol, T_boil K, ΔH_vap kJ/mol); CRC Handbook 6-139 / 4-
/// (enthalpies of fusion and vaporisation). `None` where the substance
/// decomposes or the value is not well defined.
pub static TRANSITIONS: &[Transition] = &[
    ("Fe", 1811.0, 13.81, Some((3134.0, 340.0))),
    ("Al", 933.47, 10.71, Some((2792.0, 294.0))),
    ("Cu", 1357.77, 13.26, Some((2835.0, 300.4))),
    ("Mg", 923.0, 8.48, Some((1363.0, 128.0))),
    ("Na", 370.94, 2.60, Some((1156.0, 97.42))),
    ("Al2O3", 2345.0, 111.1, None),
    ("MgO", 3098.0, 77.0, None),
    ("NaCl", 1073.8, 28.16, Some((1738.0, 170.7))),
    ("CaO", 2886.0, 79.0, None),
    ("SiO2", 1983.0, 9.6, None),
    ("CuO", 1599.0, 11.8, None),
    ("Si", 1687.0, 50.21, Some((3538.0, 359.0))),
];

/// Molar enthalpy including the latent heat of any melting / boiling passed
/// below `t` (condensed entries only), J/mol.
/// Width (K) over which a latent heat is spread around its transition
/// temperature, so H(T) stays continuous and monotonic and the reactor's
/// temperature solve conserves enthalpy exactly through a plateau.
pub const LATENT_WINDOW: f64 = 5.0;

/// Latent enthalpy (J/mol) absorbed by `s` at `t` and its T-derivative.
pub fn latent(s: &Species, t: f64) -> (f64, f64) {
    let mut h = 0.0;
    let mut dh = 0.0;
    if s.state == State::Solid {
        if let Some((_, tm, hf, boil)) = TRANSITIONS.iter().find(|x| x.0 == s.formula) {
            let mut ramp = |t0: f64, l: f64| {
                let f = ((t - (t0 - LATENT_WINDOW)) / (2.0 * LATENT_WINDOW)).clamp(0.0, 1.0);
                h += f * l * 1e3;
                if f > 0.0 && f < 1.0 {
                    dh += l * 1e3 / (2.0 * LATENT_WINDOW);
                }
            };
            ramp(*tm, *hf);
            if let Some((tb, hv)) = boil {
                ramp(*tb, *hv);
            }
        }
    }
    (h, dh)
}

/// Molar enthalpy including the latent heats of melting and boiling of a
/// condensed species above its transitions.
pub fn h_with_latent(s: &Species, t: f64) -> f64 {
    s.h(t) + latent(s, t).0
}

/// Adiabatic flame / reaction temperature at constant pressure for complete
/// conversion (no dissociation): solve H_products(T) = H_reactants(T₀),
/// passing through melting and boiling plateaus of condensed products. A
/// result sitting exactly on a transition temperature means the products
/// are partly melted / boiled there (thermite stops at iron's boiling point).
pub fn adiabatic_temperature(eq: &Equation, t0: f64, air: bool) -> Result<f64, ChemError> {
    let r = Reactor::new(eq, t0, P0, 1.0, air)?;
    let h0 = r.enthalpy();
    let n2 = species("N2", Some(State::Gas), t0);
    let inert = r.inert;
    let products: Vec<(f64, &Term)> = eq.products.iter().map(|t| (t.coef, t)).collect();
    let h_products = |t: f64| -> f64 {
        let mut h = inert * n2.map(|s| s.h(t)).unwrap_or(0.0);
        for (c, term) in &products {
            if let Some(sp) = species(&term.formula, term.state, t) {
                h += c * h_with_latent(sp, t);
            }
        }
        h
    };
    let (mut lo, mut hi) = (t0, 12_000.0);
    if h_products(lo) > h0 {
        return Ok(t0);
    }
    for _ in 0..100 {
        let mid = 0.5 * (lo + hi);
        if h_products(mid) > h0 {
            hi = mid
        } else {
            lo = mid
        }
    }
    Ok(0.5 * (lo + hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eq(s: &str) -> Equation {
        parse_equation(s).unwrap()
    }

    #[test]
    fn balances_equations_exactly() {
        let e = eq("C3H8 + O2 -> CO2 + H2O");
        let c: Vec<f64> = e
            .reactants
            .iter()
            .chain(&e.products)
            .map(|t| t.coef)
            .collect();
        assert_eq!(c, vec![1.0, 5.0, 3.0, 4.0]);
        let e = eq("Fe2O3 + Al -> Al2O3 + Fe");
        let c: Vec<f64> = e
            .reactants
            .iter()
            .chain(&e.products)
            .map(|t| t.coef)
            .collect();
        assert_eq!(c, vec![1.0, 2.0, 1.0, 2.0]);
        let e = eq("C6H12O6 + O2 -> CO2 + H2O");
        assert_eq!(e.display(), "C6H12O6 + 6 O2 → 6 CO2 + 6 H2O");
        // given coefficients are checked
        assert!(matches!(
            parse_equation("H2 + O2 -> H2O + 1 O2"),
            Err(ChemError::Unbalanced(_))
        ));
        assert!(parse_equation("2 H2 + O2 -> 2 H2O").is_ok());
        assert!(parse_equation("H2 + O2").is_err());
        assert!(parse_equation("Xy2 -> X").is_err());
        // fractional coefficients
        let e = eq("H2 + 1/2 O2 -> H2O(l)");
        assert_eq!(e.reactants[1].coef, 0.5);
    }

    #[test]
    fn standard_enthalpies_match_tabulated_reaction_values() {
        // methane combustion: −890.3 kJ/mol to liquid water (HHV), −802.3 to steam (LHV)
        let hhv = thermo(&eq("CH4 + 2 O2 -> CO2 + 2 H2O(l)"), T0).unwrap();
        let lhv = thermo(&eq("CH4 + 2 O2 -> CO2 + 2 H2O(g)"), T0).unwrap();
        assert!((hhv.dh / 1e3 + 890.3).abs() < 0.5, "{}", hhv.dh / 1e3);
        assert!((lhv.dh / 1e3 + 802.3).abs() < 0.5, "{}", lhv.dh / 1e3);
        // water formation: ΔH −285.83, ΔG −237.13 kJ/mol (CODATA)
        let w = thermo(&eq("H2 + 1/2 O2 -> H2O(l)"), T0).unwrap();
        assert!((w.dh / 1e3 + 285.83).abs() < 0.01);
        assert!((w.dg / 1e3 + 237.13).abs() < 0.05, "{}", w.dg / 1e3);
        assert!(w.exothermic() && w.spontaneous());
        // thermite: −851.5 kJ
        let t = thermo(&eq("Fe2O3 + 2 Al -> Al2O3 + 2 Fe"), T0).unwrap();
        assert!((t.dh / 1e3 + 851.5).abs() < 0.1);
        // limestone calcination is endothermic (+178 kJ) and only spontaneous hot (~1100 K)
        let c = thermo(&eq("CaCO3 -> CaO + CO2"), T0).unwrap();
        assert!((c.dh / 1e3 - 178.2).abs() < 1.0 && !c.spontaneous());
        let hot = thermo(&eq("CaCO3 -> CaO + CO2"), 1200.0).unwrap();
        assert!(hot.spontaneous());
        // graphite is the stable form of carbon; diamond → graphite is downhill
        let d = thermo(&eq("C(s) -> C(s)"), T0);
        assert!(d.is_ok());
        assert_eq!(species("C", None, T0).unwrap().name, "graphite");
    }

    #[test]
    fn stable_phase_of_water_switches_near_its_boiling_point() {
        assert_eq!(species("H2O", None, 298.15).unwrap().state, State::Liquid);
        assert_eq!(species("H2O", None, 400.0).unwrap().state, State::Gas);
        // the constant-Cp crossing lies within a few K of 373.15 K
        let mut lo = 300.0;
        let mut hi = 450.0;
        for _ in 0..60 {
            let mid = 0.5 * (lo + hi);
            if species("H2O", None, mid).unwrap().state == State::Liquid {
                lo = mid
            } else {
                hi = mid
            }
        }
        assert!((lo - 373.15).abs() < 3.0, "{}", lo);
    }

    #[test]
    fn equilibrium_constants_match_measured_values() {
        // N2O4 ⇌ 2 NO2: Kp ≈ 0.15 at 298 K
        let th = thermo(&eq("N2O4 -> 2 NO2"), T0).unwrap();
        assert!((th.k - 0.15).abs() < 0.03, "Kp {}", th.k);
        // Haber: N2 + 3H2 ⇌ 2NH3, K(298) ≈ 6e5 (ΔG° = −32.9 kJ)
        let h = thermo(&eq("N2 + 3 H2 -> 2 NH3"), T0).unwrap();
        assert!((h.dg / 1e3 + 32.9).abs() < 0.3, "{}", h.dg / 1e3);
        assert!(h.k > 4e5 && h.k < 8e5, "{}", h.k);
        // Le Chatelier: exothermic, so K falls with temperature
        assert!(thermo(&eq("N2 + 3 H2 -> 2 NH3"), 700.0).unwrap().k < 1e-3);
    }

    #[test]
    fn reactor_relaxes_to_equilibrium_and_conserves_enthalpy() {
        let e = eq("N2O4 -> 2 NO2");
        let mut r = Reactor::new(&e, T0, P0, 1.0, false).unwrap();
        r.law.a = 1e3; // slow it down so the integration is visible
        r.law.ea = 0.0;
        let h0 = r.enthalpy();
        r.advance(10.0);
        // at the end Q = K(T_final) (adiabatic: the endothermic dissociation cools it)
        let k = thermo(&e, r.t).unwrap().k;
        assert!(
            (r.quotient() / k - 1.0).abs() < 1e-3,
            "Q {} K {}",
            r.quotient(),
            k
        );
        assert!(r.t < T0);
        assert!(
            (r.enthalpy() - h0).abs() < 1e-6 * h0.abs().max(1.0),
            "{} vs {}",
            r.enthalpy(),
            h0
        );
        // 1 mol N2O4 at 1 bar, 298 K: α ≈ 0.19 by Kp = 4α²/(1−α²)
        let mut iso = Reactor::new(&e, T0, P0, 1.0, false).unwrap();
        iso.law.a = 1e3;
        iso.law.ea = 0.0;
        iso.ua = 1e9; // isothermal wall
        iso.advance(10.0);
        let alpha = iso.extent;
        let kp = thermo(&e, T0).unwrap().k;
        let expect = (kp / (4.0 + kp)).sqrt();
        assert!((alpha - expect).abs() < 2e-3, "{} vs {}", alpha, expect);
    }

    #[test]
    fn arrhenius_rates_and_n2o5_half_life() {
        let e = eq("2 N2O5 -> 4 NO2 + O2");
        let law = rate_law(&e);
        assert!(!law.estimated);
        // k(298 K) ≈ 3.4e-5 s⁻¹ → t½ ≈ 5–6 h (measured 3.38e-5)
        let k = law.k(T0);
        assert!(k > 2.5e-5 && k < 5e-5, "{}", k);
        // a 10 K rise roughly triples it here (Ea 103 kJ/mol)
        let ratio = law.k(308.15) / k;
        assert!(ratio > 3.5 && ratio < 4.0, "{}", ratio);
        let mut iso = Reactor::new(&e, T0, P0, 0.5, false).unwrap();
        iso.ua = 1e9;
        let n0 = iso.n[0];
        iso.advance(std::f64::consts::LN_2 / k);
        assert!((iso.n[0] / n0 - 0.5).abs() < 0.01, "{}", iso.n[0] / n0);
    }

    #[test]
    fn thermite_stops_at_the_boiling_point_of_iron() {
        let t = adiabatic_temperature(&eq("Fe2O3 + 2 Al -> Al2O3 + 2 Fe"), T0, false).unwrap();
        assert!((t - 3134.0).abs() < 1.0, "{}", t);
        // hydrogen in air ≈ 2500 K without dissociation (measured ≈ 2400 K)
        let h = adiabatic_temperature(&eq("2 H2 + O2 -> 2 H2O"), T0, true).unwrap();
        assert!(h > 2400.0 && h < 2650.0, "{}", h);
    }

    #[test]
    fn methane_in_air_ignites_hot_but_not_cold_and_reaches_a_flame_temperature() {
        let e = eq("CH4 + 2 O2 -> CO2 + 2 H2O");
        // complete-combustion adiabatic flame temperature in air with RRHO Cp:
        // ≈ 2330 K without dissociation (measured 2223 K; equilibrium codes
        // that freeze dissociation give ~2320–2330 K)
        let tad = adiabatic_temperature(&e, T0, true).unwrap();
        assert!(tad > 2250.0 && tad < 2450.0, "{}", tad);
        let mut cold = Reactor::new(&e, T0, 101_325.0, 1.0, true).unwrap();
        cold.advance(10.0);
        assert!(
            cold.conversion() < 1e-6,
            "no reaction at room temperature: {}",
            cold.conversion()
        );
        let mut hot = Reactor::new(&e, 1500.0, 101_325.0, 1.0, true).unwrap();
        let h0 = hot.enthalpy();
        hot.advance(0.05);
        assert!(hot.conversion() > 0.99, "{}", hot.conversion());
        assert!(hot.t > 2200.0, "self-heating to {}", hot.t);
        assert!((hot.enthalpy() - h0).abs() / h0.abs() < 1e-6);
    }
}

#[cfg(test)]
mod rrho_tests {
    use super::*;

    #[test]
    fn statistical_heat_capacities_match_janaf() {
        // JANAF Cp at 298 / 1000 / 2000 K (J/mol·K)
        let cases = [
            ("N2", 29.12, 32.70, 36.0),
            ("CO2", 37.12, 54.31, 60.35),
            ("H2O", 33.59, 41.27, 51.18),
            ("O2", 29.38, 34.88, 37.74),
        ];
        for (f, c298, c1000, c2000) in cases {
            let s = species(f, Some(State::Gas), 300.0).unwrap();
            for (t, want, tol) in [
                (298.15, c298, 0.03),
                (1000.0, c1000, 0.04),
                (2000.0, c2000, 0.06),
            ] {
                let got = s.cp_at(t);
                assert!(
                    (got - want).abs() / want < tol,
                    "{} at {} K: {} vs {}",
                    f,
                    t,
                    got,
                    want
                );
            }
        }
    }
}
