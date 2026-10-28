//! Materials Lab, Phase 2 tabs: Reactions (thermochemistry + kinetics),
//! Circuits (MNA), Computer (transistors → gates → ALU → CPU) and
//! Relativity (SR readouts, N-body with a 1PN correction). The Elements
//! tab is `materials.rs`. All state changes arrive as `AppCommand`s
//! through `apply`; the panels only push commands.

use crate::commands::AppCommand;
use crate::theme::{self, Palette, SP_SM, SP_XS};
use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Stroke, Vec2};
use egui_phosphor::regular as ic;
use obs_rust_bitstream::chem::{self, Equation, Reactor, Thermo};
use obs_rust_bitstream::circuit::{self, Circuit, MosKind, Wave, Wire};
use obs_rust_bitstream::computer::{self, CellChar, Cpu, GateKind, Library, RunReport};
use obs_rust_bitstream::relativity::{self as rel, NBody, SrReadout};

pub const TABS: [&str; 5] = [
    "Elements",
    "Reactions",
    "Circuits",
    "Computer",
    "Relativity",
];

pub fn tab_index(name: &str) -> Option<usize> {
    let n = name.to_ascii_lowercase();
    let alias = match n.as_str() {
        "elements" | "periodic" | "periodic table" | "materials" => 0,
        "reactions" | "chemistry" | "chem" | "reaction" => 1,
        "circuits" | "circuit" | "electronics" => 2,
        "computer" | "cpu" => 3,
        "relativity" | "gravity" | "nbody" | "n-body" | "orbits" => 4,
        _ => return None,
    };
    Some(alias)
}

// ---------------------------------------------------------------- reactions

pub const REACTIONS: &[(&str, &str, f64)] = &[
    ("Methane flame", "CH4 + O2 -> CO2 + H2O", 1500.0),
    ("Methane at room T", "CH4 + O2 -> CO2 + H2O", 298.15),
    ("Hydrogen", "H2 + O2 -> H2O", 1000.0),
    ("Propane", "C3H8 + O2 -> CO2 + H2O", 1400.0),
    ("Thermite (ignited)", "Fe2O3 + Al -> Al2O3 + Fe", 1500.0),
    ("Limestone", "CaCO3 -> CaO + CO2", 1200.0),
    ("N2O4 ⇌ NO2", "N2O4 <=> NO2", 298.15),
    ("N2O5 decay", "N2O5 -> NO2 + O2", 318.15),
    ("Haber", "N2 + H2 -> NH3", 700.0),
    ("Rust", "Fe + O2 -> Fe2O3", 298.15),
];

pub struct ChemRun {
    pub eq: Equation,
    pub thermo: Thermo,
    pub thermo_std: Thermo,
    pub law: chem::RateLaw,
    pub t0: f64,
    pub p: f64,
    pub air: bool,
    pub seconds: f64,
    /// (time s, T K, conversion 0-1)
    pub trace: Vec<(f64, f64, f64)>,
    pub t_ad: Option<f64>,
    pub final_q_over_k: f64,
}

pub struct ChemUi {
    pub equation: String,
    pub temp_c: f64,
    pub pressure_kpa: f64,
    pub seconds: f64,
    pub air: bool,
    pub run: Option<ChemRun>,
    pub error: Option<String>,
}

fn run_reaction(src: &str, t: f64, p: f64, seconds: f64, air: bool) -> Result<ChemRun, String> {
    let eq = chem::parse_equation(src).map_err(|e| e.to_string())?;
    let thermo = chem::thermo(&eq, t).map_err(|e| e.to_string())?;
    let thermo_std = chem::thermo(&eq, chem::T0).map_err(|e| e.to_string())?;
    let mut r = Reactor::new(&eq, t, p, 1.0, air).map_err(|e| e.to_string())?;
    let law = r.law.clone();
    let n = 160;
    let t_first = (seconds * 1e-10).max(1e-12);
    let mut trace = vec![(0.0, r.t, 0.0)];
    let mut now = 0.0;
    for i in 0..n {
        let target = t_first * (seconds / t_first).powf(i as f64 / (n - 1) as f64);
        r.advance(target - now);
        now = target;
        trace.push((now, r.t, r.conversion()));
    }
    let t_ad = if thermo.exothermic() {
        chem::adiabatic_temperature(&eq, t, air).ok()
    } else {
        None
    };
    let q = r.quotient();
    let k_now = chem::thermo(&eq, r.t).map(|t| t.k).unwrap_or(thermo.k);
    Ok(ChemRun {
        eq,
        thermo,
        thermo_std,
        law,
        t0: t,
        p,
        air,
        seconds,
        trace,
        t_ad,
        final_q_over_k: q / k_now,
    })
}

// ---------------------------------------------------------------- circuits

pub const CIRCUITS: &[(&str, &str)] = &[
    ("divider", "Voltage divider (DC operating point)"),
    ("rc", "RC step response"),
    ("rlc", "Series RLC ringing"),
    ("diode", "Diode I–V curve"),
    ("inverter", "CMOS inverter VTC"),
    ("fuse", "Wire / fuse with Joule heating"),
    ("silicon", "Doped silicon resistivity"),
];

/// One plotted line: label, colour, points.
pub type Series = (String, Color32, Vec<(f64, f64)>);

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CircParams {
    pub voltage: f64,
    pub temp_c: f64,
    pub material: String,
    pub length_mm: f64,
    pub diameter_mm: f64,
    pub doping: f64,
}

pub struct CircRun {
    pub preset: String,
    pub title: String,
    pub xlabel: String,
    pub ylabel: String,
    pub series: Vec<Series>,
    pub series2: Option<(String, Vec<Series>)>,
    pub readouts: Vec<(String, String)>,
    pub events: Vec<String>,
    pub netlist: Vec<String>,
}

pub struct CircUi {
    pub preset: String,
    pub params: CircParams,
    pub run: Option<CircRun>,
    pub error: Option<String>,
}

const C_A: Color32 = Color32::from_rgb(90, 170, 255);
const C_B: Color32 = Color32::from_rgb(255, 150, 70);
const C_C: Color32 = Color32::from_rgb(120, 210, 120);
const C_D: Color32 = Color32::from_rgb(220, 110, 220);

fn describe(c: &Circuit) -> Vec<String> {
    use circuit::Elem as E;
    let n = |i: &usize| c.nodes[*i].clone();
    c.elems
        .iter()
        .map(|e| match e {
            E::R { a, b, r, wire, .. } => match wire {
                Some(w) => format!(
                    "R  {}–{}  {} wire {:.1} mm × ⌀{:.3} mm = {}",
                    n(a),
                    n(b),
                    w.symbol,
                    w.length * 1e3,
                    (w.area * 4.0 / std::f64::consts::PI).sqrt() * 1e3,
                    si(*r, "Ω")
                ),
                None => format!("R  {}–{}  {}", n(a), n(b), si(*r, "Ω")),
            },
            E::C { a, b, c, .. } => format!("C  {}–{}  {}", n(a), n(b), si(*c, "F")),
            E::L { a, b, l, .. } => format!("L  {}–{}  {}", n(a), n(b), si(*l, "H")),
            E::V { a, b, wave } => format!("V  {}–{}  {}", n(a), n(b), wave_label(wave)),
            E::I { a, b, wave } => format!("I  {}→{}  {}", n(a), n(b), wave_label(wave)),
            E::D { a, k, .. } => format!("D  {}→{}  Si diode (Shockley)", n(a), n(k)),
            E::M { d, g, s, p } => format!(
                "M  d {} g {} s {}  {} W/L {:.1}",
                n(d),
                n(g),
                n(s),
                if p.kind == MosKind::N { "NMOS" } else { "PMOS" },
                p.w_over_l
            ),
        })
        .collect()
}

fn wave_label(w: &Wave) -> String {
    match w {
        Wave::Dc(v) => format!("DC {}", si(*v, "V")),
        Wave::Pulse { v0, v1, delay, .. } => format!(
            "step {} → {} at {}",
            si(*v0, "V"),
            si(*v1, "V"),
            si(*delay, "s")
        ),
        Wave::Sine { offset, amp, freq } => format!(
            "{} + {} sin {}",
            si(*offset, "V"),
            si(*amp, "V"),
            si(*freq, "Hz")
        ),
    }
}

