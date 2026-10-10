//! Atomic AI commit. Candidate work never touches the live document, history, selection, or the
//! applied-job ledger. A successful change is installed once and recorded as one undo step.
//!
//! The ledger is session state, not document state: undo restores the pre-commit document but
//! does not forget that the `job_id` already ran.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use deckcraft_model::text::{Action, Bullet, Hyperlink, ListStyle, Paragraph, Run, RunProps, TextBody};
use deckcraft_model::{Background, Fill, LayoutId, Master, MasterId, MediaId, MediaItem, Presentation, Shape, ShapeId, ShapeKind, Slide, SlideId};
use serde::Serialize;
use serde_json::Value;

use crate::ai_contract::template::{
    AppendDraft, LocalId, MAX_GROUP_DEPTH, MAX_MEDIA_BYTES, MAX_TOTAL_MEDIA_BYTES, PageInstance, ShapeOwner, SourceResource,
};
use crate::ai_contract::{ContractError, TextPatch, TextPatchOp, TextScope, TextTarget};
use crate::{DocState, EngineError, HistoryEntry, Selection, Session, Target, bump_content_revision, clamp_selection, push_undo};

/// Successful commits remembered for one document session. Evicting an entry would let a late
/// retry add pages again, so a full ledger refuses new jobs instead.
pub const MAX_APPLIED_JOBS: usize = 256;
const MAX_LABEL_CHARS: usize = 256;
const MAX_REF_DEPTH: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ApplyError {
    #[error("ai apply: document is not open")]
    DocumentClosed,
    #[error("ai apply: content conflict")]
    Conflict,
    #[error("ai apply: pointer interaction in progress")]
    InteractionPending,
    #[error("ai apply: applied job ledger is full")]
    LedgerFull,
    #[error("ai apply: invalid label")]
    BadLabel,
    #[error("ai apply: {0}")]
    Contract(String),
    #[error("ai apply: commit panicked")]
    Internal,
}

impl From<ApplyError> for EngineError {
    fn from(err: ApplyError) -> Self {
        EngineError::Other(err.to_string())
    }
}

impl From<ContractError> for ApplyError {
    fn from(err: ContractError) -> Self {
        ApplyError::Contract(err.to_string())
    }
}

/// One complete change. Text and template append are separate commits; each is one history step.
#[derive(Clone, Debug)]
pub enum AiCommit {
    Text { scope: TextScope, patch: TextPatch, label: String },
    Append { draft: AppendDraft, label: String },
}

impl AiCommit {
    fn snapshot(&self) -> &crate::ai_contract::SnapshotIdentity {
        match self {
            Self::Text { scope, .. } => &scope.snapshot,
            Self::Append { draft, .. } => draft.scope().snapshot(),
        }
    }

    fn label(&self) -> &str {
        match self {
            Self::Text { label, .. } | Self::Append { label, .. } => label,
        }
    }
}

/// Result of a commit. `duplicate` means this call did not write again.
/// `content_revision` is the revision produced by the original commit, not a later undo.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyOutcome {
    pub job_id: String,
    pub doc_uid: u64,
    pub content_revision: u64,
    pub changed: bool,
    pub duplicate: bool,
    pub label: String,
    pub added_slide_ids: Vec<SlideId>,
}

/// Per-document record of commits that already succeeded, including no-op commits.
#[derive(Clone, Debug, Default)]
pub struct AppliedJobs {
    entries: Vec<ApplyOutcome>,
}

impl AppliedJobs {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, job_id: &str) -> Option<&ApplyOutcome> {
        self.entries.iter().find(|entry| entry.job_id == job_id)
    }

    fn insert(&mut self, outcome: ApplyOutcome) {
        if self.get(&outcome.job_id).is_none() && self.entries.len() < MAX_APPLIED_JOBS {
            self.entries.push(outcome);
        }
    }
}

struct Prepared {
    label: String,
    added_slide_ids: Vec<SlideId>,
    edited_shapes: Vec<ShapeId>,
}

