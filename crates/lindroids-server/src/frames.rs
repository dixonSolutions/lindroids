//! Turning composed frames into what crosses the link: only the 64x64 tiles that changed since
//! the last frame, merged into runs, LZ4-compressed where that pays and as they are (RAW) where
//! it does not.

pub const TILE: u32 = 64;

/// Changed rectangles between `prev` and `cur` (both `w * h`, row-major), looking only at the
/// tiles that meet `clip` ([x0, y0, x1, y1]). Tiles that changed and sit next to each other in a
/// tile row become one rectangle.
pub fn diff(prev: &[u32], cur: &[u32], w: u32, h: u32, clip: [u32; 4]) -> Vec<[u32; 4]> {
    let mut rects = Vec::new();
    let tx = w.div_ceil(TILE);
    let ty = h.div_ceil(TILE);
    let (i0, j0) = (clip[0] / TILE, clip[1] / TILE);
    let (i1, j1) = (clip[2].div_ceil(TILE).min(tx), clip[3].div_ceil(TILE).min(ty));
    for j in j0..j1 {
        let y0 = j * TILE;
        let th = TILE.min(h - y0);
        let mut run: Option<(u32, u32)> = None; // (x0, x1)
        for i in i0..i1 {
            let x0 = i * TILE;
            let tw = TILE.min(w - x0);
            let changed = (y0..y0 + th).any(|y| {
                let a = (y * w + x0) as usize;
                prev[a..a + tw as usize] != cur[a..a + tw as usize]
            });
            match (changed, run) {
                (true, None) => run = Some((x0, x0 + tw)),
                (true, Some((s, _))) => run = Some((s, x0 + tw)),
                (false, Some((s, e))) => {
                    rects.push([s, y0, e - s, th]);
                    run = None;
                }
                (false, None) => {}
            }
        }
        if let Some((s, e)) = run {
            rects.push([s, y0, e - s, th]);
        }
    }
    // Merge vertically adjacent rects with the same horizontal extent.
    let mut merged: Vec<[u32; 4]> = Vec::with_capacity(rects.len());
    for r in rects {
        if let Some(m) = merged.iter_mut().rev().find(|m| m[0] == r[0] && m[2] == r[2] && m[1] + m[3] == r[1]) {
            m[3] += r[3];
        } else {
            merged.push(r);
        }
    }
    merged
}

/// Bytes the pixels of `rects` take, uncompressed.
pub fn raw_len(rects: &[[u32; 4]]) -> usize {
    rects.iter().map(|r| (r[2] * r[3]) as usize * 4).sum()
}

/// The pixels of `rects` from `frame` (width `w`), row by row, one rect after another, appended
/// to `out`: a RAW frame's payload, and what LZ4 compresses.
pub fn gather(frame: &[u32], w: u32, rects: &[[u32; 4]], out: &mut Vec<u8>) {
    out.reserve(raw_len(rects));
    for r in rects {
        for y in r[1]..r[1] + r[3] {
            let a = (y * w + r[0]) as usize;
            out.extend_from_slice(&as_le_bytes(&frame[a..a + r[2] as usize]));
        }
    }
}

/// How frames travel: `LINDROIDS_FRAMES=lz4` or `raw` forces one (to compare them on a device);
/// otherwise each frame's content decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Lz4,
    Raw,
}

pub fn mode() -> Mode {
    static MODE: std::sync::OnceLock<Mode> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("LINDROIDS_FRAMES").as_deref() {
        Ok("lz4") => Mode::Lz4,
        Ok("raw") => Mode::Raw,
        _ => Mode::Auto,
    })
}

/// Below this many bytes a frame is always LZ4: compressing it costs well under a millisecond.
const ALWAYS_LZ4: usize = 128 << 10;
/// LZ4 pays when the sample shrinks at least this much (the whole frame then shrinks about a
/// third more); below it, compressing here and decoding in JavaScript cost more than the bytes
/// saved on a loopback socket (dense text: 3.1x for 7 ms + 9 ms, against 0.6 ms for a copy).
const LZ4_PAYS: f32 = 4.0;
/// The sample a frame's compressibility is guessed from: this many slices of this many bytes,
/// evenly spread over the frame.
const SAMPLES: usize = 8;
const SAMPLE: usize = 8 << 10;

