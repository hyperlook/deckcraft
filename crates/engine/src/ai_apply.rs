//! Accept one AI proposal onto the page the user cited.
//!
//! The candidate is built aside. The live deck changes only when the whole result is ready, and
//! then as a single undo step. Hand edits are not merged and do not reject the proposal. If the
//! cited page has already been deleted, nothing is written.

use std::sync::Arc;

use deckcraft_model::text::{Paragraph, Run, TextBody};
use deckcraft_model::{Material, Outline, PhType, Presentation, Shape, ShapeKind, SlideId};
use serde::Serialize;
use serde_json::Value;

use crate::ai_contract::{ContractError, PageProposal};
use crate::{EngineError, HistoryEntry, Selection, Session, clamp_selection, push_undo};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ApplyError {
    #[error("the document is not open")]
    DocumentClosed,
    #[error("the cited page is gone")]
    PageGone,
    #[error("this page has no text to replace")]
    NoStory,
    #[error("a pointer edit is still in progress")]
    InteractionPending,
    #[error("{0}")]
    Contract(ContractError),
    #[error("internal error (the presentation was kept as it was)")]
    Internal,
}

impl From<ContractError> for ApplyError {
    fn from(err: ContractError) -> Self {
        ApplyError::Contract(err)
    }
}

impl From<ApplyError> for EngineError {
    fn from(err: ApplyError) -> Self {
        match err {
            ApplyError::Internal => EngineError::Internal("ai.accept".into(), "the presentation was kept as it was".into()),
            other => EngineError::Other(other.to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptOutcome {
    pub doc_uid: u64,
    pub page_id: SlideId,
    pub changed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreOutcome {
    pub doc_uid: u64,
    pub changed: bool,
}

impl Session {
    /// Replace the cited page's title and body, and record the same words in the outline.
    ///
    /// `doc_uid` selects the deck, not whichever tab is in front. A missing page or a pointer
    /// gesture still in progress leaves the deck untouched.
    pub fn accept_page(&mut self, proposal: PageProposal) -> Result<AcceptOutcome, ApplyError> {
        proposal.validate()?;
        let doc_uid = proposal.doc_uid;
        let page_id = proposal.page_id;
        let label = accept_label(&proposal);
        self.guarded_edit("ai.accept", doc_uid, label, move |doc| install_page(doc, &proposal)).map(|changed| AcceptOutcome {
            doc_uid,
            page_id,
            changed,
        })
    }

    /// Store the outline the user edited. This does not add, delete, or reorder slides.
    pub fn set_outline(&mut self, doc_uid: u64, outline: Outline) -> Result<StoreOutcome, ApplyError> {
        crate::ai_contract::validate_outline(&outline)?;
        self.guarded_edit("ai.setOutline", doc_uid, "AI: 修改大纲".into(), move |doc| {
            doc.outline = outline;
            Ok(())
        })
        .map(|changed| StoreOutcome { doc_uid, changed })
    }

    /// Store the materials for this deck. Bodies are text the host already extracted, not paths.
    pub fn set_materials(&mut self, doc_uid: u64, materials: Vec<Material>) -> Result<StoreOutcome, ApplyError> {
        crate::ai_contract::validate_materials(&materials)?;
        self.guarded_edit("ai.setMaterials", doc_uid, "AI: 修改素材".into(), move |doc| {
            doc.materials = materials;
            Ok(())
        })
        .map(|changed| StoreOutcome { doc_uid, changed })
    }

    fn guarded_edit(
        &mut self,
        id: &str,
        doc_uid: u64,
        label: String,
        edit: impl FnOnce(&mut Presentation) -> Result<(), ApplyError>,
    ) -> Result<bool, ApplyError> {
        let mut slot: Option<Result<bool, ApplyError>> = None;
        let guarded = self.guarded(id, |session| {
            let result = commit_doc(session, doc_uid, &label, edit);
            let engine = match &result {
                Ok(_) => Ok(Value::Null),
                Err(err) => Err(EngineError::from(err.clone())),
            };
            slot = Some(result);
            engine
        });
        if let Some(result) = slot {
            return result;
        }
        match guarded {
            Err(EngineError::Internal(_, message)) => {
                log::error!("{id} panicked: {message}");
                Err(ApplyError::Internal)
            }
            _ => Err(ApplyError::Internal),
        }
    }
}

fn accept_label(proposal: &PageProposal) -> String {
    if !proposal.label.is_empty() {
        return proposal.label.clone();
    }
    let title: String = if proposal.title.is_empty() { "这一页".into() } else { proposal.title.chars().take(80).collect() };
    format!("AI: 更新「{title}」")
}

fn commit_doc(
    session: &mut Session,
    doc_uid: u64,
    label: &str,
    edit: impl FnOnce(&mut Presentation) -> Result<(), ApplyError>,
) -> Result<bool, ApplyError> {
    let index = session.doc_index_by_uid(doc_uid).ok_or(ApplyError::DocumentClosed)?;
    let state = session.doc_at_mut(index).ok_or(ApplyError::DocumentClosed)?;
    if state.interaction.is_some() {
        return Err(ApplyError::InteractionPending);
    }
    let mut candidate = (*state.doc).clone();
    edit(&mut candidate)?;
    if candidate == *state.doc {
        return Ok(false);
    }
    let previous = Arc::clone(&state.doc);
    let previous_selection = state.selection.clone();
    state.doc = Arc::new(candidate);
    clamp_text_caret(&mut state.selection, &state.doc);
    state.revision = state.revision.saturating_add(1);
    push_undo(state, HistoryEntry { label: label.to_string(), doc: previous, selection: previous_selection });
    clamp_selection(state);
    Ok(true)
}

/// Title and body placeholders carry the new story. Extra body columns are cleared so an old
/// claim does not stay behind. Pictures, tables, groups, and footer placeholders are left alone.
fn install_page(doc: &mut Presentation, proposal: &PageProposal) -> Result<(), ApplyError> {
    let index = doc.slides.iter().position(|slide| slide.id == proposal.page_id).ok_or(ApplyError::PageGone)?;
    let (title_at, bodies) = story_slots(&doc.slides[index].shapes);
    if title_at.is_none() && bodies.is_empty() {
        return Err(ApplyError::NoStory);
    }
    {
        let slide = Arc::make_mut(&mut doc.slides[index]);
        if let Some(title_at) = title_at {
            if bodies.is_empty() {
                let mut lines = title_line(&proposal.title);
                lines.extend(proposal.points.iter().cloned());
                write_lines(&mut slide.shapes[title_at], &lines);
            } else {
                write_lines(&mut slide.shapes[title_at], &title_line(&proposal.title));
            }
        }
        if let Some(first) = bodies.first().copied() {
            if title_at.is_none() {
                let mut lines = title_line(&proposal.title);
                lines.extend(proposal.points.iter().cloned());
                write_lines(&mut slide.shapes[first], &lines);
            } else {
                write_lines(&mut slide.shapes[first], &proposal.points);
            }
            for extra in bodies.iter().skip(1).copied() {
                write_lines(&mut slide.shapes[extra], &[]);
            }
        }
    }
    doc.outline.upsert(proposal.page_id, proposal.title.clone(), proposal.points.clone());
    Ok(())
}

fn title_line(title: &str) -> Vec<String> {
    if title.is_empty() { Vec::new() } else { vec![title.to_string()] }
}

fn story_slots(shapes: &[Shape]) -> (Option<usize>, Vec<usize>) {
    let title = shapes.iter().position(|shape| writable(shape) && shape.ph_type().is_some_and(PhType::is_title));
    let mut bodies: Vec<usize> = shapes
        .iter()
        .enumerate()
        .filter(|(_, shape)| writable(shape) && matches!(shape.ph_type(), Some(PhType::Body | PhType::Obj)))
        .map(|(index, _)| index)
        .collect();
    if bodies.is_empty() {
        bodies = shapes
            .iter()
            .enumerate()
            .filter(|(_, shape)| writable(shape) && shape.ph_type() == Some(PhType::SubTitle))
            .map(|(index, _)| index)
            .collect();
    }
    if title.is_none()
        && bodies.is_empty()
        && let Some(index) = shapes.iter().position(|shape| writable(shape) && shape.ph.is_none() && shape.text.is_some())
    {
        bodies.push(index);
    }
    (title, bodies)
}

fn writable(shape: &Shape) -> bool {
    matches!(shape.kind, ShapeKind::Shape) && !shape.locked && shape.ph_type().is_none_or(|kind| !kind.is_footer_kind())
}

fn write_lines(shape: &mut Shape, lines: &[String]) {
    let body = shape.text.get_or_insert_with(TextBody::default);
    let sample = body.paragraphs.first().cloned();
    let level = sample.as_ref().map(|paragraph| paragraph.level).unwrap_or(0);
    let props = sample.as_ref().map(|paragraph| paragraph.props.clone()).unwrap_or_default();
    let end_props = sample.as_ref().map(|paragraph| paragraph.end_props.clone()).unwrap_or_default();
    let run_props = sample.as_ref().and_then(|paragraph| paragraph.runs.first().map(|run| run.props.clone())).unwrap_or_default();
    if lines.is_empty() {
        body.paragraphs = vec![Paragraph { level, props, runs: vec![], end_props }];
        return;
    }
    body.paragraphs = lines
        .iter()
        .map(|line| Paragraph {
            level,
            props: props.clone(),
            runs: if line.is_empty() { vec![] } else { vec![Run::with(line.clone(), run_props.clone())] },
            end_props: end_props.clone(),
        })
        .collect();
}

fn clamp_text_caret(selection: &mut Selection, doc: &Presentation) {
    let Some(text) = selection.text.clone() else { return };
    if text.notes {
        return;
    }
    let Some(shape) = doc.slides.iter().find_map(|slide| slide.shape(text.shape)) else {
        selection.text = None;
        return;
    };
    let Some(body) = shape.text.as_ref() else {
        selection.text = None;
        return;
    };
    let fits = |at: (usize, usize)| body.paragraphs.get(at.0).is_some_and(|paragraph| at.1 <= paragraph.char_len());
    if !fits(text.anchor) || !fits(text.caret) {
        selection.text = None;
    }
}

#[cfg(test)]
#[path = "ai_apply_tests.rs"]
mod tests;
