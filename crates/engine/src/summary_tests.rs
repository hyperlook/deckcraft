#![allow(clippy::unwrap_used, clippy::expect_used)]
use crate::Session;
use deckcraft_model::{ContextDraft, ContextPage, DeckSummary, SummaryPage, SummaryReview};
use serde_json::json;

fn review(s: &Session) -> SummaryReview {
    let pages: Vec<_> = s
        .active()
        .unwrap()
        .doc
        .slides
        .iter()
        .map(|p| SummaryPage { page_id: p.id, role: "未知".into(), intent: "无原文，待核对".into(), unknown: true })
        .collect();
    SummaryReview {
        version: 1,
        evidence: ContextDraft {
            pages: pages
                .iter()
                .map(|p| ContextPage {
                    page_id: p.page_id,
                    title: String::new(),
                    points: vec![],
                    unknown: true,
                    sources: vec![],
                    diagnostics: vec![],
                })
                .collect(),
            ..Default::default()
        },
        summary: DeckSummary { mainline: "基于有限文字提出问题、说明行动路径".into(), pages },
    }
}
fn store(s: &mut Session, uid: u64, review: &SummaryReview, confirm: bool) -> crate::Result<serde_json::Value> {
    s.execute("ai.setSummary", &json!({"docUid":uid,"review":review,"confirm":confirm}))
}
#[test]
fn semantic_draft_confirmation_and_native_restore_are_distinct_and_undoable() {
    let mut s = Session::with_new();
    let uid = s.active().unwrap().uid;
    let r = review(&s);
    let original = s.active().unwrap().doc.clone();
    store(&mut s, uid, &r, false).unwrap();
    assert!(s.active().unwrap().doc.confirmed_summary.is_none());
    assert_eq!(s.active().unwrap().doc.summary_draft, Some(r.clone()));
    let draft = deckcraft_format::load(&deckcraft_format::save(&s.active().unwrap().doc).unwrap()).unwrap();
    assert!(draft.confirmed_summary.is_none());
    assert_eq!(draft.summary_draft, Some(r.clone()));
    let count = s.active().unwrap().history.undo.len();
    store(&mut s, uid, &r, true).unwrap();
    assert_eq!(s.active().unwrap().history.undo.len(), count + 1);
    assert_eq!(s.active().unwrap().doc.confirmed_summary, Some(r.clone()));
    assert!(s.active().unwrap().doc.summary_draft.is_none());
    assert_eq!(s.active().unwrap().doc.slides, original.slides);
    let restored = deckcraft_format::load(&deckcraft_format::save(&s.active().unwrap().doc).unwrap()).unwrap();
    assert_eq!(restored.confirmed_summary, Some(r.clone()));
    assert_eq!(restored.confirmed_context, Some(r.evidence.clone()));
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.active().unwrap().doc.confirmed_summary.is_none());
    assert_eq!(s.active().unwrap().doc.summary_draft, Some(r.clone()));
    s.execute("edit.redo", &json!({})).unwrap();
    let count = s.active().unwrap().history.undo.len();
    assert_eq!(store(&mut s, uid, &r, true).unwrap()["changed"], false);
    assert_eq!(s.active().unwrap().history.undo.len(), count);
}
#[test]
fn stale_mapping_invalid_schema_and_closed_document_do_not_commit() {
    let mut s = Session::with_new();
    let uid = s.active().unwrap().uid;
    let r = review(&s);
    let before = s.active().unwrap().doc.clone();
    let mut bad = r.clone();
    bad.version = 0;
    assert!(store(&mut s, uid, &bad, true).is_err());
    let mut bad = r.clone();
    bad.summary.mainline = "x".repeat(2001);
    assert!(store(&mut s, uid, &bad, true).is_err());
    let mut bad = r.clone();
    bad.summary.pages[0].unknown = false;
    assert!(store(&mut s, uid, &bad, true).is_err());
    let mut bad = r.clone();
    bad.summary.pages[0].intent = "a\nb".into();
    assert!(store(&mut s, uid, &bad, false).is_err());
    assert_eq!(s.active().unwrap().doc, before);
    s.execute("slide.new", &json!({"title":"新增页"})).unwrap();
    let r = review(&s);
    let mut bad = r.clone();
    bad.summary.pages.swap(0, 1);
    bad.evidence.pages.swap(0, 1);
    assert!(store(&mut s, uid, &bad, true).is_err());
    s.execute("slide.delete", &json!({"ids":[r.summary.pages[1].page_id]})).unwrap();
    assert!(store(&mut s, uid, &r, true).is_err());
    s.execute("file.close", &json!({})).unwrap();
    assert!(s.set_summary(uid, r, true).is_err());
}
#[test]
fn explicit_uid_not_the_current_tab_is_the_commit_target_and_legacy_is_not_semantic() {
    let mut s = Session::with_new();
    let uid = s.active().unwrap().uid;
    let r = review(&s);
    s.execute("ai.setOutline", &json!({"pages":[{"pageId":r.summary.pages[0].page_id,"title":"全文摘录","points":[]}]})).unwrap();
    assert!(s.active().unwrap().doc.confirmed_summary.is_none());
    s.execute("file.new", &json!({})).unwrap();
    let other = s.active().unwrap().doc.clone();
    store(&mut s, uid, &r, true).unwrap();
    assert_eq!(s.active().unwrap().doc, other);
    assert_eq!(s.documents().iter().find(|d| d.uid == uid).unwrap().doc.confirmed_summary, Some(r));
}
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn pptx_save_is_an_export_not_a_native_metadata_save() {
    let mut s = Session::with_new();
    let uid = s.active().unwrap().uid;
    let r = review(&s);
    store(&mut s, uid, &r, true).unwrap();
    let baseline = s.active().unwrap().saved_doc.clone();
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("deckcraft-summary-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pptx = dir.join("export.pptx");
    let native = dir.join("project.deckcraft");
    let result = s.execute("file.saveAs", &json!({"path":pptx.to_str().unwrap()})).unwrap();
    assert_eq!(result["nativeSaveRequired"], true);
    assert!(s.active().unwrap().is_dirty());
    assert_eq!(s.active().unwrap().saved_doc, baseline);
    let imported = crate::cmd::file::open_presentation("export.pptx", &std::fs::read(&pptx).unwrap()).unwrap();
    assert!(imported.confirmed_summary.is_none() && imported.summary_draft.is_none());
    let result = s.execute("file.saveAs", &json!({"path":native.to_str().unwrap()})).unwrap();
    assert_eq!(result["nativeSaveRequired"], false);
    assert!(!s.active().unwrap().is_dirty());
    let loaded = crate::cmd::file::open_presentation("project.deckcraft", &std::fs::read(&native).unwrap()).unwrap();
    assert_eq!(loaded.confirmed_summary, Some(r));
    std::fs::remove_dir_all(dir).unwrap();
}
