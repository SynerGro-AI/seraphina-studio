//! Polygon meshes for the 3D workspace.
//!
//! * `PolyMesh`: vertices plus CCW (outward-facing) polygon faces.
//! * Primitives: cube, plane, cylinder.
//! * Modelling: extrude, bevel (inset + lift), edge loop (quad rings),
//!   multi-cut (split a face between two edges), bridge (two faces of
//!   equal size), quad draw (add welded quads).
//! * Booleans: BSP-tree CSG (union / difference / intersection), after
//!   Evan Wallace's csg.js (MIT).
//! * `Frame3d`: z-buffered, flat Lambert-shaded software rasterizer with
//!   depth-tested lines and a per-pixel face-id buffer for picking.

use std::collections::HashMap;

pub type V3 = [f32; 3];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub fn length(a: V3) -> f32 {
    dot(a, a).sqrt()
}
pub fn normalize(a: V3) -> V3 {
    let l = length(a);
    if l > 1e-12 {
        scale(a, 1.0 / l)
    } else {
        [0.0, 0.0, 0.0]
    }
}
pub fn lerp(a: V3, b: V3, t: f32) -> V3 {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn ekey(a: usize, b: usize) -> (usize, usize) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
#[cfg_attr(feature = "ui", derive(serde::Serialize, serde::Deserialize))]
pub struct PolyMesh {
    pub verts: Vec<V3>,
    pub faces: Vec<Vec<usize>>,
}

impl PolyMesh {
    pub fn cube() -> Self {
        let h = 0.5;
        let verts = vec![
            [-h, -h, -h],
            [h, -h, -h],
            [h, -h, h],
            [-h, -h, h],
            [-h, h, -h],
            [h, h, -h],
            [h, h, h],
            [-h, h, h],
        ];
        let faces = vec![
            vec![0, 1, 2, 3],
            vec![4, 7, 6, 5],
            vec![3, 2, 6, 7],
            vec![1, 0, 4, 5],
            vec![2, 1, 5, 6],
            vec![0, 3, 7, 4],
        ];
        Self { verts, faces }
    }

    /// Unit quad in the XZ plane facing +Y.
    pub fn plane() -> Self {
        let h = 0.5;
        Self {
            verts: vec![[-h, 0.0, -h], [h, 0.0, -h], [h, 0.0, h], [-h, 0.0, h]],
            faces: vec![vec![0, 3, 2, 1]],
        }
    }

    /// Cylinder of radius 0.5 and height 1 with `n` sides (quads) and n-gon caps.
    pub fn cylinder(n: usize) -> Self {
        let n = n.clamp(3, 64);
        let mut verts = Vec::with_capacity(2 * n);
        for y in [-0.5f32, 0.5] {
            for k in 0..n {
                let a = k as f32 / n as f32 * std::f32::consts::TAU;
                verts.push([0.5 * a.cos(), y, 0.5 * a.sin()]);
            }
        }
        let mut faces = Vec::new();
        for k in 0..n {
            let k1 = (k + 1) % n;
            faces.push(vec![k, n + k, n + k1, k1]);
        }
        faces.push((0..n).collect());
        faces.push((n..2 * n).rev().collect());
        let mut m = Self { verts, faces };
        m.orient_outward();
        m
    }

    /// Make every face of a convex mesh point away from the centroid.
    pub fn orient_outward(&mut self) {
        let c = self.centroid();
        for f in 0..self.faces.len() {
            if dot(self.face_normal(f), sub(self.face_center(f), c)) < 0.0 {
                self.faces[f].reverse();
            }
        }
    }

    pub fn centroid(&self) -> V3 {
        let n = self.verts.len().max(1) as f32;
        self.verts
            .iter()
            .fold([0.0; 3], |a, v| add(a, scale(*v, 1.0 / n)))
    }

    /// Newell normal (unit length; zero for degenerate faces).
    pub fn face_normal(&self, f: usize) -> V3 {
        let face = &self.faces[f];
        let mut n = [0.0f32; 3];
        for i in 0..face.len() {
            let a = self.verts[face[i]];
            let b = self.verts[face[(i + 1) % face.len()]];
            n[0] += (a[1] - b[1]) * (a[2] + b[2]);
            n[1] += (a[2] - b[2]) * (a[0] + b[0]);
            n[2] += (a[0] - b[0]) * (a[1] + b[1]);
        }
        normalize(n)
    }

    pub fn face_center(&self, f: usize) -> V3 {
        let face = &self.faces[f];
        let k = 1.0 / face.len().max(1) as f32;
        face.iter()
            .fold([0.0; 3], |a, &i| add(a, scale(self.verts[i], k)))
    }

    /// Unique undirected edges.
    pub fn edges(&self) -> Vec<(usize, usize)> {
        let mut seen = std::collections::BTreeSet::new();
        for f in &self.faces {
            for i in 0..f.len() {
                seen.insert(ekey(f[i], f[(i + 1) % f.len()]));
            }
        }
        seen.into_iter().collect()
    }

    fn edge_faces(&self) -> HashMap<(usize, usize), Vec<usize>> {
        let mut m: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        for (fi, f) in self.faces.iter().enumerate() {
            for i in 0..f.len() {
                m.entry(ekey(f[i], f[(i + 1) % f.len()]))
                    .or_default()
                    .push(fi);
            }
        }
        m
    }

    /// Closed 2-manifold: every edge is used by exactly two faces, once in each direction.
    pub fn is_closed(&self) -> bool {
        let mut dir: HashMap<(usize, usize), i32> = HashMap::new();
        for f in &self.faces {
            for i in 0..f.len() {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                *dir.entry((a, b)).or_insert(0) += 1;
            }
        }
        !dir.is_empty()
            && dir
                .iter()
                .all(|(&(a, b), &c)| c == 1 && dir.get(&(b, a)) == Some(&1))
    }

    /// Insert every vertex that lies on the interior of a face edge into that
    /// edge, so neighbouring faces share edges exactly (BSP splits leave
    /// T-junctions). Returns the number of insertions.
    pub fn fix_t_junctions(&mut self, eps: f32) -> usize {
        let mut n = 0;
        for fi in 0..self.faces.len() {
            let f = self.faces[fi].clone();
            let mut out = Vec::with_capacity(f.len());
            for i in 0..f.len() {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                out.push(a);
                let (pa, pb) = (self.verts[a], self.verts[b]);
                let ab = sub(pb, pa);
                let len2 = dot(ab, ab);
                if len2 < eps * eps {
                    continue;
                }
                let mut on: Vec<(f32, usize)> = Vec::new();
                for (vi, &v) in self.verts.iter().enumerate() {
                    if vi == a || vi == b || f.contains(&vi) {
                        continue;
                    }
                    let t = dot(sub(v, pa), ab) / len2;
                    if t <= 1e-4 || t >= 1.0 - 1e-4 {
                        continue;
                    }
                    if length(sub(v, add(pa, scale(ab, t)))) < eps {
                        on.push((t, vi));
                    }
                }
                on.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
                n += on.len();
                out.extend(on.into_iter().map(|(_, v)| v));
            }
            self.faces[fi] = out;
        }
        n
    }

    /// Signed volume (positive for outward-facing closed meshes).
    pub fn volume(&self) -> f32 {
        let mut v = 0.0;
        for f in &self.faces {
            for i in 1..f.len().saturating_sub(1) {
                v += dot(
                    self.verts[f[0]],
                    cross(self.verts[f[i]], self.verts[f[i + 1]]),
                ) / 6.0;
            }
        }
        v
    }

    /// The face that points most upward (ties: highest centre).
    pub fn top_face(&self) -> Option<usize> {
        (0..self.faces.len()).max_by(|&a, &b| {
            let ka = (self.face_normal(a)[1] * 1000.0).round() as i64;
            let kb = (self.face_normal(b)[1] * 1000.0).round() as i64;
            ka.cmp(&kb).then(
                self.face_center(a)[1]
                    .partial_cmp(&self.face_center(b)[1])
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        })
    }

    fn check_face(&self, f: usize) -> Result<(), String> {
        if f >= self.faces.len() {
            Err(format!(
                "face {} doesn't exist (mesh has {})",
                f,
                self.faces.len()
            ))
        } else {
            Ok(())
        }
    }

    /// Replace face `f` by a moved copy of its ring and stitch quads between them.
    fn ring_op(&mut self, f: usize, ring: Vec<V3>, outward: bool) {
        let old = self.faces[f].clone();
        let base = self.verts.len();
        self.verts.extend(ring);
        let n = old.len();
        for i in 0..n {
            let j = (i + 1) % n;
            let mut q = vec![old[i], old[j], base + j, base + i];
            if !outward {
                q.reverse();
            }
            self.faces.push(q);
        }
        self.faces[f] = (base..base + n).collect();
    }

    /// Extrude face `f` along its normal by `d`.
    pub fn extrude(&mut self, f: usize, d: f32) -> Result<(), String> {
        self.check_face(f)?;
        let n = self.face_normal(f);
        let ring = self.faces[f]
            .iter()
            .map(|&i| add(self.verts[i], scale(n, d)))
            .collect();
        self.ring_op(f, ring, d >= 0.0);
        Ok(())
    }

    /// Inset face `f` toward its centre by `inset` (0..0.45) and lift by `lift`.
    pub fn bevel(&mut self, f: usize, inset: f32, lift: f32) -> Result<(), String> {
        self.check_face(f)?;
        let inset = inset.clamp(0.0, 0.45);
        let n = self.face_normal(f);
        let c = self.face_center(f);
        let ring = self.faces[f]
            .iter()
            .map(|&i| add(lerp(self.verts[i], c, inset), scale(n, lift)))
            .collect();
        self.ring_op(f, ring, lift >= 0.0);
        Ok(())
    }

    /// Default edge for an edge loop: the most vertical edge of a quad.
    pub fn default_loop_edge(&self) -> Option<(usize, usize)> {
        let mut best: Option<((usize, usize), f32)> = None;
        for f in self.faces.iter().filter(|f| f.len() == 4) {
            for i in 0..4 {
                let (a, b) = (f[i], f[(i + 1) % 4]);
                let d = sub(self.verts[b], self.verts[a]);
                let s = d[1].abs() / length(d).max(1e-9);
                if best.map(|x| s > x.1 + 1e-4).unwrap_or(true) {
                    best = Some(((a, b), s));
                }
            }
        }
        best.map(|b| b.0)
    }

    /// Insert an edge loop through the quad ring that crosses edge `(a, b)`,
    /// placing new vertices at `t` along each ring edge. Returns the number
    /// of faces split.
    pub fn edge_loop(&mut self, a: usize, b: usize, t: f32) -> Result<usize, String> {
        let t = t.clamp(0.02, 0.98);
        let ef = self.edge_faces();
        let start = ekey(a, b);
        let adj = ef
            .get(&start)
            .cloned()
            .ok_or_else(|| format!("{}–{} isn't an edge of this mesh", a, b))?;
        // directed ring edges and the quads crossed (face, in-edge position)
        let mut ring: Vec<(usize, usize)> = vec![(a, b)];
        let mut crossed: Vec<usize> = Vec::new();
        let mut closed = false;
        let walk = |first_face: usize,
                    ring: &mut Vec<(usize, usize)>,
                    crossed: &mut Vec<usize>,
                    forward: bool|
         -> bool {
            let mut cur = if forward {
                *ring.last().unwrap_or(&(a, b))
            } else {
                ring[0]
            };
            let mut face = first_face;
            loop {
                if crossed.contains(&face) {
                    return true;
                }
                let q = &self.faces[face];
                if q.len() != 4 {
                    return false;
                }
                let Some(i) = (0..4).find(|&i| ekey(q[i], q[(i + 1) % 4]) == ekey(cur.0, cur.1))
                else {
                    return false;
                };
                let same = q[i] == cur.0;
                let next = if same {
                    (q[(i + 3) % 4], q[(i + 2) % 4])
                } else {
                    (q[(i + 2) % 4], q[(i + 3) % 4])
                };
                crossed.push(face);
                if ekey(next.0, next.1) == start {
                    return true;
                }
                if forward {
                    ring.push(next);
                } else {
                    ring.insert(0, next);
                }
                cur = next;
                let others = ef.get(&ekey(next.0, next.1)).cloned().unwrap_or_default();
                match others.into_iter().find(|&g| g != face) {
                    Some(g) => face = g,
                    None => return false,
                }
            }
        };
        if let Some(&f0) = adj.first() {
            closed = walk(f0, &mut ring, &mut crossed, true);
        }
        if !closed {
            if let Some(&f1) = adj.get(1) {
                walk(f1, &mut ring, &mut crossed, false);
            }
        }
        if crossed.is_empty() {
            return Err("edge loops need quads: no quad borders that edge".into());
        }
        let mut mid: HashMap<(usize, usize), usize> = HashMap::new();
        for &(u, v) in &ring {
            let k = ekey(u, v);
            if let std::collections::hash_map::Entry::Vacant(e) = mid.entry(k) {
                self.verts.push(lerp(self.verts[u], self.verts[v], t));
                e.insert(self.verts.len() - 1);
            }
        }
        let mut new_faces = Vec::new();
        for &fi in &crossed {
            let q = self.faces[fi].clone();
            let ring_pos: Vec<usize> = (0..4)
                .filter(|&i| mid.contains_key(&ekey(q[i], q[(i + 1) % 4])))
                .collect();
            if ring_pos.len() != 2 {
                continue;
            }
            let i = ring_pos[0];
            let (qi, qi1, qi2, qi3) = (q[i], q[(i + 1) % 4], q[(i + 2) % 4], q[(i + 3) % 4]);
            let m_in = mid[&ekey(qi, qi1)];
            let m_out = mid[&ekey(qi2, qi3)];
            self.faces[fi] = vec![qi, m_in, m_out, qi3];
            new_faces.push(vec![m_in, qi1, qi2, m_out]);
        }
        let split = new_faces.len();
        // keep neighbouring non-crossed faces watertight
        for fi in 0..self.faces.len() {
            if crossed.contains(&fi) {
                continue;
            }
            self.insert_edge_points(fi, &mid);
        }
        self.faces.extend(new_faces);
        Ok(split)
    }

    /// Insert midpoint vertices into face `fi` wherever one of its edges is in `mid`.
    fn insert_edge_points(&mut self, fi: usize, mid: &HashMap<(usize, usize), usize>) {
        let f = &self.faces[fi];
        if !(0..f.len()).any(|i| mid.contains_key(&ekey(f[i], f[(i + 1) % f.len()]))) {
            return;
        }
        let mut out = Vec::with_capacity(f.len() + 2);
        for i in 0..f.len() {
            let (u, v) = (f[i], f[(i + 1) % f.len()]);
            out.push(u);
            if let Some(&m) = mid.get(&ekey(u, v)) {
                if m != u && m != v {
                    out.push(m);
                }
            }
        }
        self.faces[fi] = out;
    }

    /// Split face `f` with a cut from edge `ei` (at `ti`) to edge `ej` (at `tj`).
    /// Edge k runs from corner k to corner k+1 of the face.
    pub fn multi_cut(
        &mut self,
        f: usize,
        ei: usize,
        ti: f32,
        ej: usize,
        tj: f32,
    ) -> Result<(), String> {
        self.check_face(f)?;
        let face = self.faces[f].clone();
        let n = face.len();
        if ei >= n || ej >= n || ei == ej {
            return Err(format!(
                "pick two different edges of face {} (it has {})",
                f, n
            ));
        }
        let (ti, tj) = (ti.clamp(0.02, 0.98), tj.clamp(0.02, 0.98));
        let mut mid = HashMap::new();
        let (a0, a1) = (face[ei], face[(ei + 1) % n]);
        let (b0, b1) = (face[ej], face[(ej + 1) % n]);
        self.verts.push(lerp(self.verts[a0], self.verts[a1], ti));
        let va = self.verts.len() - 1;
        self.verts.push(lerp(self.verts[b0], self.verts[b1], tj));
        let vb = self.verts.len() - 1;
        mid.insert(ekey(a0, a1), va);
        mid.insert(ekey(b0, b1), vb);
        let mut p1 = vec![va];
        let mut k = (ei + 1) % n;
        loop {
            p1.push(face[k]);
            if k == ej {
                break;
            }
            k = (k + 1) % n;
        }
        p1.push(vb);
        let mut p2 = vec![vb];
        let mut k = (ej + 1) % n;
        loop {
            p2.push(face[k]);
            if k == ei {
                break;
            }
            k = (k + 1) % n;
        }
        p2.push(va);
        for fi in 0..self.faces.len() {
            if fi != f {
                self.insert_edge_points(fi, &mid);
            }
        }
        self.faces[f] = p1;
        self.faces.push(p2);
        Ok(())
    }

    /// Remove faces `fa` and `fb` (same corner count) and join their
    /// borders with a ring of quads.
    pub fn bridge(&mut self, fa: usize, fb: usize) -> Result<(), String> {
        self.check_face(fa)?;
        self.check_face(fb)?;
        if fa == fb {
            return Err("bridge needs two different faces".into());
        }
        let a = self.faces[fa].clone();
        let b = self.faces[fb].clone();
        if a.len() != b.len() {
            return Err(format!(
                "bridge needs faces with the same number of corners ({} vs {})",
                a.len(),
                b.len()
            ));
        }
        let n = a.len();
        // b reversed, best rotation
        let br: Vec<usize> = b.iter().rev().copied().collect();
        let best = (0..n)
            .min_by(|&x, &y| {
                let cost = |k: usize| {
                    (0..n)
                        .map(|i| length(sub(self.verts[a[i]], self.verts[br[(i + k) % n]])))
                        .sum::<f32>()
                };
                cost(x)
                    .partial_cmp(&cost(y))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(0);
        let m: Vec<usize> = (0..n).map(|i| br[(i + best) % n]).collect();
        let mut quads = Vec::new();
        for i in 0..n {
            let j = (i + 1) % n;
            quads.push(vec![a[i], a[j], m[j], m[i]]);
        }
        let (hi, lo) = if fa > fb { (fa, fb) } else { (fb, fa) };
        self.faces.remove(hi);
        self.faces.remove(lo);
        self.faces.extend(quads);
        Ok(())
    }

    /// Append another mesh (already in this mesh's space). Returns the face offset.
    pub fn merge(&mut self, other: &PolyMesh) -> usize {
        let vo = self.verts.len();
        let fo = self.faces.len();
        self.verts.extend(other.verts.iter().copied());
        self.faces.extend(
            other
                .faces
                .iter()
                .map(|f| f.iter().map(|i| i + vo).collect()),
        );
        fo
    }

    /// Best pair of facing faces (one below `split`, one at/after it) with equal corner counts.
    pub fn facing_pair(&self, split: usize) -> Option<(usize, usize)> {
        let mut best: Option<((usize, usize), f32)> = None;
        for fa in 0..split.min(self.faces.len()) {
            let (na, ca) = (self.face_normal(fa), self.face_center(fa));
            for fb in split..self.faces.len() {
                if self.faces[fa].len() != self.faces[fb].len() {
                    continue;
                }
                let (nb, cb) = (self.face_normal(fb), self.face_center(fb));
                let dir = sub(cb, ca);
                if dot(na, nb) > -0.5 || dot(na, dir) <= 0.0 {
                    continue;
                }
                let s = length(dir) * (2.0 + dot(na, nb));
                if best.map(|x| s < x.1).unwrap_or(true) {
                    best = Some(((fa, fb), s));
                }
            }
        }
        best.map(|b| b.0)
    }

    /// Quad Draw: add a quad from four points, welding to vertices within `weld`.
    /// The face is oriented to point toward +Y (or `toward` when given).
    pub fn quad_draw(&mut self, pts: &[V3; 4], weld: f32, toward: Option<V3>) -> usize {
        let mut idx = [0usize; 4];
        for (k, p) in pts.iter().enumerate() {
            idx[k] = match self.verts.iter().position(|v| length(sub(*v, *p)) <= weld) {
                Some(i) => i,
                None => {
                    self.verts.push(*p);
                    self.verts.len() - 1
                }
            };
        }
        let mut f = idx.to_vec();
        f.dedup();
        self.faces.push(f);
        let fi = self.faces.len() - 1;
        let want = toward.unwrap_or([0.0, 1.0, 0.0]);
        if dot(self.face_normal(fi), want) < 0.0 {
            self.faces[fi].reverse();
        }
        fi
    }

    /// Fan triangulation (faces are convex for every op here).
    pub fn triangles(&self) -> Vec<(usize, [usize; 3])> {
        let mut out = Vec::new();
        for (fi, f) in self.faces.iter().enumerate() {
            for i in 1..f.len().saturating_sub(1) {
                out.push((fi, [f[0], f[i], f[i + 1]]));
            }
        }
        out
    }

    pub fn transformed(&self, m: impl Fn(V3) -> V3) -> PolyMesh {
        PolyMesh {
            verts: self.verts.iter().map(|v| m(*v)).collect(),
            faces: self.faces.clone(),
        }
    }
}

// ---- CSG booleans (BSP trees) -------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoolOp {
    Union,
    Difference,
    Intersection,
}

impl BoolOp {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "union" | "add" | "combine" => Some(BoolOp::Union),
            "difference" | "subtract" | "minus" => Some(BoolOp::Difference),
            "intersection" | "intersect" => Some(BoolOp::Intersection),
            _ => None,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            BoolOp::Union => "union",
            BoolOp::Difference => "difference",
            BoolOp::Intersection => "intersection",
        }
    }
}

const CSG_EPS: f32 = 1e-5;

#[derive(Clone, Debug)]
struct CPlane {
    n: V3,
    w: f32,
}

impl CPlane {
    fn flip(&mut self) {
        self.n = scale(self.n, -1.0);
        self.w = -self.w;
    }
}

#[derive(Clone, Debug)]
struct CPoly {
    v: Vec<V3>,
    plane: CPlane,
}

impl CPoly {
    fn new(v: Vec<V3>) -> Option<Self> {
        if v.len() < 3 {
            return None;
        }
        let m = PolyMesh {
            verts: v.clone(),
            faces: vec![(0..v.len()).collect()],
        };
        let n = m.face_normal(0);
        if length(n) < 0.5 {
            return None;
        }
        let w = dot(n, v[0]);
        Some(Self {
            v,
            plane: CPlane { n, w },
        })
    }
    fn flip(&mut self) {
        self.v.reverse();
        self.plane.flip();
    }
}

fn split_polygon(
    plane: &CPlane,
    poly: CPoly,
    cf: &mut Vec<CPoly>,
    cb: &mut Vec<CPoly>,
    front: &mut Vec<CPoly>,
    back: &mut Vec<CPoly>,
) {
    const COPLANAR: u8 = 0;
    const FRONT: u8 = 1;
    const BACK: u8 = 2;
    const SPANNING: u8 = 3;
    let mut ptype = 0u8;
    let types: Vec<u8> = poly
        .v
        .iter()
        .map(|v| {
            let t = dot(plane.n, *v) - plane.w;
            let ty = if t < -CSG_EPS {
                BACK
            } else if t > CSG_EPS {
                FRONT
            } else {
                COPLANAR
            };
            ptype |= ty;
            ty
        })
        .collect();
    match ptype {
        COPLANAR => {
            if dot(plane.n, poly.plane.n) > 0.0 {
                cf.push(poly)
            } else {
                cb.push(poly)
            }
        }
        FRONT => front.push(poly),
        BACK => back.push(poly),
        _ => {
            let n = poly.v.len();
            let (mut f, mut b) = (Vec::new(), Vec::new());
            for i in 0..n {
                let j = (i + 1) % n;
                let (ti, tj) = (types[i], types[j]);
                let (vi, vj) = (poly.v[i], poly.v[j]);
                if ti != BACK {
                    f.push(vi);
                }
                if ti != FRONT {
                    b.push(vi);
                }
                if (ti | tj) == SPANNING {
                    let t = (plane.w - dot(plane.n, vi)) / dot(plane.n, sub(vj, vi));
                    let v = lerp(vi, vj, t);
                    f.push(v);
                    b.push(v);
                }
            }
            if f.len() >= 3 {
                front.push(CPoly {
                    v: f,
                    plane: poly.plane.clone(),
                });
            }
            if b.len() >= 3 {
                back.push(CPoly {
                    v: b,
                    plane: poly.plane,
                });
            }
        }
    }
}

#[derive(Default)]
struct BspNode {
    plane: Option<CPlane>,
    front: Option<Box<BspNode>>,
    back: Option<Box<BspNode>>,
    polys: Vec<CPoly>,
}

impl BspNode {
    fn new(polys: Vec<CPoly>) -> Self {
        let mut n = BspNode::default();
        n.build(polys, 0);
        n
    }

    fn invert(&mut self) {
        for p in &mut self.polys {
            p.flip();
        }
        if let Some(p) = &mut self.plane {
            p.flip();
        }
        if let Some(f) = &mut self.front {
            f.invert();
        }
        if let Some(b) = &mut self.back {
            b.invert();
        }
        std::mem::swap(&mut self.front, &mut self.back);
    }

    fn clip_polygons(&self, polys: Vec<CPoly>) -> Vec<CPoly> {
        let Some(plane) = &self.plane else {
            return polys;
        };
        let (mut f, mut b) = (Vec::new(), Vec::new());
        for p in polys {
            let (mut cf, mut cb) = (Vec::new(), Vec::new());
            split_polygon(plane, p, &mut cf, &mut cb, &mut f, &mut b);
            f.extend(cf);
            b.extend(cb);
        }
        let mut f = match &self.front {
            Some(n) => n.clip_polygons(f),
            None => f,
        };
        let b = match &self.back {
            Some(n) => n.clip_polygons(b),
            None => Vec::new(),
        };
        f.extend(b);
        f
    }

    fn clip_to(&mut self, bsp: &BspNode) {
        self.polys = bsp.clip_polygons(std::mem::take(&mut self.polys));
        if let Some(f) = &mut self.front {
            f.clip_to(bsp);
        }
        if let Some(b) = &mut self.back {
            b.clip_to(bsp);
        }
    }

    fn all_polygons(&self) -> Vec<CPoly> {
        let mut out = self.polys.clone();
        if let Some(f) = &self.front {
            out.extend(f.all_polygons());
        }
        if let Some(b) = &self.back {
            out.extend(b.all_polygons());
        }
        out
    }

    fn build(&mut self, polys: Vec<CPoly>, depth: usize) {
        if polys.is_empty() {
            return;
        }
        if depth > 512 {
            // pathological input: keep the rest unsplit rather than overflow the stack
            self.polys.extend(polys);
            return;
        }
        let plane = self
            .plane
            .get_or_insert_with(|| polys[0].plane.clone())
            .clone();
        let (mut f, mut b, mut cf, mut cb) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for p in polys {
            split_polygon(&plane, p, &mut cf, &mut cb, &mut f, &mut b);
        }
        self.polys.extend(cf);
        self.polys.extend(cb);
        if !f.is_empty() {
            self.front
                .get_or_insert_with(Default::default)
                .build(f, depth + 1);
        }
        if !b.is_empty() {
            self.back
                .get_or_insert_with(Default::default)
                .build(b, depth + 1);
        }
    }
}

fn to_polys(m: &PolyMesh) -> Vec<CPoly> {
    m.faces
        .iter()
        .filter_map(|f| CPoly::new(f.iter().map(|&i| m.verts[i]).collect()))
        .collect()
}

fn from_polys(polys: Vec<CPoly>) -> PolyMesh {
    let mut m = PolyMesh::default();
    let mut index: HashMap<(i64, i64, i64), usize> = HashMap::new();
    for p in polys {
        let mut f = Vec::with_capacity(p.v.len());
        for v in p.v {
            let k = (
                (v[0] * 1e4).round() as i64,
                (v[1] * 1e4).round() as i64,
                (v[2] * 1e4).round() as i64,
            );
            let i = *index.entry(k).or_insert_with(|| {
                m.verts.push(v);
                m.verts.len() - 1
            });
            if f.last() != Some(&i) && f.first() != Some(&i) {
                f.push(i);
            }
        }
        if f.len() >= 3 {
            m.faces.push(f);
        }
    }
    m
}

/// Boolean of two meshes in the same space.
pub fn boolean(a: &PolyMesh, b: &PolyMesh, op: BoolOp) -> PolyMesh {
    let mut na = BspNode::new(to_polys(a));
    let mut nb = BspNode::new(to_polys(b));
    match op {
        BoolOp::Union => {
            na.clip_to(&nb);
            nb.clip_to(&na);
            nb.invert();
            nb.clip_to(&na);
            nb.invert();
            na.build(nb.all_polygons(), 0);
        }
        BoolOp::Difference => {
            na.invert();
            na.clip_to(&nb);
            nb.clip_to(&na);
            nb.invert();
            nb.clip_to(&na);
            nb.invert();
            na.build(nb.all_polygons(), 0);
            na.invert();
        }
        BoolOp::Intersection => {
            na.invert();
            nb.clip_to(&na);
            nb.invert();
            na.clip_to(&nb);
            nb.clip_to(&na);
            na.build(nb.all_polygons(), 0);
            na.invert();
        }
    }
    let mut m = from_polys(na.all_polygons());
    m.fix_t_junctions(1e-4);
    m
}

// ---- software rasterizer --------------------------------------------------------

pub const NEAR: f32 = 0.2;

/// Orbit camera around the origin (same convention as the 3D viewport).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
}

