//! CPU compositing of one Linux window (its surface tree plus popups) into one Android Surface.
//!
//! Clients hand us wl_shm buffers. On commit we copy them, converting to RGBA (the byte order an
//! Android RGBA_8888 window buffer wants) and release the client's buffer at once. Drawing a window
//! is then a row copy for the main surface and an alpha blend for menus and subsurfaces.

use std::sync::Arc;

/// Pixels of one committed surface, RGBA in memory (little-endian u32 0xAABBGGRR), premultiplied.
#[derive(Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u32>,
    /// Every pixel has alpha 255: blit without blending.
    pub opaque: bool,
}

/// One surface placed in a frame: physical position and the size it should cover.
pub struct Placed {
    pub image: Arc<Image>,
    pub x: i32,
    pub y: i32,
    /// Destination size in physical pixels (differs from the image size when scaling).
    pub w: u32,
    pub h: u32,
    /// The root surface of a toplevel draws over black, so it can be copied without blending.
    pub root: bool,
}

/// Compose `layers` (bottom to top) into `dst`.
pub fn compose(dst: &mut [u32], dw: u32, dh: u32, stride: usize, layers: &[Placed]) {
    // Clear what the root surface will not cover. Most frames the root covers everything.
    let covered = layers.first().map_or(false, |l| l.root && l.x <= 0 && l.y <= 0
        && l.x + l.w as i32 >= dw as i32 && l.y + l.h as i32 >= dh as i32);
    if !covered {
        for row in 0..dh as usize {
            dst[row * stride..row * stride + dw as usize].fill(0xff00_0000);
        }
    }
    for l in layers {
        blit(dst, dw, dh, stride, l);
    }
}

fn blit(dst: &mut [u32], dw: u32, dh: u32, stride: usize, l: &Placed) {
    let img = &l.image;
    if img.width == 0 || img.height == 0 || l.w == 0 || l.h == 0 {
        return;
    }
    let x0 = l.x.max(0);
    let y0 = l.y.max(0);
    let x1 = (l.x + l.w as i32).min(dw as i32);
    let y1 = (l.y + l.h as i32).min(dh as i32);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let scaled = l.w != img.width || l.h != img.height;
    let blend = !(l.root || img.opaque);
    // Fixed-point (16.16) source steps for nearest-neighbour scaling.
    let sx_step = ((img.width as u64) << 16) / l.w as u64;
    let sy_step = ((img.height as u64) << 16) / l.h as u64;
    for y in y0..y1 {
        let sy = if scaled {
            ((((y - l.y) as u64) * sy_step) >> 16).min(img.height as u64 - 1) as usize
        } else {
            (y - l.y) as usize
        };
        let src_row = &img.data[sy * img.width as usize..(sy + 1) * img.width as usize];
        let drow = &mut dst[y as usize * stride..y as usize * stride + dw as usize];
        if !scaled && !blend {
            let sx = (x0 - l.x) as usize;
            let n = (x1 - x0) as usize;
            drow[x0 as usize..x1 as usize].copy_from_slice(&src_row[sx..sx + n]);
            if l.root {
                for p in &mut drow[x0 as usize..x1 as usize] {
                    *p |= 0xff00_0000;
                }
            }
            continue;
        }
        // Shrinking: average 2x2 source pixels, so text stays legible instead of losing strokes.
        let shrink = l.w < img.width || l.h < img.height;
        let next_row = if shrink {
            let ny = (sy + 1).min(img.height as usize - 1);
            Some(&img.data[ny * img.width as usize..(ny + 1) * img.width as usize])
        } else {
            None
        };
        for x in x0..x1 {
            let sx = if scaled {
                ((((x - l.x) as u64) * sx_step) >> 16).min(img.width as u64 - 1) as usize
            } else {
                (x - l.x) as usize
            };
            let s = match next_row {
                Some(nr) => {
                    let sx1 = (sx + 1).min(img.width as usize - 1);
                    avg4(src_row[sx], src_row[sx1], nr[sx], nr[sx1])
                }
                None => src_row[sx],
            };
            let d = &mut drow[x as usize];
            if !blend {
                *d = s | 0xff00_0000;
            } else {
                *d = over(s, *d);
            }
        }
    }
}

