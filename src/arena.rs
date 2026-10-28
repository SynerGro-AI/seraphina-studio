//! Physics Arena: rigid bodies under real environmental conditions.
//!
//! What is simulated (SI units throughout):
//! * **Motion**: fixed timestep (1/240 s); forces are held constant over a
//!   step and positions advance with the average velocity (exact for
//!   constant forces, so mechanical energy only changes by drag work). Gravity,
//!   quadratic aerodynamic drag `F = −½ ρ C_d A |v−w| (v−w)` with wind `w`,
//!   and Archimedes buoyancy `ρ_air V g`. Translational only: bodies do not
//!   rotate (no angular momentum or torque).
//! * **Contacts**: ground plane y = 0 and axis-aligned box–box contacts,
//!   resolved with impulses: restitution `e = √(e₁e₂)` along the normal and
//!   Coulomb friction `|Δv_t| ≤ μ·|Δv_n|`, `μ = √(μ₁μ₂)`, kinetic coefficient.
//! * **Mass** = density × mesh volume, from the body's material.
//! * **Heat**: a heater term, Newtonian convection `h·A·(T_air − T)` with
//!   `h = 10 W/(m²K) × ρ_air/1.225` (natural convection in still air, scaled
//!   with air density; 0 in vacuum), and Stefan–Boltzmann radiation
//!   `εσA(T_env⁴ − T⁴)`. Temperature is uniform inside a body (lumped).
//! * **Phase changes**: at the melting / boiling point (boiling point moved
//!   with pressure by Clausius–Clapeyron, see `material_env`), heat goes into
//!   latent heat `m·L` before the temperature moves on. Liquids slump into a
//!   puddle (same volume); vaporised bodies leave the simulation.
//!
//! Constants: g₀ = 9.80665 m/s² (standard gravity, exact), σ = 5.670374419e-8
//! W/(m²K⁴) (CODATA 2018). Moon 1.62 m/s², Mars 3.721 m/s², Mars surface
//! pressure ≈ 610 Pa and air density ≈ 0.020 kg/m³ (NASA fact sheets).

use crate::material_env::{EnvMeta, Phase};

pub const G0: f32 = 9.80665;
pub const SIGMA: f64 = 5.670_374_419e-8;
pub const DT: f32 = 1.0 / 240.0;
/// Most steps one [`Arena::advance`] call runs (1 s of simulated time).
pub const MAX_CATCH_UP: usize = 240;
/// Below this many bodies the all-pairs contact loop is already cheap.
pub const SWEEP_MIN_BODIES: usize = 24;
/// Slack added around each box for the sweep-and-prune candidate search.
const SWEEP_MARGIN: f32 = 0.05;

/// How contacts find their pairs. Both give bit-identical results; `Auto`
/// sweeps once there are enough bodies for it to pay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Collide {
    Auto,
    Brute,
    Sweep,
}

impl Collide {
    fn sweep(self, n: usize) -> bool {
        match self {
            Collide::Auto => n >= SWEEP_MIN_BODIES,
            Collide::Brute => false,
            Collide::Sweep => true,
        }
    }
}

/// Sweep-and-prune on x: every pair whose boxes, grown by `margin`, overlap
/// on all three axes, as (i, j) with i < j in ascending order — the order
/// the all-pairs loop visits them.
fn near(a: &Body, b: &Body, margin: f32) -> bool {
    !a.vaporised
        && !b.vaporised
        && (0..3).all(|k| {
            a.pos[k] - a.half[k] - margin <= b.pos[k] + b.half[k] + margin
                && b.pos[k] - b.half[k] - margin <= a.pos[k] + a.half[k] + margin
        })
}