fn run_circuit(preset: &str, pr: &CircParams) -> Result<CircRun, String> {
    let t = pr.temp_c + 273.15;
    let e = |x: circuit::CircuitError| x.to_string();
    let mut readouts: Vec<(String, String)> = Vec::new();
    let mut run = CircRun {
        preset: preset.into(),
        title: String::new(),
        xlabel: "time (ms)".into(),
        ylabel: "V".into(),
        series: vec![],
        series2: None,
        readouts: vec![],
        events: vec![],
        netlist: vec![],
    };
    match preset {
        "divider" => {
            let mut c = Circuit::new();
            c.temp = t;
            let v = c.vsrc("in", "0", Wave::Dc(pr.voltage));
            let r1 = c.r("in", "out", 1000.0);
            let r2 = c.r("out", "0", 2000.0);
            c.dc().map_err(e)?;
            run.title = "Voltage divider: 1 kΩ over 2 kΩ".into();
            readouts.push(("V(out)".into(), si(c.voltage("out"), "V")));
            readouts.push(("Exact 2/3·V".into(), si(pr.voltage * 2.0 / 3.0, "V")));
            readouts.push(("I".into(), si(c.current(r1), "A")));
            readouts.push(("P(R1)".into(), si(c.power(r1), "W")));
            readouts.push(("P(R2)".into(), si(c.power(r2), "W")));
            readouts.push(("P(source)".into(), si(-c.power(v), "W")));
            let mut pts = vec![];
            for i in 0..=60 {
                let rl = 100.0 * 10f64.powf(i as f64 / 20.0);
                let mut c2 = Circuit::new();
                c2.vsrc("in", "0", Wave::Dc(pr.voltage));
                c2.r("in", "out", 1000.0);
                c2.r("out", "0", 2000.0);
                c2.r("out", "0", rl);
                c2.dc().map_err(e)?;
                pts.push((rl.log10(), c2.voltage("out")));
            }
            run.xlabel = "log₁₀ load (Ω) on the output".into();
            run.series.push(("V(out) with load".into(), C_A, pts));
            run.netlist = describe(&c);
        }
        "rc" | "rlc" => {
            let mut c = Circuit::new();
            c.temp = t;
            let wave = Wave::Pulse {
                v0: 0.0,
                v1: pr.voltage,
                delay: 0.2e-3,
                rise: 1e-6,
                width: 1.0,
                period: 0.0,
            };
            c.vsrc("in", "0", wave);
            let (tau, tend, h);
            if preset == "rc" {
                c.r("in", "out", 1000.0);
                c.c("out", "0", 1e-6);
                tau = 1e-3;
                tend = 6e-3;
                h = 5e-6;
                run.title = "RC low-pass, 1 kΩ · 1 µF (τ = 1 ms), step input".into();
            } else {
                c.r("in", "a", 20.0);
                c.l("a", "out", 10e-3);
                c.c("out", "0", 1e-6);
                tau = 2.0 * 10e-3 / 20.0;
                tend = 6e-3;
                h = 2e-6;
                run.title = "Series RLC 20 Ω · 10 mH · 1 µF, step input".into();
            }
            c.dc().map_err(e)?;
            let (mut vin, mut vout) = (vec![], vec![]);
            let mut peak: f64 = 0.0;
            let mut at_tau = None;
            while c.time < tend {
                c.step(h).map_err(e)?;
                vin.push((c.time * 1e3, c.voltage("in")));
                vout.push((c.time * 1e3, c.voltage("out")));
                peak = peak.max(c.voltage("out"));
                if at_tau.is_none() && c.time >= 0.2e-3 + tau {
                    at_tau = Some(c.voltage("out"));
                }
            }
            if preset == "rc" {
                let exact = 1.0 - (-1.0f64).exp();
                let sim = at_tau.unwrap_or(0.0) / pr.voltage;
                readouts.push(("τ = RC".into(), si(tau, "s")));
                readouts.push(("V(out)/V at t = τ".into(), format!("{:.4}", sim)));
                readouts.push(("Exact 1 − 1/e".into(), format!("{:.4}", exact)));
                readouts.push((
                    "Error (backward Euler, h = 5 µs)".into(),
                    format!("{:.2} %", (sim - exact).abs() / exact * 100.0),
                ));
            } else {
                let w0 = 1.0 / (10e-3f64 * 1e-6).sqrt();
                let zeta = 20.0 / 2.0 * (1e-6f64 / 10e-3).sqrt();
                let exact = 1.0 + (-zeta * std::f64::consts::PI / (1.0 - zeta * zeta).sqrt()).exp();
                readouts.push((
                    "f₀ = 1/2π√LC".into(),
                    si(w0 / (2.0 * std::f64::consts::PI), "Hz"),
                ));
                readouts.push(("Damping ζ".into(), format!("{:.3}", zeta)));
                readouts.push((
                    "Peak / V (simulated)".into(),
                    format!("{:.4}", peak / pr.voltage),
                ));
                readouts.push(("Peak / V (exact)".into(), format!("{:.4}", exact)));
                readouts.push(("Note".into(), "backward Euler damps slightly".into()));
            }
            run.series.push(("V(in)".into(), C_B, vin));
            run.series.push(("V(out)".into(), C_A, vout));
            run.netlist = describe(&c);
        }
        "diode" => {
            run.title = "Silicon diode, forward I–V (10 Ω series), at T and T + 50 K".into();
            run.xlabel = "V(diode) (V)".into();
            run.ylabel = "I (mA)".into();
            let mut net = vec![];
            let mut v1ma = vec![];
            for (k, tt) in [t, t + 50.0].iter().enumerate() {
                let mut pts = vec![];
                let mut at1 = None;
                for i in 0..=80 {
                    let vs = pr.voltage.abs().max(0.5) * i as f64 / 80.0;
                    let mut c = Circuit::new();
                    c.temp = *tt;
                    c.vsrc("in", "0", Wave::Dc(vs));
                    let r = c.r("in", "a", 10.0);
                    c.diode("a", "0");
                    c.dc().map_err(e)?;
                    let (vd, id) = (c.voltage("a"), c.current(r));
                    if at1.is_none() && id >= 1e-3 {
                        at1 = Some(vd);
                    }
                    pts.push((vd, id * 1e3));
                    if net.is_empty() {
                        net = describe(&c);
                    }
                }
                v1ma.push(at1);
                run.series.push((
                    format!("{:.0} °C", tt - 273.15),
                    if k == 0 { C_A } else { C_B },
                    pts,
                ));
            }
            run.netlist = net;
            if let (Some(a), Some(b)) = (v1ma[0], v1ma[1]) {
                readouts.push(("V at 1 mA".into(), si(a, "V")));
                readouts.push(("V at 1 mA, +50 K".into(), si(b, "V")));
                readouts.push((
                    "dV/dT".into(),
                    format!("{:.2} mV/K (Si ≈ −2)", (b - a) / 50.0 * 1e3),
                ));
            }
            readouts.push((
                "Thermal voltage kT/q".into(),
                si(circuit::thermal_voltage(t), "V"),
            ));
        }
        "inverter" => {
            let cc = computer::characterise(GateKind::Inv, t);
            let hot = computer::characterise(GateKind::Inv, t + 200.0);
            run.title = "CMOS inverter (180 nm-class NMOS W/L 2, PMOS W/L 4), VDD 1.8 V".into();
            run.xlabel = "V(in) (V)".into();
            run.ylabel = "V(out) (V)".into();
            run.series
                .push((format!("{:.0} °C", pr.temp_c), C_A, cc.vtc.clone()));
            run.series
                .push((format!("{:.0} °C", pr.temp_c + 200.0), C_B, hot.vtc.clone()));
            cell_readouts(&cc, &mut readouts);
            let c = computer::cell(
                GateKind::Inv,
                t,
                Wave::Dc(0.0),
                Wave::Dc(0.0),
                computer::C_LOAD,
            );
            run.netlist = describe(&c);
        }
        "fuse" => {
            let w = Wire::of(&pr.material, pr.length_mm * 1e-3, pr.diameter_mm * 1e-3).ok_or_else(|| format!("'{}' has no tabulated resistivity (try Cu, Al, Ag, Au, Fe, W, Ni, Pt, Sn, Pb, Zn)", pr.material))?;
            let r0 = w.resistance(t);
            let mut c = Circuit::new();
            c.temp = t;
            c.vsrc("in", "0", Wave::Dc(pr.voltage));
            c.r("in", "a", 0.01);
            let wi = c.wire("a", "0", w, 25.0, t);
            c.dc().map_err(e)?;
            run.title = format!(
                "{} wire {:.0} mm × ⌀{:.2} mm from a {} source (10 mΩ internal), still air",
                pr.material,
                pr.length_mm,
                pr.diameter_mm,
                si(pr.voltage, "V")
            );
            let (mut temp, mut cur) = (vec![], vec![]);
            let tau = match &c.elems[wi] {
                circuit::Elem::R {
                    thermal: Some(th), ..
                } => th.heat_capacity / th.g_ambient,
                _ => 1.0,
            };
            let tend = (6.0 * tau).clamp(0.05, 600.0);
            let h = tend / 20_000.0;
            let mut k = 0usize;
            let mut last_t = t;
            while c.time < tend {
                c.step(h).map_err(e)?;
                k += 1;
                let keep = k.is_multiple_of(10);
                if let circuit::Elem::R {
                    thermal: Some(th), ..
                } = &c.elems[wi]
                {
                    if keep || th.open {
                        temp.push((c.time, th.temp - 273.15));
                    }
                    last_t = th.temp;
                    if th.open {
                        cur.push((c.time, 0.0));
                        break;
                    }
                }
                if keep {
                    cur.push((c.time, c.current(wi)));
                }
            }
            run.xlabel = "time (s)".into();
            run.ylabel = "wire °C".into();
            run.series.push(("wire temperature".into(), C_B, temp));
            run.series2 = Some(("current (A)".into(), vec![("I".into(), C_A, cur)]));
            let (r_now, heat) = match &c.elems[wi] {
                circuit::Elem::R {
                    r,
                    thermal: Some(th),
                    ..
                } => (*r, th.heat),
                _ => (0.0, 0.0),
            };
            readouts.push(("R(T₀) = ρL/A".into(), si(r0, "Ω")));
            readouts.push(("Thermal τ = C/G".into(), si(tau, "s")));
            readouts.push(("Final wire T".into(), format!("{:.0} °C", last_t - 273.15)));
            readouts.push(("Final R".into(), si(r_now, "Ω")));
            readouts.push(("Joule heat".into(), si(heat, "J")));
            run.events = c
                .events
                .iter()
                .map(|(t, m)| format!("{:.4} s  {}", t, m))
                .collect();
            run.netlist = describe(&c);
        }
        "silicon" => {
            run.title = "n-type silicon: resistivity vs donor doping".into();
            run.xlabel = "log₁₀ N_D (cm⁻³)".into();
            run.ylabel = "log₁₀ ρ (Ω·cm)".into();
            for (k, tt) in [t, t + 200.0].iter().enumerate() {
                let pts: Vec<(f64, f64)> = (0..=60)
                    .map(|i| {
                        let nd = 10f64.powf(13.0 + 6.0 * i as f64 / 60.0);
                        (
                            nd.log10(),
                            circuit::doped_silicon(nd, 0.0, *tt).resistivity.log10(),
                        )
                    })
                    .collect();
                run.series.push((
                    format!("{:.0} °C", tt - 273.15),
                    if k == 0 { C_A } else { C_B },
                    pts,
                ));
            }
            let d = circuit::doped_silicon(pr.doping, 0.0, t);
            readouts.push(("N_D".into(), format!("{:.2e} cm⁻³", pr.doping)));
            readouts.push(("n_i (Varshni E_g)".into(), format!("{:.3e} cm⁻³", d.ni)));
            readouts.push(("electrons n".into(), format!("{:.3e} cm⁻³", d.n)));
            readouts.push(("holes p".into(), format!("{:.3e} cm⁻³", d.p)));
            readouts.push((
                "μ_n (Caughey–Thomas)".into(),
                format!("{:.0} cm²/V·s", d.mu_n),
            ));
            readouts.push(("ρ".into(), format!("{:.4e} Ω·cm", d.resistivity)));
            readouts.push(("E_g".into(), format!("{:.4} eV", circuit::si_band_gap(t))));
        }
        _ => {
            return Err(format!(
                "unknown circuit '{}' ({})",
                preset,
                CIRCUITS.iter().map(|c| c.0).collect::<Vec<_>>().join(", ")
            ))
        }
    }
    run.readouts = readouts;
    Ok(run)
}

fn cell_readouts(cc: &CellChar, r: &mut Vec<(String, String)>) {
    r.push((
        "V_OH / V_OL".into(),
        format!("{:.3} / {:.3} V", cc.voh, cc.vol),
    ));
    r.push((
        "V_IL / V_IH".into(),
        format!("{:.3} / {:.3} V", cc.vil, cc.vih),
    ));
    r.push(("Switching V_M".into(), format!("{:.3} V", cc.vm)));
    r.push((
        "Noise margins H / L".into(),
        format!("{:.3} / {:.3} V", cc.nm_high(), cc.nm_low()),
    ));
    r.push((
        "t_pHL / t_pLH (20 fF)".into(),
        format!(
            "{} / {}",
            cc.t_phl.map(|v| si(v, "s")).unwrap_or("—".into()),
            cc.t_plh.map(|v| si(v, "s")).unwrap_or("—".into())
        ),
    ));
    r.push(("Static leakage".into(), si(cc.leakage, "A")));
    r.push((
        "Works as logic".into(),
        if cc.works() {
            "yes".into()
        } else {
            "NO".into()
        },
    ));
}

// ---------------------------------------------------------------- computer

pub struct CpuState {
    pub program: String,
    pub source: String,
    pub expected: Vec<u8>,
    pub temp_c: f64,
    pub clock_mhz: f64,
    pub report: Option<RunReport>,
    pub cpu: Option<Cpu>,
    pub lib: Option<Library>,
    pub budget_ps: f64,
    /// (T K, worst settle ps, budget ps, correct)
    pub sweep: Vec<(f64, f64, f64, bool)>,
    pub sweep_fail: Option<String>,
    pub f_max: Option<f64>,
    pub follow: Option<String>,
    pub error: Option<String>,
}

fn run_cpu(
    src: &str,
    expected: &[u8],
    t: f64,
    clock_hz: f64,
) -> Result<(RunReport, Cpu, Library, f64), String> {
    let lib = Library::at(t);
    let report = computer::run_with(&lib, src, expected, clock_hz)?;
    let mut cpu = Cpu::new(computer::assemble(src)?, &lib, clock_hz);
    if report.gates_work {
        cpu.run(400);
    }
    let budget = (cpu.period_fs as f64 - cpu.setup_fs as f64 - cpu.clk_q_fs as f64) / 1e3;
    Ok((report, cpu, lib, budget))
}

/// Reference output of a custom program: 1 MHz at 300 K.
fn reference_output(src: &str) -> Result<Vec<u8>, String> {
    let lib = Library::at(300.0);
    let mut cpu = Cpu::new(computer::assemble(src)?, &lib, 1e6);
    cpu.run(400);
    if !cpu.halted {
        return Err("program does not halt within 400 cycles at 1 MHz / 27 °C".into());
    }
    Ok(cpu.out)
}

// ---------------------------------------------------------------- relativity

pub const SPEEDS: &[(&str, f64)] = &[
    ("Car 100 km/h", 27.78),
    ("Airliner", 250.0),
    ("GPS satellite", 3_874.0),
    ("ISS orbit", 7_660.0),
    ("Earth around Sun", 29_780.0),
    ("Parker Solar Probe", 191_700.0),
    ("Cosmic-ray muon", 0.995 * rel::C),
    ("LHC proton 6.8 TeV", 0.999_999_990 * rel::C),
];

pub struct RelState {
    pub beta: f64,
    pub preset: String,
    pub nb: NBody,
    pub gr: bool,
    pub exaggerate: f64,
    pub playing: bool,
    pub days_per_s: f64,
    pub extent: f64,
    pub peri0: Option<f64>,
    /// unwrapped perihelion advance since reset (rad) and the last raw angle
    pub peri_moved: f64,
    pub peri_last: f64,
    /// simulated seconds owed to the integrator (carried between frames)
    pub pending: f64,
    pub e0: f64,
    pub period: Option<f64>,
    pub precession: Option<(f64, f64)>,
    pub clock: f64,
}

fn nb_preset(name: &str, gr: bool, exaggerate: f64) -> Result<NBody, String> {
    let mut nb = rel::preset(name)
        .ok_or_else(|| format!("unknown preset '{}' ({})", name, rel::PRESETS.join(", ")))?;
    nb.gr = gr;
    if gr && name != "figure8" {
        nb.c = rel::C / exaggerate.max(1.0).sqrt();
    }
    nb.trail_every = nb.trail_every.max(1);
    Ok(nb)
}

// ---------------------------------------------------------------- state

pub struct LabState {
    pub tab: usize,
    pub chem: ChemUi,
    pub circ: CircUi,
    pub cpu: CpuState,
    pub rel: RelState,
}

fn set_program(cpu: &mut CpuState, name: &str) -> Result<(), String> {
    let (src, exp) = computer::program(name).ok_or_else(|| {
        format!(
            "unknown program '{}' (multiply, fibonacci, countdown), or pass source",
            name
        )
    })?;
    cpu.program = name.into();
    cpu.source = src.into();
    cpu.expected = exp.to_vec();
    Ok(())
}

impl LabState {
    pub fn new() -> Self {
        let (src, exp) = computer::program("fibonacci").unwrap();
        let mut s = Self {
            tab: 0,
            chem: ChemUi {
                equation: REACTIONS[0].1.into(),
                temp_c: REACTIONS[0].2 - 273.15,
                pressure_kpa: 101.325,
                seconds: 0.05,
                air: true,
                run: None,
                error: None,
            },
            circ: CircUi {
                preset: "fuse".into(),
                params: CircParams {
                    voltage: 1.0,
                    temp_c: 25.0,
                    material: "Cu".into(),
                    length_mm: 20.0,
                    diameter_mm: 0.1,
                    doping: 1e16,
                },
                run: None,
                error: None,
            },
            cpu: CpuState {
                program: "fibonacci".into(),
                source: src.into(),
                expected: exp.to_vec(),
                temp_c: 27.0,
                clock_mhz: 1000.0,
                report: None,
                cpu: None,
                lib: None,
                budget_ps: 0.0,
                sweep: vec![],
                sweep_fail: None,
                f_max: None,
                follow: None,
                error: None,
            },
            rel: RelState {
                beta: 0.995,
                preset: "mercury".into(),
                nb: nb_preset("mercury", true, 1.0).unwrap(),
                gr: true,
                exaggerate: 1.0,
                playing: false,
                days_per_s: 30.0,
                extent: 0.0,
                peri0: None,
                peri_moved: 0.0,
                peri_last: 0.0,
                pending: 0.0,
                e0: 0.0,
                period: None,
                precession: None,
                clock: 0.0,
            },
        };
        s.reset_nbody();
        s
    }

    fn reset_nbody(&mut self) {
        let r = &mut self.rel;
        r.extent = 0.0;
        r.peri0 = (r.nb.bodies.len() == 2).then(|| r.nb.perihelion_angle(1));
        r.peri_moved = 0.0;
        r.pending = 0.0;
        r.peri_last = r.peri0.unwrap_or(0.0);
        r.e0 = r.nb.energy_g();
        r.period = if r.nb.bodies.len() == 2 {
            let (a, b) = (&r.nb.bodies[0], &r.nb.bodies[1]);
            let mu = a.gm + b.gm;
            let v = sub(b.vel, a.vel);
            let sma = 1.0 / (2.0 / norm(sub(b.pos, a.pos)) - dot(v, v) / mu);
            (sma > 0.0).then(|| 2.0 * std::f64::consts::PI * (sma * sma * sma / mu).sqrt())
        } else {
            None
        };
    }