/// Whether the pixels of `rects` go RAW rather than LZ4: a large frame whose sample compresses
/// less than [`LZ4_PAYS`]. The sample (64 KiB in slices, about 1/60 of a full 1280x800 frame)
/// compresses each slice on its own, so it underestimates a little: flat content (24x) stays far
/// above the line, dense text and photos (3x and less) fall below it.
pub fn worth_raw(frame: &[u32], w: u32, rects: &[[u32; 4]], slice: &mut Vec<u8>, scratch: &mut Vec<u8>) -> bool {
    raw_len(rects) >= ALWAYS_LZ4 && sample_ratio(frame, w, rects, slice, scratch) < LZ4_PAYS
}

/// How much LZ4 shrinks a sample of the pixels of `rects` (see [`worth_raw`]).
pub fn sample_ratio(frame: &[u32], w: u32, rects: &[[u32; 4]], slice: &mut Vec<u8>, scratch: &mut Vec<u8>) -> f32 {
    // Rows of the payload, as (rect, row) in payload order, sampled without gathering it all.
    let rows: usize = rects.iter().map(|r| r[3] as usize).sum();
    let mut input = 0usize;
    let mut output = 0usize;
    for k in 0..SAMPLES {
        // The row at k/SAMPLES of the payload, and as many rows after it as make a slice.
        let mut row = rows * k / SAMPLES;
        slice.clear();
        'fill: for r in rects {
            let h = r[3] as usize;
            if row >= h {
                row -= h;
                continue;
            }
            for y in r[1] as usize + row..(r[1] + r[3]) as usize {
                let a = y * w as usize + r[0] as usize;
                let px = &frame[a..a + r[2] as usize];
                let room = (SAMPLE - slice.len()) / 4;
                slice.extend_from_slice(&as_le_bytes(&px[..px.len().min(room)]));
                if slice.len() + 4 > SAMPLE {
                    break 'fill;
                }
            }
            row = 0;
        }
        scratch.resize(lz4_flex::block::get_maximum_output_size(SAMPLE), 0);
        input += slice.len();
        output += lz4_flex::block::compress_into(slice, scratch).unwrap_or(slice.len());
    }
    input as f32 / output.max(1) as f32
}

/// Buffers a window keeps for packing its frames, so a frame allocates nothing.
#[derive(Default)]
pub struct Packer {
    raw: Vec<u8>,
    lz4: Vec<u8>,
    /// The sample [`worth_raw`] compresses, and where it compresses it to.
    sample: (Vec<u8>, Vec<u8>),
}

impl Packer {
    /// Pixels of `rects` from `frame` (width `w`), concatenated, LZ4-compressed with the size in
    /// front (lz4_flex's size-prepended block): an LZ4 frame's payload.
    pub fn lz4(&mut self, frame: &[u32], w: u32, rects: &[[u32; 4]]) -> &[u8] {
        self.raw.clear();
        gather(frame, w, rects, &mut self.raw);
        let max = 4 + lz4_flex::block::get_maximum_output_size(self.raw.len());
        // Only grown, never cleared: compress_into writes what it returns, nothing reads the rest.
        if self.lz4.len() < max {
            self.lz4.resize(max, 0);
        }
        self.lz4[..4].copy_from_slice(&(self.raw.len() as u32).to_le_bytes());
        let n = lz4_flex::block::compress_into(&self.raw, &mut self.lz4[4..]).expect("output sized by get_maximum_output_size");
        &self.lz4[..4 + n]
    }

    /// Whether this frame goes RAW (see [`worth_raw`]).
    pub fn raw(&mut self, frame: &[u32], w: u32, rects: &[[u32; 4]]) -> bool {
        worth_raw(frame, w, rects, &mut self.sample.0, &mut self.sample.1)
    }
}