impl Camera {
    /// World → (x, y, depth) in view space.
    pub fn view(&self, v: V3) -> V3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let x = v[0] * cy - v[2] * sy;
        let z0 = v[0] * sy + v[2] * cy;
        let y = v[1] * cp - z0 * sp;
        let z = v[1] * sp + z0 * cp;
        [x, y, self.dist - z]
    }

    /// World → screen (x, y, depth) for a `w`×`h` viewport.
    pub fn project(&self, v: V3, w: f32, h: f32) -> Option<[f32; 3]> {
        let [x, y, d] = self.view(v);
        if d < NEAR {
            return None;
        }
        let f = h * 1.1;
        Some([w * 0.5 + x * f / d, h * 0.5 - y * f / d, d])
    }

    /// Camera position in world space.
    pub fn eye(&self) -> V3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [self.dist * cp * sy, self.dist * sp, self.dist * cp * cy]
    }

    /// Clip a world segment to the near plane.
    pub fn clip_segment(&self, a: V3, b: V3) -> Option<(V3, V3)> {
        let (da, db) = (self.view(a)[2], self.view(b)[2]);
        if da < NEAR && db < NEAR {
            return None;
        }
        if da >= NEAR && db >= NEAR {
            return Some((a, b));
        }
        let t = (NEAR + 1e-4 - da) / (db - da);
        let m = lerp(a, b, t);
        Some(if da < NEAR { (m, b) } else { (a, m) })
    }
}

