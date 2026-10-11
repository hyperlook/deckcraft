//! Store the outline and materials, and accept one page proposal.
//!
//! Each command writes its own undo step on the deck named by `docUid` (the active deck when
//! omitted). The registry does not add a second undo step.

use deckcraft_model::{ContextDraft, Material, MaterialKind, Outline, OutlinePage, SlideId};
use serde::Deserialize;
use serde_json::Value;

use super::{CommandSpec, has_doc};
use crate::ai_apply::ApplyError;
use crate::ai_contract::PageProposal;
use crate::{EngineError, Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "ai.setOutline",
            label: "Set AI Outline",
            menu: &[],
            shortcut: None,
            params: "{docUid?, pages: [{pageId?, title, points}], context?: review} — store the agreed story; does not change slides",
            enabled: has_doc,
            run: set_outline,
            journal: false,
            undoable: false,
        },
        CommandSpec {
            id: "ai.setMaterials",
            label: "Set AI Materials",
            menu: &[],
            shortcut: None,
            params: "{docUid?, materials: [{name, kind?: text|image|file, body}]} — extracted text, not file paths",
            enabled: has_doc,
            run: set_materials,
            journal: false,
            undoable: false,
        },
        CommandSpec {
            id: "ai.setDraft",
            label: "Save Unconfirmed AI Draft",
            menu: &[],
            shortcut: None,
            params: "{docUid?, draft: context|null} — persist unconfirmed working copy without changing facts or slides",
            enabled: has_doc,
            run: set_draft,
            journal: false,
            undoable: false,
        },
        CommandSpec {
            id: "ai.accept",
            label: "Accept AI Page",
            menu: &[],
            shortcut: None,
            params: "{docUid?, pageId, title, points, label?} — replace that page's title and body",
            enabled: has_doc,
            run: accept,
            journal: false,
            undoable: false,
        },
    ]
}

fn set_outline(session: &mut Session, params: &Value) -> Result<Value> {
    let parsed: OutlineParams = parse(params)?;
    let doc_uid = doc_uid(session, parsed.doc_uid)?;
    let outline = Outline { pages: parsed.pages.into_iter().map(OutlinePage::from).collect() };
    let outcome = session.set_outline_review(doc_uid, outline, parsed.context).map_err(|err| fail("ai.setOutline", err))?;
    serde_json::to_value(&outcome).map_err(|err| EngineError::Other(err.to_string()))
}

fn set_materials(session: &mut Session, params: &Value) -> Result<Value> {
    let parsed: MaterialsParams = parse(params)?;
    let doc_uid = doc_uid(session, parsed.doc_uid)?;
    let materials = parsed.materials.into_iter().map(Material::from).collect();
    let outcome = session.set_materials(doc_uid, materials).map_err(|err| fail("ai.setMaterials", err))?;
    serde_json::to_value(&outcome).map_err(|err| EngineError::Other(err.to_string()))
}

fn set_draft(session: &mut Session, params: &Value) -> Result<Value> {
    let parsed: DraftParams = parse(params)?;
    let uid = doc_uid(session, parsed.doc_uid)?;
    let outcome = session.set_context_draft(uid, parsed.draft).map_err(|err| fail("ai.setDraft", err))?;
    serde_json::to_value(outcome).map_err(|err| EngineError::Other(err.to_string()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DraftParams {
    doc_uid: Option<u64>,
    draft: Option<ContextDraft>,
}

fn accept(session: &mut Session, params: &Value) -> Result<Value> {
    let parsed: AcceptParams = parse(params)?;
    let doc_uid = doc_uid(session, parsed.doc_uid)?;
    let proposal = PageProposal { doc_uid, page_id: SlideId(parsed.page_id), title: parsed.title, points: parsed.points, label: parsed.label };
    let outcome = session.accept_page(proposal).map_err(|err| fail("ai.accept", err))?;
    serde_json::to_value(&outcome).map_err(|err| EngineError::Other(err.to_string()))
}

fn doc_uid(session: &Session, requested: Option<u64>) -> Result<u64> {
    requested.or_else(|| session.active().map(|doc| doc.uid)).ok_or(EngineError::NoDocument)
}

fn parse<T: for<'de> Deserialize<'de>>(params: &Value) -> Result<T> {
    serde_json::from_value(params.clone()).map_err(|_| EngineError::BadParams { cmd: "ai".into(), msg: "invalid parameters".into() })
}

fn fail(cmd: &str, err: ApplyError) -> EngineError {
    match err {
        ApplyError::Internal => EngineError::Internal(cmd.into(), "the presentation was kept as it was".into()),
        other => EngineError::BadParams { cmd: cmd.into(), msg: other.to_string() },
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OutlineParams {
    doc_uid: Option<u64>,
    pages: Vec<OutlinePageParams>,
    #[serde(default)]
    context: Option<ContextDraft>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OutlinePageParams {
    page_id: Option<u32>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    points: Vec<String>,
}

impl From<OutlinePageParams> for OutlinePage {
    fn from(page: OutlinePageParams) -> Self {
        OutlinePage { page_id: page.page_id.map(SlideId), title: page.title, points: page.points }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MaterialsParams {
    doc_uid: Option<u64>,
    materials: Vec<MaterialParams>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MaterialParams {
    name: String,
    #[serde(default)]
    kind: MaterialKind,
    #[serde(default)]
    body: String,
}

impl From<MaterialParams> for Material {
    fn from(material: MaterialParams) -> Self {
        Material { name: material.name, kind: material.kind, body: material.body }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AcceptParams {
    doc_uid: Option<u64>,
    page_id: u32,
    #[serde(default)]
    title: String,
    #[serde(default)]
    points: Vec<String>,
    #[serde(default)]
    label: String,
}
