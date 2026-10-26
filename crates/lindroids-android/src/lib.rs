//! liblindroids: the native side of the Lindroids Android app.
//!
//! * `android` — JNI entry points: the app's end of each link (frames into Surfaces, input out,
//!   sound into AAudio/AudioTrack), and the in-app server for exported bundles.
//! * [`bundle`] — unpacking exported bundles and starting them under the bundled proot.

pub mod bundle;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
mod sound;

pub use lindroids_server::BUILD;