impl Session {
    /// Install one AI change on the document named by the snapshot, not the active tab.
    ///
    /// Runs inside the panic guard. Candidate construction is local; only a fully prepared result
    /// is swapped in, and only then is history and the ledger updated.
    pub fn apply_ai(&mut self, commit: AiCommit) -> Result<ApplyOutcome, ApplyError> {
        let mut outcome = None;
        let mut error = None;
        let guarded = self.guarded("ai.applyPatch", |session| match apply_unguarded(session, &commit) {
            Ok(value) => {
                outcome = Some(value);
                Ok(Value::Null)
            }
            Err(err) => {
                error = Some(err.clone());
                Err(EngineError::from(err))
            }
        });
        if let Some(value) = outcome {
            return Ok(value);
        }
        if let Some(err) = error {
            return Err(err);
        }
        match guarded {
            Err(EngineError::Internal(_, message)) => {
                log::error!("ai.applyPatch panicked: {message}");
                Err(ApplyError::Internal)
            }
            Err(err) => Err(ApplyError::Contract(err.to_string())),
            Ok(_) => Err(ApplyError::Internal),
        }
    }
}

fn apply_unguarded(session: &mut Session, commit: &AiCommit) -> Result<ApplyOutcome, ApplyError> {
    let snapshot = commit.snapshot();
    let index = session.doc_index_by_uid(snapshot.doc_uid).ok_or(ApplyError::DocumentClosed)?;
    if let Some(existing) = session.documents().get(index).and_then(|doc| doc.applied_jobs.get(&snapshot.job_id)) {
        let mut outcome = existing.clone();
        outcome.duplicate = true;
        return Ok(outcome);
    }
    let doc = session.documents().get(index).ok_or(ApplyError::DocumentClosed)?;
    if doc.applied_jobs.len() >= MAX_APPLIED_JOBS {
        return Err(ApplyError::LedgerFull);
    }
    if interaction_pending(session, index) {
        return Err(ApplyError::InteractionPending);
    }
    if doc.content_revision != snapshot.base_content_revision {
        return Err(ApplyError::Conflict);
    }
    if !commit.label().is_empty() {
        validate_label(commit.label())?;
    }

    let original = Arc::clone(&doc.doc);
    let old_selection = doc.selection.clone();
    let mut candidate = (*original).clone();
    let prepared = match commit {
        AiCommit::Text { scope, patch, label } => prepare_text(&mut candidate, scope, patch, label)?,
        AiCommit::Append { draft, label } => prepare_append(&mut candidate, draft, label)?,
    };
    if candidate == *original {
        let outcome = ApplyOutcome {
            job_id: snapshot.job_id.clone(),
            doc_uid: snapshot.doc_uid,
            content_revision: doc.content_revision,
            changed: false,
            duplicate: false,
            label: prepared.label,
            added_slide_ids: Vec::new(),
        };
        let doc = session.doc_at_mut(index).ok_or(ApplyError::DocumentClosed)?;
        if doc.applied_jobs.len() >= MAX_APPLIED_JOBS {
            return Err(ApplyError::LedgerFull);
        }
        doc.applied_jobs.insert(outcome.clone());
        return Ok(outcome);
    }

    let mut selection = old_selection;
    finish_selection(&mut selection, &candidate, &prepared);
    let doc = session.doc_at_mut(index).ok_or(ApplyError::DocumentClosed)?;
    if let Some(existing) = doc.applied_jobs.get(&snapshot.job_id) {
        let mut outcome = existing.clone();
        outcome.duplicate = true;
        return Ok(outcome);
    }
    if doc.applied_jobs.len() >= MAX_APPLIED_JOBS {
        return Err(ApplyError::LedgerFull);
    }
    let outcome = ApplyOutcome {
        job_id: snapshot.job_id.clone(),
        doc_uid: snapshot.doc_uid,
        content_revision: doc.content_revision.saturating_add(1),
        changed: true,
        duplicate: false,
        label: prepared.label.clone(),
        added_slide_ids: prepared.added_slide_ids.clone(),
    };
    install(doc, candidate, selection, &prepared.label, &outcome);
    Ok(outcome)
}

fn interaction_pending(session: &Session, index: usize) -> bool {
    let Some(doc) = session.documents().get(index) else { return true };
    doc.interaction.is_some() || (session.active_index() == Some(index) && session.tool.dragging())
}

fn validate_label(label: &str) -> Result<(), ApplyError> {
    let count = label.chars().count();
    if count == 0 || count > MAX_LABEL_CHARS || label.chars().any(char::is_control) {
        return Err(ApplyError::BadLabel);
    }
    Ok(())
}

