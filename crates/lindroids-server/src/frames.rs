//! Turning composed frames into what crosses the link: only the 64x64 tiles that changed since
//! the last frame, merged into runs, LZ4-compressed.

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

/// Pixels of `rects` from `frame` (width `w`), concatenated, LZ4-compressed.
pub fn pack(frame: &[u32], w: u32, rects: &[[u32; 4]]) -> Vec<u8> {
    let total: usize = rects.iter().map(|r| (r[2] * r[3]) as usize * 4).sum();
    let mut raw = Vec::with_capacity(total);
    for r in rects {
        for y in r[1]..r[1] + r[3] {
            let a = (y * w + r[0]) as usize;
            raw.extend_from_slice(&as_le_bytes(&frame[a..a + r[2] as usize]));
        }
    }
    lz4_flex::compress_prepend_size(&raw)
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

/// The receiving side: apply a packed payload to `frame`.
pub fn unpack(frame: &mut [u32], w: u32, rects: &[[u32; 4]], payload: &[u8]) -> Result<(), String> {
    let raw = lz4_flex::decompress_size_prepended(payload).map_err(|e| e.to_string())?;
    let mut off = 0usize;
    for r in rects {
        for y in r[1]..r[1] + r[3] {
            let a = (y * w + r[0]) as usize;
            let n = r[2] as usize;
            if off + n * 4 > raw.len() || a + n > frame.len() {
                return Err("tile data is short".into());
            }
            for (k, p) in frame[a..a + n].iter_mut().enumerate() {
                let b = &raw[off + k * 4..off + k * 4 + 4];
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
        unpack(&mut out, w, &rects, &payload).unwrap();
        assert_eq!(out, cur);
    }

    #[test]
    fn identical_frames_send_nothing() {
        let f = vec![5u32; 100 * 100];
        assert!(diff(&f, &f, 100, 100, [0, 0, 100, 100]).is_empty());
    }
}