/// Pixels of `rects` from `frame` (width `w`), concatenated, LZ4-compressed (for tests and tools;
/// the server packs with a [`Packer`]).
pub fn pack(frame: &[u32], w: u32, rects: &[[u32; 4]]) -> Vec<u8> {
    Packer::default().lz4(frame, w, rects).to_vec()
}

/// Pixels as the bytes that cross the link (each u32 little-endian): on a little-endian machine
/// (every Android ABI) the memory as it is, a row at a time instead of a pixel at a time.
#[cfg(target_endian = "little")]
fn as_le_bytes(px: &[u32]) -> &[u8] {
    // SAFETY: u32 has no padding and any byte is a valid u8; the length is in bytes.
    unsafe { std::slice::from_raw_parts(px.as_ptr() as *const u8, std::mem::size_of_val(px)) }
}

#[cfg(target_endian = "big")]
fn as_le_bytes(px: &[u32]) -> std::borrow::Cow<'_, [u8]> {
    px.iter().flat_map(|p| p.to_le_bytes()).collect::<Vec<u8>>().into()
}

/// The receiving side: apply a frame's payload to `frame`: LZ4 (size-prepended), or the pixels
/// as they are when `raw`.
pub fn unpack(frame: &mut [u32], w: u32, rects: &[[u32; 4]], payload: &[u8], raw: bool) -> Result<(), String> {
    let decoded;
    let px = if raw {
        payload
    } else {
        decoded = lz4_flex::decompress_size_prepended(payload).map_err(|e| e.to_string())?;
        &decoded[..]
    };
    let mut off = 0usize;
    for r in rects {
        for y in r[1]..r[1] + r[3] {
            let a = (y * w + r[0]) as usize;
            let n = r[2] as usize;
            if off + n * 4 > px.len() || a + n > frame.len() {
                return Err("tile data is short".into());
            }
            for (k, p) in frame[a..a + n].iter_mut().enumerate() {
                let b = &px[off + k * 4..off + k * 4 + 4];
                *p = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            }
            off += n * 4;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_changed_tiles_travel() {
        let (w, h) = (300u32, 200u32);
        let prev = vec![0u32; (w * h) as usize];
        let mut cur = prev.clone();
        cur[(150 * w + 10) as usize] = 7; // tile (0, 2)
        cur[(150 * w + 70) as usize] = 9; // tile (1, 2): same run
        let rects = diff(&prev, &cur, w, h, [0, 0, w, h]);
        assert_eq!(rects, vec![[0, 128, 128, 64]]);
        let payload = pack(&cur, w, &rects);
        let mut out = prev.clone();
        unpack(&mut out, w, &rects, &payload, false).unwrap();
        assert_eq!(out, cur);
    }

    #[test]
    fn identical_frames_send_nothing() {
        let f = vec![5u32; 100 * 100];
        assert!(diff(&f, &f, 100, 100, [0, 0, 100, 100]).is_empty());
    }

    /// A window of flat UI: a background, a bar, a few panels and buttons.
    pub(crate) fn flat_ui(w: u32, h: u32) -> Vec<u32> {
        let mut f = vec![0xfff6_f5f4u32; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let p = &mut f[(y * w + x) as usize];
                if y < 48 {
                    *p = 0xffeb_e8e6;
                } else if x < 240 {
                    *p = 0xfffa_fafa;
                } else if (y / 40) % 4 == 1 && (x / 120) % 3 == 0 {
                    *p = 0xffd9_6a35;
                }
            }
        }
        f
    }

    /// A window of dense text: 8x16 cells of glyph-like strokes with antialiased edges, a few
    /// colours (a code editor full of code).
    pub(crate) fn dense_text(w: u32, h: u32) -> Vec<u32> {
        let mut f = vec![0xff1e_1e1eu32; (w * h) as usize];
        let ramp = [0xff1e_1e1e, 0xff4a_4a4a, 0xff80_8080, 0xffb5_b5b5, 0xffd4_d4d4];
        let tint = [0xffff_ffff, 0xff9c_dcfe, 0xffce_9178, 0xff6a_9955];
        let mut seed = 0x2545_f491u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for cy in 0..h / 16 {
            let len = w / 8 - next() % 24;
            let colour = tint[(next() % 4) as usize];
            for cx in 0..len {
                let g = next();
                if g % 9 == 0 {
                    continue; // a space
                }
                for y in 3..14 {
                    let row = next();
                    for x in 1..7 {
                        // Strokes, and grey edges where antialiasing and hinting put them.
                        let bit = (g >> ((y * 3 + x) % 32)) & 1;
                        let level = if bit == 1 { 4 } else { ((row >> (x * 4)) & 3) as usize };
                        // Subpixel antialiasing: each edge pixel a slightly different colour.
                        let fringe = if bit == 1 { 0 } else { (row >> (x * 3 + 8)) & 0x0f0f };
                        f[((cy * 16 + y) * w + cx * 8 + x) as usize] = (ramp[level] & colour) ^ fringe | 0xff00_0000;
                    }
                }
            }
        }
        f
    }

    #[test]
    fn raw_frames_round_trip() {
        let (w, h) = (300u32, 200u32);
        let cur = dense_text(w, h);
        let rects = vec![[0, 0, w, h], [64, 64, 64, 64]];
        let mut payload = Vec::new();
        gather(&cur, w, &rects, &mut payload);
        assert_eq!(payload.len(), raw_len(&rects));
        let mut out = vec![0u32; (w * h) as usize];
        unpack(&mut out, w, &rects, &payload, true).unwrap();
        assert_eq!(out, cur);
        assert!(unpack(&mut out, w, &rects, &payload[..100], true).is_err());
    }

    #[test]
    fn flat_frames_go_lz4_and_dense_ones_raw() {
        let (w, h) = (1280u32, 800u32);
        let all = [[0, 0, w, h]];
        let mut p = Packer::default();
        assert!(!p.raw(&flat_ui(w, h), w, &all), "flat UI compresses well");
        assert!(p.raw(&dense_text(w, h), w, &all), "dense text does not compress enough to pay");
        // A small update is LZ4 whatever it holds.
        assert!(!p.raw(&dense_text(w, h), w, &[[0, 0, 64, 64]]));
        // Packing again into the same buffers gives what a fresh packer gives.
        let a = p.lz4(&dense_text(w, h), w, &all).to_vec();
        let b = p.lz4(&flat_ui(w, h), w, &all).to_vec();
        assert_eq!(b, pack(&flat_ui(w, h), w, &all));
        assert_eq!(a, pack(&dense_text(w, h), w, &all));
    }

    /// Where a full 1280x800 frame's time goes, LZ4 against RAW (run with
    /// `cargo test --release -p lindroids-server frames::tests::bench -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn bench() {
        use std::time::Instant;
        let (w, h) = (1280u32, 800u32);
        let all = [[0, 0, w, h]];
        for (name, f) in [("flat UI", flat_ui(w, h)), ("dense text", dense_text(w, h))] {
            let mut p = Packer::default();
            let n = 50;
            let t = Instant::now();
            let mut len = 0;
            for _ in 0..n {
                len = p.lz4(&f, w, &all).len();
            }
            let lz4 = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
            let t = Instant::now();
            let mut raw = false;
            for _ in 0..n {
                raw = p.raw(&f, w, &all);
            }
            let est = sample_ratio(&f, w, &all, &mut Vec::new(), &mut Vec::new());
            let probe = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
            let mut out = Vec::new();
            let t = Instant::now();
            for _ in 0..n {
                out.clear();
                gather(&f, w, &all, &mut out);
            }
            let copy = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
            let t = Instant::now();
            for _ in 0..n {
                let _ = pack(&f, w, &all);
            }
            let old = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
            println!(
                "{name}: LZ4 {:.1}x, pack {lz4:.2} ms (allocating each frame: {old:.2} ms); sample {probe:.3} ms guesses {est:.1}x: {}; RAW copy {copy:.2} ms",
                raw_len(&all) as f64 / len as f64,
                if raw { "RAW" } else { "LZ4" }
            );
            std::fs::write(std::env::temp_dir().join(format!("lindroids-bench-{}.lz4", name.replace(' ', "-"))), p.lz4(&f, w, &all)).ok();
        }
    }
}