    pub fn context(&self) -> String {
        let mut s = format!("Lab tab: {}. ", TABS[self.tab]);
        if let Some(r) = &self.chem.run {
            let last = r.trace.last().unwrap();
            s.push_str(&format!("Reaction {} at {:.0} K: ΔH {:.1} kJ/mol, ΔG {:.1} kJ/mol, K {:.3e}, conversion after {} s {:.4}, final T {:.0} K. ", r.eq.display(), r.t0, r.thermo.dh / 1e3, r.thermo.dg / 1e3, r.thermo.k, r.seconds, last.2, last.1));
        }
        if let Some(r) = &self.circ.run {
            s.push_str(&format!(
                "Circuit {}: {}. ",
                r.preset,
                r.readouts
                    .iter()
                    .map(|(k, v)| format!("{} {}", k, v))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        if let Some(r) = &self.cpu.report {
            s.push_str(&format!(
                "CPU '{}' at {:.0} °C, {:.0} MHz: output {:?} (expected {:?}), {}. ",
                self.cpu.program,
                r.t - 273.15,
                r.clock_hz / 1e6,
                r.out,
                r.expected,
                r.failure.clone().unwrap_or_else(|| "correct".into())
            ));
        }
        let sr = rel::sr_readout(self.rel.beta);
        s.push_str(&format!(
            "SR: β {:.6}, γ {:.6}. N-body '{}' (GR {}), t = {:.1} days.",
            sr.beta,
            sr.gamma,
            self.rel.nb.name,
            self.rel.gr,
            self.rel.nb.time / rel::DAY
        ));
        s
    }

    /// Execute a lab command. `None` = not a lab command. `die_temp` is the
    /// arena temperature (K) of the named die object, resolved by the app.
    pub fn apply(
        &mut self,
        cmd: &AppCommand,
        die_temp: Option<f64>,
    ) -> Option<Result<String, String>> {
        use AppCommand as C;
        Some(match cmd {
            C::LabTab { tab } => match tab_index(tab) {
                Some(i) => {
                    self.tab = i;
                    Ok(format!("Lab tab: {}", TABS[i]))
                }
                None => Err(format!("unknown lab tab '{}' ({})", tab, TABS.join(", "))),
            },
            C::ChemReact {
                equation,
                temperature_c,
                pressure_kpa,
                seconds,
                air,
            } => {
                self.tab = 1;
                let c = &mut self.chem;
                if let Some(e) = equation {
                    c.equation = e.clone();
                }
                if let Some(t) = temperature_c {
                    c.temp_c = *t;
                }
                if let Some(p) = pressure_kpa {
                    c.pressure_kpa = *p;
                }
                if let Some(s) = seconds {
                    c.seconds = *s;
                }
                if let Some(a) = air {
                    c.air = *a;
                }
                let t = c.temp_c + 273.15;
                if t <= 0.0 || c.pressure_kpa <= 0.0 || c.seconds <= 0.0 {
                    return Some(Err(
                        "temperature must be above 0 K, pressure and time positive".into(),
                    ));
                }
                match run_reaction(
                    &c.equation,
                    t,
                    c.pressure_kpa * 1e3,
                    c.seconds.min(1e7),
                    c.air,
                ) {
                    Ok(r) => {
                        let last = *r.trace.last().unwrap();
                        let msg = format!("{}: ΔH° {:.1} kJ/mol, ΔG({:.0} K) {:.1} kJ/mol, K {:.3e}; after {} s conversion {:.2} %, T {:.0} K{}", r.eq.display(), r.thermo_std.dh / 1e3, t, r.thermo.dg / 1e3, r.thermo.k, r.seconds, last.2 * 100.0, last.1, r.t_ad.map(|ta| format!(", adiabatic flame {:.0} K", ta)).unwrap_or_default());
                        c.run = Some(r);
                        c.error = None;
                        Ok(msg)
                    }
                    Err(e) => {
                        c.error = Some(e.clone());
                        Err(e)
                    }
                }
            }
            C::CircuitRun {
                preset,
                voltage,
                temperature_c,
                material,
                length_mm,
                diameter_mm,
                doping_cm3,
            } => {
                self.tab = 2;
                if let Some(p) = preset {
                    self.circ.preset = p.clone();
                }
                let pr = &mut self.circ.params;
                if let Some(v) = voltage {
                    pr.voltage = *v;
                }
                if let Some(t) = temperature_c {
                    pr.temp_c = *t;
                }
                if let Some(m) = material {
                    pr.material = m.clone();
                }
                if let Some(l) = length_mm {
                    pr.length_mm = l.max(0.01);
                }
                if let Some(d) = diameter_mm {
                    pr.diameter_mm = d.max(0.001);
                }
                if let Some(n) = doping_cm3 {
                    pr.doping = n.clamp(1e10, 1e21);
                }
                if pr.temp_c <= -273.15 {
                    return Some(Err("temperature must be above absolute zero".into()));
                }
                match run_circuit(&self.circ.preset, &self.circ.params) {
                    Ok(r) => {
                        let msg = format!(
                            "{}: {}{}",
                            r.title,
                            r.readouts
                                .iter()
                                .take(4)
                                .map(|(k, v)| format!("{} {}", k, v))
                                .collect::<Vec<_>>()
                                .join(", "),
                            r.events
                                .first()
                                .map(|e| format!("; {}", e))
                                .unwrap_or_default()
                        );
                        self.circ.run = Some(r);
                        self.circ.error = None;
                        Ok(msg)
                    }
                    Err(e) => {
                        self.circ.error = Some(e.clone());
                        Err(e)
                    }
                }
            }
            C::CpuRun {
                program,
                source,
                temperature_c,
                clock_mhz,
                die_object,
            } => {
                self.tab = 3;
                if let Some(s) = source {
                    match reference_output(s) {
                        Ok(out) => {
                            self.cpu.program = "custom".into();
                            self.cpu.source = s.clone();
                            self.cpu.expected = out;
                        }
                        Err(e) => {
                            self.cpu.error = Some(e.clone());
                            return Some(Err(e));
                        }
                    }
                } else if let Some(p) = program {
                    if let Err(e) = set_program(&mut self.cpu, p) {
                        return Some(Err(e));
                    }
                }
                if let Some(t) = temperature_c {
                    self.cpu.temp_c = *t;
                }
                if let Some(f) = clock_mhz {
                    if *f <= 0.0 || *f > 100_000.0 {
                        return Some(Err("clock must be 0-100000 MHz".into()));
                    }
                    self.cpu.clock_mhz = *f;
                }
                match die_object {
                    Some(o) if o.is_empty() => self.cpu.follow = None,
                    Some(o) => match die_temp {
                        Some(t) => {
                            self.cpu.follow = Some(o.clone());
                            self.cpu.temp_c = t - 273.15;
                        }
                        None => {
                            return Some(Err(format!(
                                "'{}' is not a body in the physics arena (press Play in 3D first)",
                                o
                            )))
                        }
                    },
                    None => {}
                }
                self.rerun_cpu()
            }
            C::CpuHeatSweep { program, clock_mhz } => {
                self.tab = 3;
                if let Some(p) = program {
                    if let Err(e) = set_program(&mut self.cpu, p) {
                        return Some(Err(e));
                    }
                }
                if let Some(f) = clock_mhz {
                    self.cpu.clock_mhz = f.clamp(0.001, 100_000.0);
                }
                let mut out = vec![];
                let mut fail = None;
                let mut t = 300.0;
                let step = if self.cpu.clock_mhz > 200.0 {
                    10.0
                } else {
                    25.0
                };
                while t <= 1000.0 {
                    match run_cpu(
                        &self.cpu.source,
                        &self.cpu.expected,
                        t,
                        self.cpu.clock_mhz * 1e6,
                    ) {
                        Ok((r, _, _, budget)) => {
                            out.push((t, r.worst_settle_ps, budget, r.correct));
                            if !r.correct {
                                fail = Some(format!(
                                    "fails at {:.0} °C: {}",
                                    t - 273.15,
                                    r.failure.unwrap_or_default()
                                ));
                                break;
                            }
                        }
                        Err(e) => return Some(Err(e)),
                    }
                    t += step;
                }
                self.cpu.f_max = if self.cpu.program == "custom" {
                    None
                } else {
                    computer::max_clock(&self.cpu.program, self.cpu.temp_c + 273.15).ok()
                };
                self.cpu.sweep = out;
                self.cpu.sweep_fail = fail.clone();
                Ok(format!(
                    "heat sweep at {:.0} MHz: {}",
                    self.cpu.clock_mhz,
                    fail.unwrap_or_else(|| "works up to 727 °C".into())
                ))
            }
            C::RelSpeed { beta, speed_kms } => {
                self.tab = 4;
                let b = match (beta, speed_kms) {
                    (Some(b), _) => *b,
                    (None, Some(v)) => v * 1e3 / rel::C,
                    _ => return Some(Err("give beta (v/c) or speed_kms".into())),
                };
                if !(0.0..1.0).contains(&b) {
                    return Some(Err("speed must be below c (0 ≤ β < 1)".into()));
                }
                self.rel.beta = b;
                let r = rel::sr_readout(b);
                Ok(format!("β = {:.9}: γ = {:.9}, moving clock runs {:.9} s per lab second, 1 m contracts to {:.6} m", b, r.gamma, r.tau_per_lab_second, rel::contracted_length(1.0, b)))
            }
            C::NbodyPreset {
                preset,
                gr,
                gr_exaggeration,
            } => {
                self.tab = 4;
                let name = preset.clone().unwrap_or_else(|| self.rel.preset.clone());
                let g = gr.unwrap_or(self.rel.gr);
                let x = gr_exaggeration
                    .unwrap_or(self.rel.exaggerate)
                    .clamp(1.0, 1e8);
                match nb_preset(&name, g, x) {
                    Ok(nb) => {
                        self.rel.nb = nb;
                        self.rel.preset = name;
                        self.rel.gr = g;
                        self.rel.exaggerate = x;
                        self.rel.precession = None;
                        self.reset_nbody();
                        Ok(format!(
                            "{}: {} bodies, GR {}{}",
                            self.rel.nb.name,
                            self.rel.nb.bodies.len(),
                            if g { "on (1PN)" } else { "off (Newton)" },
                            if g && x > 1.0 {
                                format!(", exaggerated ×{:.0} (c reduced to c/√{:.0})", x, x)
                            } else {
                                String::new()
                            }
                        ))
                    }
                    Err(e) => Err(e),
                }
            }
            C::NbodyPlay {
                playing,
                days_per_second,
            } => {
                self.tab = 4;
                self.rel.playing = playing.unwrap_or(!self.rel.playing);
                if let Some(d) = days_per_second {
                    self.rel.days_per_s = d.clamp(1e-4, 1e4);
                }
                Ok(format!(
                    "N-body {} at {} days/s",
                    if self.rel.playing {
                        "playing"
                    } else {
                        "paused"
                    },
                    self.rel.days_per_s
                ))
            }
            C::NbodyMeasurePrecession => {
                self.tab = 4;
                let (m, p) = rel::mercury_precession(5, 20_000);
                let per_century = 100.0 * rel::JULIAN_YEAR / (87.969 * rel::DAY) / rel::ARCSEC;
                self.rel.precession = Some((m * per_century, p * per_century));
                Ok(format!("Mercury perihelion, simulated GR part: {:.2}″/century (analytic 6πGM/(c²a(1−e²)): {:.2}″/century; observed anomaly 42.98 ± 0.04″)", m * per_century, p * per_century))
            }
            _ => return None,
        })
    }

    fn rerun_cpu(&mut self) -> Result<String, String> {
        let t = self.cpu.temp_c + 273.15;
        if t <= 0.0 {
            return Err("temperature must be above absolute zero".into());
        }
        match run_cpu(
            &self.cpu.source,
            &self.cpu.expected,
            t,
            self.cpu.clock_mhz * 1e6,
        ) {
            Ok((r, cpu, lib, budget)) => {
                let msg = format!(
                    "{} at {:.0} °C, {:.0} MHz: output {:?} — {}",
                    self.cpu.program,
                    self.cpu.temp_c,
                    self.cpu.clock_mhz,
                    r.out,
                    r.failure.clone().unwrap_or_else(|| format!(
                        "correct (carry chain {:.0} ps of {:.0} ps budget)",
                        r.worst_settle_ps, budget
                    ))
                );
                self.cpu.report = Some(r);
                self.cpu.cpu = Some(cpu);
                self.cpu.lib = Some(lib);
                self.cpu.budget_ps = budget;
                self.cpu.error = None;
                Ok(msg)
            }
            Err(e) => {
                self.cpu.error = Some(e.clone());
                Err(e)
            }
        }
    }

    /// Die follows an arena body: re-run when its temperature moves > 5 K.
    pub fn follow_die(&mut self, t_k: f64) -> Option<String> {
        if (t_k - 273.15 - self.cpu.temp_c).abs() > 5.0 || self.cpu.report.is_none() {
            self.cpu.temp_c = t_k - 273.15;
            return self.rerun_cpu().ok();
        }
        None
    }

    /// Advance the N-body clock; returns whether a repaint is needed.
    pub fn tick(&mut self, dt: f32) -> bool {
        self.rel.clock += dt as f64;
        if !self.rel.playing {
            return self.tab == 4;
        }
        let r = &mut self.rel;
        let unit = if r.preset == "figure8" {
            1.0 / 30.0
        } else {
            rel::DAY
        };
        r.pending = (r.pending + r.days_per_s * unit * dt.min(0.1) as f64).min(4000.0 * r.nb.dt);
        let steps = (r.pending / r.nb.dt).floor() as usize;
        r.pending -= steps as f64 * r.nb.dt;
        // step in slices so the perihelion angle can be unwrapped
        let slice = 64;
        let mut done = 0;
        while done < steps {
            let k = slice.min(steps - done);
            for _ in 0..k {
                r.nb.step();
            }
            done += k;
            if r.peri0.is_some() {
                let a = r.nb.perihelion_angle(1);
                r.peri_moved += wrap(a - r.peri_last);
                r.peri_last = a;
            }
        }
        true
    }
}

// ---------------------------------------------------------------- project files

/// Reactions inputs in a `.seraphina` project (results are re-simulated).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChemFile {
    pub equation: String,
    pub temp_c: f64,
    pub pressure_kpa: f64,
    pub seconds: f64,
    pub air: bool,
    pub ran: bool,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CircFile {
    pub preset: String,
    pub params: CircParams,
    pub ran: bool,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CpuFile {
    pub program: String,
    pub source: String,
    pub expected: Vec<u8>,
    pub temp_c: f64,
    pub clock_mhz: f64,
    pub ran: bool,
    pub sweep: Vec<(f64, f64, f64, bool)>,
    pub sweep_fail: Option<String>,
    pub f_max: Option<f64>,
    pub follow: Option<String>,
}

/// The relativity scene, including the integrator state, so a saved
/// orbit continues exactly where it stopped.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RelFile {
    pub beta: f64,
    pub preset: String,
    pub nb: NBody,
    pub gr: bool,
    pub exaggerate: f64,
    pub playing: bool,
    pub days_per_s: f64,
    pub extent: f64,
    pub peri0: Option<f64>,
    pub peri_moved: f64,
    pub peri_last: f64,
    pub pending: f64,
    pub e0: f64,
    pub period: Option<f64>,
    pub precession: Option<(f64, f64)>,
}

/// Materials Lab (reactions, circuits, computer, relativity).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LabFile {
    pub tab: usize,
    pub chem: ChemFile,
    pub circ: CircFile,
    pub cpu: CpuFile,
    pub rel: RelFile,
}

impl LabState {
    pub fn to_file(&self) -> LabFile {
        let (c, k, u, r) = (&self.chem, &self.circ, &self.cpu, &self.rel);
        LabFile {
            tab: self.tab,
            chem: ChemFile {
                equation: c.equation.clone(),
                temp_c: c.temp_c,
                pressure_kpa: c.pressure_kpa,
                seconds: c.seconds,
                air: c.air,
                ran: c.run.is_some(),
            },
            circ: CircFile {
                preset: k.preset.clone(),
                params: k.params.clone(),
                ran: k.run.is_some(),
            },
            cpu: CpuFile {
                program: u.program.clone(),
                source: u.source.clone(),
                expected: u.expected.clone(),
                temp_c: u.temp_c,
                clock_mhz: u.clock_mhz,
                ran: u.report.is_some(),
                sweep: u.sweep.clone(),
                sweep_fail: u.sweep_fail.clone(),
                f_max: u.f_max,
                follow: u.follow.clone(),
            },
            rel: RelFile {
                beta: r.beta,
                preset: r.preset.clone(),
                nb: r.nb.clone(),
                gr: r.gr,
                exaggerate: r.exaggerate,
                playing: r.playing,
                days_per_s: r.days_per_s,
                extent: r.extent,
                peri0: r.peri0,
                peri_moved: r.peri_moved,
                peri_last: r.peri_last,
                pending: r.pending,
                e0: r.e0,
                period: r.period,
                precession: r.precession,
            },
        }
    }

    /// Restore a saved lab; simulations that had results are re-run
    /// (they are deterministic), so their plots come back too.
    pub fn load_file(&mut self, f: LabFile) -> Result<(), String> {
        if f.tab >= TABS.len() {
            return Err(format!("unknown lab tab {}", f.tab));
        }
        let mut s = Self::new();
        s.tab = f.tab;
        s.chem.equation = f.chem.equation;
        s.chem.temp_c = f.chem.temp_c;
        s.chem.pressure_kpa = f.chem.pressure_kpa;
        s.chem.seconds = f.chem.seconds;
        s.chem.air = f.chem.air;
        if f.chem.ran {
            match run_reaction(
                &s.chem.equation,
                s.chem.temp_c + 273.15,
                s.chem.pressure_kpa * 1e3,
                s.chem.seconds.min(1e7),
                s.chem.air,
            ) {
                Ok(r) => s.chem.run = Some(r),
                Err(e) => s.chem.error = Some(e),
            }
        }
        s.circ.preset = f.circ.preset;
        s.circ.params = f.circ.params;
        if f.circ.ran {
            match run_circuit(&s.circ.preset, &s.circ.params) {
                Ok(r) => s.circ.run = Some(r),
                Err(e) => s.circ.error = Some(e),
            }
        }
        s.cpu.program = f.cpu.program;
        s.cpu.source = f.cpu.source;
        s.cpu.expected = f.cpu.expected;
        s.cpu.temp_c = f.cpu.temp_c;
        s.cpu.clock_mhz = f.cpu.clock_mhz;
        s.cpu.report = None;
        s.cpu.cpu = None;
        s.cpu.lib = None;
        if f.cpu.ran {
            let _ = s.rerun_cpu();
        }
        s.cpu.sweep = f.cpu.sweep;
        s.cpu.sweep_fail = f.cpu.sweep_fail;
        s.cpu.f_max = f.cpu.f_max;
        s.cpu.follow = f.cpu.follow;
        let r = f.rel;
        s.rel = RelState {
            beta: r.beta,
            preset: r.preset,
            nb: r.nb,
            gr: r.gr,
            exaggerate: r.exaggerate,
            playing: r.playing,
            days_per_s: r.days_per_s,
            extent: r.extent,
            peri0: r.peri0,
            peri_moved: r.peri_moved,
            peri_last: r.peri_last,
            pending: r.pending,
            e0: r.e0,
            period: r.period,
            precession: r.precession,
            clock: 0.0,
        };
        *self = s;
        Ok(())
    }
}

// ---------------------------------------------------------------- UI

impl LabState {
    pub fn tab_strip(&self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        ui.add_space(SP_XS);
        ui.horizontal(|ui| {
            ui.add_space(SP_SM);
            if let Some(t) = theme::segmented(ui, p, "lab_tabs", &TABS, self.tab, 96.0) {
                bus.push(AppCommand::LabTab {
                    tab: TABS[t].to_ascii_lowercase(),
                });
            }
        });
    }

    pub fn tools(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        match self.tab {
            1 => self.chem_tools(ui, p, bus),
            2 => self.circ_tools(ui, p, bus),
            3 => self.cpu_tools(ui, p, bus),
            _ => self.rel_tools(ui, p, bus),
        }
    }

    pub fn center(&mut self, ui: &mut egui::Ui, p: &Palette) {
        egui::ScrollArea::vertical()
            .id_salt("lab_center")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(SP_SM);
                match self.tab {
                    1 => self.chem_center(ui, p),
                    2 => self.circ_center(ui, p),
                    3 => self.cpu_center(ui, p),
                    _ => self.rel_center(ui, p),
                }
            });
    }

