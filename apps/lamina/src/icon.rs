//! Lamina's icon, drawn here rather than kept as a picture made elsewhere: three layers of a print, stacked, on a dark rounded
//! square. The window shows it, and the packages carry it as files in `assets/app-icon/`, written from this code (a test checks
//! they still match it; `LAMINA_BLESS=1` writes them again): a PNG for Linux, an ICO for Windows, an ICNS for macOS. ICO and ICNS
//! hold PNG images, which every Windows since Vista and every macOS since 10.7 reads.

use lamina_core::render::{Rgba, png_rgba};

/// The sizes the ICO holds, and the ICNS's (with each one's type code).
const ICO_SIZES: [usize; 4] = [16, 32, 48, 256];
const ICNS_SIZES: [(&[u8; 4], usize); 4] = [(b"ic07", 128), (b"ic08", 256), (b"ic09", 512), (b"ic10", 1024)];

/// Whether (`x`, `y`) is inside the rectangle from `lo` to `hi` with corners rounded to `r`.
fn inside(x: f64, y: f64, lo: [f64; 2], hi: [f64; 2], r: f64) -> bool {
    let cx = x.clamp(lo[0] + r, hi[0] - r);
    let cy = y.clamp(lo[1] + r, hi[1] - r);
    (x - cx) * (x - cx) + (y - cy) * (y - cy) <= r * r && x >= lo[0] && x <= hi[0] && y >= lo[1] && y <= hi[1]
}

/// The colour at (`x`, `y`) of the icon on a unit square, y down; none outside it.
fn colour(x: f64, y: f64) -> Option<[f64; 3]> {
    if !inside(x, y, [0.04, 0.04], [0.96, 0.96], 0.2) {
        return None;
    }
    // Three layers, the top one shifted right of the one below, as a print's walls lean when they are drawn.
    let layers = [(0.30, [245.0, 165.0, 60.0], 0.05), (0.46, [232.0, 238.0, 244.0], 0.0), (0.62, [110.0, 165.0, 225.0], -0.05)];
    for (top, c, shift) in layers {
        if inside(x, y, [0.2 + shift, top], [0.8 + shift, top + 0.12], 0.04) {
            return Some(c);
        }
    }
    // The square: dark slate, a little lighter at the top.
    let k = 1.0 - 0.25 * y;
    Some([34.0 * k + 10.0, 48.0 * k + 10.0, 61.0 * k + 10.0])
}

/// The icon `size` pixels square, RGBA, each pixel the mean of 4 by 4 samples so its edges are smooth.
pub fn pixels(size: usize) -> Vec<Rgba> {
    let size = size.clamp(1, 2048);
    let n = 4;
    let mut out = Vec::with_capacity(size * size);
    for py in 0..size {
        for px in 0..size {
            let (mut sum, mut hits) = ([0.0; 3], 0);
            for sy in 0..n {
                for sx in 0..n {
                    let x = (px as f64 + (sx as f64 + 0.5) / n as f64) / size as f64;
                    let y = (py as f64 + (sy as f64 + 0.5) / n as f64) / size as f64;
                    if let Some(c) = colour(x, y) {
                        (0..3).for_each(|k| sum[k] += c[k]);
                        hits += 1;
                    }
                }
            }
            if hits == 0 {
                out.push([0, 0, 0, 0]);
            } else {
                let c = sum.map(|v| (v / f64::from(hits)).round() as u8);
                out.push([c[0], c[1], c[2], (255 * hits / (n * n)) as u8]);
            }
        }
    }
    out
}

/// The icon as a PNG `size` pixels square.
pub fn png(size: usize) -> Vec<u8> {
    png_rgba([size, size], &pixels(size))
}

/// The icon as a Windows ICO: a directory of sizes, each a PNG.
pub fn ico() -> Vec<u8> {
    let images: Vec<(usize, Vec<u8>)> = ICO_SIZES.iter().map(|&s| (s, png(s))).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]); // reserved, and 1: an icon
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (s, data) in &images {
        let side = if *s >= 256 { 0 } else { *s as u8 }; // 0 stands for 256
        out.extend_from_slice(&[side, side, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits a pixel
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, data) in &images {
        out.extend_from_slice(data);
    }
    out
}

/// The icon as a macOS ICNS: each size a PNG under its type code.
pub fn icns() -> Vec<u8> {
    let mut body = Vec::new();
    for (code, s) in ICNS_SIZES {
        let data = png(s);
        body.extend_from_slice(code);
        body.extend_from_slice(&((data.len() + 8) as u32).to_be_bytes());
        body.extend_from_slice(&data);
    }
    let mut out = b"icns".to_vec();
    out.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

/// The files `assets/app-icon/` holds, by name, as this code draws them.
pub fn files() -> Vec<(&'static str, Vec<u8>)> {
    vec![("lamina-256.png", png(256)), ("lamina-1024.png", png(1024)), ("lamina.ico", ico()), ("lamina.icns", icns())]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_has_its_layers_on_a_clear_corner_and_its_files_are_well_formed() {
        let p = pixels(64);
        assert_eq!(p[0][3], 0, "the corner is clear");
        let at = |x: usize, y: usize| p[y * 64 + x];
        assert!(at(32, 23)[0] > 200 && at(32, 23)[2] < 100, "the top layer is orange: {:?}", at(32, 23));
        assert!(at(32, 44)[2] > 200, "the bottom layer is blue: {:?}", at(32, 44));
        assert!(at(32, 8)[0] < 60, "the square is dark: {:?}", at(32, 8));
        let ico = ico();
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 4, 0]);
        let first = u32::from_le_bytes([ico[18], ico[19], ico[20], ico[21]]) as usize;
        assert_eq!(&ico[first..first + 4], b"\x89PNG");
        let icns = icns();
        assert_eq!(&icns[..4], b"icns");
        assert_eq!(u32::from_be_bytes([icns[4], icns[5], icns[6], icns[7]]) as usize, icns.len());
        assert_eq!(&icns[8..12], b"ic07");
    }

    /// The icon files in the repository are the ones this code draws; `LAMINA_BLESS=1` writes them again after a change.
    #[test]
    fn the_icon_files_are_the_ones_drawn() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/app-icon");
        let bless = std::env::var_os("LAMINA_BLESS").is_some();
        for (name, bytes) in files() {
            let path = dir.join(name);
            if bless {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&path, &bytes).unwrap();
            }
            let kept = std::fs::read(&path).unwrap_or_default();
            assert!(kept == bytes, "{name} differs from the icon drawn: run the tests with LAMINA_BLESS=1 and look at it");
        }
    }
}
