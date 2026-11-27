//! Minimal zip reader: central directory + stored/deflate entries. No zip64, no encryption.
use anyhow::{Result, bail, ensure};
use std::io::{Read, Seek, SeekFrom};

const MAX_CD_SIZE: u64 = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 65_534;
const MAX_NAME_LEN: usize = 4096;

pub struct Entry {
    pub name: String,
    pub method: u16,
    /// Offset of the entry's local file header.
    pub offset: u64,
    pub csize: u64,
    pub size: u64,
    flags: u16,
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// Entries from the central directory (sizes come from there, so bit-3 entries work).
pub fn entries<R: Read + Seek>(r: &mut R) -> Result<Vec<Entry>> {
    let len = r.seek(SeekFrom::End(0))?;
    if len < 22 {
        bail!("not a zip file");
    }
    let tail = len.min(22 + 65535);
    r.seek(SeekFrom::Start(len - tail))?;
    let mut buf = vec![];
    r.take(tail).read_to_end(&mut buf)?;
    ensure!(buf.len() as u64 == tail, "truncated zip");
    let e = (0..=buf.len().saturating_sub(22))
        .rev()
        .find(|&i| {
            buf[i..i + 4] == *b"PK\x05\x06" && i + 22 + u16_at(&buf, i + 20) as usize == buf.len()
        })
        .ok_or_else(|| anyhow::anyhow!("not a zip file"))?;
    let (n, size, off) = (
        u16_at(&buf, e + 10) as usize,
        u32_at(&buf, e + 12) as u64,
        u32_at(&buf, e + 16) as u64,
    );
    ensure!(
        n != 0xffff && size != 0xffff_ffff && off != 0xffff_ffff,
        "zip64 is not supported"
    );
    ensure!(n <= MAX_ENTRIES, "zip has too many entries ({n})");
    ensure!(
        size <= MAX_CD_SIZE,
        "zip central directory too large ({size} bytes)"
    );
    ensure!(
        off.checked_add(size).is_some_and(|end| end <= len),
        "bad central directory"
    );
    ensure!(
        (n as u64).saturating_mul(46) <= size,
        "bad central directory entry count"
    );
    r.seek(SeekFrom::Start(off))?;
    let mut cd = Vec::with_capacity(size.min(1 << 20) as usize);
    r.take(size).read_to_end(&mut cd)?;
    ensure!(cd.len() as u64 == size, "truncated central directory");
    let (mut out, mut i) = (Vec::with_capacity(n.min(4096)), 0usize);
    for _ in 0..n {
        ensure!(
            i.checked_add(46).is_some_and(|end| end <= cd.len()) && cd[i..i + 4] == *b"PK\x01\x02",
            "bad central directory entry"
        );
        let (nlen, elen, clen) = (
            u16_at(&cd, i + 28) as usize,
            u16_at(&cd, i + 30) as usize,
            u16_at(&cd, i + 32) as usize,
        );
        ensure!(
            nlen <= MAX_NAME_LEN,
            "zip entry name too long ({nlen} bytes)"
        );
        let name_end = i
            .checked_add(46)
            .and_then(|x| x.checked_add(nlen))
            .ok_or_else(|| anyhow::anyhow!("bad central directory entry"))?;
        let next_i = name_end
            .checked_add(elen)
            .and_then(|x| x.checked_add(clen))
            .ok_or_else(|| anyhow::anyhow!("bad central directory entry"))?;
        ensure!(next_i <= cd.len(), "bad central directory entry");
        let (csize, usize_, offset) = (
            u32_at(&cd, i + 20),
            u32_at(&cd, i + 24),
            u32_at(&cd, i + 42),
        );
        ensure!(
            csize != 0xffff_ffff && usize_ != 0xffff_ffff && offset != 0xffff_ffff,
            "zip64 is not supported"
        );
        let offset_u64 = offset as u64;
        let csize_u64 = csize as u64;
        ensure!(
            offset_u64
                .checked_add(30)
                .and_then(|x| x.checked_add(csize_u64))
                .is_some_and(|end| end <= len),
            "zip entry offset or size past EOF"
        );
        out.push(Entry {
            name: String::from_utf8_lossy(&cd[i + 46..name_end]).into_owned(),
            method: u16_at(&cd, i + 10),
            flags: u16_at(&cd, i + 8),
            offset: offset_u64,
            csize: csize_u64,
            size: usize_ as u64,
        });
        i = next_i;
    }
    Ok(out)
}

/// Uncompressed bytes of `e`; an entry larger than `limit` is an error.
pub fn read<R: Read + Seek>(r: &mut R, e: &Entry, limit: usize) -> Result<Vec<u8>> {
    ensure!(e.flags & 1 == 0, "encrypted entry {}", e.name);
    ensure!(
        e.size <= limit as u64,
        "{} is larger than {limit} bytes",
        e.name
    );
    match e.method {
        0 => {
            ensure!(
                e.csize == e.size,
                "corrupt stored entry {} (compressed {} != uncompressed {})",
                e.name,
                e.csize,
                e.size
            );
            ensure!(
                e.csize <= limit as u64,
                "{} is larger than {limit} bytes",
                e.name
            );
        }
        8 => {
            // Deflate stored blocks add ~5 bytes per 64 KiB; bound csize before buffering.
            let max_csize = (limit as u64)
                .saturating_add((limit as u64) / 4)
                .saturating_add(4096);
            ensure!(
                e.csize <= max_csize,
                "{} compressed size {} exceeds limit for {limit} bytes",
                e.name,
                e.csize
            );
        }
        m => bail!("unsupported compression method {m} for {}", e.name),
    }
    let len = r.seek(SeekFrom::End(0))?;
    ensure!(
        e.offset.checked_add(30).is_some_and(|end| end <= len),
        "local header past EOF for {}",
        e.name
    );
    r.seek(SeekFrom::Start(e.offset))?;
    let mut h = [0u8; 30];
    r.read_exact(&mut h)?;
    ensure!(h[..4] == *b"PK\x03\x04", "bad local header for {}", e.name);
    // The local header's name/extra lengths may differ from the central directory's.
    let start = e
        .offset
        .checked_add(30)
        .and_then(|x| x.checked_add(u16_at(&h, 26) as u64))
        .and_then(|x| x.checked_add(u16_at(&h, 28) as u64))
        .ok_or_else(|| anyhow::anyhow!("bad local header offset for {}", e.name))?;
    ensure!(
        start.checked_add(e.csize).is_some_and(|end| end <= len),
        "entry {} extends past EOF",
        e.name
    );
    r.seek(SeekFrom::Start(start))?;
    let mut data = Vec::with_capacity((e.csize as usize).min(1 << 20));
    r.take(e.csize).read_to_end(&mut data)?;
    ensure!(data.len() as u64 == e.csize, "truncated entry {}", e.name);
    match e.method {
        0 => {
            ensure!(
                data.len() <= limit && data.len() as u64 == e.size,
                "size mismatch for {}",
                e.name
            );
            Ok(data)
        }
        8 => {
            let out = miniz_oxide::inflate::decompress_to_vec_with_limit(&data, limit)
                .map_err(|x| anyhow::anyhow!("cannot inflate {}: {:?}", e.name, x.status))?;
            ensure!(
                out.len() as u64 == e.size,
                "decoded size mismatch for {} (expected {}, got {})",
                e.name,
                e.size,
                out.len()
            );
            Ok(out)
        }
        m => bail!("unsupported compression method {m} for {}", e.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// (name, data, deflate, lfh extra bytes, bit 3 with zeroed lfh sizes)
    fn build(files: &[(&str, &[u8], bool, usize, bool)]) -> Vec<u8> {
        let (mut out, mut cd) = (vec![], vec![]);
        for (name, data, deflate, extra, bit3) in files {
            let packed = if *deflate {
                miniz_oxide::deflate::compress_to_vec(data, 6)
            } else {
                data.to_vec()
            };
            let flags: u16 = if *bit3 { 8 } else { 0 };
            let method: u16 = if *deflate { 8 } else { 0 };
            let off = out.len() as u32;
            let (lc, lu) = if *bit3 {
                (0, 0)
            } else {
                (packed.len() as u32, data.len() as u32)
            };
            out.extend(b"PK\x03\x04");
            for v in [20u16, flags, method, 0, 0] {
                out.extend(v.to_le_bytes());
            }
            for v in [0u32, lc, lu] {
                out.extend(v.to_le_bytes());
            }
            out.extend((name.len() as u16).to_le_bytes());
            out.extend((*extra as u16).to_le_bytes());
            out.extend(name.as_bytes());
            out.extend(vec![0xAA; *extra]);
            out.extend(&packed);
            if *bit3 {
                out.extend(b"PK\x07\x08");
                for v in [0u32, packed.len() as u32, data.len() as u32] {
                    out.extend(v.to_le_bytes());
                }
            }
            cd.extend(b"PK\x01\x02");
            for v in [20u16, 20, flags, method, 0, 0] {
                cd.extend(v.to_le_bytes());
            }
            for v in [0u32, packed.len() as u32, data.len() as u32] {
                cd.extend(v.to_le_bytes());
            }
            cd.extend((name.len() as u16).to_le_bytes());
            // central extra differs from the local one
            cd.extend([0u8; 2 + 2 + 2 + 2]);
            cd.extend(0u32.to_le_bytes());
            cd.extend(off.to_le_bytes());
            cd.extend(name.as_bytes());
        }
        let cd_off = out.len() as u32;
        out.extend(&cd);
        out.extend(b"PK\x05\x06");
        out.extend([0u8; 4]);
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((cd.len() as u32).to_le_bytes());
        out.extend(cd_off.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out
    }

    #[test]
    fn hand_built_zip() {
        let big = "abc".repeat(1000);
        let z = build(&[
            ("a.txt", b"hello", false, 0, false),
            ("dir/b.txt", big.as_bytes(), true, 7, false),
            ("c.bin", b"streamed", true, 3, true),
        ]);
        let mut c = Cursor::new(z);
        let es = entries(&mut c).unwrap();
        let names: Vec<_> = es.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a.txt", "dir/b.txt", "c.bin"]);
        assert_eq!(read(&mut c, &es[0], 100).unwrap(), b"hello");
        assert_eq!(read(&mut c, &es[1], 10_000).unwrap(), big.as_bytes());
        assert_eq!(read(&mut c, &es[2], 100).unwrap(), b"streamed");
        assert!(read(&mut c, &es[1], 100).is_err(), "limit enforced");
        assert!(entries(&mut Cursor::new(b"not a zip at all".to_vec())).is_err());
        assert!(entries(&mut Cursor::new(vec![])).is_err());
    }

    #[test]
    fn malicious_zip_bounds_and_limits() {
        // 1. Stored entry where csize != size and exceeds caller limit
        let mut z = build(&[("a.txt", b"hello world 1234567890", false, 0, false)]);
        let mut c = Cursor::new(z.clone());
        let mut e = entries(&mut c).unwrap().remove(0);
        e.size = 5; // forged smaller uncompressed size while csize is 22
        assert!(
            read(&mut c, &e, 10).is_err(),
            "stored csize != size rejected"
        );
        assert!(
            read(&mut c, &e, 100).is_err(),
            "stored mismatch rejected even under limit"
        );

        // 2. Deflate entry with huge forged csize rejected before buffering
        let z_def = build(&[("b.txt", b"hello", true, 0, false)]);
        let mut c_def = Cursor::new(z_def.clone());
        let mut e_def = entries(&mut c_def).unwrap().remove(0);
        e_def.csize = 500_000_000;
        assert!(read(&mut c_def, &e_def, 1024).is_err());

        // 3. Deflate entry whose decoded size disagrees with central directory e.size
        let mut c_mis = Cursor::new(z_def);
        let mut e_mis = entries(&mut c_mis).unwrap().remove(0);
        e_mis.size = 999; // actual decoded is 5
        assert!(read(&mut c_mis, &e_mis, 2000).is_err());

        // 4. Offset past EOF rejected in both entries() and read()
        let cd_pos = z.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
        z[cd_pos + 42..cd_pos + 46].copy_from_slice(&(99_999u32).to_le_bytes());
        assert!(entries(&mut Cursor::new(z)).is_err());

        // 5. Central directory size past EOF / over cap rejected
        let mut bad_cd = build(&[("a.txt", b"hi", false, 0, false)]);
        let eocd = bad_cd.windows(4).rposition(|w| w == b"PK\x05\x06").unwrap();
        bad_cd[eocd + 12..eocd + 16].copy_from_slice(&(100_000_000u32).to_le_bytes());
        assert!(entries(&mut Cursor::new(bad_cd)).is_err());

        // 6. Entry count claiming more entries than central directory bytes allow
        let mut bad_n = build(&[("a.txt", b"hi", false, 0, false)]);
        let eocd = bad_n.windows(4).rposition(|w| w == b"PK\x05\x06").unwrap();
        bad_n[eocd + 10..eocd + 12].copy_from_slice(&(500u16).to_le_bytes());
        assert!(entries(&mut Cursor::new(bad_n)).is_err());

        // 7. Absurdly long entry name (> MAX_NAME_LEN) rejected
        let long_name = "a".repeat(MAX_NAME_LEN + 10);
        let z_long = build(&[(&long_name, b"x", false, 0, false)]);
        assert!(entries(&mut Cursor::new(z_long)).is_err());
    }

    /// Read-only: skipped by default so tests never touch the real Steam directory.
    #[test]
    #[ignore = "reads real Steam install; run with --ignored"]
    fn tutorial_sav_meta() {
        let home = dirs::home_dir().expect("home dir");
        let p = home.join(
            "Library/Application Support/Steam/steamapps/common/Stellaris/curated_save_games/tutorial.sav",
        );
        let f = std::fs::File::open(&p).expect("tutorial.sav present");
        let mut f = std::io::BufReader::new(f);
        let es = entries(&mut f).unwrap();
        let meta = es.iter().find(|e| e.name == "meta").expect("meta entry");
        let text = String::from_utf8(read(&mut f, meta, 65536).unwrap()).unwrap();
        assert!(text.contains("version"), "{text}");
    }
}
