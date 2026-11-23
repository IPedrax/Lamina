//! HTTP digest authentication, from RFC 7616 (and RFC 2617 before it): how a PrusaLink printer set to a user name and password checks
//! them. The password never goes out, only a hash of it with the printer's nonce. MD5 and SHA-256 and their `-sess` forms, with
//! `qop=auth`, or RFC 2069's form for a server that offers no `qop`. MD5 (RFC 1321) and SHA-256 (FIPS 180-4) are written here, checked
//! against the standards' own examples, so the crate needs no hashing library.
//!
//! The challenge comes from the printer and is hostile: a value with a control character, or longer than 1 KB, is refused, and what
//! goes back in the `Authorization` header is quoted and escaped.

/// A server's digest challenge, from a `WWW-Authenticate` header.
#[derive(Clone, Debug, PartialEq)]
pub struct Challenge {
    pub realm: String,
    pub nonce: String,
    pub opaque: Option<String>,
    pub sha256: bool,
    pub sess: bool,
    /// Whether the server offers `qop=auth`; without it, RFC 2069's response.
    pub qop: bool,
    pub userhash: bool,
}

/// The `name=value` pairs after the scheme, names in lower case and quoted values unescaped, up to the end or the next scheme. None
/// where the text is not well formed.
fn params(text: &str) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut rest = text.trim_start();
    while !rest.is_empty() {
        let eq = rest.find('=')?;
        let name = rest[..eq].trim();
        if name.contains(char::is_whitespace) {
            break; // `Basic realm=...`: the next challenge in the same header
        }
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return None;
        }
        rest = rest[eq + 1..].trim_start();
        let value = if let Some(quoted) = rest.strip_prefix('"') {
            let mut value = String::new();
            let mut chars = quoted.char_indices();
            let mut end = None;
            while let Some((i, c)) = chars.next() {
                match c {
                    '\\' => value.push(chars.next()?.1),
                    '"' => {
                        end = Some(i + 1);
                        break;
                    }
                    c => value.push(c),
                }
            }
            rest = &quoted[end?..];
            value
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            let value = rest[..end].trim().to_string();
            rest = &rest[end..];
            value
        };
        out.push((name.to_ascii_lowercase(), value));
        rest = rest.trim_start();
        match rest.strip_prefix(',') {
            Some(r) => rest = r.trim_start(),
            None if rest.is_empty() => {}
            None => return None,
        }
    }
    Some(out)
}

/// One `WWW-Authenticate` header's digest challenge, if it is one Lamina can answer.
fn one(header: &str) -> Option<Challenge> {
    let (scheme, rest) = header.trim().split_once(char::is_whitespace)?;
    if !scheme.eq_ignore_ascii_case("digest") {
        return None;
    }
    let p = params(rest)?;
    let get = |k: &str| p.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    let (sha256, sess) = match get("algorithm").unwrap_or("MD5").to_ascii_uppercase().as_str() {
        "MD5" => (false, false),
        "MD5-SESS" => (false, true),
        "SHA-256" => (true, false),
        "SHA-256-SESS" => (true, true),
        _ => return None,
    };
    // Only `auth`: `auth-int` would hash the whole file first.
    let qop = match get("qop") {
        None => false,
        Some(q) if q.split(',').any(|o| o.trim().eq_ignore_ascii_case("auth")) => true,
        Some(_) => return None,
    };
    if sess && !qop {
        return None; // `-sess` needs the client nonce, which only `qop` carries
    }
    let text = |v: &str| (v.len() <= 1024 && !v.chars().any(char::is_control)).then(|| v.to_string());
    Some(Challenge {
        realm: text(get("realm")?)?,
        nonce: text(get("nonce")?).filter(|n| !n.is_empty())?,
        opaque: match get("opaque") {
            Some(o) => Some(text(o)?),
            None => None,
        },
        sha256,
        sess,
        qop,
        userhash: get("userhash").is_some_and(|v| v.eq_ignore_ascii_case("true")),
    })
}

