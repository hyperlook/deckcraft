#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use deckcraft_model::{Outline, OutlinePage, PhType, Presentation, SlideId};
use serde_json::json;

use super::ApplyError;
use crate::ai_contract::PageProposal;
use crate::{EngineError, Interaction, Session};

fn deck() -> (Session, u64, SlideId) {
    let mut session = Session::new();
    session.execute("file.new", &json!({})).unwrap();
    session.execute("slide.new", &json!({"title": "旧标题", "body": "甲\n乙\n丙"})).unwrap();
    let uid = session.active().unwrap().uid;
    let page = session.active().unwrap().doc.slides[1].id;
    (session, uid, page)
}

fn proposal(uid: u64, page: SlideId, title: &str, points: &[&str]) -> PageProposal {
    PageProposal {
        doc_uid: uid,
        page_id: page,
        title: title.into(),
        points: points.iter().map(|point| (*point).to_string()).collect(),
        label: String::new(),
    }
}

fn lines(doc: &Presentation, page: SlideId, kind: PhType) -> Vec<String> {
    let slide = doc.slides.iter().find(|slide| slide.id == page).unwrap();
    let shape = slide.shapes.iter().find(|shape| shape.ph_type() == Some(kind)).unwrap();
    shape.text.as_ref().unwrap().paragraphs.iter().map(|paragraph| paragraph.text()).collect()
}

fn outline_points(doc: &Presentation, page: SlideId) -> Option<Vec<String>> {
    doc.outline.pages.iter().find(|entry| entry.page_id == Some(page)).map(|entry| entry.points.clone())
}

#[test]
fn accept_can_change_how_many_points_and_undoes_in_one_step() {
    let (mut session, uid, page) = deck();
    let before = session.active().unwrap().history.undo.len();
    let outcome = session.accept_page(proposal(uid, page, "新主张", &["只留一点", "再补一点"])).unwrap();
    assert!(outcome.changed);
    let doc = &session.active().unwrap().doc;
    assert_eq!(lines(doc, page, PhType::Title), vec!["新主张".to_string()]);
    assert_eq!(lines(doc, page, PhType::Obj), vec!["只留一点".to_string(), "再补一点".to_string()]);
    assert_eq!(outline_points(doc, page).unwrap(), vec!["只留一点".to_string(), "再补一点".to_string()]);
    assert_eq!(session.active().unwrap().history.undo.len(), before + 1);
    assert_eq!(session.active().unwrap().history.undo.last().unwrap().label, "AI: 更新「新主张」");

    session.execute("edit.undo", &json!({})).unwrap();
    let doc = &session.active().unwrap().doc;
    assert_eq!(lines(doc, page, PhType::Obj), vec!["甲".to_string(), "乙".to_string(), "丙".to_string()]);
    assert!(outline_points(doc, page).is_none());

    session.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(lines(&session.active().unwrap().doc, page, PhType::Title), vec!["新主张".to_string()]);
}

#[test]
fn the_same_proposal_can_be_accepted_again_after_undo() {
    let (mut session, uid, page) = deck();
    let proposal = proposal(uid, page, "新主张", &["一点"]);
    session.accept_page(proposal.clone()).unwrap();
    session.execute("edit.undo", &json!({})).unwrap();
    let again = session.accept_page(proposal).unwrap();
    assert!(again.changed);
    assert_eq!(lines(&session.active().unwrap().doc, page, PhType::Obj), vec!["一点".to_string()]);
}