pub fn sweep_pairs(bodies: &[Body], margin: f32) -> Vec<(usize, usize)> {
    let mut idx: Vec<usize> = (0..bodies.len())
        .filter(|&i| !bodies[i].vaporised)
        .collect();
    let lo = |i: usize, k: usize| bodies[i].pos[k] - bodies[i].half[k] - margin;
    let hi = |i: usize, k: usize| bodies[i].pos[k] + bodies[i].half[k] + margin;
    idx.sort_by(|&a, &b| lo(a, 0).total_cmp(&lo(b, 0)).then(a.cmp(&b)));
    let mut active: Vec<usize> = Vec::new();
    let mut out = Vec::new();
    for &i in &idx {
        let x0 = lo(i, 0);
        active.retain(|&j| hi(j, 0) >= x0);
        for &j in &active {
            if lo(i, 1) <= hi(j, 1)
                && lo(j, 1) <= hi(i, 1)
                && lo(i, 2) <= hi(j, 2)
                && lo(j, 2) <= hi(i, 2)
            {
                out.push((i.min(j), i.max(j)));
            }
        }
        active.push(i);
    }
    out.sort_unstable();
    out
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ui", derive(serde::Serialize, serde::Deserialize))]
pub struct Environment {
    pub name: String,
    /// m/s² (downward).
    pub gravity: f32,
    /// kg/m³
    pub air_density: f32,
    /// Pa
    pub pressure: f64,
    /// K
    pub temperature: f32,
    /// m/s
    pub wind: [f32; 3],
}

impl Environment {
    pub fn earth() -> Self {
        // ISA sea level: 15 °C, 101 325 Pa, 1.225 kg/m³
        Self {
            name: "Earth".into(),
            gravity: G0,
            air_density: 1.225,
            pressure: 101_325.0,
            temperature: 288.15,
            wind: [0.0; 3],
        }
    }
    pub fn moon() -> Self {
        Self {
            name: "Moon".into(),
            gravity: 1.62,
            air_density: 0.0,
            pressure: 0.0,
            temperature: 250.0,
            wind: [0.0; 3],
        }
    }
    pub fn mars() -> Self {
        Self {
            name: "Mars".into(),
            gravity: 3.721,
            air_density: 0.020,
            pressure: 610.0,
            temperature: 210.0,
            wind: [0.0; 3],
        }
    }
    /// Earth gravity with the air pumped out (the hammer-and-feather demo).
    pub fn vacuum() -> Self {
        Self {
            name: "Vacuum chamber".into(),
            gravity: G0,
            air_density: 0.0,
            pressure: 0.0,
            temperature: 293.15,
            wind: [0.0; 3],
        }
    }
    /// Deep space: no gravity, no air, 2.725 K background.
    pub fn space() -> Self {
        Self {
            name: "Deep space".into(),
            gravity: 0.0,
            air_density: 0.0,
            pressure: 0.0,
            temperature: 2.725,
            wind: [0.0; 3],
        }
    }

    pub fn preset(name: &str) -> Option<Self> {
        Some(match name.trim().to_lowercase().as_str() {
            "earth" => Self::earth(),
            "moon" | "luna" => Self::moon(),
            "mars" => Self::mars(),
            "vacuum" | "vacuum chamber" => Self::vacuum(),
            "space" | "deep space" | "zero-g" | "zero_g" => Self::space(),
            "custom" => Self {
                name: "Custom".into(),
                ..Self::earth()
            },
            _ => return None,
        })
    }

    pub const PRESETS: [&'static str; 5] = ["earth", "moon", "mars", "vacuum", "space"];

    /// Natural-convection coefficient, W/(m²·K).
    pub fn convection_h(&self) -> f32 {
        10.0 * self.air_density / 1.225
    }
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ui", derive(serde::Serialize, serde::Deserialize))]
pub struct Body {
    pub name: String,
    pub material: String,
    pub env: EnvMeta,
    /// Box half extents (m) as built (solid).
    pub half0: [f32; 3],
    /// Current half extents (a liquid slumps).
    pub half: [f32; 3],
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    /// m³
    pub volume: f32,
    /// kg
    pub mass: f32,
    /// K (f64 so the energy ledger is not swamped by rounding: m·c·T of a
    /// tonne of iron at 300 K is ~1.4e8 J and f32 would carry ~10 J of noise)
    pub temp: f64,
    pub phase: Phase,
    /// J banked toward the next phase change (+ melting/boiling, − freezing/condensing).
    pub latent: f64,
    /// W
    pub heater: f32,
    pub fixed: bool,
    pub drag_cd: f32,
    pub vaporised: bool,
}

impl Body {
    /// A body from a material and a box; the volume is the box's (see [`Body::with_volume`]).
    pub fn new(
        name: &str,
        material: &str,
        env: EnvMeta,
        half: [f32; 3],
        pos: [f32; 3],
        temp: f32,
        pressure: f64,
    ) -> Self {
        let v = (8.0 * half[0] * half[1] * half[2]).max(1e-9);
        let phase = env.phase_at(temp, pressure);
        let rho = env.density_in(phase, temp, pressure).unwrap_or(1000.0);
        Self {
            name: name.to_string(),
            material: material.to_string(),
            env,
            half0: half,
            half,
            pos,
            vel: [0.0; 3],
            volume: v,
            mass: rho * v,
            temp: temp as f64,
            phase,
            latent: 0.0,
            heater: 0.0,
            fixed: false,
            drag_cd: 1.05, // cube face-on
            vaporised: false,
        }
    }

    /// Use the mesh's volume instead of the box's, keeping the density.
    pub fn with_volume(mut self, volume: f32) -> Self {
        let v = volume.max(1e-9);
        self.mass = self.mass / self.volume * v;
        self.volume = v;
        self
    }

    pub fn area_frontal(&self, dir: [f32; 3]) -> f32 {
        let h = self.half;
        let d = norm(dir);
        if d < 1e-9 {
            return 4.0 * h[0] * h[2];
        }
        // projected area of a box along dir
        4.0 * (h[1] * h[2] * dir[0].abs() + h[0] * h[2] * dir[1].abs() + h[0] * h[1] * dir[2].abs())
            / d
    }

    pub fn surface_area(&self) -> f32 {
        let h = self.half;
        8.0 * (h[0] * h[1] + h[1] * h[2] + h[0] * h[2])
    }

    pub fn kinetic(&self) -> f64 {
        let v = [self.vel[0] as f64, self.vel[1] as f64, self.vel[2] as f64];
        0.5 * self.mass as f64 * (v[0] * v[0] + v[1] * v[1] + v[2] * v[2])
    }

    pub fn specific_heat(&self) -> f64 {
        self.env.specific_heat.unwrap_or(1000.0) as f64
    }

    /// Heat capacity m·c (J/K).
    pub fn heat_capacity(&self) -> f64 {
        self.mass.max(1e-9) as f64 * self.specific_heat()
    }

    /// Latent heat per kg absorbed going solid → `phase` (matches `apply_heat`).
    fn latent_content_per_kg(&self) -> f64 {
        let lf = self.env.latent_fusion.unwrap_or(0.0) as f64;
        let lv = self
            .env
            .latent_vaporization
            .or(self.env.latent_fusion)
            .unwrap_or(0.0) as f64;
        let direct = self.env.sublimes || self.env.melting_k.is_none();
        match self.phase {
            Phase::Solid => 0.0,
            Phase::Liquid => lf,
            Phase::Gas if direct => lv,
            Phase::Gas => lf + self.env.latent_vaporization.unwrap_or(0.0) as f64,
        }
    }

    /// Internal (thermal) energy relative to a solid at 0 K with constant c:
    /// U = m·c·T + m·Σ L (phases passed) + banked latent heat.
    pub fn internal(&self) -> f64 {
        self.heat_capacity() * self.temp
            + self.mass as f64 * self.latent_content_per_kg()
            + self.latent
    }

    /// Effective weight factor (m − ρ_air·V): gravity minus buoyancy.
    fn weight_mass(&self, rho_air: f32) -> f64 {
        self.mass as f64 - rho_air as f64 * self.volume as f64
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
#[cfg_attr(feature = "ui", derive(serde::Serialize, serde::Deserialize))]
pub struct Events {
    pub log: Vec<(f64, String)>,
}

/// Global energy accounting. Everything that crosses the arena boundary is
/// a flow; everything inside is a stock (kinetic, potential, internal).
/// `residual = (stocks now − stocks at start) − (flows in)` must stay ≈ 0.
#[derive(Clone, Debug, PartialEq, Default)]
#[cfg_attr(feature = "ui", derive(serde::Serialize, serde::Deserialize))]
pub struct Ledger {
    /// J delivered by heaters.
    pub heater_in: f64,
    /// J gained from the surroundings by convection + radiation (negative = lost).
    pub exchange_in: f64,
    /// J of work done by the air on the bodies (drag; negative = lost to the air,
    /// positive when wind pushes).
    pub drag_work: f64,
    /// J of kinetic energy turned into heat by impacts, friction and slumping
    /// (internal transfer, informational; per step it can be slightly
    /// negative while a resting stack settles its penetration).
    pub dissipated: f64,
    /// J of heat conducted between touching bodies (internal transfer).
    pub conducted: f64,
    /// Stocks (KE + PE + U) when the ledger started.
    pub start: Option<f64>,
    /// KE + PE when the ledger started.
    pub start_mech: f64,
}

impl Ledger {
    pub fn flows_in(&self) -> f64 {
        self.heater_in + self.exchange_in + self.drag_work
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct EnergyTotals {
    pub kinetic: f64,
    pub potential: f64,
    pub internal: f64,
}

impl EnergyTotals {
    pub fn total(&self) -> f64 {
        self.kinetic + self.potential + self.internal
    }
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "ui", derive(serde::Serialize, serde::Deserialize))]
pub struct Arena {
    pub env: Environment,
    pub bodies: Vec<Body>,
    pub time: f64,
    pub ground: bool,
    /// Heat exchange with the environment (convection + radiation).
    pub exchange: bool,
    /// Heat conduction between touching bodies.
    pub conduction: bool,
    pub events: Events,
    pub ledger: Ledger,
    acc: f32,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

/// Bodies closer than this (m) along the contact axis conduct heat.
const TOUCH: f32 = 2e-3;

impl Arena {
    pub fn new(env: Environment) -> Self {
        Self {
            env,
            bodies: Vec::new(),
            time: 0.0,
            ground: true,
            exchange: true,
            conduction: true,
            events: Events::default(),
            ledger: Ledger::default(),
            acc: 0.0,
        }
    }

    pub fn add(&mut self, b: Body) -> usize {
        self.bodies.push(b);
        self.ledger.start = None;
        self.bodies.len() - 1
    }

    pub fn body(&self, name: &str) -> Option<&Body> {
        self.bodies.iter().find(|b| b.name == name)
    }

    pub fn body_mut(&mut self, name: &str) -> Option<&mut Body> {
        self.bodies.iter_mut().find(|b| b.name == name)
    }

    /// Advance by real time `secs` using fixed steps; returns the steps taken.
    /// Fixed-timestep catch-up for `secs` of wall time. At most
    /// [`MAX_CATCH_UP`] steps (1 s of simulation) run per call; anything
    /// left after a stall is dropped instead of being chased next frame,
    /// so a slow frame cannot snowball into slower ones.
    pub fn advance(&mut self, secs: f32) -> usize {
        self.acc += secs.max(0.0);
        let mut n = 0;
        while self.acc >= DT && n < MAX_CATCH_UP {
            self.step();
            self.acc -= DT;
            n += 1;
        }
        if self.acc >= DT {
            self.acc = 0.0;
        }
        n
    }

    /// Kinetic + potential (gravity minus buoyancy, at the centre of mass) +
    /// internal energy of every body, vaporised ones included (their gas is
    /// frozen where it left).
    pub fn energy(&self) -> EnergyTotals {
        let g = self.env.gravity as f64;
        let rho = self.env.air_density;
        let mut t = EnergyTotals::default();
        for b in &self.bodies {
            t.kinetic += b.kinetic();
            t.potential += b.weight_mass(rho) * g * b.pos[1] as f64;
            t.internal += b.internal();
        }
        t
    }

    /// Kinetic + potential at the moment the ledger started (so the UI can
    /// show the change in thermal energy on its own).
    pub fn ledger_start_mech(&self) -> f64 {
        self.ledger.start_mech
    }

    fn mechanical(&self) -> f64 {
        let g = self.env.gravity as f64;
        let rho = self.env.air_density;
        self.bodies
            .iter()
            .map(|b| b.kinetic() + b.weight_mass(rho) * g * b.pos[1] as f64)
            .sum()
    }

    /// Stocks − start − flows. Zero means energy is conserved.
    pub fn energy_residual(&self) -> f64 {
        match self.ledger.start {
            Some(e0) => self.energy().total() - e0 - self.ledger.flows_in(),
            None => 0.0,
        }
    }

    pub fn step(&mut self) {
        self.step_with(Collide::Auto)
    }

    /// One step with a chosen pair search (for tests and benchmarks).
    pub fn step_with(&mut self, mode: Collide) {
        if self.ledger.start.is_none() {
            self.ledger.start = Some(self.energy().total());
            self.ledger.start_mech = self.mechanical();
        }
        let dt = DT;
        let g = self.env.gravity;
        let rho_air = self.env.air_density;
        let wind = self.env.wind;
        let mut drag_work = 0.0f64;
        for b in self.bodies.iter_mut().filter(|b| !b.fixed && !b.vaporised) {
            let m = b.mass.max(1e-9);
            // gravity + buoyancy
            let f = [0.0, -m * g + rho_air * b.volume * g, 0.0];
            // drag on the velocity relative to the air (evaluated at the start of the step)
            let rel = [b.vel[0] - wind[0], b.vel[1] - wind[1], b.vel[2] - wind[2]];
            let sp = norm(rel);
            let mut fd = [0.0f32; 3];
            if sp > 1e-6 && rho_air > 0.0 {
                let a = b.area_frontal(rel);
                // implicit factor keeps the drag from reversing the relative
                // velocity in one step for very light bodies
                let k = 0.5 * rho_air * b.drag_cd * a * sp;
                let damp = 1.0 / (1.0 + k * dt / m);
                for i in 0..3 {
                    fd[i] = -k * rel[i] * damp;
                }
            }
            // constant force over the step, position from the average
            // velocity: exact for constant forces, so ΔKE + ΔPE = drag work
            for i in 0..3 {
                let v0 = b.vel[i];
                let v1 = v0 + (f[i] + fd[i]) / m * dt;
                let vavg = 0.5 * (v0 + v1);
                b.vel[i] = v1;
                b.pos[i] += vavg * dt;
                drag_work += fd[i] as f64 * vavg as f64 * dt as f64;
            }
        }
        self.ledger.drag_work += drag_work;
        let deposits = self.contacts(mode);
        for (i, e) in deposits {
            self.heat_body(i, e);
        }
        self.thermal(dt, mode);
        self.time += dt as f64;
    }

    /// Box–box contacts over sweep-and-prune candidates, visited in the same
    /// order as the all-pairs loop, with the same result. A pair left out
    /// was more than 2·margin apart on some axis when it was last checked,
    /// so it cannot touch until corrections move its bodies that far along
    /// that axis. When one body has moved most of a margin, its pairs are
    /// checked again from the current positions (any new ones that come
    /// later in the order are queued) and its tally restarts. Returns how
    /// many such re-checks were needed.
    fn box_pairs_swept(&mut self, g: f32, touched: &mut [bool]) -> usize {
        let n = self.bodies.len();
        let mut queue: std::collections::BTreeSet<(usize, usize)> =
            sweep_pairs(&self.bodies, SWEEP_MARGIN)
                .into_iter()
                .collect();
        let mut moved = vec![[0.0f32; 3]; n];
        let mut rechecks = 0;
        while let Some((i, j)) = queue.pop_first() {
            let (l, r) = self.bodies.split_at_mut(j);
            let (a, b) = (&mut l[i], &mut r[0]);
            let (pa, pb) = (a.pos, b.pos);
            if !resolve_pair(a, b, g) {
                continue;
            }
            touched[i] = true;
            touched[j] = true;
            for k in 0..3 {
                moved[i][k] += (a.pos[k] - pa[k]).abs();
                moved[j][k] += (b.pos[k] - pb[k]).abs();
            }
            for x in [i, j] {
                if moved[x].iter().all(|&d| d <= SWEEP_MARGIN * 0.9) {
                    continue;
                }
                rechecks += 1;
                moved[x] = [0.0; 3];
                for y in (0..n).filter(|&y| y != x) {
                    let p = (x.min(y), x.max(y));
                    if p > (i, j) && near(&self.bodies[x], &self.bodies[y], SWEEP_MARGIN) {
                        queue.insert(p);
                    }
                }
            }
        }
        rechecks
    }

    /// Put heat into body `i`, crediting the potential energy a melting body
    /// loses when it slumps (the collapse is dissipated as heat).
    fn heat_body(&mut self, i: usize, e: f64) {
        let p = self.env.pressure;
        let wm = self.bodies[i].weight_mass(self.env.air_density) * self.env.gravity as f64;
        let time = self.time;
        let y0 = self.bodies[i].pos[1];
        apply_heat(&mut self.bodies[i], e, p, time, &mut self.events);
        let dy = (y0 - self.bodies[i].pos[1]) as f64;
        if dy > 0.0 && !self.bodies[i].vaporised {
            let d = wm * dy;
            self.ledger.dissipated += d;
            apply_heat(&mut self.bodies[i], d, p, time, &mut self.events);
        }
    }

    /// Resolve contacts; returns heat deposits (body, J) for the kinetic
    /// energy the impulses removed, net of the potential energy the position
    /// corrections added.
    fn contacts(&mut self, mode: Collide) -> Vec<(usize, f64)> {
        let g = self.env.gravity;
        let e_before = self.mechanical();
        let mut touched = vec![false; self.bodies.len()];
        // ground
        if self.ground {
            for (idx, b) in self
                .bodies
                .iter_mut()
                .enumerate()
                .filter(|(_, b)| !b.fixed && !b.vaporised)
            {
                let bottom = b.pos[1] - b.half[1];
                if bottom < 0.0 {
                    touched[idx] = true;
                    b.pos[1] -= bottom;
                    if b.vel[1] < 0.0 {
                        let vn = -b.vel[1];
                        // liquids don't bounce
                        let e = if b.phase == Phase::Solid {
                            b.env.restitution
                        } else {
                            0.0
                        };
                        // rest threshold: below the speed gained in one step, stop bouncing
                        let e = if vn < 2.0 * g * DT + 1e-3 { 0.0 } else { e };
                        let jn = (1.0 + e) * vn;
                        b.vel[1] = e * vn;
                        friction(
                            &mut b.vel,
                            jn,
                            b.env.friction_kinetic,
                            b.env.friction_static,
                            g,
                        );
                    } else if b.vel[1].abs() < 1e-6 {
                        // resting contact: friction from the normal force over one step
                        friction(
                            &mut b.vel,
                            g * DT,
                            b.env.friction_kinetic,
                            b.env.friction_static,
                            g,
                        );
                    }
                }
            }
        }
        // box–box
        let n = self.bodies.len();
        if mode.sweep(n) {
            self.box_pairs_swept(g, &mut touched);
        } else {
            for i in 0..n {
                for j in i + 1..n {
                    let (l, r) = self.bodies.split_at_mut(j);
                    if resolve_pair(&mut l[i], &mut r[0], g) {
                        touched[i] = true;
                        touched[j] = true;
                    }
                }
            }
        }
        // the mechanical energy the contacts removed becomes heat in the
        // bodies that touched, shared by heat capacity (equal temperature
        // rise). It is measured over the whole contact phase, signed: a
        // resting stack's penetration-and-correction cycle sums to zero
        // instead of pumping heat between bodies.
        let heat = e_before - self.mechanical();
        let c_sum: f64 = self
            .bodies
            .iter()
            .zip(&touched)
            .filter(|(_, t)| **t)
            .map(|(b, _)| b.heat_capacity())
            .sum();
        let mut deposits = Vec::new();
        if c_sum > 0.0 && heat != 0.0 {
            for (i, b) in self.bodies.iter().enumerate().filter(|(i, _)| touched[*i]) {
                deposits.push((i, heat * b.heat_capacity() / c_sum));
            }
            self.ledger.dissipated += heat;
        }
        deposits
    }

    fn thermal(&mut self, dt: f32, mode: Collide) {
        let env_t = self.env.temperature as f64;
        let h = self.env.convection_h() as f64;
        let dt = dt as f64;
        // conduction between touching bodies: series resistance of the two
        // half-thicknesses, R = L_a/(k_a A) + L_b/(k_b A), integrated exactly
        // for the pair (ΔT decays as exp(−t / (R·C_red)))
        if self.conduction {
            let n = self.bodies.len();
            let mut dq = vec![0.0f64; n];
            // positions are fixed during this loop, so pairs within TOUCH
            // are exactly the ones contact_patch accepts
            let pairs: Vec<(usize, usize)> = if mode.sweep(n) {
                sweep_pairs(&self.bodies, TOUCH * 2.0)
            } else {
                (0..n)
                    .flat_map(|i| (i + 1..n).map(move |j| (i, j)))
                    .collect()
            };
            {
                for (i, j) in pairs {
                    let (a, b) = (&self.bodies[i], &self.bodies[j]);
                    if a.vaporised || b.vaporised {
                        continue;
                    }
                    let Some((area, la, lb)) = contact_patch(a, b) else {
                        continue;
                    };
                    let (ka, kb) = (
                        a.env.thermal_conductivity.unwrap_or(1.0).max(1e-3) as f64,
                        b.env.thermal_conductivity.unwrap_or(1.0).max(1e-3) as f64,
                    );
                    let r = (la as f64 / ka + lb as f64 / kb) / area as f64;
                    let (ca, cb) = (a.heat_capacity(), b.heat_capacity());
                    let cred = ca * cb / (ca + cb);
                    let dtemp = a.temp - b.temp;
                    let moved = dtemp * cred * (1.0 - (-dt / (r * cred)).exp());
                    dq[i] -= moved;
                    dq[j] += moved;
                }
            }
            for (i, q) in dq.into_iter().enumerate() {
                if q != 0.0 {
                    self.ledger.conducted += q.abs() * 0.5;
                    self.heat_body(i, q);
                }
            }
        }
        for i in 0..self.bodies.len() {
            let b = &self.bodies[i];
            if b.vaporised {
                continue;
            }
            let a = b.surface_area() as f64;
            let t = b.temp;
            let heater = b.heater as f64 * dt;
            let mut ex = 0.0;
            if self.exchange {
                ex += h * a * (env_t - t);
                ex += b.env.emissivity as f64 * SIGMA * a * (env_t.powi(4) - t.powi(4));
            }
            let ex = ex * dt;
            self.ledger.heater_in += heater;
            self.ledger.exchange_in += ex;
            self.heat_body(i, heater + ex);
        }
    }

    pub fn total_kinetic(&self) -> f64 {
        self.bodies
            .iter()
            .filter(|b| !b.vaporised)
            .map(|b| b.kinetic())
            .sum()
    }

    pub fn total_potential(&self) -> f64 {
        self.bodies
            .iter()
            .filter(|b| !b.vaporised)
            .map(|b| b.mass as f64 * self.env.gravity as f64 * (b.pos[1] - b.half[1]) as f64)
            .sum()
    }
}

/// Touching faces of two boxes: (area m², half-thickness of a, of b) along
/// the contact axis, when they are within `TOUCH` of each other.
fn contact_patch(a: &Body, b: &Body) -> Option<(f32, f32, f32)> {
    let mut lap = [0.0f32; 3];
    let mut gap = [0.0f32; 3];
    for k in 0..3 {
        let lo = (a.pos[k] - a.half[k]).max(b.pos[k] - b.half[k]);
        let hi = (a.pos[k] + a.half[k]).min(b.pos[k] + b.half[k]);
        lap[k] = hi - lo;
        gap[k] = -(hi - lo);
    }
    if lap.iter().any(|l| *l < -TOUCH) {
        return None;
    }
    // the contact axis is the one with the smallest overlap
    let ax = (0..3)
        .min_by(|&x, &y| lap[x].partial_cmp(&lap[y]).unwrap())
        .unwrap();
    let _ = gap;
    let (u, v) = ((ax + 1) % 3, (ax + 2) % 3);
    let area = lap[u].max(0.0) * lap[v].max(0.0);
    (area > 1e-8).then_some((area, a.half[ax], b.half[ax]))
}

/// Resolve one box–box contact (position correction, restitution and
/// friction impulses); false when the pair does not touch.
fn resolve_pair(a: &mut Body, b: &mut Body, g: f32) -> bool {
    if a.vaporised || b.vaporised || (a.fixed && b.fixed) {
        return false;
    }
    let d = [
        b.pos[0] - a.pos[0],
        b.pos[1] - a.pos[1],
        b.pos[2] - a.pos[2],
    ];
    let ov = [
        a.half[0] + b.half[0] - d[0].abs(),
        a.half[1] + b.half[1] - d[1].abs(),
        a.half[2] + b.half[2] - d[2].abs(),
    ];
    if ov[0] <= 0.0 || ov[1] <= 0.0 || ov[2] <= 0.0 {
        return false;
    }
    // axis of least penetration
    let ax = if ov[0] < ov[1] && ov[0] < ov[2] {
        0
    } else if ov[1] < ov[2] {
        1
    } else {
        2
    };
    let mut nrm = [0.0; 3];
    nrm[ax] = if d[ax] >= 0.0 { 1.0 } else { -1.0 };
    let (ia, ib) = (
        if a.fixed { 0.0 } else { 1.0 / a.mass.max(1e-9) },
        if b.fixed { 0.0 } else { 1.0 / b.mass.max(1e-9) },
    );
    let it = ia + ib;
    if it <= 0.0 {
        return false;
    }
    // positional correction split by inverse mass
    let pen = ov[ax];
    for (k, n) in nrm.iter().enumerate() {
        a.pos[k] -= n * pen * ia / it;
        b.pos[k] += n * pen * ib / it;
    }
    let rv = [
        b.vel[0] - a.vel[0],
        b.vel[1] - a.vel[1],
        b.vel[2] - a.vel[2],
    ];
    let vn = dot(rv, nrm);
    if vn < 0.0 {
        let solid = a.phase == Phase::Solid && b.phase == Phase::Solid;
        let e = if solid {
            (a.env.restitution * b.env.restitution).sqrt()
        } else {
            0.0
        };
        let e = if -vn < 2.0 * g * DT + 1e-3 { 0.0 } else { e };
        let jn = -(1.0 + e) * vn / it;
        for (k, n) in nrm.iter().enumerate() {
            a.vel[k] -= jn * n * ia;
            b.vel[k] += jn * n * ib;
        }
        // friction on the tangential relative velocity
        let rv = [
            b.vel[0] - a.vel[0],
            b.vel[1] - a.vel[1],
            b.vel[2] - a.vel[2],
        ];
        let vn2 = dot(rv, nrm);
        let vt = [
            rv[0] - vn2 * nrm[0],
            rv[1] - vn2 * nrm[1],
            rv[2] - vn2 * nrm[2],
        ];
        let st = norm(vt);
        if st > 1e-9 {
            let mu = (a.env.friction_kinetic * b.env.friction_kinetic).sqrt();
            let jt = (st / it).min(mu * jn);
            for (k, v) in vt.iter().enumerate() {
                a.vel[k] += jt * v / st * ia;
                b.vel[k] -= jt * v / st * ib;
            }
        }
    }
    true
}

fn friction(v: &mut [f32; 3], jn: f32, mu_k: f32, _mu_s: f32, _g: f32) {
    let vt = (v[0] * v[0] + v[2] * v[2]).sqrt();
    if vt < 1e-9 {
        return;
    }
    let dv = (mu_k * jn).min(vt);
    v[0] -= dv * v[0] / vt;
    v[2] -= dv * v[2] / vt;
}

/// Put `e` joules into a body, honouring latent heat at phase boundaries.
pub fn apply_heat(b: &mut Body, mut e: f64, pressure: f64, time: f64, ev: &mut Events) {
    let c = b.heat_capacity();
    let m = b.mass.max(1e-9) as f64;
    let melt = if b.env.sublimes {
        None
    } else {
        b.env.melting_k.map(|t| t as f64)
    };
    let boil = b.env.boiling_at(pressure).map(|t| t as f64);
    let mut guard = 0;
    while e.abs() > 1e-12 && guard < 8 {
        guard += 1;
        // the next boundary in the direction of heat flow
        let (bound, latent) = match (e > 0.0, b.phase) {
            (true, Phase::Solid) => match (melt, b.env.sublimes) {
                (Some(t), false) => (Some(t), b.env.latent_fusion),
                _ => (boil, b.env.latent_vaporization.or(b.env.latent_fusion)),
            },
            (true, Phase::Liquid) => (boil, b.env.latent_vaporization),
            (false, Phase::Liquid) => (melt, b.env.latent_fusion),
            (false, Phase::Gas) => (boil, b.env.latent_vaporization),
            _ => (None, None),
        };
        let Some(tb) = bound else {
            // no boundary ahead (a solid cooling toward 0 K is clamped there;
            // the clamp never triggers at realistic temperatures)
            b.temp = (b.temp + e / c).max(0.0);
            return;
        };
        let to_bound = (tb - b.temp) * c; // J to reach the boundary (signed)
        if (e > 0.0 && e < to_bound) || (e < 0.0 && e > to_bound) {
            b.temp += e / c;
            return;
        }
        // reach the boundary, then bank latent heat
        e -= to_bound;
        b.temp = tb;
        let need = latent.unwrap_or(0.0) as f64 * m;
        b.latent += e;
        if b.latent.abs() < need {
            return;
        }
        // phase change complete
        let spill = b.latent - need * b.latent.signum();
        b.latent = 0.0;
        let from = b.phase;
        b.phase = match (from, spill >= 0.0) {
            (Phase::Solid, true) if b.env.sublimes || melt.is_none() => Phase::Gas,
            (Phase::Solid, true) => Phase::Liquid,
            (Phase::Liquid, true) => Phase::Gas,
            (Phase::Liquid, false) => Phase::Solid,
            (Phase::Gas, false) => Phase::Liquid,
            (p, _) => p,
        };
        on_phase(b, from, time, ev);
        e = spill;
        if b.vaporised {
            // the gas leaves with its excess heat banked at the boundary
            b.temp += e / c;
            return;
        }
    }
}

fn on_phase(b: &mut Body, from: Phase, time: f64, ev: &mut Events) {
    let to = b.phase;
    ev.log.push((
        time,
        format!(
            "{} ({}) {} → {} at {:.1} K",
            b.name,
            b.material,
            from.label(),
            to.label(),
            b.temp
        ),
    ));
    match to {
        Phase::Liquid => {
            // slump into a puddle of the same volume: quarter height, double width/depth
            let h = b.half0;
            b.half = [h[0] * 2.0, h[1] * 0.25, h[2] * 2.0];
            b.pos[1] -= h[1] * 0.75;
        }
        Phase::Solid => {
            // a refrozen puddle stays puddle-shaped (it froze where it lay)
        }
        Phase::Gas => {
            b.vaporised = true;
        }
    }
}

/// A pile of `n` small boxes of mixed metals and glass dropped in loose
/// layers over a 1 m-ish footprint per 25 bodies: the heavy-arena case for
/// benchmarks and the broad-phase tests. Deterministic for a given seed.
pub fn stress_pile(env: Environment, n: usize, seed: u64) -> Arena {
    use crate::material_pbr::{preset, preset_profile};
    const MATS: [&str; 5] = ["copper", "iron", "aluminum", "glass", "gold"];
    let metas: Vec<EnvMeta> = MATS
        .iter()
        .map(|m| preset_profile(&preset(m).expect("preset")).env)
        .collect();
    let mut a = Arena::new(env);
    let side = ((n as f32 / 4.0).sqrt().ceil() as usize).max(1);
    let mut r = seed | 1;
    let mut rnd = move || {
        r ^= r << 13;
        r ^= r >> 7;
        r ^= r << 17;
        (r % 10_000) as f32 / 10_000.0
    };
    for k in 0..n {
        let (layer, cell) = (k / (side * side), k % (side * side));
        let (cx, cz) = ((cell % side) as f32, (cell / side) as f32);
        let h = 0.08 + 0.06 * rnd();
        let half = [h, 0.05 + 0.05 * rnd(), h];
        let pos = [
            (cx - side as f32 / 2.0) * 0.3 + 0.05 * rnd(),
            0.2 + layer as f32 * 0.35 + 0.1 * rnd(),
            (cz - side as f32 / 2.0) * 0.3 + 0.05 * rnd(),
        ];
        let m = k % MATS.len();
        let mut b = Body::new(
            &format!("box{k}"),
            MATS[m],
            metas[m].clone(),
            half,
            pos,
            a.env.temperature + 40.0 * rnd(),
            a.env.pressure,
        );
        b.vel = [0.5 * (rnd() - 0.5), 0.0, 0.5 * (rnd() - 0.5)];
        a.add(b);
    }
    a
}

#[cfg(test)]
mod tests {

    fn state(a: &Arena) -> String {
        format!("{:?}", a)
    }

    #[test]
    fn sweep_and_prune_matches_all_pairs_bit_for_bit() {
        for (n, seed) in [(60, 1), (150, 7), (300, 99)] {
            let mut brute = stress_pile(Environment::earth(), n, seed);
            let mut swept = brute.clone();
            for s in 0..720 {
                brute.step_with(Collide::Brute);
                swept.step_with(Collide::Sweep);
                if s % 60 == 59 {
                    assert_eq!(state(&brute), state(&swept), "n {n} step {s}");
                }
            }
            // the pile actually collided and settled
            assert!(brute.ledger.dissipated > 0.0 && brute.ledger.conducted > 0.0);
        }
    }

    #[test]
    fn sweep_pairs_finds_exactly_the_touching_pairs() {
        let a = stress_pile(Environment::earth(), 200, 3);
        let n = a.bodies.len();
        let near = |i: usize, j: usize, m: f32| {
            (0..3).all(|k| {
                let (p, q) = (&a.bodies[i], &a.bodies[j]);
                (p.pos[k] - q.pos[k]).abs() <= p.half[k] + q.half[k] + 2.0 * m
            })
        };
        for m in [0.0, 0.01, 0.1] {
            let got = sweep_pairs(&a.bodies, m);
            let want: Vec<(usize, usize)> = (0..n)
                .flat_map(|i| (i + 1..n).map(move |j| (i, j)))
                .filter(|&(i, j)| near(i, j, m))
                .collect();
            assert_eq!(got, want, "margin {m}");
        }
    }

    #[test]
    fn advance_caps_catch_up_and_drops_the_backlog() {
        let mut a = stress_pile(Environment::moon(), 10, 5);
        assert_eq!(a.advance(5.0), MAX_CATCH_UP);
        // nothing left over to chase on the next frame
        assert_eq!(a.advance(0.0), 0);
        assert_eq!(a.advance(DT * 3.5), 3);
        let mut b = stress_pile(Environment::moon(), 10, 5);
        for _ in 0..MAX_CATCH_UP + 3 {
            b.step();
        }
        assert_eq!(a.time, b.time);
        assert_eq!(a.bodies.len(), b.bodies.len());
        for (x, y) in a.bodies.iter().zip(&b.bodies) {
            assert_eq!(x.pos, y.pos);
        }
    }

    use super::*;
    use crate::material_pbr::{preset, preset_profile};

    fn env_of(key: &str) -> EnvMeta {
        preset_profile(&preset(key).unwrap()).env
    }

    fn cube(name: &str, mat: &str, y: f32, env: &Environment) -> Body {
        Body::new(
            name,
            mat,
            env_of(mat),
            [0.5, 0.5, 0.5],
            [0.0, y, 0.0],
            env.temperature,
            env.pressure,
        )
    }

    #[test]
    fn with_volume_keeps_density() {
        let e = Environment::earth();
        let a = Body::new(
            "a",
            "aluminum",
            env_of("aluminum"),
            [0.05; 3],
            [0.0; 3],
            293.0,
            e.pressure,
        );
        let s = a.clone().with_volume(0.0005);
        assert!((s.volume - 0.0005).abs() < 1e-9);
        assert!((s.mass / s.volume - a.mass / a.volume).abs() < 1e-3);
        assert!(
            (s.mass - 1.35).abs() < 0.01,
            "half a 10 cm Al cube: {}",
            s.mass
        );
    }

    #[test]
    fn mass_is_density_times_volume() {
        let e = Environment::earth();
        let g = cube("g", "gold", 1.0, &e);
        assert!((g.mass - 19_300.0).abs() < 1.0);
        let a = Body::new(
            "a",
            "aluminum",
            env_of("aluminum"),
            [0.05, 0.05, 0.05],
            [0.0; 3],
            293.0,
            e.pressure,
        );
        assert!(
            (a.mass - 2.7).abs() < 0.01,
            "10 cm Al cube = 2.7 kg: {}",
            a.mass
        );
        // water at 20 °C is liquid (997), ice below 0 °C solid (917)
        let w = Body::new(
            "w",
            "water",
            env_of("water"),
            [0.5; 3],
            [0.0; 3],
            293.0,
            e.pressure,
        );
        let ice = Body::new(
            "i",
            "water",
            env_of("water"),
            [0.5; 3],
            [0.0; 3],
            263.0,
            e.pressure,
        );
        assert_eq!(w.phase, Phase::Liquid);
        assert_eq!(ice.phase, Phase::Solid);
        assert!((w.mass - 997.0).abs() < 1.0 && (ice.mass - 917.0).abs() < 1.0);
    }

    #[test]
    fn free_fall_matches_kinematics_in_vacuum_and_on_the_moon() {
        for env in [Environment::vacuum(), Environment::moon()] {
            let mut a = Arena::new(env.clone());
            a.ground = false;
            a.add(cube("hammer", "iron", 100.0, &env));
            a.add(Body::new(
                "feather",
                "graphite",
                env_of("graphite"),
                [0.1, 0.001, 0.02],
                [3.0, 100.0, 0.0],
                env.temperature,
                env.pressure,
            ));
            for _ in 0..480 {
                a.step();
            }
            let t = a.time as f32;
            let expect = 100.0 - 0.5 * env.gravity * t * t;
            for b in &a.bodies {
                assert!(
                    (b.pos[1] - expect).abs() < 0.02 * (100.0 - expect).max(1.0),
                    "{} {}: {} vs {}",
                    env.name,
                    b.name,
                    b.pos[1],
                    expect
                );
            }
            // hammer and feather land together without air
            assert!((a.bodies[0].pos[1] - a.bodies[1].pos[1]).abs() < 1e-3);
        }
    }

    #[test]
    fn air_drag_reaches_terminal_velocity() {
        let env = Environment::earth();
        let mut a = Arena::new(env.clone());
        a.ground = false;
        // 10 cm aluminium cube
        let mut b = Body::new(
            "al",
            "aluminum",
            env_of("aluminum"),
            [0.05; 3],
            [0.0, 5000.0, 0.0],
            env.temperature,
            env.pressure,
        );
        b.drag_cd = 1.05;
        a.add(b);
        a.exchange = false;
        for _ in 0..240 * 60 {
            a.step();
        }
        let b = &a.bodies[0];
        let area = 0.01;
        // v_t = sqrt(2 (m g − ρ V g) / (ρ C_d A))
        let vt = (2.0 * (b.mass - 1.225 * b.volume) * G0 / (1.225 * 1.05 * area)).sqrt();
        assert!(
            (-b.vel[1] - vt).abs() / vt < 0.01,
            "{} vs {}",
            -b.vel[1],
            vt
        );
        // feather vs hammer in air: the light, broad one is slower
        let mut a = Arena::new(env.clone());
        a.ground = false;
        a.add(cube("hammer", "iron", 100.0, &env));
        a.add(Body::new(
            "feather",
            "graphite",
            env_of("graphite"),
            [0.1, 0.001, 0.02],
            [3.0, 100.0, 0.0],
            env.temperature,
            env.pressure,
        ));
        for _ in 0..480 {
            a.step();
        }
        assert!(a.bodies[1].pos[1] > a.bodies[0].pos[1] + 5.0);
    }

    #[test]
    fn wind_pushes_and_buoyancy_lifts() {
        let mut env = Environment::earth();
        env.wind = [10.0, 0.0, 0.0];
        let mut a = Arena::new(env.clone());
        a.ground = false;
        a.exchange = false;
        a.add(Body::new(
            "sheet",
            "graphite",
            env_of("graphite"),
            [0.001, 0.1, 0.1],
            [0.0, 100.0, 0.0],
            env.temperature,
            env.pressure,
        ));
        for _ in 0..240 {
            a.step();
        }
        assert!(
            a.bodies[0].vel[0] > 1.0,
            "wind accelerates it downwind: {:?}",
            a.bodies[0].vel
        );
        // a body less dense than air rises (helium-like)
        let mut light = env_of("water");
        light.density = Some(0.18);
        light.density_liquid = Some(0.18);
        light.melting_k = None;
        light.boiling_k = None;
        let mut a = Arena::new(Environment::earth());
        a.ground = false;
        a.exchange = false;
        a.add(Body::new(
            "balloon",
            "helium-ish",
            light,
            [0.5; 3],
            [0.0, 1.0, 0.0],
            288.0,
            101_325.0,
        ));
        for _ in 0..240 {
            a.step();
        }
        assert!(a.bodies[0].pos[1] > 1.5, "{}", a.bodies[0].pos[1]);
    }

    #[test]
    fn bounce_height_is_e_squared() {
        let env = Environment::vacuum();
        let mut a = Arena::new(env.clone());
        let mut b = cube("ball", "glass", 2.5, &env);
        b.env.restitution = 0.7;
        a.add(b);
        a.exchange = false;
        let mut peak_after = 0.0f32;
        let mut bounced = false;
        for _ in 0..240 * 3 {
            a.step();
            let b = &a.bodies[0];
            if !bounced && b.vel[1] > 0.0 {
                bounced = true;
            }
            if bounced {
                peak_after = peak_after.max(b.pos[1] - 0.5);
            }
            if bounced && b.vel[1] < 0.0 && peak_after > 0.0 {
                break;
            }
        }
        let expect = 2.0 * 0.49;
        assert!(
            (peak_after - expect).abs() < 0.03,
            "{} vs {}",
            peak_after,
            expect
        );
    }

    #[test]
    fn friction_stops_a_sliding_block_in_v2_over_2_mu_g() {
        let env = Environment::vacuum();
        let mut a = Arena::new(env.clone());
        a.exchange = false;
        let mut b = cube("block", "iron", 0.5, &env);
        b.vel = [4.0, 0.0, 0.0];
        a.add(b);
        for _ in 0..240 * 5 {
            a.step();
        }
        let mu = env_of("iron").friction_kinetic;
        let expect = 16.0 / (2.0 * mu * G0);
        let got = a.bodies[0].pos[0];
        assert!(
            (got - expect).abs() / expect < 0.03,
            "{} vs {}",
            got,
            expect
        );
        assert!(a.bodies[0].vel[0].abs() < 1e-4);
    }

    #[test]
    fn head_on_collision_conserves_momentum() {
        let env = Environment::space();
        let mut a = Arena::new(env.clone());
        a.ground = false;
        a.exchange = false;
        let mut l = cube("l", "copper", 0.0, &env);
        let mut r = cube("r", "aluminum", 0.0, &env);
        l.pos = [-2.0, 0.0, 0.0];
        r.pos = [2.0, 0.0, 0.0];
        l.vel = [3.0, 0.0, 0.0];
        r.vel = [-1.0, 0.0, 0.0];
        a.add(l);
        a.add(r);
        let p0: f32 = a.bodies.iter().map(|b| b.mass * b.vel[0]).sum();
        for _ in 0..480 {
            a.step();
        }
        let p1: f32 = a.bodies.iter().map(|b| b.mass * b.vel[0]).sum();
        assert!((p1 - p0).abs() / p0.abs() < 1e-4, "{} {}", p0, p1);
        assert!(
            a.bodies[0].vel[0] < 3.0 && a.bodies[1].vel[0] > -1.0,
            "they bounced"
        );
        // relative speed scaled by e = √(e₁e₂)
        let e = (env_of("copper").restitution * env_of("aluminum").restitution).sqrt();
        let rel = a.bodies[1].vel[0] - a.bodies[0].vel[0];
        assert!((rel - 4.0 * e).abs() < 1e-3, "{} vs {}", rel, 4.0 * e);
    }

    #[test]
    fn ice_melts_and_boils_with_real_latent_heat() {
        // 1 kg-ish ice cube, insulated, 1 kW heater: time to 0 °C, melt, 100 °C, boil
        let env = Environment::earth();
        let mut a = Arena::new(env.clone());
        a.exchange = false;
        let mut ice = Body::new(
            "ice",
            "water",
            env_of("water"),
            [0.05, 0.05, 0.05],
            [0.0, 0.05, 0.0],
            263.15,
            env.pressure,
        );
        ice.heater = 1000.0;
        let m = ice.mass;
        a.add(ice);
        let cp = 4181.0; // preset uses liquid cp for both phases (documented)
        let t_warm = m * cp * 10.0 / 1000.0;
        let t_melt = m * 3.3355e5 / 1000.0;
        let t_heat = m * cp * 100.0 / 1000.0;
        let t_boil = m * 2.2565e6 / 1000.0;
        let mut melted_at = None;
        let mut boiled_at = None;
        while a.time < (t_warm + t_melt + t_heat + t_boil) as f64 * 1.05 {
            a.step();
            let b = &a.bodies[0];
            if melted_at.is_none() && b.phase == Phase::Liquid {
                melted_at = Some(a.time as f32);
            }
            if boiled_at.is_none() && b.vaporised {
                boiled_at = Some(a.time as f32);
            }
        }
        let melted_at = melted_at.expect("melted");
        let boiled_at = boiled_at.expect("boiled");
        assert!(
            (melted_at - (t_warm + t_melt)).abs() < 0.5,
            "melt {} vs {}",
            melted_at,
            t_warm + t_melt
        );
        let total = t_warm + t_melt + t_heat + t_boil;
        assert!(
            (boiled_at - total).abs() / total < 0.002,
            "boil {} vs {}",
            boiled_at,
            total
        );
        assert!(a
            .events
            .log
            .iter()
            .any(|(_, s)| s.contains("solid → liquid")));
        // a liquid puddle is flatter and wider
        let mut b2 = Body::new(
            "w",
            "water",
            env_of("water"),
            [0.05; 3],
            [0.0, 0.05, 0.0],
            272.0,
            env.pressure,
        );
        let mut ev = Events::default();
        apply_heat(&mut b2, 1e6, env.pressure, 0.0, &mut ev);
        assert_eq!(b2.phase, Phase::Liquid);
        assert!(b2.half[1] < 0.02 && b2.half[0] > 0.09);
    }

    #[test]
    fn cools_by_convection_and_radiation_toward_ambient() {
        let env = Environment::earth();
        let mut a = Arena::new(env.clone());
        a.add(Body::new(
            "hot",
            "copper",
            env_of("copper"),
            [0.05; 3],
            [0.0, 0.05, 0.0],
            600.0,
            env.pressure,
        ));
        let t0 = a.bodies[0].temp;
        for _ in 0..240 * 60 {
            a.step();
        }
        let t1 = a.bodies[0].temp;
        assert!(t1 < t0 && t1 > env.temperature as f64, "{} → {}", t0, t1);
        // exact first step: dT = (h A ΔT + εσA(T∞⁴−T⁴)) dt / (m c)
        let mut a = Arena::new(env.clone());
        a.add(Body::new(
            "hot",
            "copper",
            env_of("copper"),
            [0.05; 3],
            [0.0, 0.05, 0.0],
            600.0,
            env.pressure,
        ));
        a.step();
        let b = &a.bodies[0];
        let area = 0.06f64;
        let q = 10.0 * area * (288.15 - 600.0)
            + 0.04 * SIGMA * area * (288.15f64.powi(4) - 600f64.powi(4));
        let dt_expect = q * DT as f64 / (b.mass as f64 * 385.0);
        assert!(
            ((b.temp - 600.0) - dt_expect).abs() < 1e-4,
            "{} vs {}",
            b.temp - 600.0,
            dt_expect
        );
        // in vacuum there is no convection, only radiation: cools slower
        let mut v = Arena::new(Environment::vacuum());
        v.add(Body::new(
            "hot",
            "copper",
            env_of("copper"),
            [0.05; 3],
            [0.0, 0.05, 0.0],
            600.0,
            0.0,
        ));
        for _ in 0..240 * 60 {
            v.step();
        }
        assert!(v.bodies[0].temp > t1);
    }

    #[test]
    fn stacking_settles_and_energy_does_not_appear_from_nowhere() {
        let env = Environment::earth();
        let mut a = Arena::new(env.clone());
        a.exchange = false;
        a.add(cube("bottom", "iron", 0.5, &env));
        a.add(cube("top", "copper", 2.0, &env));
        let e0 = a.total_kinetic() + a.total_potential();
        let mut max_e = e0;
        for _ in 0..240 * 4 {
            a.step();
            max_e = max_e.max(a.total_kinetic() + a.total_potential());
        }
        assert!(
            max_e <= e0 * 1.001 + 1.0,
            "mechanical energy never increases: {} > {}",
            max_e,
            e0
        );
        let top = a.body("top").unwrap();
        assert!(
            (top.pos[1] - 1.5).abs() < 0.02,
            "rests on the bottom cube: {}",
            top.pos[1]
        );
        assert!(top.vel[1].abs() < 0.05);
    }

    /// Largest stock or flow magnitude, for relative residuals.
    fn scale_of(a: &Arena) -> f64 {
        let e = a.energy();
        let l = &a.ledger;
        [
            e.kinetic,
            e.potential.abs(),
            l.heater_in.abs(),
            l.exchange_in.abs(),
            l.drag_work.abs(),
            l.dissipated,
        ]
        .into_iter()
        .fold(1.0, f64::max)
    }

    #[test]
    fn energy_is_conserved_through_falls_bounces_stacking_drag_and_melting() {
        // Earth air (drag + buoyancy + convection + radiation), a bouncing
        // copper cube landing on an iron one, and a heated ice cube that melts
        let env = Environment::earth();
        let mut a = Arena::new(env.clone());
        a.add(cube("bottom", "iron", 0.5, &env));
        a.add(cube("top", "copper", 3.0, &env));
        let mut ice = Body::new(
            "ice",
            "water",
            env_of("water"),
            [0.1; 3],
            [3.0, 2.0, 0.0],
            268.15,
            env.pressure,
        );
        ice.heater = 300_000.0;
        a.add(ice);
        let mut worst = 0.0f64;
        for _ in 0..240 * 30 {
            a.step();
            worst = worst.max(a.energy_residual().abs() / scale_of(&a));
        }
        assert!(
            a.body("ice").unwrap().phase == Phase::Liquid,
            "the ice melted"
        );
        assert!(
            a.ledger.dissipated > 1000.0,
            "impacts made heat: {}",
            a.ledger.dissipated
        );
        assert!(a.ledger.drag_work < 0.0 && a.ledger.exchange_in != 0.0);
        // conserved up to f32 position/velocity rounding (m·g·δy with
        // δy ≈ 1e-7 m per step): ~1e-6 of the largest term
        assert!(worst < 1e-4, "relative residual {}", worst);
    }

    #[test]
    fn impact_heats_the_body_by_g_h_over_c() {
        // a perfectly inelastic lead-like block dropped 10 m in vacuum: all of
        // m g h ends up as heat, ΔT = g h / c
        let env = Environment::vacuum();
        let mut a = Arena::new(env.clone());
        a.exchange = false;
        let mut b = cube("block", "iron", 10.5, &env);
        b.env.restitution = 0.0;
        let t0 = b.temp;
        a.add(b);
        for _ in 0..240 * 3 {
            a.step();
        }
        let b = &a.bodies[0];
        let expect = G0 as f64 * 10.0 / b.specific_heat();
        assert!(b.vel[1].abs() < 1e-3);
        assert!(
            ((b.temp - t0) - expect).abs() / expect < 1e-3,
            "ΔT {} vs {}",
            b.temp - t0,
            expect
        );
        assert!(
            (a.ledger.dissipated - b.mass as f64 * G0 as f64 * 10.0).abs() / a.ledger.dissipated
                < 1e-3
        );
    }

    #[test]
    fn conduction_between_touching_blocks_matches_the_analytic_exponential() {
        // two 10 cm copper cubes side by side in deep space, 400 K and 300 K;
        // ΔT(t) = ΔT₀ exp(−t / (R C_red)), R = (L/k + L/k)/A
        let env = Environment::space();
        let mut a = Arena::new(env.clone());
        a.ground = false;
        a.exchange = false;
        let h = 0.05;
        let mut hot = Body::new(
            "hot",
            "copper",
            env_of("copper"),
            [h; 3],
            [-h, 0.0, 0.0],
            400.0,
            0.0,
        );
        let mut cold = Body::new(
            "cold",
            "copper",
            env_of("copper"),
            [h; 3],
            [h, 0.0, 0.0],
            300.0,
            0.0,
        );
        hot.fixed = true;
        cold.fixed = true;
        a.add(hot);
        a.add(cold);
        let k = env_of("copper").thermal_conductivity.unwrap() as f64;
        let area = (2.0 * h as f64).powi(2);
        let r = 2.0 * h as f64 / k / area;
        let c = a.bodies[0].heat_capacity();
        let tau = r * c / 2.0;
        let steps = (tau / DT as f64) as usize;
        for _ in 0..steps {
            a.step();
        }
        let dt_now = a.bodies[0].temp - a.bodies[1].temp;
        let expect = 100.0 * (-(a.time) / tau).exp();
        assert!(
            (dt_now - expect).abs() < 1e-6 * 100.0 + 1e-6,
            "{} vs {}",
            dt_now,
            expect
        );
        // the mean temperature is unchanged and nothing is lost
        assert!(((a.bodies[0].temp + a.bodies[1].temp) / 2.0 - 350.0).abs() < 1e-6);
        assert!(a.energy_residual().abs() < 1e-3, "{}", a.energy_residual());
        // separated blocks do not conduct
        a.bodies[1].pos[0] += 0.01;
        let before = a.bodies[1].temp;
        a.step();
        assert_eq!(a.bodies[1].temp, before);
    }

    #[test]
    fn hot_block_on_a_cold_block_with_air_balances_its_books() {
        let env = Environment::earth();
        let mut a = Arena::new(env.clone());
        a.add(cube("cold", "iron", 0.5, &env));
        let mut hot = cube("hot", "aluminum", 1.5, &env);
        hot.temp = 600.0;
        a.add(hot);
        for _ in 0..240 * 60 {
            a.step();
        }
        assert!(a.ledger.conducted > 1e5, "{}", a.ledger.conducted);
        assert!(a.ledger.exchange_in < 0.0, "it lost heat to the room");
        let rel = a.energy_residual().abs() / scale_of(&a);
        assert!(rel < 1e-4, "{}", rel);
    }
}
