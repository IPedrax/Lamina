//! A small seeded generator for the parsers' fuzz tests (every crate's tests use it): random bytes, and good inputs mutated by flipped
//! bits, cuts, repeats and splices. Files and agents are hostile, so each parser is fed a few thousand of these and must answer with an
//! error, never a panic. Seeded, so a failure is the same failure on every run and every machine.

/// xorshift64*: small, fast and good enough to shake a parser.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number from 0 to `n - 1` (0 when `n` is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next_u64() as u8).collect()
    }

    /// `good` changed in one to four ways: bits flipped, bytes overwritten with odd values, a cut, a stretch repeated, a stretch
    /// dropped, or random bytes put in.
    pub fn mutate(&mut self, good: &[u8]) -> Vec<u8> {
        let mut b = good.to_vec();
        for _ in 0..1 + self.below(4) {
            let n = b.len();
            match self.below(6) {
                0 if n > 0 => {
                    let i = self.below(n);
                    b[i] ^= 1 << self.below(8);
                }
                1 if n > 0 => {
                    let i = self.below(n);
                    b[i] = [0, 0xFF, b'-', b'9', b'e', b'"', b'{', b'[', 0x80][self.below(9)];
                }
                2 => b.truncate(self.below(n + 1)),
                3 if n > 0 => {
                    let (i, len) = (self.below(n), 1 + self.below(64));
                    let piece: Vec<u8> = b[i..(i + len).min(n)].to_vec();
                    let at = self.below(n + 1);
                    b.splice(at..at, piece.iter().copied().cycle().take(piece.len() * (1 + self.below(4))));
                }
                4 if n > 0 => {
                    let i = self.below(n);
                    let end = (i + 1 + self.below(64)).min(n);
                    b.drain(i..end);
                }
                _ => {
                    let at = self.below(n + 1);
                    let len = 1 + self.below(16);
                    let junk = self.bytes(len);
                    b.splice(at..at, junk);
                }
            }
        }
        b
    }

    /// One of the inputs a fuzz test makes: random bytes a quarter of the time, else one of `good` mutated.
    pub fn input(&mut self, good: &[&[u8]]) -> Vec<u8> {
        if good.is_empty() || self.below(4) == 0 {
            let n = self.below(256);
            self.bytes(n)
        } else {
            let pick = good[self.below(good.len())];
            self.mutate(pick)
        }
    }
}

