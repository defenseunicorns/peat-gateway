// Each integration-test binary in `tests/*.rs` re-includes this module via
// `mod common;`, so individual binaries may exercise only a subset of the
// helpers. Allowing dead_code keeps that pattern clean.
#![allow(dead_code)]

#[cfg(feature = "nats")]
pub mod nats;