/// The challenge to answer among a reply's `WWW-Authenticate` headers: SHA-256 before MD5, as RFC 7616 asks.
pub fn challenge<'a>(headers: impl IntoIterator<Item = &'a str>) -> Option<Challenge> {
    let all: Vec<Challenge> = headers.into_iter().filter_map(one).collect();
    all.iter().find(|c| c.sha256).or(all.first()).cloned()
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The `Authorization` header that answers `c` for `method` on `uri` (the request's path and query), the `nc`th use of its nonce.
pub fn authorization(c: &Challenge, user: &str, password: &str, method: &str, uri: &str, cnonce: &str, nc: u32) -> String {
    let h = |s: &str| if c.sha256 { hex(&sha256(s.as_bytes())) } else { hex(&md5(s.as_bytes())) };
    let mut ha1 = h(&format!("{user}:{}:{password}", c.realm));
    if c.sess {
        ha1 = h(&format!("{ha1}:{}:{cnonce}", c.nonce));
    }
    let ha2 = h(&format!("{method}:{uri}"));
    let nc = format!("{nc:08x}");
    let response = if c.qop { h(&format!("{ha1}:{}:{nc}:{cnonce}:auth:{ha2}", c.nonce)) } else { h(&format!("{ha1}:{}:{ha2}", c.nonce)) };
    let name = if c.userhash { h(&format!("{user}:{}", c.realm)) } else { user.to_string() };
    let algorithm = match (c.sha256, c.sess) {
        (false, false) => "MD5",
        (false, true) => "MD5-sess",
        (true, false) => "SHA-256",
        (true, true) => "SHA-256-sess",
    };
    let mut out = format!(
        "Digest username={}, realm={}, uri={}, algorithm={algorithm}, nonce={}",
        quote(&name),
        quote(&c.realm),
        quote(uri),
        quote(&c.nonce)
    );
    if c.qop {
        out.push_str(&format!(", nc={nc}, cnonce={}, qop=auth", quote(cnonce)));
    }
    out.push_str(&format!(", response=\"{response}\""));
    if let Some(o) = &c.opaque {
        out.push_str(&format!(", opaque={}", quote(o)));
    }
    if c.userhash {
        out.push_str(", userhash=true");
    }
    out
}

/// A client nonce no one can guess: 128 bits from the standard library's randomly keyed hasher.
pub fn cnonce() -> String {
    use std::hash::{BuildHasher, RandomState};
    let now = std::time::SystemTime::now();
    format!("{:016x}{:016x}", RandomState::new().hash_one(now), RandomState::new().hash_one(now))
}

/// `data`, a one bit, zeros, and its length in bits: whole 64-byte blocks for MD5 (length little-endian) and SHA-256 (big-endian).
fn padded(data: &[u8], big_endian: bool) -> Vec<u8> {
    let mut m = data.to_vec();
    m.push(0x80);
    while m.len() % 64 != 56 {
        m.push(0);
    }
    let bits = (data.len() as u64).wrapping_mul(8);
    m.extend_from_slice(&if big_endian { bits.to_be_bytes() } else { bits.to_le_bytes() });
    m
}

/// MD5, RFC 1321.
pub fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 16] = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1,
        0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453,
        0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942,
        0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05,
        0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d,
        0x85845dd1, 0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
    ];
    let mut state: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
    for block in padded(data, false).as_chunks::<64>().0 {
        let m: [u32; 16] = std::array::from_fn(|i| u32::from_le_bytes(block.as_chunks::<4>().0[i]));
        let [mut a, mut b, mut c, mut d] = state;
        for (i, k) in K.iter().enumerate() {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let turned = a.wrapping_add(f).wrapping_add(*k).wrapping_add(m[g]).rotate_left(S[(i / 16) * 4 + i % 4]);
            (a, b, c, d) = (d, b.wrapping_add(turned), b, c);
        }
        for (s, x) in state.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(x);
        }
    }
    let mut out = [0; 16];
    for (o, s) in out.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        *o = s.to_le_bytes();
    }
    out
}

