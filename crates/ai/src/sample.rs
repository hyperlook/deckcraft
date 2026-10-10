//! Semantic catalog metadata and offline fixtures for local sample templates.
//!
//! Important licensing & asset isolation policy:
//! Third-party source scripts (.js) and native binaries (.deckcraft) are NOT committed
//! to Git because external licenses are unconfirmed. Git stores only pure semantic metadata
//! and offline contract fixtures. The local working copy path is runtime-configurable via
//! host configuration (e.g. `DECKCRAFT_TEMPLATE_DIR`).

use std::collections::BTreeSet;

use deckcraft_engine::ai_contract::{ContractError, ContractResult, decode_bounded};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SamplePageRole {
    pub slide_index: usize,
    pub role: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleTemplateEntry {
    pub id: String,
    pub name: String,
    pub category: String,
    pub slide_count: usize,
    pub width: u32,
    pub height: u32,
    pub source_sha256: String,
    pub native_sha256: String,
    pub image_slots: usize,
    pub text_slots: usize,
    pub primary_fonts: Vec<String>,
    pub roles: Vec<SamplePageRole>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleTemplateCatalog {
    pub version: u32,
    pub description: String,
    pub license_policy: String,
    pub templates: Vec<SampleTemplateEntry>,
}

impl SampleTemplateCatalog {
    pub fn decode(bytes: &[u8]) -> ContractResult<Self> {
        let catalog: Self = decode_bounded(bytes)?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn validate(&self) -> ContractResult<()> {
        if self.version != 1 {
            return Err(ContractError::SnapshotMismatch);
        }
        if self.templates.is_empty() || self.templates.len() > 64 {
            return Err(ContractError::Limit("sample template count"));
        }
        let mut seen_ids = BTreeSet::new();
        for t in &self.templates {
            if !seen_ids.insert(&t.id) {
                return Err(ContractError::DuplicateTarget);
            }
            if t.slide_count == 0 || t.slide_count > 64 {
                return Err(ContractError::Limit("sample slide count"));
            }
            if t.width == 0 || t.height == 0 {
                return Err(ContractError::Limit("sample slide dimensions"));
            }
            if t.source_sha256.len() != 64 || t.native_sha256.len() != 64 {
                return Err(ContractError::SnapshotMismatch);
            }
            if t.roles.len() > t.slide_count {
                return Err(ContractError::Limit("sample roles count"));
            }
            let mut seen_roles = BTreeSet::new();
            for r in &t.roles {
                if r.slide_index >= t.slide_count {
                    return Err(ContractError::UnauthorizedTarget);
                }
                if !seen_roles.insert(r.slide_index) {
                    return Err(ContractError::DuplicateTarget);
                }
            }
        }
        Ok(())
    }

    pub fn find(&self, id: &str) -> Option<&SampleTemplateEntry> {
        self.templates.iter().find(|t| t.id == id)
    }
}

#[cfg(test)]
#[path = "sample_tests.rs"]
mod tests;
