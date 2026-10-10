//! Explicit upload boundary for the first text-edit workflow.

use deckcraft_engine::ai_contract::{
    ContractError, ContractResult, MAX_PARAGRAPHS, MAX_TEXT_CHARS, MAX_TOTAL_TEXT_BYTES, TextScope, TextTarget, decode_bounded,
};
use deckcraft_model::Presentation;
use serde::{Deserialize, Serialize};

pub const MAX_CONTEXT_CHARS: usize = 4096;
pub const MAX_FACTS: usize = 64;
pub const MAX_CONSTRAINTS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmedFact {
    pub text: String,
    /// User-confirmed source label; not a URL the provider is allowed to fetch.
    pub source: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextIntent {
    pub instruction: String,
    pub audience: String,
    pub goal: String,
    pub narrative_summary: String,
    pub confirmed_facts: Vec<ConfirmedFact>,
    pub forbidden_changes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextInput {
    pub target: TextTarget,
    /// The target's responsibility in the deck, provided by the host/user.
    pub responsibility: String,
    pub paragraphs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextRequest {
    pub scope: TextScope,
    pub intent: TextIntent,
    pub inputs: Vec<TextInput>,
}

impl TextRequest {
    pub fn decode(bytes: &[u8], doc: &Presentation) -> ContractResult<Self> {
        let request: Self = decode_bounded(bytes)?;
        request.validate(doc)?;
        Ok(request)
    }

    /// Validate both typed and wire input. No file path, media, credentials, full document or
    /// conversational history is included. Content is data, not authority to expand this scope.
    pub fn validate(&self, doc: &Presentation) -> ContractResult<()> {
        self.scope.validate(doc)?;
        if self.inputs.len() != self.scope.editable.len() {
            return Err(ContractError::UnauthorizedTarget);
        }
        let mut total = 0usize;
        self.intent.validate_with_total(&mut total)?;
        let mut seen = std::collections::BTreeSet::new();
        for input in &self.inputs {
            if !self.scope.editable.contains(&input.target) || !seen.insert(input.target) {
                return Err(ContractError::UnauthorizedTarget);
            }
            add_text(&input.responsibility, MAX_CONTEXT_CHARS, &mut total)?;
            if input.paragraphs.is_empty() || input.paragraphs.len() > MAX_PARAGRAPHS {
                return Err(ContractError::Limit("paragraphs"));
            }
            let mut chars = 0usize;
            for p in &input.paragraphs {
                add_text(p, MAX_TEXT_CHARS, &mut total)?;
                chars = chars.checked_add(p.chars().count()).ok_or(ContractError::Limit("text characters"))?;
                if chars > MAX_TEXT_CHARS {
                    return Err(ContractError::Limit("text characters"));
                }
            }
            let body = doc
                .slide(input.target.slide_id)
                .and_then(|s| s.shapes.iter().find(|s| s.id == input.target.shape_id))
                .and_then(|s| s.text.as_ref())
                .ok_or(ContractError::UnknownShape(input.target))?;
            if body.paragraphs.len() != input.paragraphs.len()
                || body.paragraphs.iter().zip(&input.paragraphs).any(|(p, supplied)| !p.runs.iter().flat_map(|r| r.text.chars()).eq(supplied.chars()))
            {
                return Err(ContractError::SnapshotMismatch);
            }
        }
        Ok(())
    }
}

impl TextIntent {
    pub(crate) fn validate_with_total(&self, total: &mut usize) -> ContractResult<()> {
        if self.instruction.trim().is_empty() {
            return Err(ContractError::Identity("instruction"));
        }
        if self.confirmed_facts.len() > MAX_FACTS || self.forbidden_changes.len() > MAX_CONSTRAINTS {
            return Err(ContractError::Limit("intent entries"));
        }
        for text in [&self.instruction, &self.audience, &self.goal, &self.narrative_summary] {
            add_text(text, MAX_CONTEXT_CHARS, total)?;
        }
        for fact in &self.confirmed_facts {
            if fact.text.trim().is_empty() {
                return Err(ContractError::Identity("fact"));
            }
            add_text(&fact.text, MAX_CONTEXT_CHARS, total)?;
            if let Some(source) = &fact.source {
                add_text(source, MAX_CONTEXT_CHARS, total)?;
            }
        }
        for text in &self.forbidden_changes {
            add_text(text, MAX_CONTEXT_CHARS, total)?;
        }
        Ok(())
    }
}

pub(crate) fn add_text(text: &str, max_chars: usize, total: &mut usize) -> ContractResult<()> {
    if text.len() > MAX_TOTAL_TEXT_BYTES || text.chars().count() > max_chars {
        return Err(ContractError::Limit("request text"));
    }
    *total = total.checked_add(text.len()).ok_or(ContractError::Limit("request bytes"))?;
    if *total > MAX_TOTAL_TEXT_BYTES {
        return Err(ContractError::Limit("request bytes"));
    }
    Ok(())
}