#[test]
fn a_hand_edit_does_not_block_or_update_the_outline() {
    let (mut session, uid, page) = deck();
    session
        .set_outline(
            uid,
            Outline {
                pages: vec![OutlinePage { page_id: Some(page), title: "旧标题".into(), points: vec!["甲".into(), "乙".into(), "丙".into()] }]
            },
        )
        .unwrap();
    session
        .edit(|doc, _| {
            let slide = Arc::make_mut(doc.slides.iter_mut().find(|slide| slide.id == page).unwrap());
            let body = slide.shapes.iter_mut().find(|shape| shape.ph_type() == Some(PhType::Obj)).unwrap();
            body.text.as_mut().unwrap().paragraphs[0].runs[0].text = "手改".into();
            Ok(())
        })
        .unwrap();
    assert_eq!(outline_points(&session.active().unwrap().doc, page).unwrap(), vec!["甲".to_string(), "乙".to_string(), "丙".to_string()]);
    session.accept_page(proposal(uid, page, "新主张", &["按大纲来"])).unwrap();
    let doc = &session.active().unwrap().doc;
    assert_eq!(lines(doc, page, PhType::Obj), vec!["按大纲来".to_string()]);
    assert_eq!(outline_points(doc, page).unwrap(), vec!["按大纲来".to_string()]);
}

#[test]
fn citing_a_page_does_not_follow_the_slide_on_screen() {
    let (mut session, uid, page) = deck();
    let title_page = session.active().unwrap().doc.slides[0].id;
    session.execute("slide.go", &json!({"index": 0})).unwrap();
    session.accept_page(proposal(uid, page, "写到点名的那一页", &["不是封面"])).unwrap();
    assert_eq!(session.active().unwrap().selection.slide, 0);
    assert_eq!(session.active().unwrap().doc.slide(title_page).unwrap().title(), "");
    assert_eq!(lines(&session.active().unwrap().doc, page, PhType::Title), vec!["写到点名的那一页".to_string()]);
}

#[test]
fn a_deleted_page_cancels_without_writing() {
    let (mut session, uid, page) = deck();
    session.execute("slide.delete", &json!({"ids": [page.0]})).unwrap();
    let before = session.active().unwrap().doc.clone();
    let history = session.active().unwrap().history.undo.len();
    let err = session.accept_page(proposal(uid, page, "来不及了", &["不该出现"])).unwrap_err();
    assert_eq!(err, ApplyError::PageGone);
    assert!(std::sync::Arc::ptr_eq(&session.active().unwrap().doc, &before));
    assert_eq!(session.active().unwrap().history.undo.len(), history);
}

#[test]
fn another_open_deck_is_not_the_target() {
    let (mut session, uid_a, page) = deck();
    session.execute("file.new", &json!({})).unwrap();
    let uid_b = session.active().unwrap().uid;
    let untouched = Arc::clone(&session.active().unwrap().doc);
    session.accept_page(proposal(uid_a, page, "后台这份", &["只改它"])).unwrap();
    assert!(Arc::ptr_eq(&session.documents().iter().find(|doc| doc.uid == uid_b).unwrap().doc, &untouched));
    let target = session.documents().iter().find(|doc| doc.uid == uid_a).unwrap();
    assert_eq!(lines(&target.doc, page, PhType::Title), vec!["后台这份".to_string()]);
}

#[test]
fn a_closed_deck_and_a_drag_in_progress_do_not_write() {
    let (mut session, uid, page) = deck();
    let state = session.active_mut().unwrap();
    state.interaction = Some(Interaction { label: "drag".into(), doc: Arc::clone(&state.doc), selection: state.selection.clone() });
    let err = session.accept_page(proposal(uid, page, "拖着", &["不行"])).unwrap_err();
    assert_eq!(err, ApplyError::InteractionPending);
    assert_eq!(lines(&session.active().unwrap().doc, page, PhType::Obj), vec!["甲".to_string(), "乙".to_string(), "丙".to_string()]);

    session.active_mut().unwrap().interaction = None;
    session.execute("file.close", &json!({})).unwrap();
    assert_eq!(session.accept_page(proposal(uid, page, "关了", &["不行"])).unwrap_err(), ApplyError::DocumentClosed);
}