    pub fn inspector(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        match self.tab {
            1 => self.chem_inspector(ui, p),
            2 => self.circ_inspector(ui, p),
            3 => self.cpu_inspector(ui, p),
            _ => self.rel_inspector(ui, p, bus),
        }
    }

    // ---- reactions

    fn chem_tools(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        let run = |s: &ChemUi| AppCommand::ChemReact {
            equation: Some(s.equation.clone()),
            temperature_c: Some(s.temp_c),
            pressure_kpa: Some(s.pressure_kpa),
            seconds: Some(s.seconds),
            air: Some(s.air),
        };
        theme::card(ui, p, "Reaction", |ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.chem.equation)
                    .hint_text("e.g. CH4 + O2 -> CO2 + H2O")
                    .desired_width(f32::INFINITY),
            );
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                bus.push(run(&self.chem));
            }
            ui.label(theme::muted(
                p,
                "Unbalanced is fine: it is balanced exactly. Phase tags (g) (l) (s) optional.",
            ));
            egui::Grid::new("chem_params")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Start T (°C)");
                    ui.add(
                        egui::DragValue::new(&mut self.chem.temp_c)
                            .speed(5.0)
                            .range(-270.0..=5000.0),
                    );
                    ui.end_row();
                    ui.label("Pressure (kPa)");
                    ui.add(
                        egui::DragValue::new(&mut self.chem.pressure_kpa)
                            .speed(1.0)
                            .range(0.01..=1e6),
                    );
                    ui.end_row();
                    ui.label("Duration (s)");
                    ui.add(
                        egui::DragValue::new(&mut self.chem.seconds)
                            .speed(0.01)
                            .range(1e-6..=1e7),
                    );
                    ui.end_row();
                });
            ui.checkbox(&mut self.chem.air, "Burn in air (O₂ brings 3.76 N₂)");
            if ui
                .add_sized(
                    [ui.available_width(), 28.0],
                    theme::primary_button(p, &format!("{}  React", ic::FLASK)),
                )
                .clicked()
            {
                bus.push(run(&self.chem));
            }
        });
        theme::card(ui, p, "Examples", |ui| {
            for (name, eq, t) in REACTIONS {
                let on = self.chem.equation == *eq && (self.chem.temp_c + 273.15 - t).abs() < 0.5;
                if ui
                    .add_sized(
                        [ui.available_width(), 22.0],
                        theme::toggle_button(p, name, on),
                    )
                    .on_hover_text(*eq)
                    .clicked()
                {
                    let air = eq.contains("O2 ->") && !eq.starts_with("N2O5");
                    bus.push(AppCommand::ChemReact {
                        equation: Some((*eq).into()),
                        temperature_c: Some(t - 273.15),
                        pressure_kpa: Some(101.325),
                        seconds: Some(if *t > 900.0 { 0.05 } else { 600.0 }),
                        air: Some(air),
                    });
                }
            }
        });
    }

    fn chem_center(&mut self, ui: &mut egui::Ui, p: &Palette) {
        heading(ui, p, ic::FLASK, "Reaction engine", "Thermochemistry from CODATA / NIST-JANAF / CRC; kinetics from published global rate laws.");
        if let Some(e) = &self.chem.error {
            ui.label(egui::RichText::new(format!("{}  {}", ic::WARNING, e)).color(p.warn));
        }
        let Some(r) = &self.chem.run else {
            theme::empty_state(
                ui,
                p,
                ic::FLASK,
                "No reaction yet",
                "Type an equation or pick an example, then React.",
            );
            return;
        };
        ui.horizontal(|ui| {
            ui.add_space(SP_SM);
            ui.label(
                egui::RichText::new(r.eq.display())
                    .size(20.0)
                    .strong()
                    .color(p.text),
            );
            ui.label(theme::muted(
                p,
                &format!(
                    "from {:.0} K at {:.1} kPa{}",
                    r.t0,
                    r.p / 1e3,
                    if r.air { ", in air" } else { "" }
                ),
            ));
        });
        let last = *r.trace.last().unwrap();
        ui.horizontal_wrapped(|ui| {
            ui.add_space(SP_SM);
            let th = &r.thermo;
            theme::pill(
                ui,
                p,
                if th.exothermic() { p.warn } else { C_A },
                if th.exothermic() {
                    "exothermic"
                } else {
                    "endothermic"
                },
            );
            theme::pill(
                ui,
                p,
                if th.spontaneous() { p.success } else { p.muted },
                if th.spontaneous() {
                    "ΔG < 0: spontaneous"
                } else {
                    "ΔG > 0: not spontaneous here"
                },
            );
            if th.spontaneous() && last.2 < 1e-6 {
                theme::pill(ui, p, C_B, "kinetically frozen: needs ignition");
            }
            if r.final_q_over_k > 0.5 && r.final_q_over_k < 2.0 && last.2 < 0.999 {
                theme::pill(ui, p, C_C, "reached equilibrium (Q ≈ K)");
            }
        });
        ui.add_space(SP_SM);
        let w = ui.available_width() - 2.0 * SP_SM;
        let tpts: Vec<(f64, f64)> = r
            .trace
            .iter()
            .skip(1)
            .map(|(t, temp, _)| (t.log10(), *temp))
            .collect();
        let cpts: Vec<(f64, f64)> = r
            .trace
            .iter()
            .skip(1)
            .map(|(t, _, c)| (t.log10(), c * 100.0))
            .collect();
        let mut marks = vec![];
        if let Some(ta) = r.t_ad {
            marks.push(Mark::H(ta, p.warn, format!("adiabatic flame {:.0} K", ta)));
        }
        ui.horizontal(|ui| {
            ui.add_space(SP_SM);
            plot(
                ui,
                p,
                "Temperature (adiabatic, constant pressure)",
                ("log₁₀ time (s)", "K"),
                &[("T".into(), C_B, tpts)],
                &marks,
                Vec2::new(w / 2.0 - 4.0, 260.0),
            );
            plot(
                ui,
                p,
                "Conversion of the limiting reactant",
                ("log₁₀ time (s)", "%"),
                &[("conversion".into(), C_A, cpts)],
                &[],
                Vec2::new(w / 2.0 - 4.0, 260.0),
            );
        });
        ui.add_space(SP_SM);
        let gpts: Vec<(f64, f64)> = (0..=55)
            .map(|i| 250.0 + i as f64 * 50.0)
            .filter_map(|t| chem::thermo(&r.eq, t).ok().map(|th| (t, th.dg / 1e3)))
            .collect();
        ui.horizontal(|ui| {
            ui.add_space(SP_SM);
            plot(
                ui,
                p,
                "ΔG(T): forward is favoured where the curve is below zero",
                ("T (K)", "kJ/mol"),
                &[("ΔG".into(), C_C, gpts)],
                &[
                    Mark::H(0.0, p.muted, "ΔG = 0".into()),
                    Mark::V(r.t0, p.accent, "start".into()),
                ],
                Vec2::new(w, 200.0),
            );
        });
    }

    fn chem_inspector(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let Some(r) = &self.chem.run else {
            ui.label(theme::muted(
                p,
                "Run a reaction to see its thermochemistry.",
            ));
            return;
        };
        theme::card(ui, p, "Standard state (298.15 K, 1 bar)", |ui| {
            let s = &r.thermo_std;
            theme::kv(ui, p, "ΔH°", format!("{:.2} kJ/mol", s.dh / 1e3));
            theme::kv(ui, p, "ΔS°", format!("{:.2} J/mol·K", s.ds));
            theme::kv(ui, p, "ΔG°", format!("{:.2} kJ/mol", s.dg / 1e3));
            theme::kv(ui, p, "K", format!("{:.3e}", s.k));
        });
        theme::card(ui, p, &format!("At start, {:.0} K", r.t0), |ui| {
            let s = &r.thermo;
            theme::kv(ui, p, "ΔH(T)", format!("{:.2} kJ/mol", s.dh / 1e3));
            theme::kv(ui, p, "ΔS(T)", format!("{:.2} J/mol·K", s.ds));
            theme::kv(ui, p, "ΔG(T)", format!("{:.2} kJ/mol", s.dg / 1e3));
            theme::kv(ui, p, "K = e^(−ΔG/RT)", format!("{:.3e}", s.k));
            theme::kv(ui, p, "Δn(gas)", format!("{:+.2}", s.dn_gas));
            if let Some(ta) = r.t_ad {
                theme::kv(
                    ui,
                    p,
                    "Adiabatic T",
                    format!("{:.0} K ({:.0} °C)", ta, ta - 273.15),
                );
            }
        });
        theme::card(ui, p, "Kinetics", |ui| {
            let l = &r.law;
            theme::kv(ui, p, "A", format!("{:.3e}", l.a));
            theme::kv(ui, p, "Ea", format!("{:.1} kJ/mol", l.ea / 1e3));
            theme::kv(ui, p, "k(T₀)", format!("{:.3e}", l.k(r.t0)));
            theme::kv(
                ui,
                p,
                "Orders",
                l.orders
                    .iter()
                    .map(|(s, o)| format!("[{}]^{}", s, o))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            ui.label(theme::muted(p, l.source));
            if l.estimated {
                ui.label(
                    egui::RichText::new("estimated rate: equilibrium is exact, timing is not")
                        .color(p.warn)
                        .size(11.0),
                );
            }
            let last = r.trace.last().unwrap();
            theme::kv(ui, p, "Conversion", format!("{:.4} %", last.2 * 100.0));
            theme::kv(ui, p, "Final T", format!("{:.0} K", last.1));
            theme::kv(ui, p, "Final Q/K", format!("{:.3e}", r.final_q_over_k));
        });
        theme::card(ui, p, "Species (phase with min G at T)", |ui| {
            for s in &r.thermo.picked {
                ui.label(
                    egui::RichText::new(format!("{}  {}", s.label(), s.name))
                        .strong()
                        .color(p.text),
                );
                ui.label(theme::muted(
                    p,
                    &format!(
                        "ΔHf° {:.2} kJ/mol · S° {:.2} · Cp({:.0} K) {:.1} J/mol·K · {}",
                        s.dhf,
                        s.s,
                        r.t0,
                        s.cp_at(r.t0),
                        s.source
                    ),
                ));
            }
        });
    }

    // ---- circuits

    fn circ_cmd(&self) -> AppCommand {
        let pr = &self.circ.params;
        AppCommand::CircuitRun {
            preset: Some(self.circ.preset.clone()),
            voltage: Some(pr.voltage),
            temperature_c: Some(pr.temp_c),
            material: Some(pr.material.clone()),
            length_mm: Some(pr.length_mm),
            diameter_mm: Some(pr.diameter_mm),
            doping_cm3: Some(pr.doping),
        }
    }

    fn circ_tools(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        theme::card(ui, p, "Circuit", |ui| {
            for (key, name) in CIRCUITS {
                if ui
                    .add_sized(
                        [ui.available_width(), 22.0],
                        theme::toggle_button(p, name, self.circ.preset == *key),
                    )
                    .clicked()
                {
                    self.circ.preset = (*key).into();
                    bus.push(self.circ_cmd());
                }
            }
        });
        theme::card(ui, p, "Parameters", |ui| {
            let mut changed = false;
            let pr = &mut self.circ.params;
            egui::Grid::new("circ_params")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Source (V)");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut pr.voltage)
                                .speed(0.05)
                                .range(-1000.0..=1000.0),
                        )
                        .changed();
                    ui.end_row();
                    ui.label("Temperature (°C)");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut pr.temp_c)
                                .speed(1.0)
                                .range(-270.0..=1500.0),
                        )
                        .changed();
                    ui.end_row();
                    ui.label("Wire element");
                    egui::ComboBox::from_id_salt("wire_mat")
                        .selected_text(pr.material.clone())
                        .show_ui(ui, |ui| {
                            for m in [
                                "Cu", "Al", "Ag", "Au", "Fe", "W", "Ni", "Pt", "Sn", "Pb", "Zn",
                            ] {
                                changed |= ui
                                    .selectable_value(&mut pr.material, m.to_string(), m)
                                    .changed();
                            }
                        });
                    ui.end_row();
                    ui.label("Length (mm)");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut pr.length_mm)
                                .speed(0.5)
                                .range(0.01..=1e5),
                        )
                        .changed();
                    ui.end_row();
                    ui.label("Diameter (mm)");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut pr.diameter_mm)
                                .speed(0.005)
                                .range(0.001..=50.0),
                        )
                        .changed();
                    ui.end_row();
                    ui.label("Doping (cm⁻³)");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut pr.doping)
                                .speed(1e14)
                                .range(1e10..=1e21)
                                .custom_formatter(|v, _| format!("{:.1e}", v)),
                        )
                        .changed();
                    ui.end_row();
                });
            if changed {
                bus.push(self.circ_cmd());
            }
            if ui
                .add_sized(
                    [ui.available_width(), 28.0],
                    theme::primary_button(p, &format!("{}  Simulate", ic::LIGHTNING)),
                )
                .clicked()
            {
                bus.push(self.circ_cmd());
            }
        });
    }

    fn circ_center(&mut self, ui: &mut egui::Ui, p: &Palette) {
        heading(ui, p, ic::LIGHTNING, "Circuit simulator", "Modified nodal analysis, Newton–Raphson for diodes and MOSFETs, backward-Euler transients.");
        if let Some(e) = &self.circ.error {
            ui.label(egui::RichText::new(format!("{}  {}", ic::WARNING, e)).color(p.warn));
        }
        let Some(r) = &self.circ.run else {
            theme::empty_state(
                ui,
                p,
                ic::LIGHTNING,
                "Pick a circuit",
                "Choose a preset on the left to simulate it.",
            );
            return;
        };
        ui.horizontal(|ui| {
            ui.add_space(SP_SM);
            ui.label(
                egui::RichText::new(&r.title)
                    .size(16.0)
                    .strong()
                    .color(p.text),
            );
        });
        let w = ui.available_width() - 2.0 * SP_SM;
        let h = if r.series2.is_some() { 230.0 } else { 320.0 };
        ui.horizontal(|ui| {
            ui.add_space(SP_SM);
            plot(
                ui,
                p,
                "",
                (&r.xlabel, &r.ylabel),
                &r.series,
                &[],
                Vec2::new(w, h),
            );
        });
        if let Some((yl, s2)) = &r.series2 {
            ui.horizontal(|ui| {
                ui.add_space(SP_SM);
                plot(ui, p, "", (&r.xlabel, yl), s2, &[], Vec2::new(w, 160.0));
            });
        }
        for e in &r.events {
            ui.horizontal(|ui| {
                ui.add_space(SP_SM);
                ui.label(
                    egui::RichText::new(format!("{}  {}", ic::LIGHTNING_SLASH, e))
                        .color(p.warn)
                        .strong(),
                );
            });
        }
        ui.add_space(SP_SM);
        ui.horizontal(|ui| {
            ui.add_space(SP_SM);
            ui.vertical(|ui| {
                ui.label(theme::section(p, "NETLIST"));
                for l in &r.netlist {
                    ui.label(egui::RichText::new(l).monospace().size(12.0).color(p.muted));
                }
            });
        });
    }

    fn circ_inspector(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let Some(r) = &self.circ.run else {
            ui.label(theme::muted(p, "Simulate a circuit to see readouts."));
            return;
        };
        theme::card(ui, p, "Readouts", |ui| {
            for (k, v) in &r.readouts {
                theme::kv(ui, p, k, v.clone());
            }
        });
        theme::card(ui, p, "Models", |ui| {
            for l in [
                "R = ρ(T)·L/A, ρ(T) = ρ₂₀(1 + α(T − 293 K))",
                "Diode: Shockley, Iₛ ∝ T³·e^(−E_g/kT)",
                "MOSFET: EKV-style, μ ∝ T^−1.5, V_th −1.5 mV/K",
                "Wire: C·dT/dt = I²R − hA(T − T₀); melts → open",
                "Silicon: Varshni E_g(T), n_i, Caughey–Thomas μ",
            ] {
                ui.label(theme::muted(p, l));
            }
        });
    }
}

