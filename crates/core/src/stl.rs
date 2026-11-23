//! STL, binary and ASCII, from the format's public description. Vertices are welded on read (STL repeats each corner for every
//! triangle that uses it), so a closed solid comes in closed and `Mesh::is_watertight` can tell.

use std::collections::HashMap;

use crate::mesh::{MAX_TRIANGLES, Mesh};

/// The largest file read, bytes.
pub const MAX_FILE: u64 = 256 << 20;

/// A mesh from the bytes of an STL file, binary or ASCII. Degenerate triangles (two corners welded together) are dropped.
pub fn read(bytes: &[u8]) -> Result<Mesh, String> {
    let binary = bytes.len() >= 84 && {
        let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as u64;
        bytes.len() as u64 == 84 + 50 * n
    };
    let corners = if binary { binary_corners(bytes)? } else { ascii_corners(bytes)? };
    weld(&corners)
}

/// A mesh from triangle corners (three per triangle), each corner welded to the same point met before. Degenerate triangles (two
/// corners welded together) are dropped. STL and every file that lists triangles as corners come in through here.
pub fn weld(corners: &[[f32; 3]]) -> Result<Mesh, String> {
    if corners.len() / 3 > MAX_TRIANGLES {
        return Err(format!("{} triangles is more than the {MAX_TRIANGLES} a mesh may hold", corners.len() / 3));
    }
    let mut welded: HashMap<[u32; 3], u32> = HashMap::new();
    let (mut vertices, mut triangles) = (Vec::new(), Vec::new());
    for tri in corners.as_chunks::<3>().0 {
        let ids: Vec<u32> = tri
            .iter()
            .map(|c| {
                // + 0.0 turns -0.0 into 0.0, so a corner written either way welds.
                let key = c.map(|x| (x + 0.0).to_bits());
                *welded.entry(key).or_insert_with(|| {
                    vertices.push(*c);
                    vertices.len() as u32 - 1
                })
            })
            .collect();
        if ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2] {
            triangles.push([ids[0], ids[1], ids[2]]);
        }
    }
    if triangles.is_empty() {
        return Err("the file holds no triangles".into());
    }
    Mesh::new(vertices, triangles)
}

fn binary_corners(bytes: &[u8]) -> Result<Vec<[f32; 3]>, String> {
    let f = |b: &[u8]| f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let mut out = Vec::with_capacity((bytes.len() - 84) / 50 * 3);
    for record in bytes[84..].as_chunks::<50>().0 {
        for corner in 0..3 {
            let at = 12 + corner * 12;
            out.push([f(&record[at..]), f(&record[at + 4..]), f(&record[at + 8..])]);
        }
    }
    Ok(out)
}

fn ascii_corners(bytes: &[u8]) -> Result<Vec<[f32; 3]>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "not an STL file: neither binary nor ASCII text".to_string())?;
    if !text.trim_start().to_ascii_lowercase().starts_with("solid") {
        return Err("not an STL file: no binary size match and no `solid` keyword".into());
    }
    let mut out = Vec::new();
    let mut words = text.split_whitespace();
    while let Some(w) = words.next() {
        if w.eq_ignore_ascii_case("vertex") {
            let mut c = [0f32; 3];
            for slot in &mut c {
                *slot = words.next().and_then(|s| s.parse().ok()).ok_or("a `vertex` line lacks three numbers")?;
            }
            out.push(c);
        }
    }
    if out.len() % 3 != 0 {
        return Err("the vertex count is not a multiple of three".into());
    }
    Ok(out)
}

/// A binary STL of `mesh`; each triangle's normal is worked out from its corners.
pub fn write(mesh: &Mesh) -> Vec<u8> {
    let mut out = vec![0u8; 80];
    out[..6].copy_from_slice(b"Lamina");
    out.extend_from_slice(&(mesh.triangles.len() as u32).to_le_bytes());
    for i in 0..mesh.triangles.len() {
        let Some(tri) = mesh.tri(i) else { continue };
        let (u, v) = (
            [tri[1][0] - tri[0][0], tri[1][1] - tri[0][1], tri[1][2] - tri[0][2]],
            [tri[2][0] - tri[0][0], tri[2][1] - tri[0][1], tri[2][2] - tri[0][2]],
        );
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let n = if len > 0.0 { n.map(|c| (c / len) as f32) } else { [0.0; 3] };
        for c in n.iter().copied().chain(tri.iter().flatten().map(|&c| c as f32)) {
            out.extend_from_slice(&c.to_le_bytes());
        }
        out.extend_from_slice(&[0, 0]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_round_trip_keeps_a_closed_solid_closed() {
        let cube = Mesh::cuboid([10.0, 10.0, 10.0]);
        let back = read(&write(&cube)).unwrap();
        assert_eq!(back.triangles.len(), 12);
        assert_eq!(back.vertices.len(), 8);
        assert!(back.is_watertight());
        assert!((back.volume() - 1000.0).abs() < 1e-3);
    }

    #[test]
    fn ascii_is_read_and_welded() {
        let text = "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n";
        let m = read(text.as_bytes()).unwrap();
        assert_eq!((m.vertices.len(), m.triangles.len()), (3, 1));
    }

    #[test]
    fn hostile_files_are_refused_not_trusted() {
        // A header that claims four billion triangles in a file of 84 bytes is not binary and has no `solid`.
        let mut lie = vec![0u8; 84];
        lie[80..84].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read(&lie).is_err());
        assert!(read(b"").is_err());
        assert!(read(b"solid x\nvertex 1 2\n").is_err());
    }
}
