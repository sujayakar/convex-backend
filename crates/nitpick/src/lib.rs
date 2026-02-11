#![feature(try_blocks)]
#![feature(panic_update_hook)]

//! Nitpick: deterministic simulation testing for the Convex storage engine.
//!
//! The spiritual successor to `pedant`, operating at the Application layer
//! with real JavaScript UDF execution through V8 isolates.

pub mod framework;
pub mod scenarios;
pub mod simulation;

#[cfg(test)]
mod tests;