fn resolve_label(requested: &str, generated: String) -> Result<String, ApplyError> {
    if requested.is_empty() {
        validate_label(&generated)?;
        return Ok(generated);
    }
    validate_label(requested)?;
    Ok(requested.to_string())
}

fn install(doc: &mut DocState, candidate: Presentation, selection: Selection, label: &str, outcome: &ApplyOutcome) {
    let previous = Arc::clone(&doc.doc);
    let previous_selection = doc.selection.clone();
    doc.doc = Arc::new(candidate);
    doc.selection = selection;
    // Clamp against the installed document. A panic after the swap is restored by the guard,
    // including history and the ledger, which is written only after the swap.
    clamp_selection(doc);
    bump_content_revision(doc);
    doc.revision = doc.revision.saturating_add(1);
    push_undo(doc, HistoryEntry { label: label.to_string(), doc: previous, selection: previous_selection });
    doc.applied_jobs.insert(outcome.clone());
}

fn finish_selection(selection: &mut Selection, doc: &Presentation, prepared: &Prepared) {
    if !prepared.added_slide_ids.is_empty() {
        selection.shapes.clear();
        selection.text = None;
        selection.cells = None;
        selection.slides = prepared.added_slide_ids.clone();
        selection.target = Target::Slides;
        if let Some(id) = prepared.added_slide_ids.last()
            && let Some(index) = doc.slide_index(*id)
        {
            selection.slide = index;
        }
        return;
    }
    if let Some(text) = &selection.text
        && prepared.edited_shapes.contains(&text.shape)
    {
        selection.text = None;
    }
}

fn prepare_text(candidate: &mut Presentation, scope: &TextScope, patch: &TextPatch, label: &str) -> Result<Prepared, ApplyError> {
    scope.validate(candidate)?;
    patch.validate(scope, candidate)?;
    let mut edited = Vec::new();
    let mut changed = 0usize;
    for (index, op) in patch.operations.iter().enumerate() {
        let TextPatchOp::ReplaceText { target, paragraphs } = op;
        if replace_text(candidate, *target, paragraphs)? {
            changed = changed.saturating_add(1);
            edited.push(target.shape_id);
        }
        if injected_text_failure(index) {
            return Err(ApplyError::Contract("injected failure".into()));
        }
    }
    let label = resolve_label(label, format!("AI: 更新 {changed} 处文本"))?;
    Ok(Prepared { label, added_slide_ids: Vec::new(), edited_shapes: edited })
}

fn replace_text(doc: &mut Presentation, target: TextTarget, paragraphs: &[String]) -> Result<bool, ApplyError> {
    let slide = doc.slide_mut(target.slide_id).ok_or(ContractError::UnknownSlide(target.slide_id))?;
    let indexes: Vec<usize> = slide.shapes.iter().enumerate().filter(|(_, shape)| shape.id == target.shape_id).map(|(index, _)| index).collect();
    let index = match indexes.as_slice() {
        [index] => *index,
        _ => return Err(ContractError::UnknownShape(target).into()),
    };
    let shape = slide.shapes.get_mut(index).ok_or(ContractError::UnknownShape(target))?;
    if shape.locked || shape.hidden || !matches!(shape.kind, ShapeKind::Shape) {
        return Err(ContractError::UnsupportedText(target).into());
    }
    let body = shape.text.as_mut().ok_or(ContractError::UnsupportedText(target))?;
    if body.paragraphs.len() != paragraphs.len() {
        return Err(ContractError::ParagraphStructure.into());
    }
    let mut changed = false;
    for (paragraph, text) in body.paragraphs.iter_mut().zip(paragraphs) {
        if paragraph.text() == *text {
            continue;
        }
        let props = paragraph.runs.first().map(|run| run.props.clone()).unwrap_or_else(|| paragraph.end_props.clone());
        if text.is_empty() {
            paragraph.runs.clear();
        } else {
            paragraph.runs = vec![Run::with(text.clone(), props)];
        }
        changed = true;
    }
    Ok(changed)
}

