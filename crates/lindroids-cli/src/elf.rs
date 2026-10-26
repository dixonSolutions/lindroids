//! Just enough ELF to read a program's DT_NEEDED libraries: whether it speaks only X11 decides if
//! an exported bundle needs Xwayland and xwayland-satellite.

use std::path::Path;

fn u16_at(d: &[u8], o: usize) -> Option<u64> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?) as u64)
}
fn u32_at(d: &[u8], o: usize) -> Option<u64> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?) as u64)
}
fn u64_at(d: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(d.get(o..o + 8)?.try_into().ok()?))
}

/// The DT_NEEDED entries of a little-endian ELF executable or library.
pub fn needed(path: &Path) -> Option<Vec<String>> {
    let d = std::fs::read(path).ok()?;
    if d.get(..4)? != b"\x7fELF" || *d.get(5)? != 1 {
        return None;
    }
    let wide = *d.get(4)? == 2;
    let word = |o: usize| if wide { u64_at(&d, o) } else { u32_at(&d, o) };
    let (phoff, phentsize, phnum) = if wide {
        (u64_at(&d, 0x20)?, u16_at(&d, 0x36)?, u16_at(&d, 0x38)?)
    } else {
        (u32_at(&d, 0x1c)?, u16_at(&d, 0x2a)?, u16_at(&d, 0x2c)?)
    };
    // Program headers: PT_LOAD maps addresses to file offsets; PT_DYNAMIC holds the entries.
    let mut loads = Vec::new();
    let mut dynamic = None;
    for i in 0..phnum {
        let h = (phoff + i * phentsize) as usize;
        let kind = u32_at(&d, h)?;
        let (offset, vaddr, filesz) = if wide {
            (u64_at(&d, h + 8)?, u64_at(&d, h + 16)?, u64_at(&d, h + 32)?)
        } else {
            (u32_at(&d, h + 4)?, u32_at(&d, h + 8)?, u32_at(&d, h + 16)?)
        };
        match kind {
            1 => loads.push((vaddr, offset, filesz)),
            2 => dynamic = Some((offset, filesz)),
            _ => {}
        }
    }
    let (dyn_off, dyn_size) = dynamic?;
    let entry = if wide { 16 } else { 8 };
    let mut strtab = None;
    let mut needed_offsets = Vec::new();
    for i in 0..dyn_size / entry {
        let o = (dyn_off + i * entry) as usize;
        let (tag, val) = (word(o)?, word(o + entry as usize / 2)?);
        match tag {
            0 => break,
            1 => needed_offsets.push(val),
            5 => strtab = Some(val),
            _ => {}
        }
    }
    let strtab = strtab?;
    let strtab_off = loads.iter().find(|(v, _, sz)| strtab >= *v && strtab < v + sz).map(|(v, off, _)| strtab - v + off)?;
    let mut out = Vec::new();
    for n in needed_offsets {
        let start = (strtab_off + n) as usize;
        let end = start + d.get(start..)?.iter().position(|b| *b == 0)?;
        out.push(String::from_utf8_lossy(&d[start..end]).into_owned());
    }
    Some(out)
}

/// A program that draws only through X11: it links Xlib or XCB, and nothing that could take it to
/// Wayland (libwayland itself, or a toolkit that has a Wayland backend).
pub fn x11_only(libs: &[String]) -> bool {
    let has = |prefixes: &[&str]| libs.iter().any(|l| prefixes.iter().any(|p| l.starts_with(p)));
    let x11 = has(&["libX11.so", "libxcb.so", "libXt.so", "libXaw", "libXm.so"]);
    let wayland_capable = has(&[
        "libwayland-client.so",
        "libgtk-3.so",
        "libgtk-4.so",
        "libgdk-3.so",
        "libQt5Gui.so",
        "libQt6Gui.so",
        "libSDL2",
        "libSDL3",
        "libglfw.so",
        "libelementary",
    ]);
    x11 && !wayland_capable
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn reads_its_own_needed() {
        // The test binary is a dynamic ELF almost everywhere it runs (static musl builds aside).
        let me = std::env::current_exe().unwrap();
        if let Some(libs) = needed(&me) {
            assert!(libs.iter().all(|l| !l.is_empty()));
        }
    }

    #[test]
    fn x11_only_programs() {
        assert!(x11_only(&s(&["libXaw.so.7", "libXt.so.6", "libX11.so.6", "libc.so.6"])));
        assert!(!x11_only(&s(&["libgtk-3.so.0", "libX11.so.6"])));
        assert!(!x11_only(&s(&["libSDL2-2.0.so.0"])));
        assert!(!x11_only(&s(&["libwayland-client.so.0", "libxcb.so.1"])));
        assert!(!x11_only(&s(&["libc.so.6"])));
    }
}
