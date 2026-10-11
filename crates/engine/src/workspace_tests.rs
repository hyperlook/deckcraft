#![allow(clippy::unwrap_used)]
use super::*;
use deckcraft_model::{ContextDraft, PlanDraft, PlannedContent, Requirements};
use serde_json::json;
fn context() -> WorkspaceContext {
    WorkspaceContext {
        version: 1,
        requirements: Some(Requirements { topic: "公共花园".into(), purpose: "介绍".into(), audience: "公众".into(), page_count: 3 }),
        mainline: "问题到行动".into(),
        evidence: ContextDraft::default(),
    }
}
fn plan() -> PlanDraft {
    PlanDraft {
        mainline: "问题到行动".into(),
        pages: (0..3).map(|i| PlannedContent { title: format!("角色 {i}"), intent: "事实与案例待补；连接下页".into() }).collect(),
    }
}
fn params(s: &Session) -> Value {
    let d = s.active().unwrap();
    json!({"docUid":d.uid,"expectedRevision":d.revision,"context":context(),"plan":plan()})
}
#[test]
fn install_is_one_atomic_user_edit_and_revision_guard_cannot_write_to_a_changed_document() {
    let mut s = Session::with_new();
    let p = params(&s);
    s.execute("slide.new", &json!({"title":"手改"})).unwrap();
    let before = s.active().unwrap().doc.clone();
    assert!(s.execute("ai.installWorkspace", &p).is_err());
    assert_eq!(s.active().unwrap().doc, before);
    let mut s = Session::with_new();
    let before = s.active().unwrap().doc.clone();
    s.execute("ai.installWorkspace", &params(&s)).unwrap();
    let result = s.active().unwrap().doc.clone();
    assert!(result.has_planned_pages());
    assert_eq!(result.slides.len(), 3);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.active().unwrap().doc, before);
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(s.active().unwrap().doc, result);
}
#[test]
fn forged_state_overflow_invalid_version_and_derived_creation_are_atomic_failures() {
    for mode in 0..5 {
        let mut s = Session::with_new();
        if mode == 4 {
            s.edit(|p, _| {
                p.next_id = u32::MAX;
                Ok(())
            })
            .unwrap();
        }
        let mut p = params(&s);
        match mode {
            0 => p["context"]["version"] = json!(2),
            1 => p["derived"] = json!(true),
            2 => p["plan"]["pages"][0]["pageId"] = json!(999),
            3 => p["plan"]["pages"] = json!([]),
            _ => {}
        }
        let before = s.active().unwrap().doc.clone();
        assert!(s.execute("ai.installWorkspace", &p).is_err());
        assert_eq!(s.active().unwrap().doc, before);
    }
}
#[test]
fn native_roundtrip_keeps_slot_identity_and_direct_export_and_show_guards() {
    let mut s = Session::with_new();
    s.execute("ai.installWorkspace", &params(&s)).unwrap();
    let doc = &s.active().unwrap().doc;
    let bytes = deckcraft_format::save(doc).unwrap();
    let restored = deckcraft_format::load(&bytes).unwrap();
    assert_eq!(doc.slides, restored.slides);
    assert_eq!(doc.workspace, restored.workspace);
    assert!(deckcraft_pptx::export(doc).is_err());
    assert!(deckcraft_pdf::export(doc, &Default::default()).is_err());
    assert!(s.execute("file.render", &json!({"slide":0})).is_err());
}
#[test]
fn planned_page_intent_is_not_promoted_to_source_evidence_or_existing_state() {
    let mut s = Session::with_new();
    s.execute("ai.installWorkspace", &params(&s)).unwrap();
    let id = s.active().unwrap().doc.slides[0].id;
    s.execute("ai.adjustPageIntent", &json!({"pageId":id.0,"title":"用户选择","intent":"用户推断，不是事实"})).unwrap();
    let d = &s.active().unwrap().doc;
    assert!(d.slides[0].is_planned());
    assert!(d.workspace.as_ref().unwrap().evidence.pages.is_empty());
    assert!(d.outline.is_empty());
    assert!(d.confirmed_summary.is_none());
}
