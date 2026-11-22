//! A triangle mesh and the transform that places it. Millimetres, x east, y north, z up, the plate at z = 0.

use std::collections::HashMap;

pub type P3 = [f64; 3];

/// The most triangles a mesh may hold, and the farthest a vertex may lie from the origin (mm). Files are hostile.
pub const MAX_TRIANGLES: usize = 5_000_000;
pub const MAX_COORD: f64 = 1e6;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<[f32; 3]>,
    /// Counter-clockwise seen from outside, as STL and every slicer expect.
    pub triangles: Vec<[u32; 3]>,
}

impl Mesh {
    /// A mesh, after checking what a file or a command may get wrong: size, finite coordinates and indices in range.
    pub fn new(vertices: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>) -> Result<Mesh, String> {
        if triangles.len() > MAX_TRIANGLES {
            return Err(format!("{} triangles is more than the {MAX_TRIANGLES} a mesh may hold", triangles.len()));
        }
        if vertices.iter().any(|v| v.iter().any(|c| !c.is_finite() || f64::from(c.abs()) > MAX_COORD)) {
            return Err(format!("a vertex is not a finite point within {MAX_COORD} mm of the origin"));
        }
        let n = vertices.len();
        if triangles.iter().any(|t| t.iter().any(|&i| i as usize >= n)) {
            return Err("a triangle names a vertex the mesh does not have".into());
        }
        Ok(Mesh { vertices, triangles })
    }

    /// Triangle `i`'s corners.
    pub fn tri(&self, i: usize) -> Option<[P3; 3]> {
        let t = self.triangles.get(i)?;
        let p = |k: usize| self.vertices.get(t[k] as usize).map(|v| v.map(f64::from));
        Some([p(0)?, p(1)?, p(2)?])
    }

    /// The least and greatest corner, or none for an empty mesh.
    pub fn bounds(&self) -> Option<(P3, P3)> {
        let first = self.vertices.first()?.map(f64::from);
        Some(self.vertices.iter().fold((first, first), |(mut lo, mut hi), v| {
            for k in 0..3 {
                lo[k] = lo[k].min(f64::from(v[k]));
                hi[k] = hi[k].max(f64::from(v[k]));
            }
            (lo, hi)
        }))
    }

    /// The volume it encloses, mm³: positive when the triangles wind outward (the divergence theorem, one tetrahedron each).
    pub fn volume(&self) -> f64 {
        (0..self.triangles.len()).filter_map(|i| self.tri(i)).map(|[a, b, c]| dot(a, cross(b, c))).sum::<f64>() / 6.0
    }

    /// Whether it is a closed surface: every edge is shared by exactly two triangles that walk it in opposite directions.
    /// A slicer can only fill what is closed, so an open mesh is repaired or refused.
    pub fn is_watertight(&self) -> bool {
        let mut edges: HashMap<(u32, u32), (u32, i32)> = HashMap::new();
        for t in &self.triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                let e = edges.entry((a.min(b), a.max(b))).or_default();
                e.0 += 1;
                e.1 += if a < b { 1 } else { -1 };
            }
        }
        !edges.is_empty() && edges.values().all(|&(count, net)| count == 2 && net == 0)
    }

    /// The mesh moved by `t`. A transform with a positive determinant keeps the winding.
    pub fn transformed(&self, t: &Transform) -> Mesh {
        let vertices = self.vertices.iter().map(|v| t.apply(v.map(f64::from)).map(|c| c as f32)).collect();
        Mesh { vertices, triangles: self.triangles.clone() }
    }

    /// Several meshes as one, for export.
    pub fn merged<'a>(meshes: impl IntoIterator<Item = &'a Mesh>) -> Mesh {
        let mut out = Mesh::default();
        for m in meshes {
            let base = out.vertices.len() as u32;
            out.vertices.extend_from_slice(&m.vertices);
            out.triangles.extend(m.triangles.iter().map(|t| t.map(|i| i + base)));
        }
        out
    }

    /// A box of `size` with its bottom face on the plate, centred on x and y.
    pub fn cuboid(size: P3) -> Mesh {
        let (hx, hy) = (size[0] / 2.0, size[1] / 2.0);
        let v = |x: f64, y: f64, z: f64| [x as f32, y as f32, z as f32];
        let vertices = vec![
            v(-hx, -hy, 0.0),
            v(hx, -hy, 0.0),
            v(hx, hy, 0.0),
            v(-hx, hy, 0.0),
            v(-hx, -hy, size[2]),
            v(hx, -hy, size[2]),
            v(hx, hy, size[2]),
            v(-hx, hy, size[2]),
        ];
        let triangles = vec![
            [0, 2, 1],
            [0, 3, 2], // bottom
            [4, 5, 6],
            [4, 6, 7], // top
            [0, 1, 5],
            [0, 5, 4], // front
            [1, 2, 6],
            [1, 6, 5], // right
            [2, 3, 7],
            [2, 7, 6], // back
            [3, 0, 4],
            [3, 4, 7], // left
        ];
        Mesh { vertices, triangles }
    }

    /// A cylinder along z, its bottom on the plate, `segments` flat sides (at least 3).
    pub fn cylinder(radius: f64, height: f64, segments: u32) -> Mesh {
        let n = segments.max(3);
        let mut vertices = Vec::new();
        for z in [0.0, height] {
            for i in 0..n {
                let a = std::f64::consts::TAU * f64::from(i) / f64::from(n);
                vertices.push([(radius * a.cos()) as f32, (radius * a.sin()) as f32, z as f32]);
            }
        }
        let (bottom, top) = (vertices.len() as u32, vertices.len() as u32 + 1);
        vertices.push([0.0, 0.0, 0.0]);
        vertices.push([0.0, 0.0, height as f32]);
        let mut triangles = Vec::new();
        for i in 0..n {
            let j = (i + 1) % n;
            triangles.push([bottom, j, i]);
            triangles.push([top, n + i, n + j]);
            triangles.push([i, j, n + j]);
            triangles.push([i, n + j, n + i]);
        }
        Mesh { vertices, triangles }
    }

    /// A sphere resting on the plate, `segments` around and half as many from pole to pole.
    pub fn sphere(radius: f64, segments: u32) -> Mesh {
        let (around, rings) = (segments.max(3), (segments / 2).max(2));
        let mut vertices = vec![[0.0, 0.0, 0.0], [0.0, 0.0, (2.0 * radius) as f32]];
        for r in 1..rings {
            let polar = std::f64::consts::PI * f64::from(r) / f64::from(rings);
            for i in 0..around {
                let a = std::f64::consts::TAU * f64::from(i) / f64::from(around);
                let (s, z) = (polar.sin() * radius, radius - polar.cos() * radius);
                vertices.push([(s * a.cos()) as f32, (s * a.sin()) as f32, z as f32]);
            }
        }
        let ring = |r: u32, i: u32| 2 + (r - 1) * around + i % around;
        let mut triangles = Vec::new();
        for i in 0..around {
            triangles.push([0, ring(1, i + 1), ring(1, i)]);
            triangles.push([1, ring(rings - 1, i), ring(rings - 1, i + 1)]);
            for r in 1..rings - 1 {
                triangles.push([ring(r, i), ring(r, i + 1), ring(r + 1, i + 1)]);
                triangles.push([ring(r, i), ring(r + 1, i + 1), ring(r + 1, i)]);
            }
        }
        Mesh { vertices, triangles }
    }
}

