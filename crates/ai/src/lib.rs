//! The context for one AI round: the agreed outline, the cited page, the comment, and the materials.
//!
//! This crate does not call a model, write the deck, or schedule work. The host shows the returned
//! proposal and, if the user accepts it, commits it through the engine.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod round;

pub use round::{FACT_RULE, ModelBrief, Round};