fn prepare_append(candidate: &mut Presentation, draft: &AppendDraft, label: &str) -> Result<Prepared, ApplyError> {
    draft.validate()?;
    draft.scope().validate_target(candidate)?;
    if candidate.slides.is_empty() {
        adopt_empty_globals(candidate, draft.scope().bundle().native())?;
    }
    let added = copy_instances(candidate, draft)?;
    let label = resolve_label(label, format!("AI: 追加 {} 页", added.len()))?;
    Ok(Prepared { label, added_slide_ids: added, edited_shapes: Vec::new() })
}

fn adopt_empty_globals(candidate: &mut Presentation, native: &Presentation) -> Result<(), ApplyError> {
    candidate.slide_size = native.slide_size;
    candidate.default_text_style = native.default_text_style.clone();
    candidate.header_footer = native.header_footer.clone();
    let globals = serde_json::to_value((&candidate.default_text_style, &candidate.header_footer))
        .map_err(|_| ApplyError::Contract("global settings".into()))?;
    let mut media = BTreeSet::new();
    let mut slides = BTreeSet::new();
    collect_refs(&globals, 0, &mut media, &mut slides)?;
    if !media.is_empty() || !slides.is_empty() {
        // A global style cannot choose a different copy per instance. Refuse instead of guessing.
        return Err(ApplyError::Contract("empty target global settings contain resource links".into()));
    }
    Ok(())
}

fn copy_instances(candidate: &mut Presentation, draft: &AppendDraft) -> Result<Vec<SlideId>, ApplyError> {
    let mut used = BTreeSet::new();
    seed_ids(candidate, &mut used);
    let mut dest_of_local = BTreeMap::new();
    for mapping in draft.mappings() {
        let id = alloc_fresh(candidate, &mut used)?;
        if dest_of_local.insert(mapping.local, id).is_some() {
            return Err(ApplyError::Contract("duplicate local id".into()));
        }
    }
    let mut added = Vec::new();
    let mut copied_media = 0usize;
    for instance in draft.response().pages() {
        let id = copy_instance(candidate, draft, instance, &dest_of_local, &mut copied_media)?;
        added.push(id);
    }
    Ok(added)
}

fn copy_instance(
    candidate: &mut Presentation,
    draft: &AppendDraft,
    instance: &PageInstance,
    dest_of_local: &BTreeMap<LocalId, u32>,
    copied_media: &mut usize,
) -> Result<SlideId, ApplyError> {
    let native = draft.scope().bundle().native();
    let page = draft.scope().pages().iter().find(|page| page.key == instance.page_key).ok_or(ApplyError::Contract("unknown template page".into()))?;
    let source_slide = native.slide(page.source_slide_id).ok_or(ApplyError::Contract("missing template slide".into()))?;
    let (source_master, source_layout) = native.layout(source_slide.layout).ok_or(ApplyError::Contract("missing template layout".into()))?;
    let source_master_id = source_master.id;
    let source_layout_id = source_layout.id;
    copy_media(candidate, draft, native, instance.local_id, dest_of_local, copied_media)?;

    let master_id = MasterId(dest_resource(draft, dest_of_local, instance.local_id, SourceResource::Master { id: source_master_id })?);
    let layout_id = LayoutId(dest_resource(draft, dest_of_local, instance.local_id, SourceResource::Layout { id: source_layout_id })?);
    let slide_id = SlideId(dest_of_local.get(&instance.local_id).copied().ok_or(ApplyError::Contract("missing slide destination".into()))?);
    let mut master = source_master.clone();
    master.id = master_id;
    master.layouts.retain(|layout| layout.id == source_layout_id);
    if master.layouts.len() != 1 {
        return Err(ApplyError::Contract("template master kept extra layouts".into()));
    }
    let layout = master.layouts.first_mut().ok_or(ApplyError::Contract("missing selected layout".into()))?;
    layout.id = layout_id;

    let ctx = Remap { draft, dest_of_local, instance: instance.local_id };
    let master_shapes = ctx.shape_ids(ShapeOwner::Master { id: source_master_id })?;
    remap_shape_tree(&mut master.shapes, 0, &ctx, &master_shapes)?;
    let layout_shapes = ctx.shape_ids(ShapeOwner::Layout { id: source_layout_id })?;
    let layout = master.layouts.first_mut().ok_or(ApplyError::Contract("missing selected layout".into()))?;
    remap_shape_tree(&mut layout.shapes, 0, &ctx, &layout_shapes)?;
    if let Some(background) = &mut layout.background {
        remap_background(background, &ctx)?;
    }
    remap_master_refs(&mut master, &ctx)?;

    let mut slide = source_slide.clone();
    slide.id = slide_id;
    slide.layout = layout_id;
    apply_slots(&mut slide, draft, instance)?;
    let slide_shapes = ctx.shape_ids(ShapeOwner::Slide { id: page.source_slide_id })?;
    remap_shape_tree(&mut slide.shapes, 0, &ctx, &slide_shapes)?;
    remap_text_body(&mut slide.notes, &ctx)?;
    if let Some(background) = &mut slide.background {
        remap_background(background, &ctx)?;
    }
    verify_instance(candidate, &master, &slide, draft, instance.local_id, dest_of_local)?;
    candidate.masters.push(Arc::new(master));
    candidate.slides.push(Arc::new(slide));
    Ok(slide_id)
}