impl LabState {
    // ---- computer

    fn cpu_cmd(&self) -> AppCommand {
        let custom = self.cpu.program == "custom";
        AppCommand::CpuRun {
            program: (!custom).then(|| self.cpu.program.clone()),
            source: custom.then(|| self.cpu.source.clone()),
            temperature_c: Some(self.cpu.temp_c),
            clock_mhz: Some(self.cpu.clock_mhz),
            die_object: None,
        }
    }

    fn cpu_tools(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        theme::card(ui, p, "Program", |ui| {
            for (name, _, _) in computer::PROGRAMS {
                if ui
                    .add_sized(
                        [ui.available_width(), 22.0],
                        theme::toggle_button(p, name, self.cpu.program == *name),
                    )
                    .clicked()
                {
                    bus.push(AppCommand::CpuRun {
                        program: Some((*name).into()),
                        source: None,
                        temperature_c: Some(self.cpu.temp_c),
                        clock_mhz: Some(self.cpu.clock_mhz),
                        die_object: None,
                    });
                }
            }
            ui.add_space(SP_XS);
            ui.label(theme::muted(
                p,
                "Assembly: LDI LDA STA ADD ADDI SUB SUBI JMP JZ JC OUT HLT",
            ));
            ui.add(
                egui::TextEdit::multiline(&mut self.cpu.source)
                    .code_editor()
                    .desired_rows(8)
                    .desired_width(f32::INFINITY),
            );
            if ui
                .add_sized(
                    [ui.available_width(), 24.0],
                    theme::toggle_button(p, "Assemble & run as custom", false),
                )
                .clicked()
            {
                bus.push(AppCommand::CpuRun {
                    program: None,
                    source: Some(self.cpu.source.clone()),
                    temperature_c: Some(self.cpu.temp_c),
                    clock_mhz: Some(self.cpu.clock_mhz),
                    die_object: None,
                });
            }
        });
        theme::card(ui, p, "Die conditions", |ui| {
            let mut changed = false;
            egui::Grid::new("cpu_params")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Die T (°C)");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut self.cpu.temp_c)
                                .speed(1.0)
                                .range(-270.0..=1200.0),
                        )
                        .changed();
                    ui.end_row();
                    ui.label("Clock (MHz)");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut self.cpu.clock_mhz)
                                .speed(10.0)
                                .range(0.001..=100_000.0),
                        )
                        .changed();
                    ui.end_row();
                });
            if let Some(f) = &self.cpu.follow {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  die temperature follows arena body '{}'",
                        ic::THERMOMETER,
                        f
                    ))
                    .color(p.accent)
                    .size(11.0),
                );
                if ui.small_button("Stop following").clicked() {
                    bus.push(AppCommand::CpuRun {
                        program: None,
                        source: None,
                        temperature_c: None,
                        clock_mhz: None,
                        die_object: Some(String::new()),
                    });
                }
            }
            if changed {
                bus.push(self.cpu_cmd());
            }
            if ui
                .add_sized(
                    [ui.available_width(), 28.0],
                    theme::primary_button(p, &format!("{}  Run", ic::PLAY)),
                )
                .clicked()
            {
                bus.push(self.cpu_cmd());
            }
            if ui
                .add_sized(
                    [ui.available_width(), 24.0],
                    theme::toggle_button(p, &format!("{}  Heat until it fails", ic::FIRE), false),
                )
                .clicked()
            {
                bus.push(AppCommand::CpuHeatSweep {
                    program: None,
                    clock_mhz: Some(self.cpu.clock_mhz),
                });
            }
        });
    }

    fn cpu_center(&mut self, ui: &mut egui::Ui, p: &Palette) {
        heading(ui, p, ic::CPU, "Build a computer", "Transistors (MNA) → CMOS gates → 4-bit ripple-carry ALU (event-driven, real gate delays) → registers → CPU.");
        if let Some(e) = &self.cpu.error {
            ui.label(egui::RichText::new(format!("{}  {}", ic::WARNING, e)).color(p.warn));
        }
        let (Some(r), Some(lib), Some(cpu)) = (&self.cpu.report, &self.cpu.lib, &self.cpu.cpu)
        else {
            theme::empty_state(
                ui,
                p,
                ic::CPU,
                "Not run yet",
                "Pick a program and press Run.",
            );
            return;
        };
        let w = ui.available_width() - 2.0 * SP_SM;
        let nand = lib.get(GateKind::Nand2);
        let inv = lib.get(GateKind::Inv);
        let intrinsic = circuit::si_intrinsic(lib.t) > computer::WELL_DOPING;
        let levels: [(&str, String, bool); 4] = [
            (
                "1  Transistors",
                format!(
                    "inverter NM {:.2} / {:.2} V\nleakage {}",
                    inv.nm_high(),
                    inv.nm_low(),
                    si(inv.leakage, "A")
                ),
                inv.works() && !intrinsic,
            ),
            (
                "2  CMOS gates",
                format!(
                    "NAND2 t_pd {}\n{}",
                    nand.t_pd().map(|d| si(d, "s")).unwrap_or("—".into()),
                    if intrinsic {
                        "silicon intrinsic: isolation lost"
                    } else if lib.works() {
                        "all cells switch cleanly"
                    } else {
                        "a cell fails as logic"
                    }
                ),
                lib.works() && !intrinsic,
            ),
            (
                "3  4-bit ALU",
                format!(
                    "{} gates; carry chain\n{:.0} ps of {:.0} ps budget",
                    cpu.alu.gate_count(),
                    r.worst_settle_ps,
                    self.cpu.budget_ps
                ),
                r.gates_work && r.timing_violations == 0,
            ),
            (
                "4  CPU",
                format!(
                    "out {:?}\n{}",
                    r.out,
                    if r.correct { "correct" } else { "WRONG" }
                ),
                r.correct,
            ),
        ];
        ui.horizontal(|ui| {
            ui.add_space(SP_SM);
            let gap = ui.spacing().item_spacing.x;
            let bw = ((w - SP_SM - 3.0 * 22.0 - 7.0 * gap) / 4.0).max(80.0);
            for (i, (name, body, ok)) in levels.iter().enumerate() {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(bw, 62.0), egui::Sense::hover());
                let col = if *ok { p.success } else { p.danger };
                ui.painter().rect_filled(
                    rect,
                    CornerRadius::same(6),
                    theme::mix(p.surface, col, 0.12),
                );
                ui.painter().rect_stroke(
                    rect,
                    CornerRadius::same(6),
                    Stroke::new(1.5_f32, col),
                    egui::StrokeKind::Middle,
                );
                ui.painter().text(
                    rect.left_top() + Vec2::new(8.0, 6.0),
                    egui::Align2::LEFT_TOP,
                    format!(
                        "{}  {}",
                        if *ok { ic::CHECK_CIRCLE } else { ic::X_CIRCLE },
                        name
                    ),
                    egui::FontId::proportional(13.0),
                    col,
                );
                ui.painter().text(
                    rect.left_top() + Vec2::new(8.0, 24.0),
                    egui::Align2::LEFT_TOP,
                    body,
                    egui::FontId::proportional(11.5),
                    p.text,
                );
                if i < 3 {
                    let (a, _) =
                        ui.allocate_exact_size(Vec2::new(22.0, 62.0), egui::Sense::hover());
                    ui.painter().text(
                        a.center(),
                        egui::Align2::CENTER_CENTER,
                        ic::ARROW_RIGHT,
                        egui::FontId::proportional(16.0),
                        p.muted,
                    );
                }
            }
        });
        if let Some(f) = &r.failure {
            ui.horizontal(|ui| {
                ui.add_space(SP_SM);
                ui.label(
                    egui::RichText::new(format!("{}  {}", ic::WARNING, f))
                        .color(p.warn)
                        .strong(),
                );
            });
        }
        ui.add_space(SP_SM);
        ui.horizontal(|ui| {
            ui.add_space(SP_SM);
            let series: Vec<Series> = lib
                .cells
                .iter()
                .zip([C_A, C_B, C_C])
                .map(|(c, col)| (c.kind.name().to_string(), col, c.vtc.clone()))
                .collect();
            plot(
                ui,
                p,
                &format!(
                    "Level 1: transfer curves at {:.0} °C, from the transistor models",
                    lib.t - 273.15
                ),
                ("V(in) (V)", "V(out) (V)"),
                &series,
                &[],
                Vec2::new(w * 0.5 - 4.0, 250.0),
            );
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.set_width(w * 0.5 - 12.0);
                ui.label(theme::section(p, "ROM, REGISTERS AND RAM AFTER THE RUN"));
                let list = cpu.listing();
                let side = [
                    format!(
                        "ACC {:2}  Z {}  C {}  PC {}",
                        cpu.acc, cpu.z as u8, cpu.c as u8, cpu.pc
                    ),
                    format!(
                        "RAM 0-7  {}",
                        cpu.ram[..8]
                            .iter()
                            .map(|v| format!("{:X}", v))
                            .collect::<Vec<_>>()
                            .join(" ")
                    ),
                    format!(
                        "RAM 8-F  {}",
                        cpu.ram[8..]
                            .iter()
                            .map(|v| format!("{:X}", v))
                            .collect::<Vec<_>>()
                            .join(" ")
                    ),
                    format!("cycles {}  halted {}", cpu.cycles, cpu.halted),
                    format!("OUT {:?}", cpu.out),
                ];
                egui::Grid::new("cpu_listing")
                    .num_columns(4)
                    .spacing([10.0, 1.0])
                    .show(ui, |ui| {
                        for (i, l) in list.iter().enumerate() {
                            let col = if i as u8 == cpu.pc { p.accent } else { p.muted };
                            ui.label(
                                egui::RichText::new(format!("{:02}", i))
                                    .monospace()
                                    .size(11.0)
                                    .color(p.muted),
                            );
                            ui.label(
                                egui::RichText::new(format!("{:08b}", cpu.rom[i]))
                                    .monospace()
                                    .size(11.0)
                                    .color(col),
                            );
                            ui.label(egui::RichText::new(l).monospace().size(11.0).color(col));
                            ui.label(
                                egui::RichText::new(side.get(i).cloned().unwrap_or_default())
                                    .monospace()
                                    .size(11.0)
                                    .color(p.text),
                            );
                            ui.end_row();
                        }
                    });
            });
        });
        ui.add_space(SP_SM);
        if !self.cpu.sweep.is_empty() {
            let settle: Vec<(f64, f64)> =
                self.cpu.sweep.iter().map(|s| (s.0 - 273.15, s.1)).collect();
            let budget: Vec<(f64, f64)> =
                self.cpu.sweep.iter().map(|s| (s.0 - 273.15, s.2)).collect();
            let mut marks = vec![];
            if let Some(last) = self.cpu.sweep.last().filter(|s| !s.3) {
                marks.push(Mark::V(last.0 - 273.15, p.warn, "fails".into()));
            }
            ui.horizontal(|ui| {
                ui.add_space(SP_SM);
                plot(
                    ui,
                    p,
                    &format!(
                        "Heat sweep at {:.0} MHz: ALU settle time vs the clock budget",
                        self.cpu.clock_mhz
                    ),
                    ("die °C", "ps"),
                    &[
                        ("carry-chain settle".into(), C_B, settle),
                        ("budget (period − setup − clk→Q)".into(), C_A, budget),
                    ],
                    &marks,
                    Vec2::new(w, 210.0),
                );
            });
            if let Some(f) = &self.cpu.sweep_fail {
                ui.horizontal(|ui| {
                    ui.add_space(SP_SM);
                    ui.label(egui::RichText::new(f).color(p.warn));
                });
            }
        }
    }

    fn cpu_inspector(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let Some(r) = &self.cpu.report else {
            ui.label(theme::muted(p, "Run a program to see the report."));
            return;
        };
        theme::card(ui, p, "Run", |ui| {
            theme::kv(ui, p, "Program", self.cpu.program.clone());
            theme::kv(ui, p, "Die", format!("{:.1} °C", r.t - 273.15));
            theme::kv(ui, p, "Clock", si(r.clock_hz, "Hz"));
            theme::kv(ui, p, "Output", format!("{:?}", r.out));
            theme::kv(ui, p, "Expected", format!("{:?}", r.expected));
            theme::kv(ui, p, "Correct", if r.correct { "yes" } else { "NO" });
            theme::kv(ui, p, "Cycles", r.cycles.to_string());
            theme::kv(ui, p, "Timing violations", r.timing_violations.to_string());
            theme::kv(
                ui,
                p,
                "Worst settle",
                format!("{:.1} ps", r.worst_settle_ps),
            );
            theme::kv(ui, p, "Budget", format!("{:.1} ps", self.cpu.budget_ps));
            theme::kv(
                ui,
                p,
                "Power dyn / leak",
                format!("{} / {}", si(r.power_w.0, "W"), si(r.power_w.1, "W")),
            );
            if let Some(f) = self.cpu.f_max {
                theme::kv(ui, p, "f_max at this T", si(f, "Hz"));
            }
        });
        if let Some(lib) = &self.cpu.lib {
            theme::card(ui, p, "Cell library at this T", |ui| {
                for c in &lib.cells {
                    theme::kv(
                        ui,
                        p,
                        c.kind.name(),
                        format!(
                            "{}  NM {:.2}/{:.2}",
                            c.t_pd().map(|d| si(d, "s")).unwrap_or("—".into()),
                            c.nm_high(),
                            c.nm_low()
                        ),
                    );
                }
                theme::kv(
                    ui,
                    p,
                    "n_i",
                    format!("{:.2e} cm⁻³", circuit::si_intrinsic(lib.t)),
                );
                theme::kv(
                    ui,
                    p,
                    "Well doping",
                    format!("{:.0e} cm⁻³", computer::WELL_DOPING),
                );
            });
        }
        theme::card(ui, p, "Levels of simulation", |ui| {
            for l in ["Transistor: MNA + EKV MOSFETs → VTC, delays, leakage", "Gate: event-driven 0/1/X, inertial delays (fs) taken from level 1", "ALU: 52 NAND/NOR/INV gates, ripple carry", "CPU: behavioural control and registers; the ALU result is sampled at the clock edge with a setup check"] {
                ui.label(theme::muted(p, l));
            }
        });
    }

    // ---- relativity

    fn rel_tools(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        theme::card(ui, p, "Special relativity", |ui| {
            let mut b = self.rel.beta;
            if ui
                .add(
                    egui::Slider::new(&mut b, 0.0..=0.999_999)
                        .text("β = v/c")
                        .max_decimals(6),
                )
                .changed()
            {
                bus.push(AppCommand::RelSpeed {
                    beta: Some(b),
                    speed_kms: None,
                });
            }
            for (name, v) in SPEEDS {
                if ui
                    .add_sized(
                        [ui.available_width(), 22.0],
                        theme::toggle_button(p, name, (self.rel.beta - v / rel::C).abs() < 1e-12),
                    )
                    .clicked()
                {
                    bus.push(AppCommand::RelSpeed {
                        beta: Some(v / rel::C),
                        speed_kms: None,
                    });
                }
            }
        });
        theme::card(ui, p, "Gravity (N-body)", |ui| {
            for name in rel::PRESETS {
                if ui
                    .add_sized(
                        [ui.available_width(), 22.0],
                        theme::toggle_button(p, name, self.rel.preset == name),
                    )
                    .clicked()
                {
                    bus.push(AppCommand::NbodyPreset {
                        preset: Some(name.into()),
                        gr: None,
                        gr_exaggeration: None,
                    });
                }
            }
            let mut gr = self.rel.gr;
            if ui
                .checkbox(&mut gr, "GR correction (1PN, Schwarzschild)")
                .changed()
            {
                bus.push(AppCommand::NbodyPreset {
                    preset: None,
                    gr: Some(gr),
                    gr_exaggeration: None,
                });
            }
            let mut x = self.rel.exaggerate;
            if ui
                .add(
                    egui::Slider::new(&mut x, 1.0..=1e6)
                        .logarithmic(true)
                        .text("GR ×"),
                )
                .on_hover_text("Visual aid: shrinks c so the precession is visible. ×1 is real.")
                .changed()
            {
                bus.push(AppCommand::NbodyPreset {
                    preset: None,
                    gr: Some(true),
                    gr_exaggeration: Some(x),
                });
            }
            let mut d = self.rel.days_per_s;
            if ui
                .add(
                    egui::Slider::new(&mut d, 0.1..=2000.0)
                        .logarithmic(true)
                        .text("days/s"),
                )
                .changed()
            {
                bus.push(AppCommand::NbodyPlay {
                    playing: Some(self.rel.playing),
                    days_per_second: Some(d),
                });
            }
            ui.horizontal(|ui| {
                if ui
                    .add(theme::primary_button(
                        p,
                        if self.rel.playing { "Pause" } else { "Play" },
                    ))
                    .clicked()
                {
                    bus.push(AppCommand::NbodyPlay {
                        playing: None,
                        days_per_second: None,
                    });
                }
                if ui.add(theme::toggle_button(p, "Reset", false)).clicked() {
                    bus.push(AppCommand::NbodyPreset {
                        preset: None,
                        gr: None,
                        gr_exaggeration: None,
                    });
                }
            });
        });
    }

    fn rel_center(&mut self, ui: &mut egui::Ui, p: &Palette) {
        heading(ui, p, ic::PLANET, "Relativity", "Special relativity is exact; gravity is Newtonian N-body plus the first post-Newtonian (Schwarzschild) correction.");
        let w = ui.available_width() - 2.0 * SP_SM;
        let sr = rel::sr_readout(self.rel.beta);
        ui.horizontal(|ui| {
            ui.add_space(SP_SM);
            let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 150.0), egui::Sense::hover());
            let pt = ui.painter_at(rect);
            pt.rect_filled(rect, CornerRadius::same(8), p.surface);
            let lab_t = self.rel.clock;
            for (k, (label, t)) in [("lab clock", lab_t), ("moving clock", lab_t / sr.gamma)]
                .iter()
                .enumerate()
            {
                let c = rect.left_center() + Vec2::new(70.0 + k as f32 * 140.0, -8.0);
                pt.circle_stroke(c, 44.0, Stroke::new(2.0_f32, p.muted));
                for i in 0..12 {
                    let a = i as f32 / 12.0 * std::f32::consts::TAU;
                    pt.line_segment(
                        [c + Vec2::angled(a) * 38.0, c + Vec2::angled(a) * 43.0],
                        Stroke::new(1.0_f32, p.muted),
                    );
                }
                let a = (*t as f32 / 10.0).fract() * std::f32::consts::TAU
                    - std::f32::consts::FRAC_PI_2;
                pt.line_segment(
                    [c, c + Vec2::angled(a) * 36.0],
                    Stroke::new(3.0_f32, if k == 0 { C_A } else { C_B }),
                );
                pt.text(
                    c + Vec2::new(0.0, 58.0),
                    egui::Align2::CENTER_CENTER,
                    format!("{}  {:.2} s", label, t),
                    egui::FontId::proportional(12.0),
                    p.text,
                );
            }
            let x0 = rect.left() + 330.0;
            let full = (rect.right() - x0 - 40.0).max(50.0);
            let y = rect.top() + 40.0;
            pt.text(
                Pos2::new(x0, y - 18.0),
                egui::Align2::LEFT_CENTER,
                "rod at rest: 1 m",
                egui::FontId::proportional(12.0),
                p.muted,
            );
            pt.rect_filled(
                Rect::from_min_size(Pos2::new(x0, y - 6.0), Vec2::new(full, 12.0)),
                CornerRadius::same(3),
                C_A,
            );
            let l = rel::contracted_length(1.0, self.rel.beta) as f32;
            pt.text(
                Pos2::new(x0, y + 30.0),
                egui::Align2::LEFT_CENTER,
                format!(
                    "the same rod moving at β = {:.6}: {:.6} m",
                    self.rel.beta, l
                ),
                egui::FontId::proportional(12.0),
                p.muted,
            );
            pt.rect_filled(
                Rect::from_min_size(
                    Pos2::new(x0, y + 42.0),
                    Vec2::new((full * l).max(1.0), 12.0),
                ),
                CornerRadius::same(3),
                C_B,
            );
            pt.text(
                Pos2::new(x0, y + 84.0),
                egui::Align2::LEFT_CENTER,
                format!(
                    "γ = {:.6}     kinetic energy {} per kg (Newton's ½v² gives {})",
                    sr.gamma,
                    si(sr.kinetic_per_kg, "J"),
                    si(sr.newton_kinetic_per_kg, "J")
                ),
                egui::FontId::proportional(13.0),
                p.text,
            );
        });
        ui.add_space(SP_SM);
        ui.horizontal(|ui| {
            ui.add_space(SP_SM);
            let h = (ui.available_height() - 10.0).clamp(380.0, 560.0);
            let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), egui::Sense::hover());
            let pt = ui.painter_at(rect);
            pt.rect_filled(rect, CornerRadius::same(8), Color32::from_rgb(8, 10, 18));
            let nb = &self.rel.nb;
            let mut ext: f64 = self.rel.extent;
            for b in &nb.bodies {
                ext = ext.max(b.pos[0].abs()).max(b.pos[1].abs());
                for q in &b.trail {
                    ext = ext.max(q[0].abs()).max(q[1].abs());
                }
            }
            self.rel.extent = ext;
            let scale = (rect.height().min(rect.width()) as f64 * 0.45) / ext.max(1e-30);
            let c = rect.center();
            let to =
                |q: &[f64; 3]| Pos2::new(c.x + (q[0] * scale) as f32, c.y - (q[1] * scale) as f32);
            let cols = [
                Color32::from_rgb(255, 210, 90),
                C_A,
                C_B,
                C_C,
                C_D,
                Color32::from_rgb(200, 200, 200),
            ];
            let gmax = nb.bodies.iter().map(|b| b.gm).fold(0.0, f64::max);
            for (i, b) in nb.bodies.iter().enumerate() {
                let col = cols[i % cols.len()];
                if b.trail.len() > 1 {
                    let pts: Vec<Pos2> = b.trail.iter().map(&to).collect();
                    pt.add(egui::Shape::line(
                        pts,
                        Stroke::new(1.2_f32, col.gamma_multiply(0.6)),
                    ));
                }
                let rad = (3.0 + (b.gm / gmax).powf(0.15) as f32 * 6.0).min(10.0);
                pt.circle_filled(to(&b.pos), rad, col);
                pt.text(
                    to(&b.pos) + Vec2::new(rad + 4.0, -rad - 2.0),
                    egui::Align2::LEFT_BOTTOM,
                    &b.name,
                    egui::FontId::proportional(12.0),
                    p.text,
                );
            }
            if nb.bodies.len() == 2 {
                let o = to(&nb.bodies[0].pos);
                let len = rect.height() * 0.45;
                if let Some(a0) = self.rel.peri0 {
                    pt.line_segment(
                        [o, o + Vec2::angled(-a0 as f32) * len],
                        Stroke::new(1.0_f32, p.muted.gamma_multiply(0.6)),
                    );
                }
                pt.line_segment(
                    [o, o + Vec2::angled(-nb.perihelion_angle(1) as f32) * len],
                    Stroke::new(1.2_f32, p.warn),
                );
                pt.text(
                    rect.right_bottom() + Vec2::new(-12.0, -10.0),
                    egui::Align2::RIGHT_BOTTOM,
                    "orange: perihelion direction now · grey: at start",
                    egui::FontId::proportional(11.0),
                    p.muted,
                );
            }
            let t_label = if self.rel.preset == "figure8" {
                format!("t = {:.2} (G = m = 1 units)", nb.time)
            } else {
                format!("t = {:.1} days", nb.time / rel::DAY)
            };
            let gr_label = if !self.rel.gr {
                "Newtonian".to_string()
            } else if self.rel.exaggerate > 1.0 {
                format!("GR on, exaggerated ×{:.0}", self.rel.exaggerate)
            } else {
                "GR on (real strength)".into()
            };
            pt.text(
                rect.left_top() + Vec2::new(12.0, 10.0),
                egui::Align2::LEFT_TOP,
                format!("{}     {}     {}", nb.name, t_label, gr_label),
                egui::FontId::proportional(13.0),
                p.text,
            );
            let span = ext * 2.0;
            let span_label = if self.rel.preset == "figure8" {
                format!("view {:.2} units across", span)
            } else if span > 0.05 * rel::AU {
                format!("view {:.2} AU across", span / rel::AU)
            } else {
                format!("view {:.0} km across", span / 1e3)
            };
            pt.text(
                rect.left_bottom() + Vec2::new(12.0, -10.0),
                egui::Align2::LEFT_BOTTOM,
                span_label,
                egui::FontId::proportional(11.0),
                p.muted,
            );
        });
    }

    fn rel_inspector(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        let sr: SrReadout = rel::sr_readout(self.rel.beta);
        theme::card(ui, p, "Special relativity", |ui| {
            theme::kv(ui, p, "v", si(self.rel.beta * rel::C, "m/s"));
            theme::kv(ui, p, "β", format!("{:.9}", sr.beta));
            theme::kv(ui, p, "γ", format!("{:.9}", sr.gamma));
            theme::kv(
                ui,
                p,
                "Moving clock per lab s",
                format!("{:.12} s", sr.tau_per_lab_second),
            );
            theme::kv(
                ui,
                p,
                "Lost per day",
                si((1.0 - sr.tau_per_lab_second) * rel::DAY, "s"),
            );
            theme::kv(
                ui,
                p,
                "1 m contracts to",
                format!("{:.9} m", rel::contracted_length(1.0, sr.beta)),
            );
            theme::kv(ui, p, "Rapidity", format!("{:.6}", sr.rapidity));
            theme::kv(
                ui,
                p,
                "Doppler approach / recede",
                format!("×{:.4} / ×{:.4}", sr.doppler_approach, sr.doppler_recede),
            );
            theme::kv(
                ui,
                p,
                "β ⊕ β",
                format!("{:.9}", rel::add_velocities(sr.beta, sr.beta)),
            );
        });
        theme::card(ui, p, "N-body", |ui| {
            let nb = &self.rel.nb;
            let e = nb.energy_g();
            theme::kv(
                ui,
                p,
                "Integrator",
                if nb.gr {
                    "RK4 + 1PN"
                } else {
                    "leapfrog (symplectic)"
                },
            );
            theme::kv(
                ui,
                p,
                "Step",
                if self.rel.preset == "figure8" {
                    format!("{:.0e}", nb.dt)
                } else {
                    si(nb.dt, "s")
                },
            );
            let drift = format!(
                "{:.2e}",
                ((e - self.rel.e0) / self.rel.e0.abs().max(1e-300)).abs()
            );
            theme::kv(
                ui,
                p,
                if nb.gr {
                    "Newtonian-energy change"
                } else {
                    "Energy drift"
                },
                drift,
            );
            if nb.gr {
                ui.label(theme::muted(p, "With 1PN on, the Newtonian energy is not the conserved quantity, so it changes at the 1PN level (more with exaggeration)."));
            }
            if let (Some(_), Some(per)) = (self.rel.peri0, self.rel.period) {
                let orbits = nb.time / per;
                let d = self.rel.peri_moved;
                theme::kv(ui, p, "Orbits", format!("{:.2}", orbits));
                theme::kv(
                    ui,
                    p,
                    "Perihelion moved",
                    format!("{:.3}″", d / rel::ARCSEC),
                );
                if nb.gr {
                    let b = &nb.bodies;
                    let mu = b[0].gm + b[1].gm;
                    let (rv, vv) = (sub(b[1].pos, b[0].pos), sub(b[1].vel, b[0].vel));
                    let a = 1.0 / (2.0 / norm(rv) - dot(vv, vv) / mu);
                    let ecc = (1.0 - norm(cross(rv, vv)).powi(2) / (mu * a))
                        .max(0.0)
                        .sqrt();
                    let pred =
                        6.0 * std::f64::consts::PI * mu / (nb.c * nb.c * a * (1.0 - ecc * ecc));
                    if orbits >= 1.0 {
                        theme::kv(
                            ui,
                            p,
                            "Per orbit (sim)",
                            format!("{:.4}″", d / orbits.floor() / rel::ARCSEC),
                        );
                    }
                    theme::kv(
                        ui,
                        p,
                        "Per orbit (6πGM/c²a(1−e²))",
                        format!("{:.4}″", pred / rel::ARCSEC),
                    );
                }
                if self.rel.preset == "mercury" {
                    let b = &nb.bodies;
                    let rr = norm(sub(b[1].pos, b[0].pos));
                    let v2 = dot(sub(b[1].vel, b[0].vel), sub(b[1].vel, b[0].vel));
                    let slow = b[0].gm / (rr * rel::C * rel::C) + v2 / (2.0 * rel::C * rel::C);
                    theme::kv(
                        ui,
                        p,
                        "Mercury clock (real c)",
                        format!("−{}/day", si(slow * rel::DAY, "s")),
                    );
                }
            }
            ui.label(theme::muted(p, "Perihelion is read from the Laplace–Runge–Lenz vector; it wobbles within an orbit at 1PN order, so compare whole orbits."));
            if ui
                .add_sized(
                    [ui.available_width(), 24.0],
                    theme::toggle_button(p, "Measure Mercury precession (5 orbits, real c)", false),
                )
                .clicked()
            {
                bus.push(AppCommand::NbodyMeasurePrecession);
            }
            if let Some((m, pr)) = self.rel.precession {
                theme::kv(ui, p, "Simulated", format!("{:.2}″/century", m));
                theme::kv(ui, p, "Analytic", format!("{:.2}″/century", pr));
                theme::kv(ui, p, "Observed anomaly", "42.98 ± 0.04″/century");
            }
        });
        theme::card(ui, p, "Weak-field GR", |ui| {
            let (g, v, n) = rel::gps_drift_per_day(26_559.7e3);
            theme::kv(ui, p, "GPS gravity", format!("+{:.2} µs/day", g * 1e6));
            theme::kv(ui, p, "GPS velocity", format!("{:.2} µs/day", v * 1e6));
            theme::kv(ui, p, "GPS net", format!("+{:.2} µs/day", n * 1e6));
            theme::kv(
                ui,
                p,
                "Sun r_s",
                si(rel::schwarzschild_radius(rel::GM_SUN), "m"),
            );
            theme::kv(
                ui,
                p,
                "Light bent at the Sun's limb",
                format!(
                    "{:.3}″",
                    rel::light_deflection(rel::GM_SUN, rel::R_SUN) / rel::ARCSEC
                ),
            );
        });
    }
}

