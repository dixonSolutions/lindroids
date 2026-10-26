//! Stream formats: what a client asks for, and how a format is described back to it.
//!
//! A client either gives a plain sample spec or, through `pa_stream_new_extended` (GStreamer's
//! pulsesink does this to probe), a list of format infos whose properties are small JSON values:
//! `format.sample_format = "\"s16le\""`, `format.rate = "48000"`. Replies always carry the
//! chosen format as such a format info, because libpulse rebuilds the stream's sample spec from
//! it.

use pulseaudio::protocol::{self as pa, ChannelMap, ChannelPosition, FormatEncoding, FormatInfo, SampleFormat, SampleSpec};

const SAMPLE_FORMAT: &std::ffi::CStr = c"format.sample_format";
const RATE: &std::ffi::CStr = c"format.rate";
const CHANNELS: &std::ffi::CStr = c"format.channels";
const CHANNEL_MAP: &std::ffi::CStr = c"format.channel_map";

const FORMAT_NAMES: [(SampleFormat, &str); 13] = [
    (SampleFormat::U8, "u8"),
    (SampleFormat::Alaw, "aLaw"),
    (SampleFormat::Ulaw, "uLaw"),
    (SampleFormat::S16Le, "s16le"),
    (SampleFormat::S16Be, "s16be"),
    (SampleFormat::Float32Le, "float32le"),
    (SampleFormat::Float32Be, "float32be"),
    (SampleFormat::S32Le, "s32le"),
    (SampleFormat::S32Be, "s32be"),
    (SampleFormat::S24Le, "s24le"),
    (SampleFormat::S24Be, "s24be"),
    (SampleFormat::S24In32Le, "s24-32le"),
    (SampleFormat::S24In32Be, "s24-32be"),
];

pub fn valid(s: &SampleSpec) -> bool {
    s.format != SampleFormat::Invalid
        && (1..=pa::sample_spec::MAX_CHANNELS).contains(&s.channels)
        && (1..=pa::sample_spec::MAX_RATE).contains(&s.sample_rate)
}

/// The spec and channel map a new stream plays with, or None if nothing offered is PCM we can
/// take.
pub fn negotiate(spec: &SampleSpec, map: &ChannelMap, formats: &[FormatInfo]) -> Option<(SampleSpec, ChannelMap)> {
    let chosen = if formats.is_empty() {
        valid(spec).then(|| (*spec, (map.num_channels() == spec.channels).then_some(*map)))
    } else {
        formats.iter().filter(|f| f.encoding == FormatEncoding::Pcm).find_map(|f| from_format(f, spec))
    };
    chosen.map(|(s, m)| (s, m.filter(|m| m.num_channels() == s.channels).unwrap_or_else(|| default_map(s.channels))))
}

/// A PCM format info: its properties where it has them, the client's spec (or CD-ish defaults)
/// where it leaves them open.
fn from_format(f: &FormatInfo, spec: &SampleSpec) -> Option<(SampleSpec, Option<ChannelMap>)> {
    let mut s = if valid(spec) { *spec } else { SampleSpec { format: SampleFormat::S16Le, channels: 2, sample_rate: 48000 } };
    if let Some(v) = prop(f, SAMPLE_FORMAT) {
        let name = json_string(&v)?;
        s.format = FORMAT_NAMES.iter().find(|(_, n)| *n == name).map(|(f, _)| *f)?;
    }
    if let Some(v) = prop(f, RATE) {
        s.sample_rate = json_int(&v)?;
    }
    if let Some(v) = prop(f, CHANNELS) {
        s.channels = json_int(&v)?.try_into().ok()?;
    }
    let map = prop(f, CHANNEL_MAP).and_then(|v| json_string(&v)).and_then(|m| parse_map(&m));
    valid(&s).then_some((s, map))
}

/// The format info PulseAudio reports for a PCM stream.
pub fn info(spec: &SampleSpec, map: &ChannelMap) -> FormatInfo {
    let mut f = FormatInfo::new(FormatEncoding::Pcm);
    if let Some((_, name)) = FORMAT_NAMES.iter().find(|(fmt, _)| *fmt == spec.format) {
        set(&mut f, SAMPLE_FORMAT, &format!("\"{name}\""));
    }
    set(&mut f, RATE, &spec.sample_rate.to_string());
    set(&mut f, CHANNELS, &spec.channels.to_string());
    let names: Vec<&str> = map.into_iter().map(position_name).collect();
    set(&mut f, CHANNEL_MAP, &format!("\"{}\"", names.join(",")));
    f
}

fn set(f: &mut FormatInfo, key: &std::ffi::CStr, value: &str) {
    let mut v = value.as_bytes().to_vec();
    v.push(0);
    f.props.set_bytes(key, v);
}

fn prop(f: &FormatInfo, key: &std::ffi::CStr) -> Option<String> {
    let b = f.props.get_bytes(key)?;
    let b = b.strip_suffix(&[0]).unwrap_or(b);
    Some(String::from_utf8_lossy(b).trim().to_string())
}

/// A JSON string, or the first one of a list of them.
fn json_string(v: &str) -> Option<String> {
    let start = v.find('"')? + 1;
    let len = v[start..].find('"')?;
    Some(v[start..start + len].to_string())
}

