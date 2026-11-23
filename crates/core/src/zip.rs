//! Just enough of the ZIP format (PKWARE's public APPNOTE) to read and write 3MF packages: stored and deflated entries, no
//! encryption, no ZIP64, no spanning. Deflate is `miniz_oxide`. An archive is hostile: every offset and size is checked against the
//! bytes there are, an entry is inflated only up to a limit, and its CRC must match.

use std::collections::BTreeMap;

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const END: u32 = 0x0605_4b50;
/// The most entries an archive may list.
pub const MAX_ENTRIES: usize = 10_000;

/// The CRC-32 of `bytes` (the IEEE polynomial ZIP uses).
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 == 1 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *slot = c;
    }
    !bytes.iter().fold(!0u32, |c, &b| table[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8))
}

fn u16_at(b: &[u8], at: usize) -> Result<u16, String> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(|| "the archive is cut short".to_string())
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or_else(|| "the archive is cut short".to_string())
}

/// An entry of an archive being read.
#[derive(Clone, Debug)]
struct Entry {
    method: u16,
    crc: u32,
    compressed: usize,
    size: usize,
    local: usize,
}

/// An archive read from its bytes: entries by name.
pub struct Archive<'a> {
    bytes: &'a [u8],
    entries: BTreeMap<String, Entry>,
}

impl<'a> Archive<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Archive<'a>, String> {
        // The end record is the last 22 bytes, unless a comment of up to 65535 bytes follows it.
        let lowest = bytes.len().saturating_sub(22 + 0xFFFF);
        let end = (lowest..=bytes.len().saturating_sub(22))
            .rev()
            .find(|&i| u32_at(bytes, i).is_ok_and(|s| s == END))
            .ok_or("not a ZIP archive: no end of central directory")?;
        let count = usize::from(u16_at(bytes, end + 10)?);
        let at = u32_at(bytes, end + 16)? as usize;
        if count == 0xFFFF || at == 0xFFFF_FFFF {
            return Err("ZIP64 archives are not read".into());
        }
        if count > MAX_ENTRIES {
            return Err(format!("the archive lists {count} entries, more than the {MAX_ENTRIES} read"));
        }
        let mut entries = BTreeMap::new();
        let mut p = at;
        for _ in 0..count {
            if u32_at(bytes, p)? != CENTRAL {
                return Err("the archive's central directory is damaged".into());
            }
            let flags = u16_at(bytes, p + 8)?;
            let method = u16_at(bytes, p + 10)?;
            let crc = u32_at(bytes, p + 16)?;
            let compressed = u32_at(bytes, p + 20)?;
            let size = u32_at(bytes, p + 24)?;
            let (name_len, extra_len, comment_len) =
                (usize::from(u16_at(bytes, p + 28)?), usize::from(u16_at(bytes, p + 30)?), usize::from(u16_at(bytes, p + 32)?));
            let local = u32_at(bytes, p + 42)?;
            if [compressed, size, local].contains(&0xFFFF_FFFF) {
                return Err("ZIP64 archives are not read".into());
            }
            if flags & 1 == 1 {
                return Err("encrypted archives are not read".into());
            }
            let name = bytes.get(p + 46..p + 46 + name_len).ok_or("the archive is cut short")?;
            let name = String::from_utf8_lossy(name).trim_start_matches('/').to_ascii_lowercase();
            entries.insert(name, Entry { method, crc, compressed: compressed as usize, size: size as usize, local: local as usize });
            p += 46 + name_len + extra_len + comment_len;
        }
        Ok(Archive { bytes, entries })
    }

    /// The entries' names, lower case, without a leading `/`.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// The bytes of the entry `name` (in any case, with or without a leading `/`), refused when it would be larger than `limit`.
    pub fn read(&self, name: &str, limit: usize) -> Result<Vec<u8>, String> {
        let key = name.trim_start_matches('/').to_ascii_lowercase();
        let e = self.entries.get(&key).ok_or_else(|| format!("the archive has no `{name}`"))?;
        if e.size > limit {
            return Err(format!("`{name}` is {} bytes unpacked, more than the {limit} read", e.size));
        }
        if u32_at(self.bytes, e.local)? != LOCAL {
            return Err(format!("`{name}` is damaged in the archive"));
        }
        let start = e.local + 30 + usize::from(u16_at(self.bytes, e.local + 26)?) + usize::from(u16_at(self.bytes, e.local + 28)?);
        let data = self.bytes.get(start..start.saturating_add(e.compressed)).ok_or("the archive is cut short")?;
        let out = match e.method {
            0 => data.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(data, limit)
                .map_err(|_| format!("`{name}` does not inflate, or is larger than the {limit} bytes read"))?,
            m => return Err(format!("`{name}` is packed with method {m}; only stored and deflated entries are read")),
        };
        if out.len() != e.size || crc32(&out) != e.crc {
            return Err(format!("`{name}` is damaged: its size or checksum does not match"));
        }
        Ok(out)
    }
}

