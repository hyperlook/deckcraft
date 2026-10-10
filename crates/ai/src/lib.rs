//! Pure AI business contracts. No network, runtime, UI, or mutable Session ownership.
//! The host freezes requests, sends minimal text slices, and owns all validation/apply events.
//! Providers return restricted text patches or template recipes, never native resources, engine commands or task-control events.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod request;
pub mod task;
pub mod template;

#[cfg(any(test, feature = "test-support"))]
pub mod fake;

#[cfg(test)]
mod tests;