fn copy_media(
    candidate: &mut Presentation,
    draft: &AppendDraft,
    native: &Presentation,
    instance: LocalId,
    dest_of_local: &BTreeMap<LocalId, u32>,
    copied_media: &mut usize,
) -> Result<(), ApplyError> {
    for mapping in draft.mappings() {
        if mapping.instance != instance {
            continue;
        }
        let SourceResource::Media { id } = mapping.source else { continue };
        let source = native.media(id).ok_or(ApplyError::Contract("missing template media".into()))?;
        if source.link.is_some() || source.data.is_empty() {
            return Err(ApplyError::Contract("template media is not embedded".into()));
        }
        *copied_media = copied_media.checked_add(source.data.len()).ok_or(ApplyError::Contract("draft media bytes".into()))?;
        if source.data.len() > MAX_MEDIA_BYTES || *copied_media > MAX_TOTAL_MEDIA_BYTES {
            return Err(ApplyError::Contract("draft media bytes".into()));
        }
        let dest = dest_of_local.get(&mapping.local).copied().ok_or(ApplyError::Contract("missing media destination".into()))?;
        if candidate.media.iter().any(|item| item.id.0 == dest) {
            return Err(ApplyError::Contract("media id collision".into()));
        }
        candidate.media.push(MediaItem {
            id: MediaId(dest),
            name: source.name.clone(),
            content_type: source.content_type.clone(),
            data: Arc::clone(&source.data),
            link: None,
        });
    }
    Ok(())
}

fn apply_slots(slide: &mut Slide, draft: &AppendDraft, instance: &PageInstance) -> Result<(), ApplyError> {
    let page = draft.scope().pages().iter().find(|page| page.key == instance.page_key).ok_or(ApplyError::Contract("unknown template page".into()))?;
    // Replace by source shape id before destination ids are assigned.
    for replacement in &instance.replacements {
        let slot = page.slots.iter().find(|slot| slot.key == replacement.slot_key).ok_or(ApplyError::Contract("unknown slot".into()))?;
        replace_slide_text(slide, slot.shape_id, &replacement.paragraphs)?;
    }
    Ok(())
}

fn replace_slide_text(slide: &mut Slide, shape_id: ShapeId, paragraphs: &[String]) -> Result<(), ApplyError> {
    let indexes: Vec<usize> = slide.shapes.iter().enumerate().filter(|(_, shape)| shape.id == shape_id).map(|(index, _)| index).collect();
    let index = match indexes.as_slice() {
        [index] => *index,
        _ => return Err(ApplyError::Contract("slot shape is missing".into())),
    };
    let shape = slide.shapes.get_mut(index).ok_or(ApplyError::Contract("slot shape is missing".into()))?;
    let body = shape.text.as_mut().ok_or(ApplyError::Contract("slot has no text".into()))?;
    if body.paragraphs.len() != paragraphs.len() {
        return Err(ContractError::ParagraphStructure.into());
    }
    for (paragraph, text) in body.paragraphs.iter_mut().zip(paragraphs) {
        if paragraph.text() == *text {
            continue;
        }
        let props = paragraph.runs.first().map(|run| run.props.clone()).unwrap_or_else(|| paragraph.end_props.clone());
        if text.is_empty() {
            paragraph.runs.clear();
        } else {
            paragraph.runs = vec![Run::with(text.clone(), props)];
        }
    }
    Ok(())
}