/// An archive of `entries` (name and bytes), each deflated, with a fixed date so the same entries give the same bytes.
pub fn write(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    // 1980-01-01 00:00, the earliest date ZIP can say.
    let (time, date): (u16, u16) = (0, (1 << 5) | 1);
    for (name, data) in entries {
        let packed = miniz_oxide::deflate::compress_to_vec(data, 6);
        let (crc, local) = (crc32(data), out.len() as u32);
        let mut header = Vec::new();
        header.extend_from_slice(&20u16.to_le_bytes()); // version needed: 2.0
        header.extend_from_slice(&0u16.to_le_bytes()); // flags
        header.extend_from_slice(&8u16.to_le_bytes()); // deflate
        header.extend_from_slice(&time.to_le_bytes());
        header.extend_from_slice(&date.to_le_bytes());
        header.extend_from_slice(&crc.to_le_bytes());
        header.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        header.extend_from_slice(&(data.len() as u32).to_le_bytes());
        header.extend_from_slice(&(name.len() as u16).to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes()); // extra length
        out.extend_from_slice(&LOCAL.to_le_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&packed);
        central.extend_from_slice(&CENTRAL.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&header);
        central.extend_from_slice(&[0; 8]); // comment length, disk, internal and external attributes (in part)
        central.extend_from_slice(&[0; 2]);
        central.extend_from_slice(&local.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let at = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&END.to_le_bytes());
    out.extend_from_slice(&[0; 4]); // this disk, the disk the directory starts on
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crc_is_the_standard_one() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn entries_go_out_and_come_back() {
        let big = "vertex ".repeat(10_000);
        let bytes = write(&[("3D/3dmodel.model", big.as_bytes()), ("_rels/.rels", b"<x/>")]);
        let a = Archive::new(&bytes).unwrap();
        assert_eq!(a.names().collect::<Vec<_>>(), ["3d/3dmodel.model", "_rels/.rels"]);
        assert_eq!(a.read("/3D/3DModel.model", 1 << 20).unwrap(), big.as_bytes());
        assert!(a.read("3D/3dmodel.model", 100).unwrap_err().contains("more than"));
        assert!(a.read("nothing", 100).is_err());
        assert!(bytes.len() < big.len() / 10, "deflated");
    }

    #[test]
    fn a_damaged_archive_is_an_error() {
        let bytes = write(&[("a.txt", b"hello hello hello")]);
        assert!(Archive::new(b"not a zip").is_err());
        assert!(Archive::new(&bytes[..bytes.len() - 5]).is_err());
        // A flipped byte in the packed data fails the checksum or the inflate, never passes.
        let mut bad = bytes.clone();
        bad[30 + 5 + 2] ^= 0x55;
        assert!(Archive::new(&bad).unwrap().read("a.txt", 1000).is_err());
        // Every cut of the archive is an error, not a panic.
        for n in 0..bytes.len() {
            if let Ok(a) = Archive::new(&bytes[..n]) {
                let _ = a.read("a.txt", 1000);
            }
        }
    }
}
