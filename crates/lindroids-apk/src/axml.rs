//! Android's binary XML (the compiled `AndroidManifest.xml`): just enough to change the strings
//! and integer attributes of a manifest aapt2 made. The string pool is decoded and written again
//! (so a string may grow or shrink); every other chunk is copied byte for byte, since they refer
//! to strings by index and the indices do not change.

const RES_XML_TYPE: u16 = 0x0003;
const RES_STRING_POOL_TYPE: u16 = 0x0001;
const RES_XML_START_ELEMENT_TYPE: u16 = 0x0102;
const UTF8_FLAG: u32 = 1 << 8;
const TYPE_STRING: u8 = 0x03;
const TYPE_INT_DEC: u8 = 0x10;

fn u16_at(b: &[u8], o: usize) -> Result<u16, String> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(|| "binary XML: truncated".to_string())
}

fn u32_at(b: &[u8], o: usize) -> Result<u32, String> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or_else(|| "binary XML: truncated".to_string())
}

pub struct Manifest {
    strings: Vec<String>,
    utf8: bool,
    /// The pool's flags other than UTF-8 (SORTED is dropped: the strings change).
    styles: Vec<u8>,
    style_count: u32,
    /// The chunks after the string pool, as they were.
    rest: Vec<u8>,
}

impl Manifest {
    pub fn parse(b: &[u8]) -> Result<Manifest, String> {
        if u16_at(b, 0)? != RES_XML_TYPE {
            return Err("not a binary XML file".into());
        }
        let first = u16_at(b, 2)? as usize;
        let total = (u32_at(b, 4)? as usize).min(b.len());
        if u16_at(b, first)? != RES_STRING_POOL_TYPE {
            return Err("binary XML: the string pool is not the first chunk".into());
        }
        let p = first;
        let header = u16_at(b, p + 2)? as usize;
        let size = u32_at(b, p + 4)? as usize;
        let count = u32_at(b, p + 8)? as usize;
        let style_count = u32_at(b, p + 12)?;
        let flags = u32_at(b, p + 16)?;
        let strings_start = u32_at(b, p + 20)? as usize;
        let styles_start = u32_at(b, p + 24)? as usize;
        let utf8 = flags & UTF8_FLAG != 0;
        let mut strings = Vec::with_capacity(count);
        for i in 0..count {
            let off = p + strings_start + u32_at(b, p + header + 4 * i)? as usize;
            strings.push(if utf8 { read_utf8(b, off)? } else { read_utf16(b, off)? });
        }
        let styles = if style_count > 0 {
            // Style offsets and spans, kept as they are (aapt2 writes none for a manifest).
            let offs = &b[p + header + 4 * count..p + header + 4 * count + 4 * style_count as usize];
            let mut s = offs.to_vec();
            s.extend_from_slice(&b[p + styles_start..p + size]);
            s
        } else {
            Vec::new()
        };
        Ok(Manifest { strings, utf8, styles, style_count, rest: b[p + size..total].to_vec() })
    }

    /// Replaces every string equal to `from`. Returns how many were replaced.
    pub fn replace(&mut self, from: &str, to: &str) -> usize {
        let mut n = 0;
        for s in &mut self.strings {
            if s == from {
                *s = to.to_string();
                n += 1;
            }
        }
        n
    }

    fn index_of(&self, s: &str) -> Option<u32> {
        self.strings.iter().position(|x| x == s).map(|i| i as u32)
    }

    /// Visits every attribute named `name` (any namespace) of every element: (offset of the
    /// attribute in `rest`).
    fn attributes(&self, name: &str) -> Result<Vec<usize>, String> {
        let Some(want) = self.index_of(name) else { return Ok(Vec::new()) };
        let b = &self.rest;
        let mut out = Vec::new();
        let mut o = 0;
        while o + 8 <= b.len() {
            let ty = u16_at(b, o)?;
            let header = u16_at(b, o + 2)? as usize;
            let size = u32_at(b, o + 4)? as usize;
            if size < 8 || o + size > b.len() {
                return Err("binary XML: a chunk runs past the end".into());
            }
            if ty == RES_XML_START_ELEMENT_TYPE {
                let ext = o + header;
                let start = u16_at(b, ext + 8)? as usize;
                let asize = u16_at(b, ext + 10)? as usize;
                let count = u16_at(b, ext + 12)? as usize;
                for i in 0..count {
                    let a = ext + start + i * asize;
                    if u32_at(b, a + 4)? == want {
                        out.push(a);
                    }
                }
            }
            o += size;
        }
        Ok(out)
    }

    /// Sets an integer attribute (`versionCode`) on every element that has it.
    pub fn set_int(&mut self, name: &str, value: u32) -> Result<usize, String> {
        let at = self.attributes(name)?;
        for &a in &at {
            if self.rest[a + 15] != TYPE_INT_DEC {
                return Err(format!("binary XML: {name} is not an integer"));
            }
            self.rest[a + 16..a + 20].copy_from_slice(&value.to_le_bytes());
        }
        Ok(at.len())
    }

