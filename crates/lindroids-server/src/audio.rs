//! Sound: a PulseAudio-protocol server for Linux programs, played through Android (AAudio, or
//! AudioTrack on Android 7).

use std::path::Path;

pub fn start(_runtime_dir: &Path) -> Result<(), String> {
    Err("not built yet".into())
}