/// Flat Lambert shading: key light plus a soft headlight and ambient.
pub fn shade(normal: V3, to_light: V3, to_eye: V3, base: [u8; 3]) -> [u8; 3] {
    let mut n = normalize(normal);
    if dot(n, to_eye) < 0.0 {
        n = scale(n, -1.0); // two-sided for open meshes
    }
    let k = 0.2
        + 0.6 * dot(n, normalize(to_light)).max(0.0)
        + 0.25 * dot(n, normalize(to_eye)).max(0.0);
    let k = k.min(1.15);
    [
        (base[0] as f32 * k).min(255.0) as u8,
        (base[1] as f32 * k).min(255.0) as u8,
        (base[2] as f32 * k).min(255.0) as u8,
    ]
}

/// Surface parameters for [`shade_surface`] (linear-RGB base colour).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
    pub base: [f32; 3],
    pub metalness: f32,
    pub roughness: f32,
    pub ior: f32,
    /// 0 = opaque … 1 = clear (glass, water); drawn blended over what's behind.
    pub transmission: f32,
}

impl Surface {
    /// The default untextured grey (dielectric).
    pub fn grey() -> Self {
        Self::from_srgb8([168, 172, 182], 0.0, 0.55, 1.5)
    }

    pub fn from_srgb8(c: [u8; 3], metalness: f32, roughness: f32, ior: f32) -> Self {
        Self {
            base: [
                srgb_to_linear(c[0]),
                srgb_to_linear(c[1]),
                srgb_to_linear(c[2]),
            ],
            metalness,
            roughness,
            ior,
            transmission: 0.0,
        }
    }