struct Remap<'a> {
    draft: &'a AppendDraft,
    dest_of_local: &'a BTreeMap<LocalId, u32>,
    instance: LocalId,
}

impl Remap<'_> {
    fn media(&self, id: MediaId) -> Result<MediaId, ApplyError> {
        Ok(MediaId(dest_resource(self.draft, self.dest_of_local, self.instance, SourceResource::Media { id })?))
    }

    fn slide(&self, id: SlideId) -> Result<SlideId, ApplyError> {
        let instance = self
            .draft
            .response()
            .pages()
            .iter()
            .find(|page| page.local_id == self.instance)
            .ok_or(ApplyError::Contract("missing instance".into()))?;
        let mut found = None;
        for link in &instance.links {
            let page = self
                .draft
                .scope()
                .pages()
                .iter()
                .find(|page| page.key == link.target_page_key)
                .ok_or(ApplyError::Contract("unknown link page".into()))?;
            if page.source_slide_id != id {
                continue;
            }
            if found.is_some() {
                return Err(ApplyError::Contract("ambiguous slide link".into()));
            }
            let dest = self.dest_of_local.get(&link.target_local_id).copied().ok_or(ApplyError::Contract("missing link destination".into()))?;
            found = Some(SlideId(dest));
        }
        found.ok_or(ApplyError::Contract("slide link has no page link".into()))
    }

    fn shape_ids(&self, owner: ShapeOwner) -> Result<BTreeMap<ShapeId, ShapeId>, ApplyError> {
        let mut ids = BTreeMap::new();
        for mapping in self.draft.mappings() {
            if mapping.instance != self.instance {
                continue;
            }
            let SourceResource::Shape { owner: mapped, id } = mapping.source else { continue };
            if mapped != owner {
                continue;
            }
            let dest = self.dest_of_local.get(&mapping.local).copied().ok_or(ApplyError::Contract("missing shape destination".into()))?;
            if dest == 0 || ids.insert(id, ShapeId(dest)).is_some() {
                return Err(ApplyError::Contract("duplicate shape destination".into()));
            }
        }
        Ok(ids)
    }
}

fn dest_resource(draft: &AppendDraft, dest_of_local: &BTreeMap<LocalId, u32>, instance: LocalId, source: SourceResource) -> Result<u32, ApplyError> {
    let mapping = draft
        .mappings()
        .iter()
        .find(|mapping| mapping.instance == instance && mapping.source == source)
        .ok_or(ApplyError::Contract("missing resource mapping".into()))?;
    let id = dest_of_local.get(&mapping.local).copied().ok_or(ApplyError::Contract("missing destination id".into()))?;
    if id == 0 {
        return Err(ApplyError::Contract("destination id is zero".into()));
    }
    Ok(id)
}

fn remap_master_refs(master: &mut Master, ctx: &Remap<'_>) -> Result<(), ApplyError> {
    if master.raw_ext.is_some() || master.theme.raw_extra.is_some() {
        return Err(ApplyError::Contract("unsupported template master".into()));
    }
    for fill in master.theme.format.fills.iter_mut().chain(master.theme.format.bg_fills.iter_mut()) {
        remap_fill(fill, ctx)?;
    }
    for line in &mut master.theme.format.lines {
        if let Some(fill) = &mut line.fill {
            remap_fill(fill, ctx)?;
        }
    }
    if let Some(background) = &mut master.background {
        remap_background(background, ctx)?;
    }
    remap_list_style(&mut master.title_style, ctx)?;
    remap_list_style(&mut master.body_style, ctx)?;
    remap_list_style(&mut master.other_style, ctx)?;
    Ok(())
}