    /// Sets a string attribute (`versionName`) by changing the string it points at.
    pub fn set_string(&mut self, name: &str, value: &str) -> Result<usize, String> {
        let at = self.attributes(name)?;
        for &a in &at {
            if self.rest[a + 15] != TYPE_STRING {
                return Err(format!("binary XML: {name} is not a string"));
            }
            let i = u32_at(&self.rest, a + 16)? as usize;
            let s = self.strings.get_mut(i).ok_or("binary XML: a string index is out of range")?;
            *s = value.to_string();
        }
        Ok(at.len())
    }

    pub fn strings(&self) -> &[String] {
        &self.strings
    }

    pub fn write(&self) -> Vec<u8> {
        let count = self.strings.len();
        let header = 28usize;
        let mut data = Vec::new();
        let mut offsets = Vec::with_capacity(count);
        for s in &self.strings {
            offsets.push(data.len() as u32);
            if self.utf8 {
                write_utf8(&mut data, s);
            } else {
                write_utf16(&mut data, s);
            }
        }
        while data.len() % 4 != 0 {
            data.push(0);
        }
        let strings_start = header + 4 * count + 4 * self.style_count as usize;
        let (style_offsets, style_data) = self.styles.split_at((4 * self.style_count as usize).min(self.styles.len()));
        let styles_start = if self.style_count > 0 { strings_start + data.len() } else { 0 };
        let pool_size = strings_start + data.len() + style_data.len();
        let mut pool = Vec::with_capacity(pool_size);
        pool.extend_from_slice(&RES_STRING_POOL_TYPE.to_le_bytes());
        pool.extend_from_slice(&(header as u16).to_le_bytes());
        pool.extend_from_slice(&(pool_size as u32).to_le_bytes());
        pool.extend_from_slice(&(count as u32).to_le_bytes());
        pool.extend_from_slice(&self.style_count.to_le_bytes());
        pool.extend_from_slice(&(if self.utf8 { UTF8_FLAG } else { 0 }).to_le_bytes());
        pool.extend_from_slice(&(strings_start as u32).to_le_bytes());
        pool.extend_from_slice(&(styles_start as u32).to_le_bytes());
        for o in offsets {
            pool.extend_from_slice(&o.to_le_bytes());
        }
        pool.extend_from_slice(style_offsets);
        pool.extend_from_slice(&data);
        pool.extend_from_slice(style_data);

        let total = 8 + pool.len() + self.rest.len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&RES_XML_TYPE.to_le_bytes());
        out.extend_from_slice(&8u16.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&pool);
        out.extend_from_slice(&self.rest);
        out
    }
}

fn read_utf16(b: &[u8], o: usize) -> Result<String, String> {
    let mut len = u16_at(b, o)? as usize;
    let mut p = o + 2;
    if len & 0x8000 != 0 {
        len = ((len & 0x7fff) << 16) | u16_at(b, p)? as usize;
        p += 2;
    }
    let units: Result<Vec<u16>, String> = (0..len).map(|i| u16_at(b, p + 2 * i)).collect();
    String::from_utf16(&units?).map_err(|_| "binary XML: a string is not UTF-16".into())
}

fn read_utf8(b: &[u8], o: usize) -> Result<String, String> {
    let mut p = o;
    let byte = |p: usize| b.get(p).copied().ok_or_else(|| "binary XML: truncated".to_string());
    // The length in UTF-16 units, then in bytes; each one or two bytes.
    let a = byte(p)?;
    p += if a & 0x80 != 0 { 2 } else { 1 };
    let l0 = byte(p)? as usize;
    let len = if l0 & 0x80 != 0 {
        let l = ((l0 & 0x7f) << 8) | byte(p + 1)? as usize;
        p += 2;
        l
    } else {
        p += 1;
        l0
    };
    let s = b.get(p..p + len).ok_or("binary XML: truncated")?;
    String::from_utf8(s.to_vec()).map_err(|_| "binary XML: a string is not UTF-8".into())
}

fn write_utf16(out: &mut Vec<u8>, s: &str) {
    let units: Vec<u16> = s.encode_utf16().collect();
    let n = units.len();
    if n > 0x7fff {
        out.extend_from_slice(&((((n >> 16) as u16) | 0x8000).to_le_bytes()));
        out.extend_from_slice(&((n & 0xffff) as u16).to_le_bytes());
    } else {
        out.extend_from_slice(&(n as u16).to_le_bytes());
    }
    for u in units {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out.extend_from_slice(&[0, 0]);
}

fn write_utf8(out: &mut Vec<u8>, s: &str) {
    let len8 = |out: &mut Vec<u8>, n: usize| {
        if n > 0x7f {
            out.push((((n >> 8) & 0x7f) | 0x80) as u8);
            out.push((n & 0xff) as u8);
        } else {
            out.push(n as u8);
        }
    };
    len8(out, s.encode_utf16().count());
    len8(out, s.len());
    out.extend_from_slice(s.as_bytes());
    out.push(0);
}
