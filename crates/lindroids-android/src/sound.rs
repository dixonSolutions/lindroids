//! Playing the server's sound streams through Android.

use lindroids_proto::AudioFormat;

pub fn open(_link: i32, _stream: u32, _rate: u32, _channels: u32, _format: AudioFormat) {}
pub fn write(_link: i32, _stream: u32, _pcm: Vec<u8>) {}
pub fn cork(_link: i32, _stream: u32, _corked: bool) {}
pub fn close(_link: i32, _stream: u32) {}
pub fn close_link(_link: i32) {}
