//! Sample conversion from what a PulseAudio client sends to what the app plays, and from what the
//! app's microphone captures to what a recording client asked for.
//!
//! The app opens one Android stream per PulseAudio stream and Android mixes them, so the only
//! work here is to make each stream something Android takes everywhere: s16le or f32le, mono or
//! stereo, at the client's own rate (AAudio and AudioTrack resample). Volume is applied here
//! too, because the app has no per-stream volume of its own.
//!
//! The microphone comes the other way as s16le, one or two channels, already at the record
//! stream's rate (the page resamples): [`Capture`] spreads it over the stream's channels and
//! encodes it in its sample format.

use lindroids_proto::AudioFormat;
use pulseaudio::protocol::{ChannelMap, ChannelPosition, SampleFormat, SampleSpec};

pub struct Converter {
    format: SampleFormat,
    in_channels: usize,
    /// How much of each input channel goes to the left and the right output.
    mix: Vec<[f32; 2]>,
    out_channels: usize,
    out_format: AudioFormat,
    /// Same sample format and channel layout in and out: bytes can be copied at unity gain.
    same_layout: bool,
}

impl Converter {
    /// None for formats that carry no PCM (or that the protocol calls invalid).
    pub fn new(spec: &SampleSpec, map: &ChannelMap) -> Option<Converter> {
        let in_channels = spec.channels as usize;
        if in_channels == 0 || spec.format.bytes_per_sample() == 0 {
            return None;
        }
        let out_format = match spec.format {
            SampleFormat::Float32Le | SampleFormat::Float32Be => AudioFormat::F32le,
            _ => AudioFormat::S16le,
        };
        let out_channels = in_channels.min(2);
        let mix = if in_channels <= 2 {
            (0..in_channels).map(|c| if c == 0 { [1.0, 0.0] } else { [0.0, 1.0] }).collect()
        } else {
            downmix(map, in_channels)
        };
        let same_layout = in_channels <= 2
            && matches!((spec.format, out_format), (SampleFormat::S16Le, AudioFormat::S16le) | (SampleFormat::Float32Le, AudioFormat::F32le));
        Some(Converter { format: spec.format, in_channels, mix, out_channels, out_format, same_layout })
    }

    pub fn out_format(&self) -> AudioFormat {
        self.out_format
    }

    pub fn out_channels(&self) -> usize {
        self.out_channels
    }

    pub fn in_frame(&self) -> usize {
        self.format.bytes_per_sample() * self.in_channels
    }

    pub fn out_frame(&self) -> usize {
        self.out_format.bytes() as usize * self.out_channels
    }

    /// Converts whole frames of `input` (a trailing partial frame is ignored), scaling input
    /// channel `c` by `gains[c]`, and appends the result to `out`.
    pub fn convert(&self, input: &[u8], gains: &[f32], out: &mut Vec<u8>) {
        let frame = self.in_frame();
        let input = &input[..input.len() - input.len() % frame];
        let unity = gains.iter().all(|&g| g == 1.0);
        if self.same_layout && unity {
            out.extend_from_slice(input);
            return;
        }
        let bps = self.format.bytes_per_sample();
        out.reserve(input.len() / frame * self.out_frame());
        for f in input.chunks_exact(frame) {
            let mut acc = [0f32; 2];
            for (c, s) in f.chunks_exact(bps).enumerate() {
                let v = decode(self.format, s) * gains.get(c).copied().unwrap_or(1.0);
                acc[0] += v * self.mix[c][0];
                acc[1] += v * self.mix[c][1];
            }
            for v in &acc[..self.out_channels] {
                match self.out_format {
                    AudioFormat::S16le => {
                        let s = (v * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
                        out.extend_from_slice(&s.to_le_bytes());
                    }
                    AudioFormat::F32le => out.extend_from_slice(&v.to_le_bytes()),
                }
            }
        }
    }
}

/// Microphone samples (s16le, [`Capture::in_channels`] of them) into a record stream's format.
pub struct Capture {
    format: SampleFormat,
    in_channels: usize,
    /// The input channel each output channel takes; None: the average of both.
    pick: Vec<Option<usize>>,
}

impl Capture {
    /// None for formats that carry no PCM.
    pub fn new(spec: &SampleSpec, map: &ChannelMap) -> Option<Capture> {
        use ChannelPosition::*;
        let out = spec.channels as usize;
        if out == 0 || spec.format.bytes_per_sample() == 0 {
            return None;
        }
        let in_channels = out.min(2);
        let positions: Vec<ChannelPosition> = map.into_iter().collect();
        let pick = (0..out)
            .map(|c| {
                if in_channels == 1 {
                    return Some(0);
                }
                if out == 2 || positions.len() != out {
                    return Some(c % 2);
                }
                match positions[c] {
                    FrontLeft | FrontLeftOfCenter | RearLeft | SideLeft | TopFrontLeft | TopRearLeft => Some(0),
                    FrontRight | FrontRightOfCenter | RearRight | SideRight | TopFrontRight | TopRearRight => Some(1),
                    _ => None,
                }
            })
            .collect();
        Some(Capture { format: spec.format, in_channels, pick })
    }