fn remap_shape_tree(shapes: &mut [Shape], depth: usize, ctx: &Remap<'_>, ids: &BTreeMap<ShapeId, ShapeId>) -> Result<(), ApplyError> {
    if depth > MAX_GROUP_DEPTH {
        return Err(ApplyError::Contract("group depth".into()));
    }
    for shape in shapes {
        let dest = ids.get(&shape.id).copied().ok_or(ApplyError::Contract("unmapped shape".into()))?;
        shape.id = dest;
        match &mut shape.kind {
            ShapeKind::Shape => {}
            ShapeKind::Picture { fill } => fill.media = ctx.media(fill.media)?,
            ShapeKind::Group { children, .. } => remap_shape_tree(children, depth.saturating_add(1), ctx, ids)?,
            _ => return Err(ApplyError::Contract("unsupported template shape".into())),
        }
        if let Some(fill) = &mut shape.fill {
            remap_fill(fill, ctx)?;
        }
        if let Some(line) = &mut shape.line
            && let Some(fill) = &mut line.fill
        {
            remap_fill(fill, ctx)?;
        }
        if let Some(link) = &mut shape.click {
            remap_link(link, ctx)?;
        }
        if let Some(link) = &mut shape.hover {
            remap_link(link, ctx)?;
        }
        if let Some(text) = &mut shape.text {
            remap_text_body(text, ctx)?;
        }
        if shape.raw_ext.is_some() {
            return Err(ApplyError::Contract("unsupported template shape".into()));
        }
    }
    Ok(())
}

fn remap_text_body(body: &mut TextBody, ctx: &Remap<'_>) -> Result<(), ApplyError> {
    remap_list_style(&mut body.list_style, ctx)?;
    for paragraph in &mut body.paragraphs {
        remap_paragraph(paragraph, ctx)?;
    }
    Ok(())
}

fn remap_list_style(style: &mut ListStyle, ctx: &Remap<'_>) -> Result<(), ApplyError> {
    for level in style.levels.iter_mut().flatten() {
        remap_bullet(&mut level.para.bullet, ctx)?;
        remap_run_props(&mut level.run, ctx)?;
    }
    Ok(())
}

fn remap_paragraph(paragraph: &mut Paragraph, ctx: &Remap<'_>) -> Result<(), ApplyError> {
    remap_bullet(&mut paragraph.props.bullet, ctx)?;
    remap_run_props(&mut paragraph.end_props, ctx)?;
    for run in &mut paragraph.runs {
        remap_run_props(&mut run.props, ctx)?;
    }
    Ok(())
}

fn remap_run_props(props: &mut RunProps, ctx: &Remap<'_>) -> Result<(), ApplyError> {
    if let Some(link) = &mut props.link {
        remap_link(link, ctx)?;
    }
    Ok(())
}

fn remap_bullet(bullet: &mut Option<Bullet>, ctx: &Remap<'_>) -> Result<(), ApplyError> {
    if let Some(Bullet::Picture { media }) = bullet {
        *media = ctx.media(*media)?;
    }
    Ok(())
}

fn remap_fill(fill: &mut Fill, ctx: &Remap<'_>) -> Result<(), ApplyError> {
    if let Fill::Picture(picture) = fill {
        picture.media = ctx.media(picture.media)?;
    }
    Ok(())
}

fn remap_background(background: &mut Background, ctx: &Remap<'_>) -> Result<(), ApplyError> {
    if let Background::Fill { fill } = background {
        remap_fill(fill, ctx)?;
    }
    Ok(())
}

fn remap_link(link: &mut Hyperlink, ctx: &Remap<'_>) -> Result<(), ApplyError> {
    match &mut link.action {
        Action::Slide { slide } => *slide = ctx.slide(*slide)?,
        Action::Url { url } if url.starts_with("https://") && !url.contains('@') && !url.chars().any(|c| c.is_control() || c.is_whitespace()) => {}
        _ => return Err(ApplyError::Contract("unsupported link action".into())),
    }
    Ok(())
}

fn verify_instance(
    candidate: &Presentation,
    master: &Master,
    slide: &Slide,
    draft: &AppendDraft,
    instance: LocalId,
    dest_of_local: &BTreeMap<LocalId, u32>,
) -> Result<(), ApplyError> {
    let value = serde_json::to_value((slide, master)).map_err(|_| ApplyError::Contract("resource verification".into()))?;
    let mut media = BTreeSet::new();
    let mut slides = BTreeSet::new();
    collect_refs(&value, 0, &mut media, &mut slides)?;
    let dest_media: BTreeSet<u32> = draft
        .mappings()
        .iter()
        .filter(|mapping| mapping.instance == instance && matches!(mapping.source, SourceResource::Media { .. }))
        .filter_map(|mapping| dest_of_local.get(&mapping.local).copied())
        .collect();
    let dest_slides: BTreeSet<u32> = draft.response().pages().iter().filter_map(|page| dest_of_local.get(&page.local_id).copied()).collect();
    if media.iter().any(|id| !dest_media.contains(id)) || slides.iter().any(|id| !dest_slides.contains(id)) {
        return Err(ApplyError::Contract("unmapped template reference".into()));
    }
    if media.iter().any(|id| candidate.media(MediaId(*id)).is_none()) {
        return Err(ApplyError::Contract("copied media is missing".into()));
    }
    if candidate.layout(slide.layout).is_none() && master.layouts.first().is_none_or(|layout| layout.id != slide.layout) {
        return Err(ApplyError::Contract("copied layout is missing".into()));
    }
    Ok(())
}

