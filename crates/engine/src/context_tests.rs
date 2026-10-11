#![allow(clippy::unwrap_used, clippy::expect_used)]
use crate::Session;
use deckcraft_model::{ContextDraft, ContextPage, Material, MaterialKind, SlideId};
use serde_json::json;

fn draft(session: &Session) -> ContextDraft {
    ContextDraft {
        pages: session
            .active()
            .unwrap()
            .doc
            .slides
            .iter()
            .enumerate()
            .map(|(i, slide)| ContextPage {
                page_id: slide.id,
                title: format!("人工核对 {i}"),
                points: vec!["已核对要点".into()],
                unknown: i == 0,
                sources: vec![],
                diagnostics: vec![],
            })
            .collect(),
        materials: vec![Material { name: "受众".into(), kind: MaterialKind::Text, body: "非敏感测试".into() }],
        diagnostics: vec![],
    }
}
fn session() -> Session {
    let mut s = Session::with_new();
    s.execute("slide.new", &json!({"title":"原页", "body":"原正文"})).unwrap();
    s
}
fn confirm(s: &mut Session, uid: u64, d: &ContextDraft) -> crate::Result<serde_json::Value> {
    s.execute("ai.setOutline", &json!({"docUid":uid, "pages": d.outline().pages, "context":d}))
}

#[test]
fn draft_facts_confirmation_and_one_step_undo_are_separate() {
    let mut s = session();
    let uid = s.active().unwrap().uid;
    let slides = s.active().unwrap().doc.slides.clone();
    let d = draft(&s);
    s.execute("ai.setDraft", &json!({"docUid": uid, "draft":d})).unwrap();
    assert!(s.active().unwrap().doc.outline.is_empty());
    assert!(s.active().unwrap().doc.materials.is_empty());
    assert_eq!(s.active().unwrap().doc.slides, slides);
    let before = s.active().unwrap().history.undo.len();
    confirm(&mut s, uid, &d).unwrap();
    assert_eq!(s.active().unwrap().history.undo.len(), before + 1);
    assert_eq!(s.active().unwrap().doc.outline.pages.len(), 1, "unknown page is not a fact");
    assert!(s.active().unwrap().doc.context_draft.is_none());
    assert_eq!(s.active().unwrap().doc.materials, d.materials);
    assert_eq!(s.active().unwrap().doc.slides, slides);
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.active().unwrap().doc.outline.is_empty());
    assert_eq!(s.active().unwrap().doc.context_draft, Some(d.clone()));
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(s.active().unwrap().doc.confirmed_context, Some(d));
    assert_eq!(s.active().unwrap().doc.slides, slides);
}

#[test]
fn saved_draft_restores_as_unconfirmed_and_cancel_does_not_clear_facts() {
    let mut s = session();
    let uid = s.active().unwrap().uid;
    let mut d = draft(&s);
    confirm(&mut s, uid, &d).unwrap();
    let agreed = s.active().unwrap().doc.outline.clone();
    d.pages[1].title = "未确认的新标题".into();
    s.execute("ai.setDraft", &json!({"docUid":uid, "draft":d})).unwrap();
    let bytes = deckcraft_format::save(&s.active().unwrap().doc).unwrap();
    let restored = deckcraft_format::load(&bytes).unwrap();
    assert_eq!(restored.outline, agreed);
    assert_eq!(restored.context_draft, Some(d));
    s.execute("ai.setDraft", &json!({"docUid":uid, "draft":null})).unwrap();
    assert_eq!(s.active().unwrap().doc.outline, agreed);
}

#[test]
fn tab_switch_and_reordering_do_not_retarget_confirmation() {
    let mut s = session();
    let uid = s.active().unwrap().uid;
    let d = draft(&s);
    s.execute("slide.move", &json!({"from":1,"to":0})).unwrap();
    s.execute("file.new", &json!({})).unwrap();
    let other = s.active().unwrap().doc.clone();
    confirm(&mut s, uid, &d).unwrap();
    assert_eq!(s.active().unwrap().doc, other);
    let original = s.documents().iter().find(|state| state.uid == uid).unwrap();
    assert_eq!(original.doc.outline.pages[0].page_id, Some(d.pages[1].page_id));
}

#[test]
fn deleted_page_wrong_facts_and_limits_are_atomic_errors() {
    let mut s = session();
    let uid = s.active().unwrap().uid;
    let mut d = draft(&s);
    let before = s.active().unwrap().doc.clone();
    assert!(s.execute("ai.setOutline", &json!({"docUid":uid, "pages":[], "context":d})).is_err());
    assert_eq!(s.active().unwrap().doc, before);
    d.pages[1].points = vec!["x".into(); 33];
    assert!(s.execute("ai.setDraft", &json!({"docUid":uid, "draft":d})).is_err());
    assert_eq!(s.active().unwrap().doc, before);
    d.pages[1].points = vec![];
    s.execute("slide.delete", &json!({"ids":[d.pages[1].page_id]})).unwrap();
    let before = s.active().unwrap().doc.clone();
    assert!(confirm(&mut s, uid, &d).is_err());
    assert_eq!(s.active().unwrap().doc, before);
    assert!(s.set_context_draft(u64::MAX, None).is_err());
    d.pages[1].page_id = SlideId(0);
    assert!(s.set_context_draft(uid, Some(d)).is_err());
}

#[test]
fn confirmed_context_survives_native_save_but_is_not_assumed_on_pptx_import() {
    let mut s = session();
    let uid = s.active().unwrap().uid;
    let d = draft(&s);
    confirm(&mut s, uid, &d).unwrap();
    let native = deckcraft_format::save(&s.active().unwrap().doc).unwrap();
    let restored = deckcraft_format::load(&native).unwrap();
    assert_eq!(restored.confirmed_context, Some(d.clone()));
    assert_eq!(restored.outline, d.outline());
    let pptx = deckcraft_pptx::export(&restored).unwrap();
    s.execute("file.openBytes", &json!({"name":"fixture.pptx","data":crate::cmd::base64_encode(&pptx)})).unwrap();
    assert!(s.active().unwrap().doc.outline.is_empty());
    assert!(s.active().unwrap().doc.context_draft.is_none());
    assert!(s.active().unwrap().doc.confirmed_context.is_none());
}