/// A JSON integer, the first of a list, or the top of a `{"min": .., "max": ..}` range.
fn json_int(v: &str) -> Option<u32> {
    let digits = |s: &str| -> Option<u32> {
        let s = s.trim_start_matches(|c: char| !c.is_ascii_digit());
        let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
        s[..end].parse().ok()
    };
    match v.find("\"max\"") {
        Some(i) => digits(&v[i + 5..]),
        None => digits(v),
    }
}

const POSITION_NAMES: [(ChannelPosition, &str); 19] = [
    (ChannelPosition::Mono, "mono"),
    (ChannelPosition::FrontLeft, "front-left"),
    (ChannelPosition::FrontRight, "front-right"),
    (ChannelPosition::FrontCenter, "front-center"),
    (ChannelPosition::RearCenter, "rear-center"),
    (ChannelPosition::RearLeft, "rear-left"),
    (ChannelPosition::RearRight, "rear-right"),
    (ChannelPosition::Lfe, "lfe"),
    (ChannelPosition::FrontLeftOfCenter, "front-left-of-center"),
    (ChannelPosition::FrontRightOfCenter, "front-right-of-center"),
    (ChannelPosition::SideLeft, "side-left"),
    (ChannelPosition::SideRight, "side-right"),
    (ChannelPosition::TopCenter, "top-center"),
    (ChannelPosition::TopFrontLeft, "top-front-left"),
    (ChannelPosition::TopFrontRight, "top-front-right"),
    (ChannelPosition::TopFrontCenter, "top-front-center"),
    (ChannelPosition::TopRearLeft, "top-rear-left"),
    (ChannelPosition::TopRearRight, "top-rear-right"),
    (ChannelPosition::TopRearCenter, "top-rear-center"),
];

fn position_name(p: ChannelPosition) -> &'static str {
    POSITION_NAMES.iter().find(|(q, _)| *q == p).map_or("aux0", |(_, n)| n)
}

fn parse_map(s: &str) -> Option<ChannelMap> {
    let mut map = ChannelMap::empty();
    for name in s.split(',') {
        let (p, _) = POSITION_NAMES.iter().find(|(_, n)| *n == name.trim())?;
        if map.num_channels() >= pa::sample_spec::MAX_CHANNELS {
            return None;
        }
        map.push(*p);
    }
    (map.num_channels() > 0).then_some(map)
}

/// PulseAudio's default layouts (ALSA order) for a channel count that came without a map.
pub fn default_map(channels: u8) -> ChannelMap {
    use ChannelPosition::*;
    let surround = [FrontLeft, FrontRight, RearLeft, RearRight, FrontCenter, Lfe, SideLeft, SideRight];
    match channels {
        1 => ChannelMap::mono(),
        2 => ChannelMap::stereo(),
        n @ 3..=8 => ChannelMap::new(surround.into_iter().take(n as usize)),
        n => ChannelMap::new((0..n).map(|i| if i % 2 == 0 { FrontLeft } else { FrontRight })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(props: &[(&std::ffi::CStr, &str)]) -> FormatInfo {
        let mut f = FormatInfo::new(FormatEncoding::Pcm);
        for (k, v) in props {
            set(&mut f, k, v);
        }
        f
    }

    #[test]
    fn plain_specs_pass() {
        let spec = SampleSpec { format: SampleFormat::Float32Le, channels: 6, sample_rate: 44100 };
        let (s, m) = negotiate(&spec, &ChannelMap::stereo(), &[]).unwrap();
        assert_eq!(s, spec);
        assert_eq!(m.num_channels(), 6, "a map that does not fit is replaced");
        assert!(negotiate(&SampleSpec::default(), &ChannelMap::mono(), &[]).is_none(), "invalid format");
    }

    #[test]
    fn extended_streams_pick_the_first_pcm_format() {
        let ac3 = FormatInfo::new(FormatEncoding::Ac3Iec61937);
        let f = pcm(&[(SAMPLE_FORMAT, "\"s24le\""), (RATE, "96000"), (CHANNELS, "1"), (CHANNEL_MAP, "\"mono\"")]);
        let (s, m) = negotiate(&SampleSpec::default(), &ChannelMap::empty(), &[ac3, f]).unwrap();
        assert_eq!(s, SampleSpec { format: SampleFormat::S24Le, channels: 1, sample_rate: 96000 });
        assert_eq!(m, ChannelMap::mono());
    }

    #[test]
    fn open_properties_fall_back() {
        let (s, _) = negotiate(&SampleSpec::default(), &ChannelMap::empty(), &[pcm(&[(RATE, "[ 22050, 44100 ]")])]).unwrap();
        assert_eq!(s, SampleSpec { format: SampleFormat::S16Le, channels: 2, sample_rate: 22050 });
        assert_eq!(json_int("{ \"min\": 8000, \"max\": 192000 }"), Some(192000));
        assert!(negotiate(&SampleSpec::default(), &ChannelMap::empty(), &[pcm(&[(SAMPLE_FORMAT, "\"dsd\"")])]).is_none());
    }

    #[test]
    fn info_round_trips() {
        let spec = SampleSpec { format: SampleFormat::S16Le, channels: 2, sample_rate: 48000 };
        let f = info(&spec, &ChannelMap::stereo());
        assert_eq!(prop(&f, CHANNEL_MAP).as_deref(), Some("\"front-left,front-right\""));
        assert_eq!(negotiate(&SampleSpec::default(), &ChannelMap::empty(), &[f]), Some((spec, ChannelMap::stereo())));
    }
}
