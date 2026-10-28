//! Circuits: modified nodal analysis (MNA) with device models and
//! electro-thermal coupling.
//!
//! * **Solver.** Node voltages, plus one current per voltage source, are
//!   found by dense Gaussian elimination with partial pivoting. Non-linear
//!   devices use Newton–Raphson with step damping, and every node carries a
//!   1e-12 S gmin. Transient analysis uses backward-Euler companion models
//!   for C (G = C/h) and L (G = h/L).
//! * **Wires from geometry.** R = ρ(T)·L/A, with ρ(20 °C) from the element
//!   table and ρ(T) = ρ₂₀·(1 + α(T − 293.15)). α comes from the CRC
//!   Handbook (12-41) and is linear only, good to ~10 % up to a few hundred
//!   °C.
//! * **Diode.** Shockley I = I_s(e^{V/nV_T} − 1), with V_T = kT/q and
//!   I_s(T) ∝ T³·e^{−E_g/kT}. No series resistance or breakdown.
//! * **MOSFET.** The EKV charge-based long-channel model:
//!   `I = 2nβV_T²[ln²(1+e^{(V_GS−V_th)/2nV_T}) − ln²(1+e^{(V_GS−V_th−nV_DS)/2nV_T})](1+λV_DS)`.
//!   It is continuous from subthreshold (leakage, ~n·V_T·ln10 per decade)
//!   through saturation. Temperature effects: μ ∝ (T/300)^−1.5 and
//!   V_th falls 1.5 mV/K (typical bulk-CMOS figures, not a fit to a
//!   foundry PDK). No velocity saturation, short-channel effects or gate
//!   tunnelling.
//! * **Doped silicon.** n_i(T) = √(N_c N_v)·e^{−E_g/2kT}, with
//!   N_c = 2.8e19·(T/300)^1.5 and N_v = 1.04e19·(T/300)^1.5 cm⁻³ and
//!   Varshni E_g(T). Donors and acceptors are fully ionised, and
//!   n = N/2 + √((N/2)² + n_i²). Mobility uses the Caughey–Thomas model
//!   with Arora/Masetti parameters and lattice scaling (T/300)^−2.42 (e) /
//!   −2.2 (h). σ = q(nμ_n + pμ_p).
//! * **Joule heating.** Each thermal resistor has a lumped heat capacity
//!   (mass from density × volume) and a loss h·A to ambient, and is heated
//!   by I²R. R(T) follows, and a wire that reaches its melting point opens
//!   (a fuse).

use crate::element_data;
use crate::element_props;

/// Elementary charge, C (CODATA 2018, exact).
pub const Q: f64 = 1.602_176_634e-19;
/// Boltzmann constant, J/K (CODATA 2018, exact).
pub const KB: f64 = 1.380_649e-23;

pub fn thermal_voltage(t: f64) -> f64 {
    KB * t / Q
}

