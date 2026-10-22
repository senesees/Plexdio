//! Player-side bookkeeping that does not belong on the audio thread: playback reporting to the
//! server and the saved session (queue and position) used to resume after a restart.

pub mod report;
pub mod session;