    /// Channels the app is asked to capture: the stream's, at most two.
    pub fn in_channels(&self) -> usize {
        self.in_channels
    }

    pub fn in_frame(&self) -> usize {
        2 * self.in_channels
    }

    pub fn out_frame(&self) -> usize {
        self.format.bytes_per_sample() * self.pick.len()
    }

    /// Converts whole frames of `input`, scaling output channel `c` by `gains[c]`, and appends
    /// them to `out`.
    pub fn convert(&self, input: &[u8], gains: &[f32], out: &mut Vec<u8>) {
        let frame = self.in_frame();
        let input = &input[..input.len() - input.len() % frame];
        if self.format == SampleFormat::S16Le && self.pick.len() == self.in_channels && gains.iter().all(|&g| g == 1.0) {
            out.extend_from_slice(input);
            return;
        }
        out.reserve(input.len() / frame * self.out_frame());
        for f in input.chunks_exact(frame) {
            let v = |c: usize| i16::from_le_bytes([f[2 * c], f[2 * c + 1]]) as f32 / 32768.0;
            for (c, pick) in self.pick.iter().enumerate() {
                let s = match pick {
                    Some(i) => v(*i),
                    None => (v(0) + v(1)) / 2.0,
                };
                encode(self.format, s * gains.get(c).copied().unwrap_or(1.0), out);
            }
        }
    }