fn wrap(a: f64) -> f64 {
    let t = std::f64::consts::TAU;
    (a + t / 2.0).rem_euclid(t) - t / 2.0
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn heading(ui: &mut egui::Ui, p: &Palette, icon: &str, title: &str, sub: &str) {
    ui.horizontal(|ui| {
        ui.add_space(SP_SM);
        ui.label(
            egui::RichText::new(format!("{}  {}", icon, title))
                .size(16.0)
                .strong()
                .color(p.text),
        );
        ui.label(theme::muted(p, sub));
    });
    ui.add_space(SP_XS);
}

/// SI-prefixed value: 4700 Ω → "4.70 kΩ".
pub fn si(v: f64, unit: &str) -> String {
    if v == 0.0 {
        return format!("0 {}", unit);
    }
    if !v.is_finite() {
        return format!("{} {}", v, unit);
    }
    let pre = [
        (1e12, "T"),
        (1e9, "G"),
        (1e6, "M"),
        (1e3, "k"),
        (1.0, ""),
        (1e-3, "m"),
        (1e-6, "µ"),
        (1e-9, "n"),
        (1e-12, "p"),
        (1e-15, "f"),
        (1e-18, "a"),
    ];
    let a = v.abs();
    for (s, n) in pre {
        if a >= s * 0.9995 {
            let x = v / s;
            let num = if x.abs() >= 99.95 {
                format!("{:.0}", x)
            } else if x.abs() >= 9.995 {
                format!("{:.1}", x)
            } else {
                format!("{:.2}", x)
            };
            return format!("{} {}{}", num, n, unit);
        }
    }
    format!("{:.3e} {}", v, unit)
}

pub enum Mark {
    V(f64, Color32, String),
    H(f64, Color32, String),
}

fn tick_label(v: f64, step: f64) -> String {
    if v.abs() < step * 1e-6 {
        return "0".into();
    }
    if v.abs() >= 1e5 || v.abs() < 1e-3 {
        return format!("{:.1e}", v);
    }
    let dec = (-step.log10().floor()).max(0.0) as usize;
    format!("{:.*}", dec, v)
}

/// A painter line plot with auto range, grid, ticks, markers and a legend.
pub fn plot(
    ui: &mut egui::Ui,
    p: &Palette,
    title: &str,
    (xl, yl): (&str, &str),
    series: &[Series],
    marks: &[Mark],
    size: Vec2,
) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let pt = ui.painter_at(rect);
    pt.rect_filled(rect, CornerRadius::same(8), p.surface);
    let font = egui::FontId::proportional(11.0);
    let top = if title.is_empty() { 22.0 } else { 36.0 };
    if !title.is_empty() {
        pt.text(
            rect.left_top() + Vec2::new(10.0, 8.0),
            egui::Align2::LEFT_TOP,
            title,
            egui::FontId::proportional(12.5),
            p.text,
        );
    }
    let area = Rect::from_min_max(
        rect.left_top() + Vec2::new(62.0, top),
        rect.right_bottom() - Vec2::new(14.0, 34.0),
    );
    let (mut x0, mut x1, mut y0, mut y1) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for (_, _, pts) in series {
        for (x, y) in pts.iter().filter(|(x, y)| x.is_finite() && y.is_finite()) {
            x0 = x0.min(*x);
            x1 = x1.max(*x);
            y0 = y0.min(*y);
            y1 = y1.max(*y);
        }
    }
    if !x0.is_finite() {
        pt.text(
            area.center(),
            egui::Align2::CENTER_CENTER,
            "no data",
            font,
            p.muted,
        );
        return;
    }
    for m in marks {
        match m {
            Mark::H(y, ..) => {
                y0 = y0.min(*y);
                y1 = y1.max(*y);
            }
            Mark::V(x, ..) => {
                x0 = x0.min(*x);
                x1 = x1.max(*x);
            }
        }
    }
    if x1 - x0 <= 0.0 {
        x1 = x0 + 1.0;
    }
    if y1 - y0 <= 1e-9 * y0.abs().max(1e-300) {
        let d = y0.abs().max(1.0) * 0.05;
        y0 -= d;
        y1 += d;
    }
    let pad = (y1 - y0) * 0.06;
    y0 -= pad;
    y1 += pad;
    let tx = |x: f64| area.left() + ((x - x0) / (x1 - x0)) as f32 * area.width();
    let ty = |y: f64| area.bottom() - ((y - y0) / (y1 - y0)) as f32 * area.height();
    let nice = |span: f64| {
        let raw = span / 5.0;
        let mag = 10f64.powf(raw.log10().floor());
        let n = raw / mag;
        mag * if n < 1.5 {
            1.0
        } else if n < 3.5 {
            2.0
        } else if n < 7.5 {
            5.0
        } else {
            10.0
        }
    };
    let grid = Stroke::new(1.0_f32, theme::mix(p.surface, p.muted, 0.18));
    let sx = nice(x1 - x0);
    let mut x = (x0 / sx).ceil() * sx;
    while x <= x1 + 1e-9 * sx {
        pt.line_segment(
            [
                Pos2::new(tx(x), area.top()),
                Pos2::new(tx(x), area.bottom()),
            ],
            grid,
        );
        pt.text(
            Pos2::new(tx(x), area.bottom() + 4.0),
            egui::Align2::CENTER_TOP,
            tick_label(x, sx),
            font.clone(),
            p.muted,
        );
        x += sx;
    }
    let sy = nice(y1 - y0);
    let mut y = (y0 / sy).ceil() * sy;
    while y <= y1 + 1e-9 * sy {
        pt.line_segment(
            [
                Pos2::new(area.left(), ty(y)),
                Pos2::new(area.right(), ty(y)),
            ],
            grid,
        );
        pt.text(
            Pos2::new(area.left() - 6.0, ty(y)),
            egui::Align2::RIGHT_CENTER,
            tick_label(y, sy),
            font.clone(),
            p.muted,
        );
        y += sy;
    }
    pt.text(
        Pos2::new(area.center().x, rect.bottom() - 4.0),
        egui::Align2::CENTER_BOTTOM,
        xl,
        font.clone(),
        p.muted,
    );
    pt.text(
        Pos2::new(rect.left() + 8.0, area.top() - 4.0),
        egui::Align2::LEFT_BOTTOM,
        yl,
        font.clone(),
        p.muted,
    );
    for m in marks {
        match m {
            Mark::H(yv, c, l) => {
                pt.line_segment(
                    [
                        Pos2::new(area.left(), ty(*yv)),
                        Pos2::new(area.right(), ty(*yv)),
                    ],
                    Stroke::new(1.0_f32, *c),
                );
                pt.text(
                    Pos2::new(area.right() - 4.0, ty(*yv) - 2.0),
                    egui::Align2::RIGHT_BOTTOM,
                    l,
                    font.clone(),
                    *c,
                );
            }
            Mark::V(xv, c, l) => {
                pt.line_segment(
                    [
                        Pos2::new(tx(*xv), area.top()),
                        Pos2::new(tx(*xv), area.bottom()),
                    ],
                    Stroke::new(1.0_f32, *c),
                );
                pt.text(
                    Pos2::new(tx(*xv) + 4.0, area.top() + 2.0),
                    egui::Align2::LEFT_TOP,
                    l,
                    font.clone(),
                    *c,
                );
            }
        }
    }
    for (_, c, pts) in series {
        let line: Vec<Pos2> = pts
            .iter()
            .filter(|(x, y)| x.is_finite() && y.is_finite())
            .map(|(x, y)| Pos2::new(tx(*x), ty(*y)))
            .collect();
        if line.len() > 1 {
            pt.add(egui::Shape::line(line, Stroke::new(2.0_f32, *c)));
        }
    }
    if series.len() > 1 {
        let mut lx = area.left() + 10.0;
        let ly = area.top() - 12.0;
        for (n, c, _) in series {
            pt.line_segment(
                [Pos2::new(lx, ly), Pos2::new(lx + 16.0, ly)],
                Stroke::new(2.5_f32, *c),
            );
            let r = pt.text(
                Pos2::new(lx + 20.0, ly),
                egui::Align2::LEFT_CENTER,
                n,
                font.clone(),
                p.text,
            );
            lx = r.right() + 14.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn react(eq: Option<&str>, t_c: Option<f64>, s: Option<f64>, air: Option<bool>) -> AppCommand {
        AppCommand::ChemReact {
            equation: eq.map(|e| e.into()),
            temperature_c: t_c,
            pressure_kpa: None,
            seconds: s,
            air,
        }
    }

    fn circ(preset: Option<&str>) -> AppCommand {
        AppCommand::CircuitRun {
            preset: preset.map(|p| p.into()),
            voltage: None,
            temperature_c: None,
            material: None,
            length_mm: None,
            diameter_mm: None,
            doping_cm3: None,
        }
    }

    #[test]
    fn every_example_reaction_runs() {
        let mut lab = LabState::new();
        for (name, eq, t) in REACTIONS {
            let r = lab
                .apply(
                    &react(Some(eq), Some(t - 273.15), Some(1.0), Some(false)),
                    None,
                )
                .unwrap();
            assert!(r.is_ok(), "{}: {:?}", name, r);
        }
        assert_eq!(lab.tab, 1);
    }

    #[test]
    fn methane_burns_hot_but_is_frozen_at_room_temperature() {
        let mut lab = LabState::new();
        lab.apply(
            &react(
                Some("CH4 + O2 -> CO2 + H2O"),
                Some(1500.0 - 273.15),
                Some(0.05),
                Some(true),
            ),
            None,
        )
        .unwrap()
        .unwrap();
        let r = lab.chem.run.as_ref().unwrap();
        assert!(r.trace.last().unwrap().2 > 0.99);
        // liquid water is the stable product at 298 K: the higher heating value
        assert!(
            (r.thermo_std.dh / 1e3 + 890.3).abs() < 1.0,
            "{}",
            r.thermo_std.dh
        );
        lab.apply(&react(None, Some(25.0), Some(3600.0), None), None)
            .unwrap()
            .unwrap();
        let r = lab.chem.run.as_ref().unwrap();
        assert!(r.thermo.spontaneous() && r.trace.last().unwrap().2 < 1e-6);
        assert!(lab
            .apply(&react(Some("Xx + O2 -> XxO"), None, None, None), None)
            .unwrap()
            .is_err());
    }

    #[test]
    fn every_circuit_preset_simulates() {
        let mut lab = LabState::new();
        for (k, _) in CIRCUITS {
            let r = lab.apply(&circ(Some(k)), None).unwrap();
            assert!(r.is_ok(), "{}: {:?}", k, r);
            assert!(!lab.circ.run.as_ref().unwrap().series.is_empty());
        }
        // 1 V across 20 mm of 0.1 mm copper: ~23 A, melts it
        lab.apply(
            &AppCommand::CircuitRun {
                preset: Some("fuse".into()),
                voltage: Some(1.0),
                temperature_c: Some(25.0),
                material: Some("Cu".into()),
                length_mm: Some(20.0),
                diameter_mm: Some(0.1),
                doping_cm3: None,
            },
            None,
        )
        .unwrap()
        .unwrap();
        assert!(lab
            .circ
            .run
            .as_ref()
            .unwrap()
            .events
            .iter()
            .any(|e| e.contains("fuse blown")));
        // 0.05 V: stays solid
        lab.apply(
            &AppCommand::CircuitRun {
                preset: None,
                voltage: Some(0.05),
                temperature_c: None,
                material: None,
                length_mm: None,
                diameter_mm: None,
                doping_cm3: None,
            },
            None,
        )
        .unwrap()
        .unwrap();
        assert!(lab.circ.run.as_ref().unwrap().events.is_empty());
        assert!(lab
            .apply(
                &AppCommand::CircuitRun {
                    preset: Some("fuse".into()),
                    voltage: None,
                    temperature_c: None,
                    material: Some("Xe".into()),
                    length_mm: None,
                    diameter_mm: None,
                    doping_cm3: None
                },
                None
            )
            .unwrap()
            .is_err());
        assert!(lab
            .apply(&circ(Some("flux_capacitor")), None)
            .unwrap()
            .is_err());
    }

    #[test]
    fn cpu_runs_fails_hot_and_follows_a_die() {
        let mut lab = LabState::new();
        let r = lab
            .apply(
                &AppCommand::CpuRun {
                    program: Some("fibonacci".into()),
                    source: None,
                    temperature_c: Some(27.0),
                    clock_mhz: Some(1000.0),
                    die_object: None,
                },
                None,
            )
            .unwrap()
            .unwrap();
        assert!(r.contains("correct"), "{}", r);
        assert!(lab.cpu.report.as_ref().unwrap().correct);
        lab.apply(
            &AppCommand::CpuHeatSweep {
                program: None,
                clock_mhz: Some(2000.0),
            },
            None,
        )
        .unwrap()
        .unwrap();
        let last = *lab.cpu.sweep.last().unwrap();
        assert!(
            !last.3 && last.0 < 500.0,
            "fast clock fails by timing well below 500 K: {:?}",
            last
        );
        // custom program: expected output comes from a safe reference run
        lab.apply(
            &AppCommand::CpuRun {
                program: None,
                source: Some("LDI 7\nADDI 6\nOUT\nHLT".into()),
                temperature_c: Some(27.0),
                clock_mhz: Some(100.0),
                die_object: None,
            },
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(lab.cpu.report.as_ref().unwrap().out, vec![13]);
        assert!(lab
            .apply(
                &AppCommand::CpuRun {
                    program: None,
                    source: Some("BOGUS 3".into()),
                    temperature_c: None,
                    clock_mhz: None,
                    die_object: None
                },
                None
            )
            .unwrap()
            .is_err());
        // die bound to a body that isn't in an arena
        assert!(lab
            .apply(
                &AppCommand::CpuRun {
                    program: None,
                    source: None,
                    temperature_c: None,
                    clock_mhz: None,
                    die_object: Some("Cube".into())
                },
                None
            )
            .unwrap()
            .is_err());
        lab.apply(
            &AppCommand::CpuRun {
                program: Some("countdown".into()),
                source: None,
                temperature_c: None,
                clock_mhz: Some(10.0),
                die_object: Some("Cube".into()),
            },
            Some(320.0),
        )
        .unwrap()
        .unwrap();
        assert_eq!(lab.cpu.follow.as_deref(), Some("Cube"));
        assert!(lab.cpu.report.as_ref().unwrap().correct);
        assert!(
            lab.follow_die(322.0).is_none(),
            "small changes don't re-run"
        );
        assert!(lab.follow_die(900.0).is_some());
        assert!(lab
            .cpu
            .report
            .as_ref()
            .unwrap()
            .failure
            .as_ref()
            .unwrap()
            .contains("intrinsic"));
    }

    #[test]
    fn relativity_commands() {
        let mut lab = LabState::new();
        lab.apply(
            &AppCommand::RelSpeed {
                beta: None,
                speed_kms: Some(7.66),
            },
            None,
        )
        .unwrap()
        .unwrap();
        assert!((lab.rel.beta - 7660.0 / rel::C).abs() < 1e-12);
        assert!(lab
            .apply(
                &AppCommand::RelSpeed {
                    beta: Some(1.0),
                    speed_kms: None
                },
                None
            )
            .unwrap()
            .is_err());
        lab.apply(
            &AppCommand::NbodyPreset {
                preset: Some("inner".into()),
                gr: Some(false),
                gr_exaggeration: None,
            },
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(lab.rel.nb.bodies.len(), 5);
        lab.apply(
            &AppCommand::NbodyPlay {
                playing: Some(true),
                days_per_second: Some(100.0),
            },
            None,
        )
        .unwrap()
        .unwrap();
        for _ in 0..60 {
            lab.tick(1.0 / 60.0);
        }
        assert!(
            (lab.rel.nb.time / rel::DAY - 100.0).abs() < 2.0,
            "{}",
            lab.rel.nb.time / rel::DAY
        );
        assert!(((lab.rel.nb.energy_g() - lab.rel.e0) / lab.rel.e0).abs() < 1e-6);
        assert!(lab
            .apply(
                &AppCommand::NbodyPreset {
                    preset: Some("pluto".into()),
                    gr: None,
                    gr_exaggeration: None
                },
                None
            )
            .unwrap()
            .is_err());
        lab.apply(
            &AppCommand::NbodyPreset {
                preset: Some("mercury".into()),
                gr: Some(true),
                gr_exaggeration: Some(1e4),
            },
            None,
        )
        .unwrap()
        .unwrap();
        assert!((lab.rel.nb.c - rel::C / 100.0).abs() < 1.0);
        assert!(lab
            .apply(
                &AppCommand::LabTab {
                    tab: "chemistry".into()
                },
                None
            )
            .unwrap()
            .is_ok());
        assert_eq!(lab.tab, 1);
        assert!(lab
            .apply(
                &AppCommand::LabTab {
                    tab: "alchemy".into()
                },
                None
            )
            .unwrap()
            .is_err());
        assert!(lab.apply(&AppCommand::ArenaReset, None).is_none());
    }

    #[test]
    fn si_prefixes() {
        assert_eq!(si(4700.0, "Ω"), "4.70 kΩ");
        assert_eq!(si(3.09e-11, "s"), "30.9 ps");
        assert_eq!(si(0.0, "V"), "0 V");
        assert_eq!(si(-0.0123, "A"), "-12.3 mA");
    }
}
