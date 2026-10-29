//! The ZIP side of an APK: read the template's entries, write them back with some replaced.
//! Entries are copied with their compressed bytes as they were; new ones are stored uncompressed.
//! Stored entries start on a 4-byte boundary (what `zipalign -p 4` does): Android 11 and later
//! refuse an app whose `resources.arsc` is compressed or unaligned.

pub struct Entry {
    pub name: String,
    pub method: u16,
    pub crc: u32,
    pub size: u32,
    /// The bytes as they are in the archive (compressed when `method` is 8).
    pub raw: Vec<u8>,
}

impl Entry {
    pub fn stored(name: &str, data: Vec<u8>) -> Entry {
        Entry { name: name.to_string(), method: 0, crc: crc32fast::hash(&data), size: data.len() as u32, raw: data }
    }

    /// The entry's contents, inflated when needed.
    pub fn data(&self) -> Result<Vec<u8>, String> {
        match self.method {
            0 => Ok(self.raw.clone()),
            8 => {
                use std::io::Read;
                let mut out = Vec::with_capacity(self.size as usize);
                flate2::read::DeflateDecoder::new(&self.raw[..])
                    .read_to_end(&mut out)
                    .map_err(|e| format!("{}: {e}", self.name))?;
                Ok(out)
            }
            m => Err(format!("{}: compression method {m} is not supported", self.name)),
        }
    }
}

fn u16_at(b: &[u8], o: usize) -> Result<u16, String> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(|| "zip: truncated".to_string())
}

fn u32_at(b: &[u8], o: usize) -> Result<u32, String> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or_else(|| "zip: truncated".to_string())
}

/// The offset of the End of Central Directory record.
pub fn eocd(b: &[u8]) -> Result<usize, String> {
    if b.len() < 22 {
        return Err("zip: too short".into());
    }
    let lowest = b.len().saturating_sub(22 + 0xffff);
    (lowest..=b.len() - 22)
        .rev()
        .find(|&o| b[o..o + 4] == [0x50, 0x4b, 0x05, 0x06] && o + 22 + u16_at(b, o + 20).unwrap_or(0) as usize == b.len())
        .ok_or_else(|| "zip: no end of central directory".into())
}

pub fn read(b: &[u8]) -> Result<Vec<Entry>, String> {
    let e = eocd(b)?;
    let count = u16_at(b, e + 10)? as usize;
    let mut o = u32_at(b, e + 16)? as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if u32_at(b, o)? != 0x0201_4b50 {
            return Err("zip: bad central directory".into());
        }
        let method = u16_at(b, o + 10)?;
        let crc = u32_at(b, o + 16)?;
        let csize = u32_at(b, o + 20)? as usize;
        let size = u32_at(b, o + 24)?;
        let nlen = u16_at(b, o + 28)? as usize;
        let xlen = u16_at(b, o + 30)? as usize;
        let clen = u16_at(b, o + 32)? as usize;
        let local = u32_at(b, o + 42)? as usize;
        let name = String::from_utf8(b.get(o + 46..o + 46 + nlen).ok_or("zip: truncated")?.to_vec()).map_err(|_| "zip: a name is not UTF-8")?;
        if u32_at(b, local)? != 0x0403_4b50 {
            return Err(format!("zip: {name}: bad local header"));
        }
        let start = local + 30 + u16_at(b, local + 26)? as usize + u16_at(b, local + 28)? as usize;
        let raw = b.get(start..start + csize).ok_or("zip: truncated")?.to_vec();
        out.push(Entry { name, method, crc, size, raw });
        o += 46 + nlen + xlen + clen;
    }
    Ok(out)
}

/// Writes the entries as a ZIP archive, stored entries aligned to 4 bytes. Fixed timestamps: the
/// same program builds the same bytes.
pub fn write(entries: &[Entry]) -> Vec<u8> {
    const TIME: u16 = 0;
    const DATE: u16 = (2026 - 1980) << 9 | 1 << 5 | 1;
    let mut out = Vec::new();
    let mut central = Vec::new();
    for e in entries {
        let offset = out.len() as u32;
        let name = e.name.as_bytes();
        let mut pad = 0usize;
        if e.method == 0 {
            let data_at = out.len() + 30 + name.len();
            pad = (4 - data_at % 4) % 4;
        }
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&e.method.to_le_bytes());
        out.extend_from_slice(&TIME.to_le_bytes());
        out.extend_from_slice(&DATE.to_le_bytes());
        out.extend_from_slice(&e.crc.to_le_bytes());
        out.extend_from_slice(&(e.raw.len() as u32).to_le_bytes());
        out.extend_from_slice(&e.size.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&(pad as u16).to_le_bytes());
        out.extend_from_slice(name);
        out.extend(std::iter::repeat(0u8).take(pad));
        out.extend_from_slice(&e.raw);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&e.method.to_le_bytes());
        central.extend_from_slice(&TIME.to_le_bytes());
        central.extend_from_slice(&DATE.to_le_bytes());
        central.extend_from_slice(&e.crc.to_le_bytes());
        central.extend_from_slice(&(e.raw.len() as u32).to_le_bytes());
        central.extend_from_slice(&e.size.to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 12]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }
    let cd_at = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}
