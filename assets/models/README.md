# Models for Ride (the game on the Now playing page)

Embedded in the exe by `crates/plexdio/src/game/ai.rs`.

| File | What | Source |
|---|---|---|
| `beat_this_small.onnx` | Beat This! *small* (2.1 M parameters): beat and downbeat activations from a log-mel spectrogram | Foscarin, Schlüter & Widmer, "Beat This! Accurate Beat Tracking Without DBN Postprocessing", ISMIR 2024 (MIT). ONNX export from [danigb/beat-this-rs](https://github.com/danigb/beat-this-rs) `models/` |
| `mel_spectrogram.onnx` | The matching mel front end (22 050 Hz mono, 1024 window, 441 hop, 128 bands) | same |

Both run on the pure-Rust [rten](https://crates.io/crates/rten) runtime through the
[`beat-this`](https://crates.io/crates/beat-this) crate: no DLLs. rten and its crates are built
with `opt-level = 3` (see the workspace `Cargo.toml`); the size-optimised default makes
inference slow.