/// An affine map: `m` is the rows of a 3x3 matrix, `t` the translation applied after it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub m: [[f64; 3]; 3],
    pub t: P3,
}

impl Transform {
    pub const IDENTITY: Transform = Transform { m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] };

    pub fn translate(by: P3) -> Transform {
        Transform { t: by, ..Transform::IDENTITY }
    }

    /// Scale by a positive factor on each axis. Negative factors would mirror the mesh and flip its winding, so the commands refuse them.
    pub fn scale(by: P3) -> Transform {
        Transform { m: [[by[0], 0.0, 0.0], [0.0, by[1], 0.0], [0.0, 0.0, by[2]]], t: [0.0; 3] }
    }

    /// A turn of `degrees` about the axis `axis` (0 = x, 1 = y, 2 = z), counter-clockwise looking down the axis toward the origin.
    pub fn rotate(axis: usize, degrees: f64) -> Transform {
        let (s, c) = degrees.to_radians().sin_cos();
        let m = match axis {
            0 => [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]],
            1 => [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]],
            _ => [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
        };
        Transform { m, t: [0.0; 3] }
    }

    /// `self` first, then `next`.
    pub fn then(&self, next: &Transform) -> Transform {
        let mut m = [[0.0; 3]; 3];
        for (r, row) in m.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell = (0..3).map(|k| next.m[r][k] * self.m[k][c]).sum();
            }
        }
        Transform { m, t: next.apply(self.t) }
    }

    pub fn apply(&self, p: P3) -> P3 {
        let row = |r: usize| self.m[r][0] * p[0] + self.m[r][1] * p[1] + self.m[r][2] * p[2] + self.t[r];
        [row(0), row(1), row(2)]
    }

    pub fn is_finite(&self) -> bool {
        self.m.iter().flatten().chain(self.t.iter()).all(|c| c.is_finite())
    }
}

fn cross(a: P3, b: P3) -> P3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: P3, b: P3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_are_closed_and_enclose_their_volume() {
        let cube = Mesh::cuboid([10.0, 20.0, 30.0]);
        assert!(cube.is_watertight());
        assert!((cube.volume() - 6000.0).abs() < 1e-6);
        let cyl = Mesh::cylinder(5.0, 10.0, 96);
        assert!(cyl.is_watertight());
        assert!(cyl.volume() > 0.0 && (cyl.volume() - std::f64::consts::PI * 250.0).abs() < 5.0);
        let ball = Mesh::sphere(5.0, 48);
        assert!(ball.is_watertight());
        assert!((ball.volume() - 4.0 / 3.0 * std::f64::consts::PI * 125.0).abs() < 5.0);
        assert_eq!(ball.bounds().map(|(lo, hi)| (lo[2], hi[2])), Some((0.0, 10.0)));
    }

    #[test]
    fn an_open_mesh_is_not_watertight() {
        let mut cube = Mesh::cuboid([1.0, 1.0, 1.0]);
        cube.triangles.pop();
        assert!(!cube.is_watertight());
    }

    #[test]
    fn hostile_meshes_are_refused() {
        assert!(Mesh::new(vec![[f32::NAN, 0.0, 0.0]], vec![]).is_err());
        assert!(Mesh::new(vec![[2e6, 0.0, 0.0]], vec![]).is_err());
        assert!(Mesh::new(vec![[0.0; 3]], vec![[0, 0, 1]]).is_err());
    }

    #[test]
    fn transforms_compose_in_order() {
        let t = Transform::scale([2.0, 2.0, 2.0]).then(&Transform::translate([1.0, 0.0, 0.0]));
        assert_eq!(t.apply([1.0, 1.0, 1.0]), [3.0, 2.0, 2.0]);
        let r = Transform::rotate(2, 90.0).apply([1.0, 0.0, 0.0]);
        assert!(r[0].abs() < 1e-12 && (r[1] - 1.0).abs() < 1e-12);
    }
}