/// Temperature coefficient of resistivity α (1/K) near 20 °C, CRC 12-41.
pub fn alpha_of(symbol: &str) -> f64 {
    match symbol {
        "Cu" => 0.00393,
        "Al" => 0.00403,
        "Ag" => 0.0038,
        "Au" => 0.0034,
        "Fe" => 0.00651,
        "W" => 0.0045,
        "Ni" => 0.00641,
        "Pt" => 0.003927,
        "Sn" => 0.0042,
        "Pb" => 0.0039,
        "Zn" => 0.0037,
        _ => 0.004,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Wave {
    Dc(f64),
    /// v0 → v1 after `delay`, linear `rise`, held `width`, back over `rise`, repeating every `period` (0 = once)
    Pulse {
        v0: f64,
        v1: f64,
        delay: f64,
        rise: f64,
        width: f64,
        period: f64,
    },
    Sine {
        offset: f64,
        amp: f64,
        freq: f64,
    },
}

impl Wave {
    pub fn at(&self, t: f64) -> f64 {
        match *self {
            Wave::Dc(v) => v,
            Wave::Sine { offset, amp, freq } => {
                offset + amp * (2.0 * std::f64::consts::PI * freq * t).sin()
            }
            Wave::Pulse {
                v0,
                v1,
                delay,
                rise,
                width,
                period,
            } => {
                if t < delay {
                    return v0;
                }
                let mut tt = t - delay;
                if period > 0.0 {
                    tt %= period;
                }
                let rise = rise.max(1e-15);
                if tt < rise {
                    v0 + (v1 - v0) * tt / rise
                } else if tt < rise + width {
                    v1
                } else if tt < 2.0 * rise + width {
                    v1 + (v0 - v1) * (tt - rise - width) / rise
                } else {
                    v0
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MosKind {
    N,
    P,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MosParams {
    pub kind: MosKind,
    /// |V_th| at 300 K, V
    pub vth: f64,
    /// μC_ox at 300 K, A/V²
    pub kp: f64,
    pub w_over_l: f64,
    /// subthreshold slope factor
    pub n: f64,
    /// channel-length modulation, 1/V
    pub lambda: f64,
}

impl MosParams {
    /// A generic 180 nm-class bulk device (V_dd 1.8 V). Representative values, not a foundry PDK.
    pub fn nmos(w_over_l: f64) -> Self {
        Self {
            kind: MosKind::N,
            vth: 0.45,
            kp: 300e-6,
            w_over_l,
            n: 1.3,
            lambda: 0.05,
        }
    }
    pub fn pmos(w_over_l: f64) -> Self {
        Self {
            kind: MosKind::P,
            vth: 0.45,
            kp: 100e-6,
            w_over_l,
            n: 1.3,
            lambda: 0.05,
        }
    }

    fn softplus(x: f64) -> f64 {
        x.max(0.0) + (-x.abs()).exp().ln_1p()
    }

    /// Drain current of an n-channel device for V_GS, V_DS ≥ 0 at T.
    fn ids_n(&self, vgs: f64, vds: f64, t: f64) -> f64 {
        let vt = thermal_voltage(t);
        let mu = (t / 300.0).powf(-1.5);
        let vth = self.vth - 1.5e-3 * (t - 300.0);
        let beta = self.n * self.kp * mu * self.w_over_l;
        let vp = (vgs - vth) / self.n;
        let f = Self::softplus(vp / (2.0 * vt)).powi(2);
        let r = Self::softplus((vp - vds) / (2.0 * vt)).powi(2);
        2.0 * self.n * beta * vt * vt * (f - r) * (1.0 + self.lambda * vds)
    }

    /// Current into the drain terminal (out of the source), any polarity.
    pub fn ids(&self, vg: f64, vd: f64, vs: f64, t: f64) -> f64 {
        let (vg, vd, vs, sign) = match self.kind {
            MosKind::N => (vg, vd, vs, 1.0),
            MosKind::P => (-vg, -vd, -vs, -1.0),
        };
        // symmetric device: the lower terminal acts as the source
        if vd >= vs {
            sign * self.ids_n(vg - vs, vd - vs, t)
        } else {
            -sign * self.ids_n(vg - vd, vs - vd, t)
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Wire {
    pub symbol: &'static str,
    pub rho20: f64,
    pub alpha: f64,
    pub length: f64,
    pub area: f64,
    pub melting_k: Option<f64>,
}

impl Wire {
    /// A round wire of an element (`diameter` and `length` in m).
    pub fn of(symbol: &str, length: f64, diameter: f64) -> Option<Self> {
        let e = element_data::by_symbol(symbol)?;
        let pr = element_props::props(e.z)?;
        let rho20 = pr.electrical_resistivity?;
        Some(Self {
            symbol: e.symbol,
            rho20,
            alpha: alpha_of(e.symbol),
            length,
            area: std::f64::consts::PI * diameter * diameter / 4.0,
            melting_k: e.melting_k.map(|m| m as f64),
        })
    }
    pub fn resistance(&self, t: f64) -> f64 {
        self.rho20 * (1.0 + self.alpha * (t - 293.15)).max(0.05) * self.length / self.area
    }
    pub fn volume(&self) -> f64 {
        self.length * self.area
    }
    pub fn surface(&self) -> f64 {
        let d = (4.0 * self.area / std::f64::consts::PI).sqrt();
        std::f64::consts::PI * d * self.length
    }
}

/// Lumped thermal mass attached to a resistor.
#[derive(Clone, Debug, PartialEq)]
pub struct Thermal {
    /// J/K
    pub heat_capacity: f64,
    /// W/K to ambient
    pub g_ambient: f64,
    pub temp: f64,
    pub ambient: f64,
    /// J dissipated so far
    pub heat: f64,
    pub open: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Elem {
    R {
        a: usize,
        b: usize,
        r: f64,
        wire: Option<Wire>,
        thermal: Option<Thermal>,
    },
    C {
        a: usize,
        b: usize,
        c: f64,
        v: f64,
    },
    L {
        a: usize,
        b: usize,
        l: f64,
        i: f64,
    },
    V {
        a: usize,
        b: usize,
        wave: Wave,
    },
    I {
        a: usize,
        b: usize,
        wave: Wave,
    },
    D {
        a: usize,
        k: usize,
        is300: f64,
        n: f64,
    },
    M {
        d: usize,
        g: usize,
        s: usize,
        p: MosParams,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Circuit {
    pub nodes: Vec<String>,
    pub elems: Vec<Elem>,
    /// device temperature, K
    pub temp: f64,
    pub time: f64,
    /// last solution: node voltages (index 0 = ground = 0 V)
    pub v: Vec<f64>,
    /// currents through voltage sources (a → b inside the source), in element order
    pub i_src: Vec<f64>,
    pub events: Vec<(f64, String)>,
    pub newton_iters: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CircuitError {
    Singular,
    NoConvergence(usize),
    UnknownNode(String),
}

impl std::fmt::Display for CircuitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CircuitError::Singular => write!(
                f,
                "singular matrix (a floating node or a loop of voltage sources)"
            ),
            CircuitError::NoConvergence(n) => {
                write!(f, "Newton did not converge in {} iterations", n)
            }
            CircuitError::UnknownNode(n) => write!(f, "unknown node {}", n),
        }
    }
}

const GMIN: f64 = 1e-12;

fn solve_dense(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-300 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        let (top, rest) = a.split_at_mut(col + 1);
        let prow = &top[col];
        for (k, row) in rest.iter_mut().enumerate() {
            let r = col + 1 + k;
            let f = row[col] / prow[col];
            if f != 0.0 {
                for (v, p) in row[col..n].iter_mut().zip(&prow[col..n]) {
                    *v -= f * p;
                }
                b[r] -= f * b[col];
            }
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let s: f64 = (r + 1..n).map(|c| a[r][c] * x[c]).sum();
        x[r] = (b[r] - s) / a[r][r];
    }
    Some(x)
}

impl Circuit {
    pub fn new() -> Self {
        Self {
            nodes: vec!["0".into()],
            elems: Vec::new(),
            temp: 300.0,
            time: 0.0,
            v: vec![0.0],
            i_src: Vec::new(),
            events: Vec::new(),
            newton_iters: 0,
        }
    }

    /// Node index by name, created on first use ("0" / "gnd" is ground).
    pub fn node(&mut self, name: &str) -> usize {
        if name == "0" || name.eq_ignore_ascii_case("gnd") {
            return 0;
        }
        if let Some(i) = self.nodes.iter().position(|n| n == name) {
            return i;
        }
        self.nodes.push(name.into());
        self.v.push(0.0);
        self.nodes.len() - 1
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        if name == "0" || name.eq_ignore_ascii_case("gnd") {
            return Some(0);
        }
        self.nodes.iter().position(|n| n == name)
    }

    pub fn voltage(&self, name: &str) -> f64 {
        self.find(name).map(|i| self.v[i]).unwrap_or(f64::NAN)
    }

    pub fn add(&mut self, e: Elem) -> usize {
        self.elems.push(e);
        self.elems.len() - 1
    }
    pub fn r(&mut self, a: &str, b: &str, r: f64) -> usize {
        let (a, b) = (self.node(a), self.node(b));
        self.add(Elem::R {
            a,
            b,
            r,
            wire: None,
            thermal: None,
        })
    }
    pub fn c(&mut self, a: &str, b: &str, c: f64) -> usize {
        let (a, b) = (self.node(a), self.node(b));
        self.add(Elem::C { a, b, c, v: 0.0 })
    }
    pub fn l(&mut self, a: &str, b: &str, l: f64) -> usize {
        let (a, b) = (self.node(a), self.node(b));
        self.add(Elem::L { a, b, l, i: 0.0 })
    }
    pub fn vsrc(&mut self, a: &str, b: &str, wave: Wave) -> usize {
        let (a, b) = (self.node(a), self.node(b));
        self.add(Elem::V { a, b, wave })
    }
    pub fn isrc(&mut self, a: &str, b: &str, wave: Wave) -> usize {
        let (a, b) = (self.node(a), self.node(b));
        self.add(Elem::I { a, b, wave })
    }
    /// Silicon junction diode, I_s = 1e-14 A at 300 K unless given.
    pub fn diode(&mut self, anode: &str, cathode: &str) -> usize {
        let (a, k) = (self.node(anode), self.node(cathode));
        self.add(Elem::D {
            a,
            k,
            is300: 1e-14,
            n: 1.0,
        })
    }
    pub fn mos(&mut self, d: &str, g: &str, s: &str, p: MosParams) -> usize {
        let (d, g, s) = (self.node(d), self.node(g), self.node(s));
        self.add(Elem::M { d, g, s, p })
    }
    /// A wire of element `symbol` (lengths in m) with Joule heating in still
    /// air (`h` W/m²K) from `ambient` K.
    pub fn wire(&mut self, a: &str, b: &str, w: Wire, h: f64, ambient: f64) -> usize {
        let (a, b) = (self.node(a), self.node(b));
        let z = element_data::by_symbol(w.symbol).map(|e| e.z).unwrap_or(29);
        let rho_mass = element_data::get(z)
            .and_then(|e| e.density)
            .map(|d| d as f64 * 1000.0)
            .unwrap_or(8000.0);
        let cp = element_props::props(z)
            .and_then(|p| p.specific_heat)
            .map(|c| c as f64 * 1000.0)
            .unwrap_or(400.0);
        let thermal = Thermal {
            heat_capacity: rho_mass * w.volume() * cp,
            g_ambient: h * w.surface(),
            temp: ambient,
            ambient,
            heat: 0.0,
            open: false,
        };
        let r = w.resistance(ambient);
        self.add(Elem::R {
            a,
            b,
            r,
            wire: Some(w),
            thermal: Some(thermal),
        })
    }

    fn n_unknowns(&self) -> (usize, usize) {
        let nv = self.nodes.len() - 1;
        let ns = self
            .elems
            .iter()
            .filter(|e| matches!(e, Elem::V { .. }))
            .count();
        (nv, ns)
    }

    /// Diode saturation current at T: I_s ∝ T³·exp(−E_g/kT), E_g(Si) Varshni.
    fn is_at(is300: f64, t: f64) -> f64 {
        let eg = |t: f64| si_band_gap(t);
        is300 * (t / 300.0).powi(3) * ((eg(300.0) / (KB * 300.0 / Q)) - eg(t) / (KB * t / Q)).exp()
    }

    /// One Newton solve at time `t`; `h` = Some(step) for transient companions, None for DC.
    fn solve_at(&mut self, t: f64, h: Option<f64>) -> Result<(), CircuitError> {
        let (nv, ns) = self.n_unknowns();
        let n = nv + ns;
        let temp = self.temp;
        let mut x: Vec<f64> = self.v[1..].to_vec();
        x.extend(
            self.i_src
                .iter()
                .copied()
                .chain(std::iter::repeat(0.0))
                .take(ns),
        );
        let idx = |node: usize| if node == 0 { None } else { Some(node - 1) };
        let nonlinear = self
            .elems
            .iter()
            .any(|e| matches!(e, Elem::D { .. } | Elem::M { .. }));
        let max_it = if nonlinear { 300 } else { 1 };
        for it in 0..max_it {
            let mut a = vec![vec![0.0; n]; n];
            let mut rhs = vec![0.0; n];
            let vnode = |x: &Vec<f64>, node: usize| if node == 0 { 0.0 } else { x[node - 1] };
            let stamp_g = |a: &mut Vec<Vec<f64>>, p: usize, q: usize, g: f64| {
                if let Some(i) = idx(p) {
                    a[i][i] += g;
                }
                if let Some(j) = idx(q) {
                    a[j][j] += g;
                }
                if let (Some(i), Some(j)) = (idx(p), idx(q)) {
                    a[i][j] -= g;
                    a[j][i] -= g;
                }
            };
            // current `i` flowing from p to q through the element (out of p, into q)
            let stamp_i = |rhs: &mut Vec<f64>, p: usize, q: usize, i: f64| {
                if let Some(k) = idx(p) {
                    rhs[k] -= i;
                }
                if let Some(k) = idx(q) {
                    rhs[k] += i;
                }
            };
            for (k, row) in a.iter_mut().enumerate().take(nv) {
                row[k] += GMIN;
            }
            let mut si = 0;
            for e in &self.elems {
                match e {
                    Elem::R {
                        a: p,
                        b: q,
                        r,
                        thermal,
                        ..
                    } => {
                        let r = if thermal.as_ref().is_some_and(|t| t.open) {
                            1e12
                        } else {
                            r.max(1e-9)
                        };
                        stamp_g(&mut a, *p, *q, 1.0 / r);
                    }
                    Elem::C { a: p, b: q, c, v } => {
                        if let Some(h) = h {
                            let g = c / h;
                            stamp_g(&mut a, *p, *q, g);
                            stamp_i(&mut rhs, *p, *q, -g * v);
                        }
                    }
                    Elem::L { a: p, b: q, l, i } => match h {
                        Some(h) => {
                            stamp_g(&mut a, *p, *q, h / l);
                            stamp_i(&mut rhs, *p, *q, *i);
                        }
                        None => stamp_g(&mut a, *p, *q, 1e6),
                    },
                    Elem::V { a: p, b: q, wave } => {
                        let row = nv + si;
                        if let Some(i) = idx(*p) {
                            a[i][row] += 1.0;
                            a[row][i] += 1.0;
                        }
                        if let Some(j) = idx(*q) {
                            a[j][row] -= 1.0;
                            a[row][j] -= 1.0;
                        }
                        rhs[row] = wave.at(t);
                        si += 1;
                    }
                    Elem::I { a: p, b: q, wave } => stamp_i(&mut rhs, *p, *q, wave.at(t)),
                    Elem::D {
                        a: p,
                        k: q,
                        is300,
                        n: nd,
                    } => {
                        let vt = nd * thermal_voltage(temp);
                        let is = Self::is_at(*is300, temp);
                        let vd = vnode(&x, *p) - vnode(&x, *q);
                        // linear continuation of the exponential above 40 V_T
                        let vc = 40.0 * vt;
                        let (id, gd) = if vd > vc {
                            let e = (vc / vt).exp();
                            (is * (e - 1.0) + is * e / vt * (vd - vc), is * e / vt)
                        } else {
                            let e = (vd / vt).exp();
                            (is * (e - 1.0), is * e / vt)
                        };
                        stamp_g(&mut a, *p, *q, gd + GMIN);
                        stamp_i(&mut rhs, *p, *q, id - gd * vd);
                    }
                    Elem::M { d, g, s, p } => {
                        let (vd, vg, vs) = (vnode(&x, *d), vnode(&x, *g), vnode(&x, *s));
                        let i0 = p.ids(vg, vd, vs, temp);
                        let hh = 1e-6;
                        let dg = (p.ids(vg + hh, vd, vs, temp) - p.ids(vg - hh, vd, vs, temp))
                            / (2.0 * hh);
                        let dd = (p.ids(vg, vd + hh, vs, temp) - p.ids(vg, vd - hh, vs, temp))
                            / (2.0 * hh);
                        let ds = (p.ids(vg, vd, vs + hh, temp) - p.ids(vg, vd, vs - hh, temp))
                            / (2.0 * hh);
                        let ieq = i0 - dg * vg - dd * vd - ds * vs;
                        // I(d→s) = dg·vg + dd·vd + ds·vs + ieq
                        for (row, sign) in [(*d, 1.0), (*s, -1.0)] {
                            if let Some(r) = idx(row) {
                                for (col, gval) in [(*g, dg), (*d, dd), (*s, ds)] {
                                    if let Some(c) = idx(col) {
                                        a[r][c] += sign * gval;
                                    }
                                }
                                rhs[r] -= sign * ieq;
                            }
                        }
                        // keep the drain/source nodes from floating when the device is off
                        stamp_g(&mut a, *d, *s, GMIN);
                    }
                }
            }
            let xn = solve_dense(a, rhs).ok_or(CircuitError::Singular)?;
            if !nonlinear {
                x = xn;
                self.newton_iters = 1;
                break;
            }
            // damped update: at most 0.3 V per node per iteration
            let mut maxd: f64 = 0.0;
            for k in 0..nv {
                maxd = maxd.max((xn[k] - x[k]).abs());
            }
            let scale = if maxd > 0.3 { 0.3 / maxd } else { 1.0 };
            let mut conv = true;
            for k in 0..n {
                let d = (xn[k] - x[k]) * scale;
                if d.abs() > 1e-9 + 1e-6 * xn[k].abs() {
                    conv = false;
                }
                x[k] += d;
            }
            self.newton_iters = it + 1;
            if conv && scale == 1.0 {
                break;
            }
            if it + 1 == max_it {
                return Err(CircuitError::NoConvergence(max_it));
            }
        }
        self.v = std::iter::once(0.0)
            .chain(x[..nv].iter().copied())
            .collect();
        self.i_src = x[nv..].to_vec();
        Ok(())
    }

    /// DC operating point at time `time` (C open, L short). Capacitor
    /// voltages and inductor currents are initialised from it, so a
    /// following transient starts from the operating point (as SPICE does).
    pub fn dc(&mut self) -> Result<(), CircuitError> {
        let t = self.time;
        self.solve_at(t, None)?;
        let v = self.v.clone();
        for e in &mut self.elems {
            match e {
                Elem::C { a, b, v: vc, .. } => *vc = v[*a] - v[*b],
                Elem::L { a, b, i, .. } => *i = 1e6 * (v[*a] - v[*b]),
                _ => {}
            }
        }
        Ok(())
    }

    /// One backward-Euler transient step of `h` seconds (with Joule heating).
    pub fn step(&mut self, h: f64) -> Result<(), CircuitError> {
        let t = self.time + h;
        self.solve_at(t, Some(h))?;
        // update companion state
        let v = self.v.clone();
        for e in &mut self.elems {
            match e {
                Elem::C { a, b, v: vc, .. } => *vc = v[*a] - v[*b],
                Elem::L { a, b, l, i } => *i += h / *l * (v[*a] - v[*b]),
                _ => {}
            }
        }
        self.joule(h);
        self.time = t;
        Ok(())
    }

    fn joule(&mut self, h: f64) {
        let v = self.v.clone();
        let time = self.time + h;
        let mut events = Vec::new();
        for e in &mut self.elems {
            if let Elem::R {
                a,
                b,
                r,
                wire,
                thermal: Some(th),
            } = e
            {
                if th.open {
                    continue;
                }
                let dv = v[*a] - v[*b];
                let p = dv * dv / *r;
                th.heat += p * h;
                // implicit loss to ambient
                let f = th.g_ambient * h / th.heat_capacity;
                th.temp = (th.temp + p * h / th.heat_capacity + f * th.ambient) / (1.0 + f);
                if let Some(w) = wire {
                    *r = w.resistance(th.temp);
                    if let Some(m) = w.melting_k {
                        if th.temp >= m {
                            th.open = true;
                            events.push((
                                time,
                                format!(
                                    "{} wire melted at {:.0} K: circuit open (fuse blown)",
                                    w.symbol, th.temp
                                ),
                            ));
                        }
                    }
                }
            }
        }
        self.events.extend(events);
    }

    /// Current through element `i` (a → b; drain → source; anode → cathode).
    pub fn current(&self, i: usize) -> f64 {
        let v = &self.v;
        let temp = self.temp;
        match &self.elems[i] {
            Elem::R {
                a, b, r, thermal, ..
            } => {
                if thermal.as_ref().is_some_and(|t| t.open) {
                    0.0
                } else {
                    (v[*a] - v[*b]) / r
                }
            }
            Elem::V { .. } => {
                let k = self.elems[..i]
                    .iter()
                    .filter(|e| matches!(e, Elem::V { .. }))
                    .count();
                self.i_src.get(k).copied().unwrap_or(0.0)
            }
            Elem::I { wave, .. } => wave.at(self.time),
            Elem::D { a, k, is300, n } => {
                let vt = n * thermal_voltage(temp);
                Self::is_at(*is300, temp) * (((v[*a] - v[*k]) / vt).min(80.0).exp() - 1.0)
            }
            Elem::M { d, g, s, p } => p.ids(v[*g], v[*d], v[*s], temp),
            Elem::C { a, b, c, .. } => {
                let _ = (a, b, c);
                f64::NAN
            }
            Elem::L { i, .. } => *i,
        }
    }

    /// Power dissipated in element `i` (W); sources report what they deliver as negative.
    pub fn power(&self, i: usize) -> f64 {
        let v = &self.v;
        let (pa, pb) = match &self.elems[i] {
            Elem::R { a, b, .. } | Elem::V { a, b, .. } | Elem::I { a, b, .. } => (*a, *b),
            Elem::D { a, k, .. } => (*a, *k),
            Elem::M { d, s, .. } => (*d, *s),
            _ => return 0.0,
        };
        let i_el = self.current(i);
        match &self.elems[i] {
            // MNA source current flows a → b inside the source: it delivers v·(−i)
            Elem::V { .. } => (v[pa] - v[pb]) * i_el,
            Elem::I { .. } => -(v[pb] - v[pa]) * i_el,
            _ => (v[pa] - v[pb]) * i_el,
        }
    }
}

impl Default for Circuit {
    fn default() -> Self {
        Self::new()
    }
}

/// Silicon band gap (eV), Varshni: E_g = 1.170 − 4.73e-4·T²/(T + 636).
pub fn si_band_gap(t: f64) -> f64 {
    1.170 - 4.73e-4 * t * t / (t + 636.0)
}

/// Intrinsic carrier concentration of Si, cm⁻³.
pub fn si_intrinsic(t: f64) -> f64 {
    let nc = 2.8e19 * (t / 300.0).powf(1.5);
    let nv = 1.04e19 * (t / 300.0).powf(1.5);
    (nc * nv).sqrt() * (-si_band_gap(t) / (2.0 * KB * t / Q)).exp()
}

/// Caughey–Thomas mobility (cm²/V·s) with Arora/Masetti parameters for Si.
pub fn si_mobility(electrons: bool, n_total: f64, t: f64) -> f64 {
    let (mmin, mmax, nref, a, texp) = if electrons {
        (68.5, 1414.0, 9.2e16, 0.711, -2.42)
    } else {
        (44.9, 470.5, 2.23e17, 0.719, -2.2)
    };
    let lattice = mmax * (t / 300.0).powf(texp);
    mmin + (lattice - mmin).max(0.0) / (1.0 + (n_total / nref).powf(a))
}

/// Carriers and resistivity of doped silicon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Doped {
    /// cm⁻³
    pub n: f64,
    pub p: f64,
    pub mu_n: f64,
    pub mu_p: f64,
    /// Ω·cm
    pub resistivity: f64,
    pub ni: f64,
}

/// Silicon with donor `nd` and acceptor `na` densities (cm⁻³) at T.
pub fn doped_silicon(nd: f64, na: f64, t: f64) -> Doped {
    let ni = si_intrinsic(t);
    let net = nd - na;
    let (n, p) = if net >= 0.0 {
        let n = net / 2.0 + ((net / 2.0).powi(2) + ni * ni).sqrt();
        (n, ni * ni / n)
    } else {
        let p = -net / 2.0 + ((net / 2.0).powi(2) + ni * ni).sqrt();
        (ni * ni / p, p)
    };
    let total = nd + na;
    let mu_n = si_mobility(true, total, t);
    let mu_p = si_mobility(false, total, t);
    let sigma = Q * (n * mu_n + p * mu_p);
    Doped {
        n,
        p,
        mu_n,
        mu_p,
        resistivity: 1.0 / sigma,
        ni,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divider_and_kirchhoff() {
        let mut c = Circuit::new();
        c.vsrc("in", "0", Wave::Dc(10.0));
        c.r("in", "mid", 1000.0);
        c.r("mid", "0", 2000.0);
        c.dc().unwrap();
        // exact up to the 1e-12 S gmin on every node (~nV here)
        assert!((c.voltage("mid") - 20.0 / 3.0).abs() < 1e-7);
        // the source delivers V²/R_total
        assert!((c.power(0) + 100.0 / 3000.0).abs() < 1e-9, "{}", c.power(0));
        assert!(
            (c.power(1) + c.power(2) + c.power(0)).abs() < 1e-9,
            "power balance (gmin aside)"
        );
    }

    #[test]
    fn rc_charges_to_63_percent_in_one_time_constant() {
        let mut c = Circuit::new();
        c.vsrc("in", "0", Wave::Dc(1.0));
        c.r("in", "out", 1000.0);
        c.c("out", "0", 1e-6);
        let tau = 1e-3;
        let h = tau / 2000.0;
        for _ in 0..2000 {
            c.step(h).unwrap();
        }
        let want = 1.0 - (-1.0f64).exp();
        assert!(
            (c.voltage("out") - want).abs() < 5e-4,
            "{} vs {}",
            c.voltage("out"),
            want
        );
    }

    #[test]
    fn lc_rings_at_its_resonant_frequency() {
        let mut c = Circuit::new();
        c.vsrc(
            "in",
            "0",
            Wave::Pulse {
                v0: 0.0,
                v1: 1.0,
                delay: 0.0,
                rise: 1e-9,
                width: 1.0,
                period: 0.0,
            },
        );
        c.r("in", "a", 1.0);
        c.l("a", "b", 1e-3);
        c.c("b", "0", 1e-6);
        let f0 = 1.0 / (2.0 * std::f64::consts::PI * (1e-3f64 * 1e-6).sqrt());
        let h = 1.0 / f0 / 4000.0;
        let mut crossings = Vec::new();
        let mut prev = 0.0;
        for _ in 0..4000 * 5 {
            c.step(h).unwrap();
            let v = c.voltage("b") - 1.0;
            if prev < 0.0 && v >= 0.0 {
                crossings.push(c.time);
            }
            prev = v;
        }
        let period = (crossings[3] - crossings[1]) / 2.0;
        assert!(
            (1.0 / period - f0).abs() / f0 < 0.01,
            "{} vs {}",
            1.0 / period,
            f0
        );
    }

    #[test]
    fn diode_forward_drop_and_temperature() {
        let mut c = Circuit::new();
        c.vsrc("in", "0", Wave::Dc(5.0));
        c.r("in", "a", 4300.0);
        c.diode("a", "0");
        c.dc().unwrap();
        let vd = c.voltage("a");
        let i = (5.0 - vd) / 4300.0;
        // Shockley exactly: V = V_T ln(I/I_s + 1)
        assert!(
            (vd - thermal_voltage(300.0) * (i / 1e-14 + 1.0).ln()).abs() < 1e-6,
            "{}",
            vd
        );
        assert!(vd > 0.6 && vd < 0.7, "{}", vd);
        // hotter junction: about −2 mV/K at constant current
        c.temp = 350.0;
        c.dc().unwrap();
        let slope = (c.voltage("a") - vd) / 50.0;
        assert!(slope < -1.5e-3 && slope > -2.5e-3, "{} V/K", slope);
        // reverse bias: only I_s flows
        let mut r = Circuit::new();
        r.vsrc("in", "0", Wave::Dc(-5.0));
        r.r("in", "a", 1000.0);
        r.diode("a", "0");
        r.dc().unwrap();
        assert!(r.current(2).abs() < 1e-12);
    }

    #[test]
    fn cmos_inverter_switches_and_has_gain() {
        let vtc = |vin: f64, t: f64| {
            let mut c = Circuit::new();
            c.temp = t;
            c.vsrc("vdd", "0", Wave::Dc(1.8));
            c.vsrc("in", "0", Wave::Dc(vin));
            c.mos("out", "in", "vdd", MosParams::pmos(6.0));
            c.mos("out", "in", "0", MosParams::nmos(2.0));
            c.dc().unwrap();
            c.voltage("out")
        };
        assert!(vtc(0.0, 300.0) > 1.79);
        assert!(vtc(1.8, 300.0) < 0.01);
        // matched sizing (k'p·Wp = k'n·Wn): switching threshold ≈ V_dd/2
        let mut lo = 0.0;
        let mut hi = 1.8;
        for _ in 0..40 {
            let m = 0.5 * (lo + hi);
            if vtc(m, 300.0) > m {
                lo = m
            } else {
                hi = m
            }
        }
        assert!((lo - 0.9).abs() < 0.05, "V_M {}", lo);
        let gain = (vtc(0.95, 300.0) - vtc(0.85, 300.0)) / 0.1;
        assert!(gain < -5.0, "{}", gain);
    }

    #[test]
    fn mosfet_square_law_and_subthreshold_slope() {
        let p = MosParams::nmos(1.0);
        // saturation: I ≈ k'/2·(V_GS − V_th)²·(1+λV_DS)
        let i = p.ids(1.45, 1.8, 0.0, 300.0);
        let want = 300e-6 / 2.0 * 1.0 * (1.0 + 0.05 * 1.8);
        assert!((i - want).abs() / want < 0.05, "{} vs {}", i, want);
        // subthreshold: n·V_T·ln10 per decade ≈ 77 mV/dec at 300 K
        let i1 = p.ids(0.10, 1.0, 0.0, 300.0);
        let i2 = p.ids(0.20, 1.0, 0.0, 300.0);
        let mv_dec = 100.0 / (i2 / i1).log10();
        assert!(
            (mv_dec - 1.3 * thermal_voltage(300.0) * 1e3 * std::f64::consts::LN_10).abs() < 3.0,
            "{}",
            mv_dec
        );
        // leakage at V_GS = 0 grows by orders of magnitude when hot
        assert!(p.ids(0.0, 1.8, 0.0, 400.0) > 100.0 * p.ids(0.0, 1.8, 0.0, 300.0));
    }

    #[test]
    fn wire_resistance_from_geometry_and_material() {
        // 1 m of 1 mm² copper: 0.0168–0.0172 Ω at 20 °C
        let w = Wire::of("Cu", 1.0, (4e-6 / std::f64::consts::PI).sqrt()).unwrap();
        let r = w.resistance(293.15);
        assert!(r > 0.0165 && r < 0.0175, "{}", r);
        // +39 % at 120 °C (α = 0.00393)
        assert!((w.resistance(393.15) / r - 1.393).abs() < 1e-3);
        let al = Wire::of("Al", 1.0, (4e-6 / std::f64::consts::PI).sqrt()).unwrap();
        assert!(al.resistance(293.15) > 1.5 * r);
    }

    #[test]
    fn doped_silicon_resistivity_follows_the_irvin_curve() {
        // intrinsic Si ~2–4e5 Ω·cm at 300 K
        let i = doped_silicon(0.0, 0.0, 300.0);
        assert!(i.ni > 5e9 && i.ni < 1.6e10, "{}", i.ni);
        assert!(
            i.resistivity > 1e5 && i.resistivity < 6e5,
            "{}",
            i.resistivity
        );
        // n-type 1e16: ≈ 0.45–0.5 Ω·cm; p-type 1e16: ≈ 1.4 Ω·cm
        let n = doped_silicon(1e16, 0.0, 300.0);
        assert!(
            n.resistivity > 0.4 && n.resistivity < 0.6,
            "{}",
            n.resistivity
        );
        let p = doped_silicon(0.0, 1e16, 300.0);
        assert!(
            p.resistivity > 1.2 && p.resistivity < 1.6,
            "{}",
            p.resistivity
        );
        // hot enough and intrinsic carriers swamp the doping (the device stops being a device)
        let hot = doped_silicon(1e15, 0.0, 600.0);
        assert!(hot.ni > 1e15, "{}", hot.ni);
    }

    #[test]
    fn joule_heating_reaches_steady_state_and_blows_a_fuse() {
        // 0.2 A through 2 cm of 0.1 mm copper in still air: settles where I²R(T) = hA·ΔT
        let w = Wire::of("Cu", 0.02, 1e-4).unwrap();
        let mut c = Circuit::new();
        c.isrc("0", "a", Wave::Dc(0.2));
        let wi = c.wire("a", "0", w.clone(), 50.0, 293.15);
        // thermal time constant C/G ≈ 1.7 s: run 15 τ
        for _ in 0..2500 {
            c.step(1e-2).unwrap();
        }
        let Elem::R {
            thermal: Some(th),
            r,
            ..
        } = &c.elems[wi]
        else {
            panic!()
        };
        let p = 0.04 * r;
        assert!(
            (p - th.g_ambient * (th.temp - 293.15)).abs() / p < 0.01,
            "{} vs {}",
            p,
            th.g_ambient * (th.temp - 293.15)
        );
        // 10 A melts it within a fraction of a second and the circuit opens
        let mut f = Circuit::new();
        f.vsrc("in", "0", Wave::Dc(1.0));
        f.r("in", "a", 0.06);
        let fi = f.wire("a", "0", w, 50.0, 293.15);
        let mut t_blow = None;
        for _ in 0..5000 {
            f.step(1e-4).unwrap();
            if t_blow.is_none() && f.events.iter().any(|(_, s)| s.contains("fuse")) {
                t_blow = Some(f.time);
            }
        }
        let t_blow = t_blow.expect("fuse blew");
        assert!(t_blow < 0.5, "{}", t_blow);
        assert_eq!(f.current(fi), 0.0);
    }
}
