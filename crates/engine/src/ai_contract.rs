//! Read-only contracts for AI edits. No Session mutation or history is performed here.
//!
//! A scope is frozen by the host, never granted by a provider response. [`crate::Session::apply_ai`]
//! keeps the independent content revision and revalidates these contracts before installing a
//! candidate. Template append has a separate host-owned resource bundle and restricted wire protocol.

#[path = "ai_contract_template.rs"]
pub mod template;

use std::collections::BTreeSet;

use deckcraft_model::text::RunKind;
use deckcraft_model::{Presentation, ShapeId, ShapeKind, SlideId};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const MAX_WIRE_BYTES: usize = 256 * 1024;
pub const MAX_TARGET_SLIDES: usize = 64;
pub const MAX_TEXT_TARGETS: usize = 256;
pub const MAX_PATCH_OPS: usize = 256;
pub const MAX_PARAGRAPHS: usize = 128;
pub const MAX_RUNS_PER_PARAGRAPH: usize = 128;
pub const MAX_TEXT_CHARS: usize = 16_384;
pub const MAX_TOTAL_TEXT_BYTES: usize = 128 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ContractError {
    #[error("invalid JSON schema")]
    Schema,
    #[error("limit exceeded: {0}")]
    Limit(&'static str),
    #[error("invalid identity: {0}")]
    Identity(&'static str),
    #[error("duplicate target")]
    DuplicateTarget,
    #[error("response does not match the frozen snapshot")]
    SnapshotMismatch,
    #[error("target is outside the frozen scope")]
    UnauthorizedTarget,
    #[error("unknown slide {0}")]
    UnknownSlide(SlideId),
    #[error("unknown shape {0:?}")]
    UnknownShape(TextTarget),
    #[error("text target is unsupported: {0:?}")]
    UnsupportedText(TextTarget),
    #[error("paragraph structure must be preserved")]
    ParagraphStructure,
    #[error("paragraph contains control characters")]
    ControlCharacter,
    #[error("invalid template resource graph or local mapping")]
    ResourceGraph,
    #[error("unsupported template content")]
    UnsupportedTemplate,
}

pub type ContractResult<T> = std::result::Result<T, ContractError>;

/// Check byte size *before* serde can allocate strings/vectors. Serde's recursion limit remains
/// enabled. Callers must also cap the transport body while reading it, not only after buffering.
pub fn decode_bounded<T: DeserializeOwned>(bytes: &[u8]) -> ContractResult<T> {
    if bytes.len() > MAX_WIRE_BYTES {
        return Err(ContractError::Limit("wire bytes"));
    }
    serde_json::from_slice(bytes).map_err(|_| ContractError::Schema)
}

pub fn validate_key(key: &str) -> ContractResult<()> {
    if key.is_empty() || key.len() > 64 || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(ContractError::Identity("key"));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotIdentity {
    pub job_id: String,
    pub doc_uid: u64,
    pub base_content_revision: u64,
    pub target_slide_ids: Vec<SlideId>,
    pub plan_version: u64,
    pub template_version: Option<u64>,
}

impl SnapshotIdentity {
    pub fn validate(&self) -> ContractResult<()> {
        self.validate_for_append()?;
        if self.target_slide_ids.is_empty() {
            return Err(ContractError::Limit("target slides"));
        }
        Ok(())
    }

    /// Empty targets are valid only for append/generation, never for text edit authorization.
    pub fn validate_for_append(&self) -> ContractResult<()> {
        validate_key(&self.job_id)?;
        if self.doc_uid == 0 || self.base_content_revision == 0 || self.plan_version == 0 || self.template_version == Some(0) {
            return Err(ContractError::Identity("version or document"));
        }
        if self.target_slide_ids.len() > MAX_TARGET_SLIDES {
            return Err(ContractError::Limit("target slides"));
        }
        let mut seen = BTreeSet::new();
        for id in &self.target_slide_ids {
            if id.0 == 0 {
                return Err(ContractError::Identity("slide"));
            }
            if !seen.insert(*id) {
                return Err(ContractError::DuplicateTarget);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextTarget {
    pub slide_id: SlideId,
    pub shape_id: ShapeId,
}

/// Host-owned authorization, separate from the returned patch. Selecting a slide does not
/// authorize every shape on it. The host lists only user-approved, safely replaceable fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextScope {
    pub snapshot: SnapshotIdentity,
    pub editable: Vec<TextTarget>,
}

impl TextScope {
    pub fn validate(&self, doc: &Presentation) -> ContractResult<()> {
        self.snapshot.validate()?;
        if self.editable.is_empty() || self.editable.len() > MAX_TEXT_TARGETS {
            return Err(ContractError::Limit("text targets"));
        }
        for id in &self.snapshot.target_slide_ids {
            let mut matches = doc.slides.iter().filter(|s| s.id == *id);
            if matches.next().is_none() {
                return Err(ContractError::UnknownSlide(*id));
            }
            if matches.next().is_some() {
                return Err(ContractError::DuplicateTarget);
            }
        }
        let mut seen = BTreeSet::new();
        for target in &self.editable {
            if target.shape_id.0 == 0 {
                return Err(ContractError::Identity("shape"));
            }
            if !self.snapshot.target_slide_ids.contains(&target.slide_id) {
                return Err(ContractError::UnauthorizedTarget);
            }
            if !seen.insert(*target) {
                return Err(ContractError::DuplicateTarget);
            }
            safe_paragraph_count(doc, *target)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum TextPatchOp {
    /// Plain paragraph strings, not Markdown or rich text. Preserve paragraph count and each
    /// paragraph's existing formatting; P1 must not implement this by calling lossy `text.set`.
    ReplaceText { target: TextTarget, paragraphs: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextPatch {
    pub snapshot: SnapshotIdentity,
    pub operations: Vec<TextPatchOp>,
}

impl TextPatch {
    pub fn decode(bytes: &[u8], scope: &TextScope, doc: &Presentation) -> ContractResult<Self> {
        let patch: Self = decode_bounded(bytes)?;
        patch.validate(scope, doc)?;
        Ok(patch)
    }

    /// Pure validation, usable again at the engine boundary even for in-process typed input.
    /// This does NOT check a live document revision: the host supplies the frozen scope and P1
    /// must compare its identity with the live DocState before preparing any candidate.
    pub fn validate(&self, scope: &TextScope, doc: &Presentation) -> ContractResult<()> {
        scope.validate(doc)?;
        self.snapshot.validate()?;
        if self.snapshot != scope.snapshot {
            return Err(ContractError::SnapshotMismatch);
        }
        if self.operations.len() > MAX_PATCH_OPS {
            return Err(ContractError::Limit("patch operations"));
        }
        let mut seen = BTreeSet::new();
        let mut total_bytes = 0usize;
        for TextPatchOp::ReplaceText { target, paragraphs } in &self.operations {
            if !scope.editable.contains(target) {
                return Err(ContractError::UnauthorizedTarget);
            }
            if !seen.insert(*target) {
                return Err(ContractError::DuplicateTarget);
            }
            if paragraphs.len() != safe_paragraph_count(doc, *target)? {
                return Err(ContractError::ParagraphStructure);
            }
            let mut chars = 0usize;
            for paragraph in paragraphs {
                total_bytes = total_bytes.checked_add(paragraph.len()).ok_or(ContractError::Limit("text bytes"))?;
                if total_bytes > MAX_TOTAL_TEXT_BYTES {
                    return Err(ContractError::Limit("text bytes"));
                }
                chars = chars.checked_add(paragraph.chars().count()).ok_or(ContractError::Limit("text characters"))?;
                if chars > MAX_TEXT_CHARS {
                    return Err(ContractError::Limit("text characters"));
                }
                if paragraph.chars().any(char::is_control) {
                    return Err(ContractError::ControlCharacter);
                }
            }
        }
        Ok(())
    }
}

fn safe_paragraph_count(doc: &Presentation, target: TextTarget) -> ContractResult<usize> {
    let slide = doc.slide(target.slide_id).ok_or(ContractError::UnknownSlide(target.slide_id))?;
    // This slice supports top-level shapes only. Group/table/notes/master edits need a separate
    // explicit contract. Reject ambiguous duplicate IDs rather than picking the first match.
    let mut matches = slide.shapes.iter().filter(|s| s.id == target.shape_id);
    let shape = matches.next().ok_or(ContractError::UnknownShape(target))?;
    if matches.next().is_some() || shape.locked || shape.hidden || !matches!(shape.kind, ShapeKind::Shape) {
        return Err(ContractError::UnsupportedText(target));
    }
    let body = shape.text.as_ref().ok_or(ContractError::UnsupportedText(target))?;
    if body.paragraphs.is_empty() || body.paragraphs.len() > MAX_PARAGRAPHS {
        return Err(ContractError::UnsupportedText(target));
    }
    let mut text_bytes = 0usize;
    for p in &body.paragraphs {
        if p.runs.len() > MAX_RUNS_PER_PARAGRAPH || p.end_props.link.is_some() {
            return Err(ContractError::UnsupportedText(target));
        }
        let props = p.runs.first().map(|r| &r.props);
        for r in &p.runs {
            if r.kind != RunKind::Text || r.props.link.is_some() || Some(&r.props) != props {
                return Err(ContractError::UnsupportedText(target));
            }
            text_bytes = text_bytes.checked_add(r.text.len()).ok_or(ContractError::Limit("source text bytes"))?;
            if text_bytes > MAX_TOTAL_TEXT_BYTES {
                return Err(ContractError::Limit("source text bytes"));
            }
        }
    }
    Ok(body.paragraphs.len())
}
