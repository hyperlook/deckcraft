//! Minimal semantic upload for template recipes. Actual native resources remain in AppendScope.
use std::collections::BTreeSet;

use deckcraft_engine::ai_contract::template::AppendScope;
use deckcraft_engine::ai_contract::{ContractError, ContractResult, MAX_TEXT_CHARS, SnapshotIdentity, decode_bounded};
use serde::{Deserialize, Serialize};

use crate::request::{TextIntent, add_text};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotInput {
    pub slot_key: String,
    pub paragraphs: Vec<String>,
    /// Unicode scalar limit per replacement object, with the original paragraph count preserved.
    pub max_chars: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageInput {
    pub page_key: String,
    pub slots: Vec<SlotInput>,
    pub linked_page_keys: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateRequest {
    pub snapshot: SnapshotIdentity,
    pub intent: TextIntent,
    pub max_instances: usize,
    pub pages: Vec<PageInput>,
}

impl TemplateRequest {
    pub fn from_scope(scope: &AppendScope, intent: TextIntent) -> ContractResult<Self> {
        let mut pages = Vec::new();
        for page in scope.pages() {
            let slide = scope.bundle().native().slide(page.source_slide_id).ok_or(ContractError::UnauthorizedTarget)?;
            let mut slots = Vec::new();
            for slot in &page.slots {
                let body = slide.shape(slot.shape_id).and_then(|s| s.text.as_ref()).ok_or(ContractError::UnauthorizedTarget)?;
                slots.push(SlotInput {
                    slot_key: slot.key.clone(),
                    paragraphs: body.paragraphs.iter().map(|p| p.text()).collect(),
                    max_chars: MAX_TEXT_CHARS,
                });
            }
            // Dependency discovery is host-authorized, not interpreted from provider content.
            let linked_page_keys = scope.linked_pages(&page.key)?.into_iter().map(|p| p.key.clone()).collect();
            pages.push(PageInput { page_key: page.key.clone(), slots, linked_page_keys });
        }
        let request = Self { snapshot: scope.snapshot().clone(), intent, max_instances: scope.max_instances(), pages };
        request.validate(scope)?;
        Ok(request)
    }

    pub fn decode(bytes: &[u8], scope: &AppendScope) -> ContractResult<Self> {
        let request: Self = decode_bounded(bytes)?;
        request.validate(scope)?;
        Ok(request)
    }

    pub fn validate(&self, scope: &AppendScope) -> ContractResult<()> {
        if &self.snapshot != scope.snapshot() || self.max_instances != scope.max_instances() {
            return Err(ContractError::SnapshotMismatch);
        }
        if self.pages.len() != scope.pages().len() {
            return Err(ContractError::UnauthorizedTarget);
        }
        let mut total = 0usize;
        self.intent.validate_with_total(&mut total)?;
        let mut seen = BTreeSet::new();
        for input in &self.pages {
            let page = scope.pages().iter().find(|p| p.key == input.page_key).ok_or(ContractError::UnauthorizedTarget)?;
            if !seen.insert(&input.page_key) || input.slots.len() != page.slots.len() {
                return Err(ContractError::UnauthorizedTarget);
            }
            let expected_links: Vec<_> = scope.linked_pages(&input.page_key)?.into_iter().map(|p| p.key.as_str()).collect();
            if !input.linked_page_keys.iter().map(String::as_str).eq(expected_links) {
                return Err(ContractError::SnapshotMismatch);
            }
            let mut fields = BTreeSet::new();
            for slot in &input.slots {
                let permission = page.slots.iter().find(|s| s.key == slot.slot_key).ok_or(ContractError::UnauthorizedTarget)?;
                if !fields.insert(&slot.slot_key) || slot.max_chars != MAX_TEXT_CHARS {
                    return Err(ContractError::UnauthorizedTarget);
                }
                let body = scope
                    .bundle()
                    .native()
                    .slide(page.source_slide_id)
                    .and_then(|s| s.shape(permission.shape_id))
                    .and_then(|s| s.text.as_ref())
                    .ok_or(ContractError::UnauthorizedTarget)?;
                if body.paragraphs.len() != slot.paragraphs.len()
                    || body
                        .paragraphs
                        .iter()
                        .zip(&slot.paragraphs)
                        .any(|(p, supplied)| !p.runs.iter().flat_map(|r| r.text.chars()).eq(supplied.chars()))
                {
                    return Err(ContractError::SnapshotMismatch);
                }
                let mut chars = 0usize;
                for text in &slot.paragraphs {
                    add_text(text, MAX_TEXT_CHARS, &mut total)?;
                    chars = chars.checked_add(text.chars().count()).ok_or(ContractError::Limit("source text characters"))?;
                    if chars > MAX_TEXT_CHARS {
                        return Err(ContractError::Limit("source text characters"));
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "template_tests.rs"]
mod tests;