    /// Normal-incidence reflectance per channel: the base colour for metals,
    /// ((n−1)/(n+1))² for dielectrics, blended by metalness.
    pub fn f0(&self) -> [f32; 3] {
        let d = ((self.ior - 1.0) / (self.ior + 1.0)).powi(2);
        let m = self.metalness.clamp(0.0, 1.0);
        [
            d + (self.base[0] - d) * m,
            d + (self.base[1] - d) * m,
            d + (self.base[2] - d) * m,
        ]
    }

    /// Blinn-Phong exponent from roughness (Walter et al. 2007 mapping α → 2/α² − 2, α = r²).
    pub fn shininess(&self) -> f32 {
        let a = (self.roughness.clamp(0.02, 1.0)).powi(2);
        (2.0 / (a * a) - 2.0).clamp(1.0, 8192.0)
    }
}

pub fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

/// Studio environment radiance seen along direction `d` (linear): dark
/// ground, mid horizon, bright sky. Stands in for image-based lighting so
/// metals have something to reflect.
pub fn sky(d: V3) -> f32 {
    let y = d[1].clamp(-1.0, 1.0);
    if y < 0.0 {
        0.30 + (0.03 - 0.30) * (-y * 4.0).min(1.0)
    } else {
        0.30 + (1.0 - 0.30) * y.powf(0.6)
    }
}

