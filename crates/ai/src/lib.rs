//! AI content contracts: round context plus the tool-free model-entry request lifecycle.
//!
//! This crate does not call a model, write the deck, or schedule work. The host shows the returned
//! proposal and, if the user accepts it, commits it through the engine.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod context;
pub mod request;
mod round;
pub use context::{extract_context, review_existing};

pub use round::{FACT_RULE, ModelBrief, Round};
