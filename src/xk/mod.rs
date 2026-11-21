//! The vector line parser for events and clicks.
//!
//! `iface.rs` and `k_free.rs` in this folder are taken from the express-lane
//! project (https://github.com/jason-scheffel/express-lane, commit 7546ac1),
//! Copyright (c) 2026 Team 67, used under the MIT License; the licence text
//! is in `LICENSE` beside this file. The only change is the module path of
//! `iface`. Everything that feeds this parser and consumes its batches
//! (`dense.rs`) is this project's own.
//!
//! The parser needs AVX-512 (BW, VL), BMI2 and LZCNT at compile time, so it
//! exists only in the build made for such CPUs; see `main.rs` for how that
//! build is chosen at start-up.
#![allow(dead_code, unused_unsafe, unexpected_cfgs, clippy::all)]
pub mod iface;
pub mod k_free;