/// A random JSON value, for command parameters: numbers that are huge, tiny, negative or not whole, strings that are long or odd,
/// arrays and objects nested a little, with keys a command may or may not know.
pub fn json(r: &mut Rng, keys: &[&str], depth: usize) -> serde_json::Value {
    use serde_json::{Value, json};
    match r.below(if depth == 0 { 4 } else { 6 }) {
        0 => Value::Null,
        1 => json!(r.below(2) == 1),
        2 => {
            let pick = [0.0, -1.0, 1.0, 0.5, 1e300, -1e300, 1e-300, 3.0, 255.0, 4_294_967_296.0, 1e9, 20.0, -7.25];
            json!(pick[r.below(pick.len())])
        }
        3 => {
            let pick = ["", "x", "box", "cylinder", "sphere", "z", "../../etc/passwd", "\u{0}", "nan", "infinity"];
            json!(pick[r.below(pick.len())])
        }
        4 => Value::Array((0..r.below(5)).map(|_| json(r, keys, depth - 1)).collect()),
        _ => {
            let mut m = serde_json::Map::new();
            for _ in 0..r.below(5) {
                let key = if !keys.is_empty() && r.below(4) != 0 { keys[r.below(keys.len())].to_string() } else { "unknown".into() };
                m.insert(key, json(r, keys, depth - 1));
            }
            Value::Object(m)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Mesh, Session, base64, project, stl, threemf, zip};
    use serde_json::json;

    const ROUNDS: usize = 3000;

    #[test]
    fn the_generator_is_the_same_every_run() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        assert_eq!((0..100).map(|_| a.next_u64()).collect::<Vec<_>>(), (0..100).map(|_| b.next_u64()).collect::<Vec<_>>());
        assert_ne!(Rng::new(1).mutate(b"hello world"), b"hello world");
    }

    #[test]
    fn stl_never_panics() {
        let (binary, ascii) = (
            stl::write(&Mesh::cylinder(3.0, 5.0, 12)),
            b"solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n",
        );
        let mut r = Rng::new(0x5714);
        for _ in 0..ROUNDS {
            let _ = stl::read(&r.input(&[&binary, ascii]));
        }
    }

    #[test]
    fn projects_never_panic() {
        let mut s = Session::new();
        s.execute("add_primitive", &json!({"kind": "box", "size": [5, 5, 5]})).unwrap();
        s.execute("add_primitive", &json!({"kind": "sphere", "radius": 3, "segments": 16, "at": [10, 0]})).unwrap();
        let good = project::write(&s.scene, &json!({"profile": {"layer_height": 0.2}}));
        let mut r = Rng::new(0x9a0);
        for _ in 0..ROUNDS {
            let bytes = r.input(&[good.as_bytes()]);
            let _ = project::read(&String::from_utf8_lossy(&bytes));
        }
    }

    #[test]
    fn zip_and_3mf_never_panic() {
        let part = threemf::Part { name: "p".into(), mesh: Mesh::cuboid([5.0, 5.0, 5.0]), xf: crate::Transform::IDENTITY };
        let good = threemf::write(&[part]);
        let stored = zip::write(&[("a.txt", b"hello")]);
        let mut r = Rng::new(0x3f3f);
        for _ in 0..ROUNDS {
            let bytes = r.input(&[&good, &stored]);
            let _ = threemf::read(&bytes);
            if let Ok(a) = zip::Archive::new(&bytes) {
                let names: Vec<String> = a.names().map(str::to_string).collect();
                for n in names {
                    let _ = a.read(&n, 1 << 20);
                }
            }
        }
    }

    #[test]
    fn obj_and_gltf_never_panic() {
        let obj = b"o a\nv 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 3 2\nf 1 2 4\nf 1 4 3\nf 2 3 4\nusemtl x\nf -1 -2 -3\n";
        let gltf = br#"{"asset":{"version":"2.0"},"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0,"matrix":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]}],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}],"bufferViews":[{"buffer":0,"byteLength":36}],"buffers":[{"byteLength":36,"uri":"data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAA"}]}"#;
        let mut r = Rng::new(0x0b1);
        for _ in 0..ROUNDS {
            let bytes = r.input(&[obj, gltf]);
            let _ = crate::mesh_in::obj(&String::from_utf8_lossy(&bytes), 1.0, r.below(2) == 1, |_| None);
            let _ = crate::mesh_in::gltf(&bytes, |_| None);
        }
    }

    #[test]
    fn base64_never_panics() {
        let good = base64::encode(b"any bytes at all \x00\xff");
        let mut r = Rng::new(0xb64);
        for _ in 0..ROUNDS {
            let bytes = r.input(&[good.as_bytes()]);
            let _ = base64::decode(&String::from_utf8_lossy(&bytes));
        }
    }

    #[test]
    fn command_parameters_never_panic_and_never_break_the_scene() {
        let mut s = Session::new();
        s.execute("add_primitive", &json!({"kind": "box", "size": [5, 5, 5]})).unwrap();
        let keys = [
            "id",
            "kind",
            "size",
            "radius",
            "height",
            "segments",
            "at",
            "by",
            "axis",
            "degrees",
            "factor",
            "name",
            "ids",
            "radius_top",
            "sides",
            "inner",
            "tube",
            "corner",
        ];
        let commands: Vec<String> = s.catalog().as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).map(str::to_string).collect();
        let mut r = Rng::new(0xc0de);
        for _ in 0..ROUNDS {
            let c = &commands[r.below(commands.len())];
            // Commands that touch files are given no path to a real file, so nothing outside the test is read or written.
            if s.find(c).is_some_and(|c| c.files != crate::Files::None) {
                continue;
            }
            let p = json(&mut r, &keys, 3);
            if let Err(e) = s.execute(c, &p) {
                assert!(!e.contains("internal error"), "`{c} {p}` panicked inside: {e}");
            }
            assert!(s.scene.check().is_ok(), "`{c} {p}` left the scene broken");
            assert!(s.scene.objects.len() < 200, "the plate fills only as fast as commands add to it");
        }
    }
}