#[test]
fn accepting_the_same_words_keeps_redo_when_nothing_changes() {
    let (mut session, uid, page) = deck();
    session.accept_page(proposal(uid, page, "旧标题", &["甲", "乙", "丙"])).unwrap();
    session.execute("slide.new", &json!({"title": "随后一页"})).unwrap();
    session.execute("edit.undo", &json!({})).unwrap();
    assert!(!session.active().unwrap().history.redo.is_empty());
    let outcome = session.accept_page(proposal(uid, page, "旧标题", &["甲", "乙", "丙"])).unwrap();
    assert!(!outcome.changed);
    assert!(!session.active().unwrap().history.redo.is_empty());
}

#[test]
fn extra_text_columns_are_cleared_and_other_shapes_stay() {
    let mut session = Session::new();
    session.execute("file.new", &json!({})).unwrap();
    session.execute("slide.new", &json!({"layout": "two", "title": "对比", "body": "左栏"})).unwrap();
    let uid = session.active().unwrap().uid;
    let page = session.active().unwrap().current_slide().unwrap().id;
    session
        .edit(|doc, _| {
            let slide = Arc::make_mut(doc.slides.iter_mut().find(|slide| slide.id == page).unwrap());
            let mut columns: Vec<_> = slide.shapes.iter_mut().filter(|shape| shape.ph_type() == Some(PhType::Obj)).collect();
            columns[1].text.as_mut().unwrap().paragraphs = vec![deckcraft_model::Paragraph::new("右栏旧话")];
            Ok(())
        })
        .unwrap();
    session.execute("shape.insert", &json!({"preset": "rect", "rect": [20, 20, 40, 40]})).unwrap();
    let shapes_before = session.active().unwrap().current_slide().unwrap().shapes.len();
    session.accept_page(proposal(uid, page, "对比", &["新的一点"])).unwrap();
    let slide = session.active().unwrap().current_slide().unwrap();
    assert_eq!(slide.shapes.len(), shapes_before);
    let columns: Vec<_> = slide.shapes.iter().filter(|shape| shape.ph_type() == Some(PhType::Obj)).collect();
    assert_eq!(columns[0].text.as_ref().unwrap().text(), "新的一点");
    assert_eq!(columns[1].text.as_ref().unwrap().text(), "");
    assert!(slide.shapes.iter().any(|shape| shape.ph.is_none() && !shape.is_placeholder()));
}

#[test]
fn materials_and_outline_survive_in_the_file_and_commands_reject_unknown_fields() {
    let (mut session, uid, page) = deck();
    session.execute("ai.setOutline", &json!({"docUid": uid, "pages": [{"pageId": page.0, "title": "旧标题", "points": ["甲", "乙"]}]})).unwrap();
    session.execute("ai.setMaterials", &json!({"materials": [{"name": "访谈", "kind": "file", "body": "用户原话在这里"}]})).unwrap();
    let bytes = deckcraft_format::save(&session.active().unwrap().doc).unwrap();
    let loaded = deckcraft_format::load(&bytes).unwrap();
    assert_eq!(loaded.outline.pages[0].title, "旧标题");
    assert_eq!(loaded.materials[0].body, "用户原话在这里");

    let err = session.execute("ai.accept", &json!({"pageId": page.0, "title": "x", "points": ["y"], "extra": true})).unwrap_err();
    assert!(matches!(err, EngineError::BadParams { .. }), "{err}");
    let err = session.accept_page(proposal(uid, page, "带换行\n不行", &["y"])).unwrap_err();
    assert!(matches!(err, ApplyError::Contract(_)), "{err}");
}

#[test]
fn a_panic_after_accept_restores_the_deck() {
    let (mut session, uid, page) = deck();
    let before = Arc::clone(&session.active().unwrap().doc);
    let err = session
        .guarded("ai.accept", |session| {
            session.accept_page(proposal(uid, page, "不该留下", &["半截"])).map_err(EngineError::from)?;
            panic!("boom");
        })
        .unwrap_err();
    assert!(err.to_string().contains("boom"), "{err}");
    assert!(Arc::ptr_eq(&session.active().unwrap().doc, &before));
}