    /// `frames` frames of silence in the stream's format.
    pub fn silence(&self, frames: usize, out: &mut Vec<u8>) {
        let start = out.len();
        for _ in 0..self.pick.len() {
            encode(self.format, 0.0, out);
        }
        let one = out[start..].to_vec();
        for _ in 1..frames {
            out.extend_from_slice(&one);
        }
        if frames == 0 {
            out.truncate(start);
        }
    }
}

/// One sample from a float in -1.0..1.0 (clipped).
fn encode(format: SampleFormat, v: f32, out: &mut Vec<u8>) {
    use SampleFormat::*;
    let v = v.clamp(-1.0, 1.0);
    let int = |scale: f64| -> i64 { ((v as f64) * scale).round().clamp(-scale, scale - 1.0) as i64 };
    let s16 = int(32768.0) as i16;
    match format {
        U8 => out.push((int(128.0) + 128) as u8),
        Alaw => out.push(alaw_encode(s16)),
        Ulaw => out.push(ulaw_encode(s16)),
        S16Le => out.extend_from_slice(&s16.to_le_bytes()),
        S16Be => out.extend_from_slice(&s16.to_be_bytes()),
        Float32Le => out.extend_from_slice(&v.to_le_bytes()),
        Float32Be => out.extend_from_slice(&v.to_be_bytes()),
        S32Le => out.extend_from_slice(&(int(2_147_483_648.0) as i32).to_le_bytes()),
        S32Be => out.extend_from_slice(&(int(2_147_483_648.0) as i32).to_be_bytes()),
        S24Le => out.extend_from_slice(&(int(8_388_608.0) as i32).to_le_bytes()[..3]),
        S24Be => out.extend_from_slice(&(int(8_388_608.0) as i32).to_be_bytes()[1..]),
        S24In32Le => out.extend_from_slice(&(int(8_388_608.0) as i32).to_le_bytes()),
        S24In32Be => out.extend_from_slice(&(int(8_388_608.0) as i32).to_be_bytes()),
        Invalid => {}
    }
}

/// Linear 16-bit to G.711 mu-law.
fn ulaw_encode(s: i16) -> u8 {
    let mut x = s as i32;
    let sign = if x < 0 {
        x = -x;
        0x80
    } else {
        0
    };
    x = x.min(32635) + 0x84;
    let mut exp = 7;
    while exp > 0 && x & (0x80 << exp) == 0 {
        exp -= 1;
    }
    let mant = (x >> (exp + 3)) & 0x0f;
    !(sign | (exp << 4) | mant) as u8
}

/// Linear 16-bit to G.711 A-law.
fn alaw_encode(s: i16) -> u8 {
    let mut pcm = (s as i32) >> 3;
    let mask = if pcm >= 0 {
        0xD5
    } else {
        pcm = -pcm - 1;
        0x55
    };
    let Some(seg) = (0..8).find(|&i| pcm <= (0x1F << i) | ((1 << i) - 1)) else { return (0x7F ^ mask) as u8 };
    let mant = if seg < 2 { (pcm >> 1) & 0x0f } else { (pcm >> seg) & 0x0f };
    (((seg << 4) | mant) ^ mask) as u8
}

/// Weights folding a surround layout into stereo: each side gets its own channels, centre
/// channels go to both at -3 dB, LFE is dropped (as PulseAudio does by default), and each side is
/// normalised so a full-scale input cannot clip.
fn downmix(map: &ChannelMap, channels: usize) -> Vec<[f32; 2]> {
    use ChannelPosition::*;
    const H: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let positions: Vec<ChannelPosition> = map.into_iter().collect();
    let known = positions.len() == channels;
    let mut mix: Vec<[f32; 2]> = (0..channels)
        .map(|c| {
            if !known {
                return if c % 2 == 0 { [1.0, 0.0] } else { [0.0, 1.0] };
            }
            match positions[c] {
                FrontLeft | FrontLeftOfCenter => [1.0, 0.0],
                FrontRight | FrontRightOfCenter => [0.0, 1.0],
                RearLeft | SideLeft | TopFrontLeft | TopRearLeft => [H, 0.0],
                RearRight | SideRight | TopFrontRight | TopRearRight => [0.0, H],
                Mono | FrontCenter | RearCenter | TopCenter | TopFrontCenter | TopRearCenter => [H, H],
                Lfe => [0.0, 0.0],
                _ => {
                    if c % 2 == 0 {
                        [1.0, 0.0]
                    } else {
                        [0.0, 1.0]
                    }
                }
            }
        })
        .collect();
    for side in 0..2 {
        let sum: f32 = mix.iter().map(|w| w[side]).sum();
        if sum > 1.0 {
            mix.iter_mut().for_each(|w| w[side] /= sum);
        }
    }
    mix
}

/// One sample as a float in -1.0..1.0.
fn decode(format: SampleFormat, b: &[u8]) -> f32 {
    use SampleFormat::*;
    const S16: f32 = 32768.0;
    const S24: f32 = 8_388_608.0;
    const S32: f32 = 2_147_483_648.0;
    match format {
        U8 => (b[0] as f32 - 128.0) / 128.0,
        Alaw => alaw(b[0]) as f32 / S16,
        Ulaw => ulaw(b[0]) as f32 / S16,
        S16Le => i16::from_le_bytes([b[0], b[1]]) as f32 / S16,
        S16Be => i16::from_be_bytes([b[0], b[1]]) as f32 / S16,
        Float32Le => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        Float32Be => f32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        S32Le => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / S32,
        S32Be => i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f32 / S32,
        // Shift the 24 bits to the top of an i32 and back down to sign-extend them.
        S24Le => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / S24,
        S24Be => (i32::from_be_bytes([b[0], b[1], b[2], 0]) >> 8) as f32 / S24,
        S24In32Le => (i32::from_le_bytes([b[0], b[1], b[2], b[3]]) << 8 >> 8) as f32 / S24,
        S24In32Be => (i32::from_be_bytes([b[0], b[1], b[2], b[3]]) << 8 >> 8) as f32 / S24,
        Invalid => 0.0,
    }
}

/// G.711 mu-law to linear 16-bit.
fn ulaw(u: u8) -> i16 {
    let u = !u;
    let exp = (u >> 4) & 7;
    let s = ((((u & 0x0f) as i32) << 3) + 0x84) << exp;
    let s = s - 0x84;
    (if u & 0x80 != 0 { -s } else { s }) as i16
}

/// G.711 A-law to linear 16-bit.
fn alaw(a: u8) -> i16 {
    let a = a ^ 0x55;
    let exp = (a >> 4) & 7;
    let mant = (a & 0x0f) as i32;
    let s = if exp == 0 { (mant << 4) + 8 } else { ((mant << 4) + 0x108) << (exp - 1) };
    (if a & 0x80 != 0 { s } else { -s }) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(format: SampleFormat, channels: u8) -> SampleSpec {
        SampleSpec { format, channels, sample_rate: 48000 }
    }

    fn s16(v: &[i16]) -> Vec<u8> {
        v.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    fn to_i16(b: &[u8]) -> Vec<i16> {
        b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect()
    }

    #[test]
    fn s16_stereo_passes_through() {
        let c = Converter::new(&spec(SampleFormat::S16Le, 2), &ChannelMap::stereo()).unwrap();
        assert_eq!((c.out_format(), c.out_channels()), (AudioFormat::S16le, 2));
        let input = s16(&[1, -2, 32767, -32768]);
        let mut out = Vec::new();
        c.convert(&input, &[1.0, 1.0], &mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn partial_frames_are_left_out() {
        let c = Converter::new(&spec(SampleFormat::S16Le, 2), &ChannelMap::stereo()).unwrap();
        let mut out = Vec::new();
        c.convert(&[1, 0, 2, 0, 3], &[1.0, 1.0], &mut out);
        assert_eq!(out, vec![1, 0, 2, 0]);
    }

    #[test]
    fn volume_scales_samples() {
        let c = Converter::new(&spec(SampleFormat::S16Le, 2), &ChannelMap::stereo()).unwrap();
        let mut out = Vec::new();
        c.convert(&s16(&[1000, 1000]), &[0.5, 0.0], &mut out);
        assert_eq!(to_i16(&out), vec![500, 0]);
    }

    #[test]
    fn float_stays_float() {
        let c = Converter::new(&spec(SampleFormat::Float32Le, 1), &ChannelMap::mono()).unwrap();
        assert_eq!((c.out_format(), c.out_channels(), c.out_frame()), (AudioFormat::F32le, 1, 4));
        let mut out = Vec::new();
        c.convert(&0.25f32.to_le_bytes(), &[2.0], &mut out);
        assert_eq!(f32::from_le_bytes(out[..4].try_into().unwrap()), 0.5);
    }

    #[test]
    fn other_formats_become_s16() {
        let mut out = Vec::new();
        let u8c = Converter::new(&spec(SampleFormat::U8, 1), &ChannelMap::mono()).unwrap();
        u8c.convert(&[128, 255, 0], &[1.0], &mut out);
        assert_eq!(to_i16(&out), vec![0, 32512, -32768]);

        out.clear();
        let s24 = Converter::new(&spec(SampleFormat::S24Le, 1), &ChannelMap::mono()).unwrap();
        // 0x400000 = half scale, 0xC00000 = minus half scale.
        s24.convert(&[0x00, 0x00, 0x40, 0x00, 0x00, 0xC0], &[1.0], &mut out);
        assert_eq!(to_i16(&out), vec![16384, -16384]);

        out.clear();
        let s32 = Converter::new(&spec(SampleFormat::S32Le, 1), &ChannelMap::mono()).unwrap();
        s32.convert(&(i32::MIN / 4).to_le_bytes(), &[1.0], &mut out);
        assert_eq!(to_i16(&out), vec![-8192]);

        out.clear();
        let s24in32 = Converter::new(&spec(SampleFormat::S24In32Le, 1), &ChannelMap::mono()).unwrap();
        s24in32.convert(&[0x00, 0x00, 0xC0, 0xAB], &[1.0], &mut out);
        assert_eq!(to_i16(&out), vec![-16384], "the top byte is ignored");

        out.clear();
        let be = Converter::new(&spec(SampleFormat::S16Be, 1), &ChannelMap::mono()).unwrap();
        be.convert(&[0x12, 0x34], &[1.0], &mut out);
        assert_eq!(to_i16(&out), vec![0x1234]);
    }

    #[test]
    fn g711_decodes() {
        assert_eq!(ulaw(0xff), 0);
        assert_eq!(ulaw(0x80), 32124);
        assert_eq!(ulaw(0x00), -32124);
        assert_eq!(alaw(0xd5), 8);
        assert_eq!(alaw(0xaa), 32256);
        assert_eq!(alaw(0x2a), -32256);
    }

    #[test]
    fn surround_folds_into_stereo() {
        use ChannelPosition::*;
        let map = ChannelMap::new([FrontLeft, FrontRight, FrontCenter, Lfe, RearLeft, RearRight]);
        let c = Converter::new(&spec(SampleFormat::S16Le, 6), &map).unwrap();
        assert_eq!((c.out_channels(), c.out_frame()), (2, 4));
        let mut out = Vec::new();
        // Front left only, then LFE only.
        c.convert(&s16(&[16384, 0, 0, 0, 0, 0, 0, 0, 0, 16384, 0, 0]), &[1.0; 6], &mut out);
        let v = to_i16(&out);
        assert!(v[0] > 6000 && v[0] < 16384, "left is normalised: {}", v[0]);
        assert_eq!(v[1], 0);
        assert_eq!(&v[2..], &[0, 0], "LFE is dropped");

        // Full scale on every channel must not clip.
        out.clear();
        c.convert(&s16(&[32767; 6]), &[1.0; 6], &mut out);
        assert!(to_i16(&out).iter().all(|&s| s > 30000), "summed to just under full scale");
    }

    #[test]
    fn unknown_layouts_alternate_sides() {
        let c = Converter::new(&spec(SampleFormat::S16Le, 4), &ChannelMap::stereo()).unwrap();
        let mut out = Vec::new();
        c.convert(&s16(&[100, 200, 100, 200]), &[1.0; 4], &mut out);
        assert_eq!(to_i16(&out), vec![100, 200]);
    }

    #[test]
    fn capture_spreads_and_encodes() {
        use ChannelPosition::*;
        // Mono into stereo: both sides.
        let c = Capture::new(&spec(SampleFormat::S16Le, 2), &ChannelMap::stereo()).unwrap();
        assert_eq!((c.in_channels(), c.in_frame(), c.out_frame()), (2, 4, 4));
        let m = Capture::new(&spec(SampleFormat::S16Le, 1), &ChannelMap::mono()).unwrap();
        let mut out = Vec::new();
        m.convert(&s16(&[1000, -1000, 7]), &[1.0], &mut out);
        assert_eq!(to_i16(&out), vec![1000, -1000, 7], "s16 mono passes through");

        // Stereo into 5.1: sides by position, the centre and LFE get the average.
        let map = ChannelMap::new([FrontLeft, FrontRight, FrontCenter, Lfe, RearLeft, RearRight]);
        let c = Capture::new(&spec(SampleFormat::S16Le, 6), &map).unwrap();
        out.clear();
        c.convert(&s16(&[1000, 3000]), &[1.0; 6], &mut out);
        assert_eq!(to_i16(&out), vec![1000, 3000, 2000, 2000, 1000, 3000]);

        // Float, with volume.
        let f = Capture::new(&spec(SampleFormat::Float32Le, 1), &ChannelMap::mono()).unwrap();
        out.clear();
        f.convert(&s16(&[16384]), &[0.5], &mut out);
        assert_eq!(f32::from_le_bytes(out[..4].try_into().unwrap()), 0.25);

        // Every format decodes back to what went in.
        for (fmt, _) in [(SampleFormat::U8, 0), (SampleFormat::S16Be, 0), (SampleFormat::S32Le, 0), (SampleFormat::S24Le, 0), (SampleFormat::S24Be, 0), (SampleFormat::S24In32Be, 0), (SampleFormat::Float32Be, 0), (SampleFormat::Ulaw, 0), (SampleFormat::Alaw, 0)] {
            for v in [0.0f32, 0.5, -0.5, 0.999, -1.0] {
                let mut b = Vec::new();
                encode(fmt, v, &mut b);
                assert_eq!(b.len(), fmt.bytes_per_sample());
                let back = decode(fmt, &b);
                let tol = match fmt {
                    SampleFormat::U8 => 0.01,
                    SampleFormat::Ulaw | SampleFormat::Alaw => 0.04,
                    _ => 0.0001,
                };
                assert!((back - v).abs() <= tol, "{fmt:?} {v} -> {back}");
            }
        }
    }

    #[test]
    fn capture_silence_is_silent_in_every_format() {
        for fmt in [SampleFormat::U8, SampleFormat::Ulaw, SampleFormat::Alaw, SampleFormat::S16Le, SampleFormat::Float32Le] {
            let c = Capture::new(&spec(fmt, 2), &ChannelMap::stereo()).unwrap();
            let mut out = Vec::new();
            c.silence(3, &mut out);
            assert_eq!(out.len(), 3 * c.out_frame());
            assert!(out.chunks_exact(fmt.bytes_per_sample()).all(|b| decode(fmt, b).abs() < 0.001), "{fmt:?}: {out:?}");
        }
    }

    #[test]
    fn invalid_format_is_refused() {
        assert!(Converter::new(&spec(SampleFormat::Invalid, 2), &ChannelMap::stereo()).is_none());
        assert!(Converter::new(&spec(SampleFormat::S16Le, 0), &ChannelMap::stereo()).is_none());
    }
}
