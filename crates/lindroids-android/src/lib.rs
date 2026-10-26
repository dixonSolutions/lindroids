//! liblindroids: the native side of the Lindroids Android app.
//!
//! * `android` — JNI entry points: the app's end of each link (frames into Surfaces, input out,
//!   sound into AAudio/AudioTrack), and the in-app server for exported bundles.
//! * [`bundle`] — unpacking exported bundles and starting them under the bundled proot.
//! * [`portal`] — Android integration for Linux programs (system state, opening, notifications,
//!   the file picker): the decisions, kept apart from the JNI wiring so they test on the host.

pub mod bundle;
pub mod portal;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
mod sound;

pub use lindroids_server::BUILD;