/// SHA-256, FIPS 180-4.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be,
        0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa,
        0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85,
        0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
        0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f,
        0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    for block in padded(data, true).as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for (k, w) in K.iter().zip(w) {
            let [a, b, c, d, e, f, g, hh] = v;
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let t1 = hh.wrapping_add(s1).wrapping_add((e & f) ^ (!e & g)).wrapping_add(*k).wrapping_add(w);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let t2 = s0.wrapping_add((a & b) ^ (a & c) ^ (b & c));
            v = [t1.wrapping_add(t2), a, b, c, d.wrapping_add(t1), e, f, g];
        }
        for (s, x) in h.iter_mut().zip(v) {
            *s = s.wrapping_add(x);
        }
    }
    let mut out = [0; 32];
    for (o, s) in out.as_chunks_mut::<4>().0.iter_mut().zip(h) {
        *o = s.to_be_bytes();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_and_sha256_give_their_standards_examples() {
        // RFC 1321, appendix A.5.
        for (text, want) in [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            ("12345678901234567890123456789012345678901234567890123456789012345678901234567890", "57edf4a22be3c955ac49da2e2107b67a"),
        ] {
            assert_eq!(hex(&md5(text.as_bytes())), want, "{text}");
        }
        // FIPS 180-4's examples: one block, two blocks, and the empty message.
        for (text, want) in [
            ("abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
            (
                "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
            ("", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        ] {
            assert_eq!(hex(&sha256(text.as_bytes())), want, "{text}");
        }
        assert_eq!(hex(&sha256(&[b'a'; 1_000_000])), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }

    #[test]
    fn the_rfcs_worked_examples_come_out() {
        // RFC 7616, 3.9.1: the same challenge answered with SHA-256 and with MD5.
        let header = r#"Digest
            realm="http-auth@example.org",
            qop="auth, auth-int",
            algorithm=SHA-256,
            nonce="7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v",
            opaque="FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS""#;
        let md5_header = header.replace("SHA-256", "MD5");
        let c = challenge([md5_header.as_str(), header]).unwrap();
        assert!(c.sha256 && c.qop && !c.sess, "SHA-256 is taken first: {c:?}");
        let cnonce = "f2/wE4q74E6zIJEtWaHKaf5wv/H5QzzpXusqGemxURZJ";
        let a = authorization(&c, "Mufasa", "Circle of Life", "GET", "/dir/index.html", cnonce, 1);
        assert!(a.contains(r#"response="753927fa0e85d155564e2e272a28d1802ca10daf4496794697cf8db5856cb6c1""#), "{a}");
        assert!(
            a.contains("nc=00000001") && a.contains("qop=auth,") && a.contains(r#"opaque="FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS""#)
        );
        let c = challenge([md5_header.as_str()]).unwrap();
        let a = authorization(&c, "Mufasa", "Circle of Life", "GET", "/dir/index.html", cnonce, 1);
        assert!(a.contains(r#"response="8ca523f5e9506fed4657c9700eebdbec""#) && a.contains("algorithm=MD5,"), "{a}");
        // RFC 2617, 3.5.
        let c = challenge([
            r#"Digest realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#,
        ])
        .unwrap();
        let a = authorization(&c, "Mufasa", "Circle Of Life", "GET", "/dir/index.html", "0a4f113b", 1);
        assert!(a.contains(r#"response="6629fae49393a05397450978507c4ef1""#), "{a}");
    }

    #[test]
    fn odd_challenges_are_refused_or_read_with_care() {
        // No qop: RFC 2069's response, H(HA1:nonce:HA2), and no nc or cnonce.
        let c = challenge([r#"Digest realm="Printer API", nonce="0123456789abcdef", stale=false"#]).unwrap();
        assert!(!c.qop && !c.sha256);
        let a = authorization(&c, "maker", "pw", "PUT", "/api/v1/files/usb/a.gcode", "x", 1);
        let ha1 = hex(&md5(b"maker:Printer API:pw"));
        let ha2 = hex(&md5(b"PUT:/api/v1/files/usb/a.gcode"));
        assert!(a.contains(&format!("response=\"{}\"", hex(&md5(format!("{ha1}:0123456789abcdef:{ha2}").as_bytes())))), "{a}");
        assert!(!a.contains("cnonce") && !a.contains("nc="));
        // A quote in a value comes back escaped; a line break, an unknown algorithm or only auth-int is refused.
        let c = challenge([r#"Digest realm="a \"b\" c", nonce="n""#]).unwrap();
        assert_eq!(c.realm, "a \"b\" c");
        assert!(authorization(&c, "u", "p", "GET", "/", "x", 1).contains(r#"realm="a \"b\" c""#));
        for bad in [
            "Digest realm=\"a\r\nX-Evil: 1\", nonce=\"n\"",
            r#"Digest realm="a", nonce="n", algorithm=SHA-512-256"#,
            r#"Digest realm="a", nonce="n", qop="auth-int""#,
            r#"Digest realm="a", nonce="n", algorithm=MD5-sess"#,
            r#"Digest realm="a""#,
            r#"Basic realm="a""#,
            r#"Digest realm="unterminated, nonce="n"#,
        ] {
            assert_eq!(challenge([bad]), None, "{bad:?}");
        }
        // Two challenges in one header: the digest one is read up to the next.
        let c = challenge([r#"Digest realm="a", nonce="n", Basic realm="b""#]).unwrap();
        assert_eq!((c.realm.as_str(), c.nonce.as_str()), ("a", "n"));
        assert_ne!(cnonce(), cnonce());
    }

    #[test]
    fn a_hostile_challenge_never_panics() {
        let mut g = lamina_core::fuzz::Rng::new(0xd16e57);
        let good: &[&[u8]] = &[
            br#"Digest realm="Printer API", nonce="0123", qop="auth", algorithm=SHA-256, opaque="o", userhash=true"#,
            br#"Digest realm="a \"b\"", nonce="n", Basic realm="b""#,
        ];
        for _ in 0..5000 {
            let text = String::from_utf8_lossy(&g.input(good)).into_owned();
            if let Some(c) = challenge([text.as_str()]) {
                let a = authorization(&c, "maker", "pw", "GET", "/api/v1/status", "c", 1);
                assert!(!a.chars().any(char::is_control), "{a:?}");
            }
        }
    }
}
