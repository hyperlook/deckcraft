//! One AI round: the agreed outline, the page the user cited, their comment, and the materials.
//!
//! Nothing here writes the deck. Accepting a proposal is [`crate::ai_apply`]. The old text-box
//! patch, paragraph-count lock, content-revision conflict, and template-append protocol are gone.

use deckcraft_model::{ContextDraft, Material, Outline, OutlinePage, SlideId};
use serde::{Deserialize, Serialize};

pub const MAX_OUTLINE_PAGES: usize = 64;
pub const MAX_POINTS: usize = 32;
pub const MAX_TITLE_CHARS: usize = 500;
pub const MAX_POINT_CHARS: usize = 2_000;
pub const MAX_COMMENT_CHARS: usize = 8_000;
pub const MAX_MATERIALS: usize = 32;
pub const MAX_MATERIAL_NAME_CHARS: usize = 200;
pub const MAX_MATERIAL_BODY_CHARS: usize = 32_000;
pub const MAX_LABEL_CHARS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ContractError {
    #[error("invalid text")]
    Schema,
    #[error("limit exceeded: {0}")]
    Limit(&'static str),
    #[error("the target page needs confirmed context first")]
    MissingContext,
    #[error("duplicate outline page")]
    DuplicatePage,
    #[error("duplicate material name")]
    DuplicateMaterial,
}

pub type ContractResult<T> = Result<T, ContractError>;

/// A proposal for the cited page. The number of points is whatever the new page needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageProposal {
    pub doc_uid: u64,
    pub page_id: SlideId,
    pub title: String,
    pub points: Vec<String>,
    /// Empty means the engine names the undo step from the title.
    #[serde(default)]
    pub label: String,
}

impl PageProposal {
    pub fn validate(&self) -> ContractResult<()> {
        if self.doc_uid == 0 || self.page_id.0 == 0 {
            return Err(ContractError::Schema);
        }
        plain_line(&self.title, MAX_TITLE_CHARS, "title")?;
        if self.points.len() > MAX_POINTS {
            return Err(ContractError::Limit("points"));
        }
        for point in &self.points {
            plain_line(point, MAX_POINT_CHARS, "point")?;
        }
        if self.label.chars().count() > MAX_LABEL_CHARS || self.label.chars().any(char::is_control) {
            return Err(ContractError::Schema);
        }
        Ok(())
    }
}

pub fn validate_comment(comment: &str) -> ContractResult<()> {
    if comment.chars().count() > MAX_COMMENT_CHARS || has_disallowed_control(comment) {
        return Err(ContractError::Schema);
    }
    Ok(())
}

pub fn validate_outline(outline: &Outline) -> ContractResult<()> {
    if outline.pages.len() > MAX_OUTLINE_PAGES {
        return Err(ContractError::Limit("outline pages"));
    }
    let mut seen = Vec::new();
    for page in &outline.pages {
        validate_outline_page(page)?;
        if let Some(id) = page.page_id {
            if seen.contains(&id) {
                return Err(ContractError::DuplicatePage);
            }
            seen.push(id);
        }
    }
    Ok(())
}

pub fn validate_materials(materials: &[Material]) -> ContractResult<()> {
    if materials.len() > MAX_MATERIALS {
        return Err(ContractError::Limit("materials"));
    }
    let mut seen = Vec::new();
    for material in materials {
        validate_material(material)?;
        if seen.iter().any(|name: &String| name == &material.name) {
            return Err(ContractError::DuplicateMaterial);
        }
        seen.push(material.name.clone());
    }
    Ok(())
}

/// Persistence has the same bounds as facts, with separately bounded provenance.
pub fn validate_context(draft: &ContextDraft) -> ContractResult<()> {
    if draft.pages.len() > MAX_OUTLINE_PAGES || draft.diagnostics.len() > 16 {
        return Err(ContractError::Limit("context pages / diagnostics"));
    }
    validate_materials(&draft.materials)?;
    let mut seen = Vec::new();
    for page in &draft.pages {
        if page.page_id.0 == 0 || seen.contains(&page.page_id) {
            return Err(ContractError::DuplicatePage);
        }
        seen.push(page.page_id);
        plain_line(&page.title, MAX_TITLE_CHARS, "title")?;
        if page.points.len() > MAX_POINTS {
            return Err(ContractError::Limit("points"));
        }
        for point in &page.points {
            plain_line(point, MAX_POINT_CHARS, "point")?;
        }
        if !page.unknown && page.title.trim().is_empty() {
            return Err(ContractError::Schema);
        }
        if page.sources.len() > 64 || page.diagnostics.len() > 16 {
            return Err(ContractError::Limit("context sources / diagnostics"));
        }
        for source in &page.sources {
            if source.shape_path.is_empty() || source.shape_path.len() > 16 || source.shape_path.iter().any(|id| id.0 == 0) || source.paragraph > 4096
            {
                return Err(ContractError::Schema);
            }
            plain_line(&source.text, MAX_POINT_CHARS, "source")?;
        }
    }
    Ok(())
}

fn validate_outline_page(page: &OutlinePage) -> ContractResult<()> {
    if page.page_id.is_some_and(|id| id.0 == 0) {
        return Err(ContractError::Schema);
    }
    plain_line(&page.title, MAX_TITLE_CHARS, "title")?;
    if page.points.len() > MAX_POINTS {
        return Err(ContractError::Limit("points"));
    }
    for point in &page.points {
        plain_line(point, MAX_POINT_CHARS, "point")?;
    }
    Ok(())
}

fn validate_material(material: &Material) -> ContractResult<()> {
    if material.name.is_empty() || material.name.chars().count() > MAX_MATERIAL_NAME_CHARS || material.name.chars().any(char::is_control) {
        return Err(ContractError::Schema);
    }
    if material.body.chars().count() > MAX_MATERIAL_BODY_CHARS || has_disallowed_control(&material.body) {
        return Err(ContractError::Schema);
    }
    Ok(())
}

/// A title or a point is one line. Newlines belong in a separate point.
fn plain_line(text: &str, max_chars: usize, what: &'static str) -> ContractResult<()> {
    if text.chars().count() > max_chars || text.chars().any(char::is_control) {
        return Err(ContractError::Limit(what));
    }
    Ok(())
}

/// Comments and material text may contain newlines. Other control characters are rejected.
fn has_disallowed_control(text: &str) -> bool {
    text.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
}