/// Blinn-Phong with Schlick Fresnel and energy-normalised specular
/// ((s+8)/8π) for the key light, plus a sky/ground environment: metals
/// reflect it tinted by F0 (no diffuse), dielectrics get sky irradiance on
/// the diffuse and a weak Fresnel reflection. Rough surfaces see a blurred
/// (averaged) environment. Linear in, sRGB out.
pub fn shade_surface(normal: V3, to_light: V3, to_eye: V3, s: &Surface) -> [u8; 3] {
    let mut n = normalize(normal);
    let v = normalize(to_eye);
    if dot(n, v) < 0.0 {
        n = scale(n, -1.0);
    }
    let l = normalize(to_light);
    let h = normalize(add(l, v));
    let ndl = dot(n, l).max(0.0);
    let ndh = dot(n, h).max(0.0);
    let vdh = dot(v, h).max(0.0);
    let ndv = dot(n, v).max(0.0);
    let f0 = s.f0();
    let shin = s.shininess();
    let spec = (shin + 8.0) / (8.0 * std::f32::consts::PI) * ndh.powf(shin) * ndl;
    let fres = (1.0 - vdh).powi(5);
    let fres_v = (1.0 - ndv).powi(5);
    let metal = s.metalness.clamp(0.0, 1.0);
    let rough = s.roughness.clamp(0.0, 1.0);
    // mirror direction r = 2(n·v)n − v; roughness blurs toward the average
    let r = sub(scale(n, 2.0 * dot(n, v)), v);
    let env_spec = sky(r) * (1.0 - rough) + 0.35 * rough;
    let env_diff = 0.35 + 0.65 * (sky(n) - 0.03) / 0.97;
    const LIGHT: f32 = 2.2;
    const ENV: f32 = 0.55;
    let mut out = [0.0f32; 3];
    for k in 0..3 {
        let f = f0[k] + (1.0 - f0[k]) * fres;
        let fv = f0[k] + (1.0 - f0[k]) * fres_v * (1.0 - rough);
        let diffuse = s.base[k] * (1.0 - metal) * (1.0 - f);
        out[k] = ENV * (0.30 * diffuse * env_diff + fv * env_spec)
            + LIGHT * (diffuse * ndl * 0.6 + f * spec * 0.25);
    }
    // hue-preserving Reinhard shoulder on the max channel, so a bright
    // copper highlight stays copper instead of clipping to white
    let m = out[0].max(out[1]).max(out[2]);
    let k = if m > 0.6 {
        (0.6 + (m - 0.6) / (1.0 + (m - 0.6) / 0.4)) / m
    } else {
        1.0
    };
    [
        linear_to_srgb(out[0] * k),
        linear_to_srgb(out[1] * k),
        linear_to_srgb(out[2] * k),
    ]
}

/// Colour + depth + id buffers.
pub struct Frame3d {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
    pub depth: Vec<f32>,
    pub ids: Vec<u32>,
}