fn collect_refs(value: &Value, depth: usize, media: &mut BTreeSet<u32>, slides: &mut BTreeSet<u32>) -> Result<(), ApplyError> {
    if depth > MAX_REF_DEPTH {
        return Err(ApplyError::Contract("reference depth".into()));
    }
    match value {
        Value::Object(fields) => {
            if let Some(id) = fields.get("media") {
                let id = id.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or(ApplyError::Contract("invalid media reference".into()))?;
                media.insert(id);
            }
            if let Some(action) = fields.get("action") {
                match action.get("kind").and_then(Value::as_str) {
                    Some("slide") => {
                        let id = action
                            .get("slide")
                            .and_then(Value::as_u64)
                            .and_then(|n| u32::try_from(n).ok())
                            .ok_or(ApplyError::Contract("invalid slide link".into()))?;
                        slides.insert(id);
                    }
                    Some("url") => {
                        let url = action.get("url").and_then(Value::as_str).ok_or(ApplyError::Contract("unsupported link action".into()))?;
                        if !url.starts_with("https://") || url.contains('@') || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
                            return Err(ApplyError::Contract("unsupported link action".into()));
                        }
                    }
                    _ => return Err(ApplyError::Contract("unsupported link action".into())),
                }
            }
            for child in fields.values() {
                collect_refs(child, depth.saturating_add(1), media, slides)?;
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_refs(child, depth.saturating_add(1), media, slides)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn seed_ids(doc: &Presentation, used: &mut BTreeSet<u32>) {
    for slide in &doc.slides {
        remember(used, slide.id.0);
        remember(used, slide.layout.0);
        deckcraft_model::walk(&slide.shapes, &mut |shape, _| remember(used, shape.id.0));
    }
    for master in doc
        .masters
        .iter()
        .map(|master| master.as_ref())
        .chain(doc.notes_master.iter().map(|master| master.as_ref()))
        .chain(doc.handout_master.iter().map(|master| master.as_ref()))
    {
        remember(used, master.id.0);
        deckcraft_model::walk(&master.shapes, &mut |shape, _| remember(used, shape.id.0));
        for layout in &master.layouts {
            remember(used, layout.id.0);
            deckcraft_model::walk(&layout.shapes, &mut |shape, _| remember(used, shape.id.0));
        }
    }
    for media in &doc.media {
        remember(used, media.id.0);
    }
}

fn remember(used: &mut BTreeSet<u32>, id: u32) {
    if id != 0 {
        used.insert(id);
    }
}

fn alloc_fresh(doc: &mut Presentation, used: &mut BTreeSet<u32>) -> Result<u32, ApplyError> {
    let max_used = used.iter().next_back().copied().unwrap_or(0);
    if doc.next_id < max_used {
        doc.next_id = max_used;
    }
    let id = doc.alloc_id();
    if id == 0 || !used.insert(id) {
        return Err(ApplyError::Contract("id space exhausted".into()));
    }
    Ok(id)
}

#[cfg(test)]
thread_local! {
    static FAIL_AFTER_TEXT_OPS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

fn injected_text_failure(index: usize) -> bool {
    #[cfg(test)]
    {
        fail_after_text_op(index.saturating_add(1))
    }
    #[cfg(not(test))]
    {
        let _ = index;
        false
    }
}

#[cfg(test)]
fn fail_after_text_op(done: usize) -> bool {
    FAIL_AFTER_TEXT_OPS.with(|slot| slot.get() == Some(done))
}

#[cfg(test)]
pub(crate) fn set_fail_after_text_ops(after: Option<usize>) {
    FAIL_AFTER_TEXT_OPS.with(|slot| slot.set(after));
}

#[cfg(test)]
#[path = "ai_apply_tests.rs"]
mod tests;