/// Average of four pixels, per channel.
#[inline]
fn avg4(a: u32, b: u32, c: u32, d: u32) -> u32 {
    let lo = |p: u32| p & 0x00ff_00ff;
    let hi = |p: u32| (p >> 8) & 0x00ff_00ff;
    let l = (lo(a) + lo(b) + lo(c) + lo(d) + 0x0002_0002) >> 2 & 0x00ff_00ff;
    let h = (hi(a) + hi(b) + hi(c) + hi(d) + 0x0002_0002) >> 2 & 0x00ff_00ff;
    l | (h << 8)
}

/// Premultiplied "source over destination" for one RGBA pixel.
#[inline]
pub fn over(s: u32, d: u32) -> u32 {
    let a = s >> 24;
    if a == 255 {
        return s;
    }
    if a == 0 {
        return d;
    }
    let inv = 255 - a;
    let rb = ((d & 0x00ff_00ff) * inv + 0x0080_0080) >> 8 & 0x00ff_00ff;
    let ga = (((d >> 8) & 0x00ff_00ff) * inv + 0x0080_0080) & 0xff00_ff00;
    (s.wrapping_add(rb | ga)) | 0xff00_0000
}

/// Convert one row of wl_shm pixels into RGBA. Returns whether every pixel was opaque.
pub fn convert_row(format: u32, src: &[u8], dst: &mut [u32]) -> bool {
    use wayland_server::protocol::wl_shm::Format;
    let mut opaque = true;
    let n = dst.len();
    let read = |i: usize| u32::from_le_bytes([src[i * 4], src[i * 4 + 1], src[i * 4 + 2], src[i * 4 + 3]]);
    if format == Format::Argb8888 as u32 || format == Format::Xrgb8888 as u32 {
        let x = format == Format::Xrgb8888 as u32;
        for (i, d) in dst.iter_mut().enumerate().take(n) {
            let v = read(i);
            let v = (v & 0xff00_ff00) | ((v >> 16) & 0xff) | ((v & 0xff) << 16);
            *d = if x { v | 0xff00_0000 } else { v };
            opaque &= (*d >> 24) == 255;
        }
    } else {
        // Abgr8888 / Xbgr8888 are already RGBA in memory.
        let x = format == Format::Xbgr8888 as u32;
        for (i, d) in dst.iter_mut().enumerate().take(n) {
            let v = read(i);
            *d = if x { v | 0xff00_0000 } else { v };
            opaque &= (*d >> 24) == 255;
        }
    }
    opaque
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swizzles_argb_to_rgba() {
        // 0xAARRGGBB = opaque red, little-endian bytes B,G,R,A
        let src = 0xffff_0000u32.to_le_bytes();
        let mut out = [0u32; 1];
        assert!(convert_row(0, &src, &mut out));
        assert_eq!(out[0], 0xff00_00ff); // RGBA in memory: R=ff, G=00, B=00, A=ff
    }

    #[test]
    fn blends_half_transparent() {
        let s = 0x8000_0080u32; // premultiplied half red
        let d = 0xff00_ff00u32; // opaque green
        let o = over(s, d);
        assert_eq!(o & 0xff, 0x80);
        assert!((o >> 8 & 0xff) >= 0x7e && (o >> 8 & 0xff) <= 0x80);
    }

    #[test]
    fn composes_scaled_root() {
        let img = Arc::new(Image { width: 2, height: 2, data: vec![0xff11_1111; 4], opaque: true });
        let mut buf = vec![0u32; 16];
        compose(&mut buf, 4, 4, 4, &[Placed { image: img, x: 0, y: 0, w: 4, h: 4, root: true }]);
        assert!(buf.iter().all(|&p| p == 0xff11_1111));
    }
}