impl Frame3d {
    pub fn new(w: usize, h: usize, bg: [u8; 3]) -> Self {
        let mut rgba = vec![255u8; w * h * 4];
        for px in rgba.as_chunks_mut::<4>().0 {
            px[0] = bg[0];
            px[1] = bg[1];
            px[2] = bg[2];
        }
        Self {
            w,
            h,
            rgba,
            depth: vec![f32::INFINITY; w * h],
            ids: vec![0; w * h],
        }
    }

    /// Fill a screen-space triangle (x, y, view depth) with depth testing.
    pub fn tri(&mut self, p: [[f32; 3]; 3], color: [u8; 3], id: u32) {
        let edge = |a: [f32; 3], b: [f32; 3], x: f32, y: f32| {
            (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0])
        };
        let area = edge(p[0], p[1], p[2][0], p[2][1]);
        if area.abs() < 1e-8 {
            return;
        }
        let minx = p
            .iter()
            .map(|v| v[0])
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as usize;
        let maxx = (p
            .iter()
            .map(|v| v[0])
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil() as i64)
            .min(self.w as i64 - 1);
        let miny = p
            .iter()
            .map(|v| v[1])
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as usize;
        let maxy = (p
            .iter()
            .map(|v| v[1])
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil() as i64)
            .min(self.h as i64 - 1);
        if maxx < 0 || maxy < 0 {
            return;
        }
        let inv = [1.0 / p[0][2], 1.0 / p[1][2], 1.0 / p[2][2]];
        for y in miny..=maxy as usize {
            let fy = y as f32 + 0.5;
            for x in minx..=maxx as usize {
                let fx = x as f32 + 0.5;
                let w0 = edge(p[1], p[2], fx, fy) / area;
                let w1 = edge(p[2], p[0], fx, fy) / area;
                let w2 = edge(p[0], p[1], fx, fy) / area;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let d = 1.0 / (w0 * inv[0] + w1 * inv[1] + w2 * inv[2]);
                let i = y * self.w + x;
                if d < self.depth[i] {
                    self.depth[i] = d;
                    self.ids[i] = id;
                    self.rgba[i * 4] = color[0];
                    self.rgba[i * 4 + 1] = color[1];
                    self.rgba[i * 4 + 2] = color[2];
                }
            }
        }
    }

    /// Like [`tri`] but blends `color` over the existing pixel by `alpha`
    /// (transparent surfaces drawn after opaque ones).
    pub fn tri_alpha(&mut self, p: [[f32; 3]; 3], color: [u8; 3], alpha: f32, id: u32) {
        let edge = |a: [f32; 3], b: [f32; 3], x: f32, y: f32| {
            (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0])
        };
        let area = edge(p[0], p[1], p[2][0], p[2][1]);
        if area.abs() < 1e-8 {
            return;
        }
        let minx = p
            .iter()
            .map(|v| v[0])
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as usize;
        let maxx = (p
            .iter()
            .map(|v| v[0])
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil() as i64)
            .min(self.w as i64 - 1);
        let miny = p
            .iter()
            .map(|v| v[1])
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as usize;
        let maxy = (p
            .iter()
            .map(|v| v[1])
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil() as i64)
            .min(self.h as i64 - 1);
        if maxx < 0 || maxy < 0 {
            return;
        }
        let a = alpha.clamp(0.0, 1.0);
        let inv = [1.0 / p[0][2], 1.0 / p[1][2], 1.0 / p[2][2]];
        for y in miny..=maxy as usize {
            let fy = y as f32 + 0.5;
            for x in minx..=maxx as usize {
                let fx = x as f32 + 0.5;
                let w0 = edge(p[1], p[2], fx, fy) / area;
                let w1 = edge(p[2], p[0], fx, fy) / area;
                let w2 = edge(p[0], p[1], fx, fy) / area;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let d = 1.0 / (w0 * inv[0] + w1 * inv[1] + w2 * inv[2]);
                let i = y * self.w + x;
                if d < self.depth[i] {
                    self.ids[i] = id;
                    for (c, &k) in color.iter().enumerate().take(3) {
                        let o = self.rgba[i * 4 + c] as f32;
                        self.rgba[i * 4 + c] = (o + (k as f32 - o) * a).round() as u8;
                    }
                }
            }
        }
    }

    /// Depth-tested line (`bias` pulls it slightly toward the camera).
    pub fn line(&mut self, a: [f32; 3], b: [f32; 3], color: [u8; 4], bias: f32) {
        let steps = (b[0] - a[0]).abs().max((b[1] - a[1]).abs()).ceil().max(1.0) as usize;
        if steps > 20000 {
            return;
        }
        let (ia, ib) = (1.0 / a[2], 1.0 / b[2]);
        let al = color[3] as f32 / 255.0;
        for s in 0..=steps {
            let t = s as f32 / steps as f32;
            let x = a[0] + (b[0] - a[0]) * t;
            let y = a[1] + (b[1] - a[1]) * t;
            if x < 0.0 || y < 0.0 || x >= self.w as f32 || y >= self.h as f32 {
                continue;
            }
            let d = 1.0 / (ia + (ib - ia) * t) * (1.0 - bias);
            let i = y as usize * self.w + x as usize;
            if d <= self.depth[i] {
                for (c, &k) in color.iter().enumerate().take(3) {
                    let o = self.rgba[i * 4 + c] as f32;
                    self.rgba[i * 4 + c] = (o + (k as f32 - o) * al).round() as u8;
                }
            }
        }
    }

    pub fn id_at(&self, x: f32, y: f32) -> u32 {
        if x < 0.0 || y < 0.0 || x >= self.w as f32 || y >= self.h as f32 {
            return 0;
        }
        self.ids[y as usize * self.w + x as usize]
    }

    /// Draw a mesh already placed in world space (flat Lambert).
    #[allow(clippy::too_many_arguments)]
    pub fn mesh(
        &mut self,
        cam: &Camera,
        m: &PolyMesh,
        base: [u8; 3],
        light_pos: V3,
        id_base: u32,
        wire: Option<[u8; 4]>,
    ) {
        self.mesh_with(cam, m, light_pos, id_base, wire, |n, l, v| {
            shade(n, l, v, base)
        });
    }

    /// Draw a mesh with a material surface (Blinn-Phong); `tint` mixes the
    /// shaded colour toward a highlight colour (selection) by `tint.3`.
    #[allow(clippy::too_many_arguments)]
    pub fn mesh_surface(
        &mut self,
        cam: &Camera,
        m: &PolyMesh,
        s: &Surface,
        light_pos: V3,
        id_base: u32,
        wire: Option<[u8; 4]>,
        tint: Option<([u8; 3], f32)>,
    ) {
        if s.transmission > 0.0 {
            return self.mesh_clear(cam, m, s, light_pos, id_base, wire, tint);
        }
        self.mesh_with(cam, m, light_pos, id_base, wire, |n, l, v| {
            let c = shade_surface(n, l, v, s);
            match tint {
                Some((t, k)) => {
                    [0, 1, 2].map(|i| (c[i] as f32 + (t[i] as f32 - c[i] as f32) * k).round() as u8)
                }
                None => c,
            }
        });
    }

    /// Transparent mesh: each face blends its shaded colour over the scene
    /// behind with opacity 1 − 0.85·transmission, raised by the Fresnel
    /// reflectance at the view angle (glass edges look more reflective).
    /// No refraction or depth sorting between transparent objects.
    #[allow(clippy::too_many_arguments)]
    fn mesh_clear(
        &mut self,
        cam: &Camera,
        m: &PolyMesh,
        s: &Surface,
        light_pos: V3,
        id_base: u32,
        wire: Option<[u8; 4]>,
        tint: Option<([u8; 3], f32)>,
    ) {
        let (w, h) = (self.w as f32, self.h as f32);
        let eye = cam.eye();
        let proj: Vec<Option<[f32; 3]>> = m.verts.iter().map(|v| cam.project(*v, w, h)).collect();
        let f0 = s.f0()[1];
        for (fi, tri) in m.triangles() {
            let (Some(a), Some(b), Some(c)) = (proj[tri[0]], proj[tri[1]], proj[tri[2]]) else {
                continue;
            };
            let n = m.face_normal(fi);
            let ctr = m.face_center(fi);
            let v = normalize(sub(eye, ctr));
            let mut col = shade_surface(n, sub(light_pos, ctr), sub(eye, ctr), s);
            if let Some((t, k)) = tint {
                col = [0, 1, 2]
                    .map(|i| (col[i] as f32 + (t[i] as f32 - col[i] as f32) * k).round() as u8);
            }
            let cos = dot(normalize(n), v).abs();
            let fres = f0 + (1.0 - f0) * (1.0 - cos).powi(5);
            let alpha = ((1.0 - 0.85 * s.transmission) + fres).clamp(0.08, 1.0);
            self.tri_alpha([a, b, c], col, alpha, id_base + fi as u32);
        }
        if let Some(wc) = wire {
            for (u, v) in m.edges() {
                if let Some((p, q)) = cam.clip_segment(m.verts[u], m.verts[v]) {
                    if let (Some(a), Some(b)) = (cam.project(p, w, h), cam.project(q, w, h)) {
                        self.line(a, b, wc, 0.004);
                    }
                }
            }
        }
    }

    fn mesh_with(
        &mut self,
        cam: &Camera,
        m: &PolyMesh,
        light_pos: V3,
        id_base: u32,
        wire: Option<[u8; 4]>,
        sh: impl Fn(V3, V3, V3) -> [u8; 3],
    ) {
        let (w, h) = (self.w as f32, self.h as f32);
        let eye = cam.eye();
        let proj: Vec<Option<[f32; 3]>> = m.verts.iter().map(|v| cam.project(*v, w, h)).collect();
        for (fi, tri) in m.triangles() {
            let (Some(a), Some(b), Some(c)) = (proj[tri[0]], proj[tri[1]], proj[tri[2]]) else {
                continue;
            };
            let n = m.face_normal(fi);
            let ctr = m.face_center(fi);
            let col = sh(n, sub(light_pos, ctr), sub(eye, ctr));
            self.tri([a, b, c], col, id_base + fi as u32);
        }
        if let Some(wc) = wire {
            for (u, v) in m.edges() {
                if let Some((p, q)) = cam.clip_segment(m.verts[u], m.verts[v]) {
                    if let (Some(a), Some(b)) = (cam.project(p, w, h), cam.project(q, w, h)) {
                        self.line(a, b, wc, 0.004);
                    }
                }
            }
        }
    }

    /// World-space depth-tested line.
    pub fn world_line(&mut self, cam: &Camera, a: V3, b: V3, color: [u8; 4]) {
        let (w, h) = (self.w as f32, self.h as f32);
        if let Some((p, q)) = cam.clip_segment(a, b) {
            if let (Some(pa), Some(pb)) = (cam.project(p, w, h), cam.project(q, w, h)) {
                self.line(pa, pb, color, 0.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn primitives_are_closed_and_outward() {
        let c = PolyMesh::cube();
        assert!(c.is_closed());
        assert!(near(c.volume(), 1.0, 1e-5));
        let cy = PolyMesh::cylinder(16);
        assert!(cy.is_closed());
        let expect = 16.0 / 2.0 * 0.25 * (std::f32::consts::TAU / 16.0).sin();
        assert!(near(cy.volume(), expect, 1e-4), "{}", cy.volume());
        let p = PolyMesh::plane();
        assert!(!p.is_closed());
        assert!(p.face_normal(0)[1] > 0.99);
        assert_eq!(c.top_face(), Some(1));
    }

    #[test]
    fn extrude_and_bevel_stay_closed() {
        let mut c = PolyMesh::cube();
        let top = c.top_face().unwrap();
        c.extrude(top, 0.5).unwrap();
        assert_eq!(c.faces.len(), 10);
        assert!(c.is_closed());
        assert!(near(c.volume(), 1.5, 1e-4));
        let top = c.top_face().unwrap();
        c.bevel(top, 0.25, 0.1).unwrap();
        assert_eq!(c.faces.len(), 14);
        assert!(c.is_closed());
        assert!(c.volume() > 1.5);
        assert!(c.extrude(99, 1.0).is_err());
    }

    #[test]
    fn edge_loop_on_cube_cylinder_and_plane() {
        let mut c = PolyMesh::cube();
        let (a, b) = c.default_loop_edge().unwrap();
        assert!((c.verts[a][1] - c.verts[b][1]).abs() > 0.9, "vertical edge");
        let split = c.edge_loop(a, b, 0.5).unwrap();
        assert_eq!(split, 4);
        assert_eq!(c.faces.len(), 10);
        assert_eq!(c.verts.len(), 12);
        assert!(c.is_closed());
        assert!(near(c.volume(), 1.0, 1e-5));
        // new ring sits at mid height
        assert!(c.verts[8..].iter().all(|v| v[1].abs() < 1e-5));

        let mut cy = PolyMesh::cylinder(8);
        let (a, b) = cy.default_loop_edge().unwrap();
        assert_eq!(cy.edge_loop(a, b, 0.25).unwrap(), 8);
        assert!(cy.is_closed());

        let mut p = PolyMesh::plane();
        assert_eq!(p.edge_loop(0, 3, 0.5).unwrap(), 1);
        assert_eq!(p.faces.len(), 2);

        let mut tri = PolyMesh {
            verts: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            faces: vec![vec![0, 2, 1]],
        };
        assert!(tri.edge_loop(0, 1, 0.5).is_err());
        assert!(PolyMesh::cube().edge_loop(0, 6, 0.5).is_err());
    }

    #[test]
    fn multi_cut_splits_one_face_and_stays_watertight() {
        let mut c = PolyMesh::cube();
        let top = c.top_face().unwrap();
        c.multi_cut(top, 0, 0.5, 2, 0.5).unwrap();
        assert_eq!(c.faces.len(), 7);
        assert_eq!(c.verts.len(), 10);
        assert!(c.is_closed());
        assert!(near(c.volume(), 1.0, 1e-5));
        assert!(c.multi_cut(top, 1, 0.5, 1, 0.5).is_err());
        let mut p = PolyMesh::plane();
        p.multi_cut(0, 0, 0.3, 2, 0.7).unwrap();
        assert_eq!(p.faces.len(), 2);
    }

    #[test]
    fn bridge_two_cubes_into_one_closed_mesh() {
        let mut a = PolyMesh::cube();
        let b = PolyMesh::cube().transformed(|v| [v[0] + 2.0, v[1], v[2]]);
        let split = a.merge(&b);
        let (fa, fb) = a.facing_pair(split).unwrap();
        assert!(a.face_normal(fa)[0] > 0.99 && a.face_normal(fb)[0] < -0.99);
        a.bridge(fa, fb).unwrap();
        assert_eq!(a.faces.len(), 14);
        assert!(a.is_closed());
        assert!(near(a.volume(), 3.0, 1e-4), "{}", a.volume());
        assert!(PolyMesh::cube().bridge(0, 0).is_err());
    }

    #[test]
    fn booleans_have_the_right_volume() {
        let a = PolyMesh::cube();
        let b = PolyMesh::cube().transformed(|v| [v[0] + 0.5, v[1], v[2]]);
        let u = boolean(&a, &b, BoolOp::Union);
        let d = boolean(&a, &b, BoolOp::Difference);
        let i = boolean(&a, &b, BoolOp::Intersection);
        assert!(near(u.volume(), 1.5, 1e-3), "union {}", u.volume());
        assert!(near(d.volume(), 0.5, 1e-3), "difference {}", d.volume());
        assert!(near(i.volume(), 0.5, 1e-3), "intersection {}", i.volume());
        // a cylinder drilled through a cube
        let drill = PolyMesh::cylinder(12).transformed(|v| [v[0] * 0.6, v[1] * 2.0, v[2] * 0.6]);
        let holed = boolean(&a, &drill, BoolOp::Difference);
        assert!(
            holed.is_closed(),
            "drilled cube is watertight after T-junction repair"
        );
        // same-height cylinder: coplanar caps on both ends
        let flush = boolean(
            &PolyMesh::cube(),
            &PolyMesh::cylinder(16).transformed(|v| [v[0] * 0.6, v[1], v[2] * 0.6]),
            BoolOp::Difference,
        );
        assert!(flush.is_closed(), "flush difference is watertight");
        let hole = 0.5 * 16.0 * 0.09 * (std::f32::consts::TAU / 16.0).sin();
        assert!(
            (flush.volume() - (1.0 - hole)).abs() < 1e-3,
            "{}",
            flush.volume()
        );
        let hole = 12.0 / 2.0 * 0.09 * (std::f32::consts::TAU / 12.0).sin();
        assert!(near(holed.volume(), 1.0 - hole, 2e-3), "{}", holed.volume());
        assert_eq!(BoolOp::parse("Subtract"), Some(BoolOp::Difference));
    }

    #[test]
    fn quad_draw_welds_shared_edges() {
        let mut m = PolyMesh::default();
        m.quad_draw(
            &[
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
            ],
            0.05,
            None,
        );
        m.quad_draw(
            &[
                [1.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [2.0, 0.0, 1.0],
                [1.01, 0.0, 1.0],
            ],
            0.05,
            None,
        );
        assert_eq!(m.verts.len(), 6);
        assert_eq!(m.faces.len(), 2);
        assert!((0..2).all(|f| m.face_normal(f)[1] > 0.99));
    }

    #[test]
    fn zbuffer_keeps_the_nearest_surface() {
        for order in [0, 1] {
            let mut f = Frame3d::new(20, 20, [0, 0, 0]);
            let far = [[0.0, 0.0, 10.0], [20.0, 0.0, 10.0], [10.0, 20.0, 10.0]];
            let nearer = [[0.0, 0.0, 5.0], [20.0, 0.0, 5.0], [10.0, 20.0, 5.0]];
            if order == 0 {
                f.tri(far, [255, 0, 0], 1);
                f.tri(nearer, [0, 0, 255], 2);
            } else {
                f.tri(nearer, [0, 0, 255], 2);
                f.tri(far, [255, 0, 0], 1);
            }
            let i = (8 * 20 + 10) * 4;
            assert_eq!(&f.rgba[i..i + 3], &[0, 0, 255]);
            assert_eq!(f.id_at(10.0, 8.0), 2);
        }
    }

    #[test]
    fn blinn_phong_metal_vs_dielectric() {
        let copper = Surface {
            base: [0.955, 0.638, 0.538],
            metalness: 1.0,
            roughness: 0.3,
            ior: 1.5,
            transmission: 0.0,
        };
        let red_plastic = Surface {
            base: [0.8, 0.05, 0.05],
            metalness: 0.0,
            roughness: 0.3,
            ior: 1.5,
            transmission: 0.0,
        };
        let n = [0.0, 1.0, 0.0];
        // mirror configuration: light and eye symmetric about the normal → peak highlight
        let (l, v) = ([1.0, 1.0, 0.0], [-1.0, 1.0, 0.0]);
        let cu = shade_surface(n, l, v, &copper);
        assert!(
            cu[0] as i32 > cu[2] as i32 + 20,
            "metal highlight is tinted: {:?}",
            cu
        );
        let pl = shade_surface(n, l, v, &red_plastic);
        // dielectric highlight is white: green/blue get a big specular lift
        let off = shade_surface(n, [1.0, 1.0, 0.0], [0.0, 1.0, 1.0], &red_plastic);
        assert!(
            pl[1] as i32 > off[1] as i32 + 40,
            "white specular on plastic: {:?} vs {:?}",
            pl,
            off
        );
        // a metal has no diffuse: off-specular it is much darker than at the peak
        let cu_off = shade_surface(n, [1.0, 1.0, 0.0], [0.0, 1.0, 1.0], &copper);
        assert!(
            cu[0] as i32 > cu_off[0] as i32 + 30,
            "{:?} {:?}",
            cu,
            cu_off
        );
        // smoother → tighter, brighter peak
        let rough = Surface {
            roughness: 0.8,
            ..copper
        };
        assert!(shade_surface(n, l, v, &copper)[1] > shade_surface(n, l, v, &rough)[1]);
        // Fresnel: water IOR 1.333 has F0 ≈ 0.02, glass 1.5 ≈ 0.04
        assert!(
            (Surface {
                ior: 1.333,
                ..red_plastic
            }
            .f0()[0]
                - 0.0204)
                .abs()
                < 0.001
        );
        assert!(
            (Surface {
                ior: 1.5,
                ..red_plastic
            }
            .f0()[0]
                - 0.04)
                .abs()
                < 0.001
        );
        // sRGB round trip
        for c in [0u8, 1, 50, 128, 200, 255] {
            assert_eq!(linear_to_srgb(srgb_to_linear(c)), c);
        }
    }

    #[test]
    fn shaded_cube_renders_lit_and_picks_faces() {
        let cam = Camera {
            yaw: 0.6,
            pitch: 0.45,
            dist: 4.0,
        };
        let mut f = Frame3d::new(160, 120, [10, 10, 10]);
        f.mesh(
            &cam,
            &PolyMesh::cube(),
            [200, 200, 200],
            [2.0, 4.0, 3.0],
            100,
            Some([255, 200, 0, 255]),
        );
        let covered = f.ids.iter().filter(|&&i| i >= 100).count();
        assert!(covered > 1500, "{}", covered);
        let ids: std::collections::BTreeSet<u32> =
            f.ids.iter().copied().filter(|&i| i >= 100).collect();
        assert_eq!(ids.len(), 3, "three faces visible from a corner view");
        // distinct shading per visible face
        let mut cols = std::collections::BTreeSet::new();
        for (i, id) in f.ids.iter().enumerate() {
            if *id >= 100 && f.depth[i].is_finite() {
                cols.insert((f.rgba[i * 4], f.rgba[i * 4 + 1]));
            }
        }
        assert!(cols.len() >= 3);
        assert!(
            shade(
                [0.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [100, 100, 100]
            )[0] > shade(
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                [100, 100, 100]
            )[0]
        );
    }
}
