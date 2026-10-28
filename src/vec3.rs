//! Vec3: three floats. The materials use it as a linear RGB colour.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    #[inline]
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_keeps_components() {
        let v = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!((v.x, v.y, v.z), (1.0, 2.0, 3.0));
    }
}
