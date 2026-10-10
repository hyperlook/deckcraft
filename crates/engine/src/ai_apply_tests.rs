#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::field_reassign_with_default)]

use std::sync::Arc;

use deckcraft_model::style::{Fill, PictureFill};
use deckcraft_model::text::{Action, Bullet, Hyperlink, LevelStyle, ListStyle, Paragraph, Run, RunProps};
use deckcraft_model::{MediaId, MediaItem, Presentation, Shape, ShapeId, ShapeKind, Slide, SlideId, TextBody, Xfrm};
use serde_json::json;

use super::{ApplyError, set_fail_after_text_ops};
use crate::ai_apply::AiCommit;
use crate::ai_contract::template::{
    AppendDraft, AppendResponse, AppendScope, LocalId, PageInstance, PageLink, SlotReplacement, TemplateBundle, TemplatePage, TemplateSlot,
};
use crate::ai_contract::{SnapshotIdentity, TextPatch, TextPatchOp, TextScope, TextTarget};
use crate::{DocState, Interaction, PointerEvent, PointerKind, Selection, Session};

struct Mark {
    doc: Arc<Presentation>,
    selection: Selection,
    content_revision: u64,
    undo: usize,
    redo: usize,
    jobs: usize,
    interaction: bool,
}

fn mark(session: &Session, uid: u64) -> Mark {
    let doc = session.documents().iter().find(|doc| doc.uid == uid).unwrap();
    Mark {
        doc: Arc::clone(&doc.doc),
        selection: doc.selection.clone(),
        content_revision: doc.content_revision,
        undo: doc.history.undo.len(),
        redo: doc.history.redo.len(),
        jobs: doc.applied_jobs.len(),
        interaction: doc.interaction.is_some(),
    }
}

fn unchanged(session: &Session, uid: u64, before: &Mark) {
    let doc = session.documents().iter().find(|doc| doc.uid == uid).unwrap();
    assert!(Arc::ptr_eq(&doc.doc, &before.doc));
    assert_eq!(doc.selection, before.selection);
    assert_eq!(doc.content_revision, before.content_revision);
    assert_eq!(doc.history.undo.len(), before.undo);
    assert_eq!(doc.history.redo.len(), before.redo);
    assert_eq!(doc.applied_jobs.len(), before.jobs);
    assert_eq!(doc.interaction.is_some(), before.interaction);
}

fn open(doc: Presentation) -> (Session, u64) {
    let mut session = Session::new();
    let state = DocState::new(doc, None, "Test".into());
    let uid = state.uid;
    session.add_document(state);
    (session, uid)
}

fn text_doc() -> Presentation {
    let mut doc = Presentation::default();
    let layout = doc.masters[0].layouts[0].id;
    doc.slides = vec![
        Arc::new(Slide { id: SlideId(10), layout, shapes: vec![styled_shape(11, "旧标题", "旧要点")], ..Default::default() }),
        Arc::new(Slide {
            id: SlideId(20),
            layout,
            shapes: vec![Shape { id: ShapeId(21), text: Some(TextBody::from_text("第二页")), ..Default::default() }],
            ..Default::default()
        }),
        Arc::new(Slide {
            id: SlideId(30),
            layout,
            shapes: vec![Shape { id: ShapeId(31), text: Some(TextBody::from_text("第三页")), ..Default::default() }],
            ..Default::default()
        }),
    ];
    doc.fix_next_id();
    doc
}

fn styled_shape(id: u32, title: &str, point: &str) -> Shape {
    let mut bold = RunProps::default();
    bold.bold = Some(true);
    let mut italic = RunProps::default();
    italic.italic = Some(true);
    Shape {
        id: ShapeId(id),
        xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 80.0)),
        text: Some(TextBody {
            paragraphs: vec![
                Paragraph { level: 0, runs: vec![Run::with(title, bold.clone())], end_props: bold, ..Default::default() },
                Paragraph {
                    level: 1,
                    runs: vec![Run::with(point, italic.clone()), Run::with("保留", italic.clone())],
                    end_props: italic,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn identity(uid: u64, revision: u64, slides: Vec<SlideId>) -> SnapshotIdentity {
    SnapshotIdentity {
        job_id: "job-1".into(),
        doc_uid: uid,
        base_content_revision: revision,
        target_slide_ids: slides,
        plan_version: 1,
        template_version: None,
    }
}

fn scope_for(doc: &Presentation, uid: u64) -> TextScope {
    TextScope {
        snapshot: identity(uid, 1, doc.slides.iter().map(|slide| slide.id).collect()),
        editable: vec![
            TextTarget { slide_id: SlideId(10), shape_id: ShapeId(11) },
            TextTarget { slide_id: SlideId(20), shape_id: ShapeId(21) },
            TextTarget { slide_id: SlideId(30), shape_id: ShapeId(31) },
        ],
    }
}

fn patch(scope: &TextScope, ops: Vec<TextPatchOp>) -> TextPatch {
    TextPatch { snapshot: scope.snapshot.clone(), operations: ops }
}

fn replace(slide: u32, shape: u32, paragraphs: &[&str]) -> TextPatchOp {
    TextPatchOp::ReplaceText {
        target: TextTarget { slide_id: SlideId(slide), shape_id: ShapeId(shape) },
        paragraphs: paragraphs.iter().map(|text| (*text).to_string()).collect(),
    }
}

fn text_commit(scope: TextScope, operations: Vec<TextPatchOp>, label: &str) -> AiCommit {
    let patch = patch(&scope, operations);
    AiCommit::Text { scope, patch, label: label.into() }
}

#[test]
fn selection_does_not_change_content_identity_but_edit_undo_and_redo_do() {
    let (mut session, _) = open(text_doc());
    assert_eq!(session.active().unwrap().content_revision, 1);
    session.select(|_, selection| selection.slide = 2).unwrap();
    session.execute("edit.select", &json!({"ids": [31]})).unwrap();
    assert_eq!(session.active().unwrap().content_revision, 1);
    assert!(session.active().unwrap().revision > 1);

    session.execute("shape.insert", &json!({"preset": "rect", "rect": [20.0, 20.0, 40.0, 30.0]})).unwrap();
    assert_eq!(session.active().unwrap().content_revision, 2);
    session.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(session.active().unwrap().content_revision, 3);
    session.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(session.active().unwrap().content_revision, 4);
}

#[test]
fn multi_slide_text_commit_is_one_undo_step_and_keeps_formatting() {
    let doc = text_doc();
    let (mut session, uid) = open(doc.clone());
    session
        .select(|_, selection| {
            selection.slide = 0;
            selection.shapes = vec![ShapeId(11)];
            selection.text = Some(crate::TextSel { shape: ShapeId(11), anchor: (1, 1), caret: (1, 4), ..Default::default() });
        })
        .unwrap();
    let scope = scope_for(&doc, uid);
    let outcome = session
        .apply_ai(text_commit(
            scope,
            vec![replace(10, 11, &["新标题", "新要点"]), replace(20, 21, &["更短"]), replace(30, 31, &["结论"])],
            "AI: 精简三页",
        ))
        .unwrap();
    assert!(!outcome.duplicate && outcome.changed);
    assert_eq!(outcome.label, "AI: 精简三页");
    assert_eq!(outcome.content_revision, 2);
    let state = session.documents().iter().find(|doc| doc.uid == uid).unwrap();
    assert_eq!(state.history.undo.len(), 1);
    assert_eq!(state.history.undo[0].label, "AI: 精简三页");
    assert!(state.history.redo.is_empty());
    assert!(state.selection.text.is_none());
    assert_eq!(state.selection.slide, 0);
    let body = state.doc.slide(SlideId(10)).unwrap().shape(ShapeId(11)).unwrap().text.as_ref().unwrap();
    assert_eq!(body.paragraphs[0].text(), "新标题");
    assert_eq!(body.paragraphs[0].level, 0);
    assert_eq!(body.paragraphs[0].runs[0].props.bold, Some(true));
    assert_eq!(body.paragraphs[0].end_props.bold, Some(true));
    assert_eq!(body.paragraphs[1].text(), "新要点");
    assert_eq!(body.paragraphs[1].level, 1);
    assert_eq!(body.paragraphs[1].runs.len(), 1);
    assert_eq!(body.paragraphs[1].runs[0].props.italic, Some(true));
    assert_eq!(body.paragraphs[1].end_props.italic, Some(true));
    assert_eq!(state.doc.slide(SlideId(20)).unwrap().shape(ShapeId(21)).unwrap().text.as_ref().unwrap().text(), "更短");
    assert_eq!(state.doc.slide(SlideId(30)).unwrap().shape(ShapeId(31)).unwrap().text.as_ref().unwrap().text(), "结论");

    session.execute("edit.undo", &json!({})).unwrap();
    let state = session.active().unwrap();
    assert_eq!(state.doc.slide(SlideId(10)).unwrap().shape(ShapeId(11)).unwrap().text.as_ref().unwrap().paragraphs[0].text(), "旧标题");
    assert_eq!(state.doc.slide(SlideId(20)).unwrap().shape(ShapeId(21)).unwrap().text.as_ref().unwrap().text(), "第二页");
    assert_eq!(state.selection.text.as_ref().unwrap().caret, (1, 4));
    assert_eq!(state.content_revision, 3);
    session.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(session.active().unwrap().doc.slide(SlideId(30)).unwrap().shape(ShapeId(31)).unwrap().text.as_ref().unwrap().text(), "结论");
    assert_eq!(session.active().unwrap().content_revision, 4);
}

#[test]
fn nth_patch_failure_and_injected_candidate_failure_leave_no_trace() {
    let doc = text_doc();
    let (mut session, uid) = open(doc.clone());
    let scope = scope_for(&doc, uid);
    let before = mark(&session, uid);
    let err = session
        .apply_ai(text_commit(
            scope.clone(),
            vec![replace(10, 11, &["甲", "乙"]), replace(20, 21, &["丙"]), replace(30, 31, &["多了一段", "不该出现"])],
            "AI: 失败",
        ))
        .unwrap_err();
    assert!(matches!(err, ApplyError::Contract(_)));
    unchanged(&session, uid, &before);

    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            set_fail_after_text_ops(None);
        }
    }
    let _reset = Reset;
    set_fail_after_text_ops(Some(2));
    let err = session
        .apply_ai(text_commit(scope, vec![replace(10, 11, &["甲", "乙"]), replace(20, 21, &["丙"]), replace(30, 31, &["丁"])], "AI: 中断"))
        .unwrap_err();
    assert!(matches!(err, ApplyError::Contract(_)));
    unchanged(&session, uid, &before);
    assert!(session.active().unwrap().applied_jobs.is_empty());
}

#[test]
fn unchanged_text_does_not_write_history_or_clear_redo() {
    let doc = text_doc();
    let (mut session, uid) = open(doc.clone());
    session.execute("shape.insert", &json!({"preset": "ellipse", "rect": [300.0, 40.0, 40.0, 30.0]})).unwrap();
    session.execute("edit.undo", &json!({})).unwrap();
    let revision = session.active().unwrap().content_revision;
    assert_eq!(session.active().unwrap().history.redo.len(), 1);
    let mut scope = scope_for(&doc, uid);
    scope.snapshot.base_content_revision = revision;
    let before = mark(&session, uid);
    let outcome = session.apply_ai(text_commit(scope, vec![replace(10, 11, &["旧标题", "旧要点保留"])], "")).unwrap();
    assert!(!outcome.changed && !outcome.duplicate);
    assert_eq!(outcome.content_revision, revision);
    let state = session.active().unwrap();
    assert!(Arc::ptr_eq(&state.doc, &before.doc));
    assert_eq!(state.selection, before.selection);
    assert_eq!(state.content_revision, before.content_revision);
    assert_eq!(state.history.undo.len(), before.undo);
    assert_eq!(state.history.redo.len(), before.redo);
    assert_eq!(state.applied_jobs.len(), before.jobs + 1);
    session.execute("edit.redo", &json!({})).unwrap();
    assert!(session.active().unwrap().doc.slides[0].shapes.iter().any(|shape| shape.kind == ShapeKind::Shape && shape.xfrm.is_some()));
}

#[test]
fn duplicate_job_after_undo_does_not_apply_again() {
    let doc = text_doc();
    let (mut session, uid) = open(doc.clone());
    let scope = scope_for(&doc, uid);
    let commit = text_commit(scope, vec![replace(20, 21, &["已应用"])], "AI: 改第二页");
    session.apply_ai(commit.clone()).unwrap();
    session.execute("edit.undo", &json!({})).unwrap();
    let before = mark(&session, uid);
    assert_eq!(session.active().unwrap().doc.slide(SlideId(20)).unwrap().shape(ShapeId(21)).unwrap().text.as_ref().unwrap().text(), "第二页");
    let outcome = session.apply_ai(commit).unwrap();
    assert!(outcome.duplicate && outcome.changed);
    assert_eq!(outcome.label, "AI: 改第二页");
    unchanged(&session, uid, &before);
}

#[test]
fn content_change_conflicts_and_selection_change_does_not() {
    let doc = text_doc();
    let (mut session, uid) = open(doc.clone());
    let mut scope = scope_for(&doc, uid);
    session.select(|_, selection| selection.slide = 2).unwrap();
    let outcome = session.apply_ai(text_commit(scope.clone(), vec![replace(30, 31, &["只改选区后仍可提交"])], "AI: 选区无关")).unwrap();
    assert!(outcome.changed);
    session.execute("edit.undo", &json!({})).unwrap();
    scope.snapshot.job_id = "job-stale".into();
    let before = mark(&session, uid);
    let err = session.apply_ai(text_commit(scope, vec![replace(30, 31, &["过期"])], "AI: 过期")).unwrap_err();
    assert_eq!(err, ApplyError::Conflict);
    unchanged(&session, uid, &before);
}

#[test]
fn commit_addresses_the_snapshot_document_not_the_active_tab() {
    let (mut session, uid_a) = open(text_doc());
    let other = DocState::new(Presentation::default(), None, "Other".into());
    let uid_b = other.uid;
    let other_doc = Arc::clone(&other.doc);
    session.add_document(other);
    assert_eq!(session.active().unwrap().uid, uid_b);
    let scope = scope_for(session.documents().iter().find(|doc| doc.uid == uid_a).unwrap().doc.as_ref(), uid_a);
    session.apply_ai(text_commit(scope, vec![replace(10, 11, &["后台文档", "已改"])], "AI: 后台")).unwrap();
    assert_eq!(session.active().unwrap().uid, uid_b);
    assert!(Arc::ptr_eq(&session.active().unwrap().doc, &other_doc));
    let target = session.documents().iter().find(|doc| doc.uid == uid_a).unwrap();
    assert_eq!(target.doc.slide(SlideId(10)).unwrap().shape(ShapeId(11)).unwrap().text.as_ref().unwrap().paragraphs[0].text(), "后台文档");
    assert_eq!(target.history.undo.len(), 1);
    assert!(session.active().unwrap().history.undo.is_empty());
    session.close_document(session.doc_index_by_uid(uid_a).unwrap());
    let err = session.apply_ai(text_commit(scope_for(&text_doc(), uid_a), vec![replace(10, 11, &["关闭后", "不再写"])], "AI: 关闭")).unwrap_err();
    assert_eq!(err, ApplyError::DocumentClosed);
}

#[test]
fn pointer_interaction_is_not_swallowed_and_is_rechecked_after_it_ends() {
    let doc = text_doc();
    let (mut session, uid) = open(doc.clone());
    let scope = scope_for(&doc, uid);
    let commit = text_commit(scope, vec![replace(10, 11, &["拖动中", "不能写"])], "AI: 拖动");
    session.pointer(PointerEvent { kind: PointerKind::Down, x: 180.0, y: 140.0, tol: 4.0, ..Default::default() }).unwrap();
    session.pointer(PointerEvent { kind: PointerKind::Drag, x: 220.0, y: 180.0, tol: 4.0, ..Default::default() }).unwrap();
    assert!(session.tool.dragging());
    assert!(session.active().unwrap().interaction.is_some());
    let before = mark(&session, uid);
    assert_eq!(session.apply_ai(commit.clone()).unwrap_err(), ApplyError::InteractionPending);
    unchanged(&session, uid, &before);
    session.pointer(PointerEvent { kind: PointerKind::Up, x: 220.0, y: 180.0, tol: 4.0, ..Default::default() }).unwrap();
    assert!(session.active().unwrap().interaction.is_none());
    assert_eq!(session.active().unwrap().history.undo.len(), 1);
    assert_eq!(session.active().unwrap().history.undo[0].label, "Move");
    assert_eq!(session.apply_ai(commit).unwrap_err(), ApplyError::Conflict);
    assert_eq!(session.active().unwrap().history.undo.len(), 1);
    assert!(session.active().unwrap().applied_jobs.is_empty());
}

#[test]
fn finished_pointer_gesture_without_content_change_can_still_apply() {
    let doc = text_doc();
    let (mut session, uid) = open(doc.clone());
    session.pointer(PointerEvent { kind: PointerKind::Down, x: 20.0, y: 20.0, tol: 4.0, ..Default::default() }).unwrap();
    assert_eq!(
        session.apply_ai(text_commit(scope_for(&doc, uid), vec![replace(30, 31, &["太早"])], "AI: 太早")).unwrap_err(),
        ApplyError::InteractionPending
    );
    session.pointer(PointerEvent { kind: PointerKind::Up, x: 20.0, y: 20.0, tol: 4.0, ..Default::default() }).unwrap();
    assert!(!session.tool.dragging());
    assert_eq!(session.active().unwrap().content_revision, 1);
    let outcome = session.apply_ai(text_commit(scope_for(&doc, uid), vec![replace(30, 31, &["手势结束后"])], "AI: 手势后")).unwrap();
    assert!(outcome.changed);
    assert_eq!(session.active().unwrap().history.undo.len(), 1);
    assert_eq!(session.active().unwrap().history.undo[0].label, "AI: 手势后");
}

#[test]
fn command_rejects_unknown_fields_and_names_one_history_entry() {
    let doc = text_doc();
    let (mut session, uid) = open(doc.clone());
    let scope = scope_for(&doc, uid);
    let patch = patch(&scope, vec![replace(20, 21, &["命令"])]);
    let before = mark(&session, uid);
    let err = session.execute("ai.applyPatch", &json!({"scope": scope.clone(), "patch": patch.clone(), "extra": true})).unwrap_err();
    assert!(err.to_string().contains("unknown field"));
    unchanged(&session, uid, &before);
    let outcome = session.execute("ai.applyPatch", &json!({"label": "AI: 命令精简", "scope": scope, "patch": patch})).unwrap();
    assert_eq!(outcome["label"], "AI: 命令精简");
    assert_eq!(outcome["changed"], true);
    assert_eq!(outcome["duplicate"], false);
    assert_eq!(session.active().unwrap().history.undo[0].label, "AI: 命令精简");
    assert!(session.journal.is_empty());
    let again = session.execute("ai.applyPatch", &json!({"scope": scope, "patch": patch})).unwrap();
    assert_eq!(again["duplicate"], true);
    assert_eq!(session.active().unwrap().history.undo.len(), 1);
}

#[test]
fn ledger_is_bounded_and_keeps_existing_jobs() {
    let doc = text_doc();
    let (mut session, uid) = open(doc.clone());
    let revision = 1;
    for n in 0..super::MAX_APPLIED_JOBS {
        let mut scope = scope_for(&doc, uid);
        scope.snapshot.job_id = format!("job-{n}");
        scope.snapshot.base_content_revision = revision;
        session.apply_ai(text_commit(scope, vec![replace(10, 11, &["旧标题", "旧要点保留"])], "")).unwrap();
    }
    assert_eq!(session.active().unwrap().history.undo.len(), 0);
    let mut scope = scope_for(&doc, uid);
    scope.snapshot.job_id = "job-overflow".into();
    let before = mark(&session, uid);
    assert_eq!(session.apply_ai(text_commit(scope, vec![replace(20, 21, &["装不下"])], "AI: 溢出")).unwrap_err(), ApplyError::LedgerFull);
    unchanged(&session, uid, &before);
    let mut first = scope_for(&doc, uid);
    first.snapshot.job_id = "job-0".into();
    let outcome = session.apply_ai(text_commit(first, vec![replace(20, 21, &["不该再写"])], "AI: 重复")).unwrap();
    assert!(outcome.duplicate && !outcome.changed);
    unchanged(&session, uid, &before);
}

#[test]
fn panic_restores_document_history_content_revision_and_ledger() {
    let doc = text_doc();
    let (mut session, uid) = open(doc.clone());
    session.apply_ai(text_commit(scope_for(&doc, uid), vec![replace(20, 21, &["先成功"])], "AI: 已提交")).unwrap();
    let state = session.active_mut().unwrap();
    state.interaction = Some(Interaction { label: "Move".into(), doc: Arc::clone(&state.doc), selection: state.selection.clone() });
    let before = mark(&session, uid);
    let err = session
        .guarded("ai.applyPatch", |session| {
            let state = session.active_mut().unwrap();
            state.doc = Arc::new(Presentation::default());
            state.content_revision = 90;
            state.history.undo.clear();
            state.history.redo.push(crate::HistoryEntry { label: "污染".into(), doc: Arc::clone(&state.doc), selection: Selection::default() });
            state.applied_jobs = Default::default();
            state.interaction = None;
            panic!("boom");
        })
        .unwrap_err();
    assert!(err.to_string().contains("boom"));
    unchanged(&session, uid, &before);
    assert_eq!(session.active().unwrap().history.undo[0].label, "AI: 已提交");
    session.execute("shape.insert", &json!({"preset": "rect", "rect": [12.0, 12.0, 20.0, 20.0]})).unwrap();
}

fn png() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(1, 1).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn link(slide: u32) -> Hyperlink {
    Hyperlink { action: Action::Slide { slide: SlideId(slide) }, tooltip: String::new(), highlight_click: false }
}

fn url() -> Hyperlink {
    Hyperlink { action: Action::Url { url: "https://example.com/source".into() }, tooltip: String::new(), highlight_click: false }
}

fn rich_bundle() -> Presentation {
    let mut doc = Presentation::default();
    let layout = doc.masters[0].layouts[0].id;
    {
        let master = Arc::make_mut(&mut doc.masters[0]);
        master.theme.format.fills.push(Fill::Picture(PictureFill { media: MediaId(300), ..Default::default() }));
        let mut style = ListStyle::default();
        style.set(0, LevelStyle { run: RunProps { link: Some(link(200)), ..Default::default() }, ..Default::default() });
        master.body_style = style;
    }
    let mut title = Paragraph::new("Original title");
    title.props.bullet = Some(Bullet::Picture { media: MediaId(300) });
    let mut child = Shape { id: ShapeId(104), text: Some(TextBody::from_text("child")), ..Default::default() };
    child.text.as_mut().unwrap().paragraphs[0].runs[0].props.link = Some(link(200));
    doc.slides = vec![
        Arc::new(Slide {
            id: SlideId(100),
            layout,
            shapes: vec![
                Shape { id: ShapeId(101), text: Some(TextBody { paragraphs: vec![title], ..Default::default() }), ..Default::default() },
                Shape {
                    id: ShapeId(102),
                    kind: ShapeKind::Picture { fill: PictureFill { media: MediaId(300), ..Default::default() } },
                    click: Some(link(200)),
                    hover: Some(url()),
                    ..Default::default()
                },
                Shape {
                    id: ShapeId(103),
                    kind: ShapeKind::Group { children: vec![child], child: Xfrm::new(0.0, 0.0, 80.0, 40.0) },
                    ..Default::default()
                },
            ],
            ..Default::default()
        }),
        Arc::new(Slide {
            id: SlideId(200),
            layout,
            shapes: vec![Shape {
                id: ShapeId(201),
                text: Some(TextBody::from_text("Original conclusion")),
                click: Some(link(100)),
                ..Default::default()
            }],
            ..Default::default()
        }),
    ];
    doc.media.push(MediaItem { id: MediaId(300), name: "original.png".into(), content_type: "image/png".into(), data: Arc::new(png()), link: None });
    doc.fix_next_id();
    doc
}

fn append_commit(target: &Presentation, uid: u64, revision: u64, slides: Vec<SlideId>, label: &str) -> AiCommit {
    let mut snapshot = SnapshotIdentity {
        job_id: "append-job".into(),
        doc_uid: uid,
        base_content_revision: revision,
        target_slide_ids: slides,
        plan_version: 1,
        template_version: Some(9),
    };
    if snapshot.target_slide_ids.is_empty() {
        snapshot.job_id = "append-empty".into();
    }
    let _ = target;
    let bundle = TemplateBundle::freeze(9, rich_bundle()).unwrap();
    let pages = vec![
        TemplatePage {
            key: "cover".into(),
            source_slide_id: SlideId(100),
            slots: vec![TemplateSlot { key: "title".into(), shape_id: ShapeId(101) }],
        },
        TemplatePage { key: "end".into(), source_slide_id: SlideId(200), slots: vec![TemplateSlot { key: "body".into(), shape_id: ShapeId(201) }] },
    ];
    let scope = AppendScope::freeze(snapshot.clone(), bundle, pages, 4, target).unwrap();
    let response = AppendResponse::AppendTemplate {
        snapshot,
        pages: vec![
            PageInstance {
                page_key: "cover".into(),
                local_id: LocalId(1),
                replacements: vec![SlotReplacement { slot_key: "title".into(), paragraphs: vec!["新的主张".into()] }],
                links: vec![PageLink { target_page_key: "end".into(), target_local_id: LocalId(2) }],
            },
            PageInstance {
                page_key: "end".into(),
                local_id: LocalId(2),
                replacements: vec![],
                links: vec![
                    PageLink { target_page_key: "cover".into(), target_local_id: LocalId(1) },
                    PageLink { target_page_key: "end".into(), target_local_id: LocalId(2) },
                ],
            },
        ],
    };
    AiCommit::Append { draft: AppendDraft::bind(scope, response).unwrap(), label: label.into() }
}

#[test]
fn template_append_installs_once_and_a_late_retry_does_not_add_pages() {
    let mut target = Presentation::default();
    target.notes_size = deckcraft_model::Size::new(50.0, 40.0);
    let notes = target.notes_size;
    target.slides.clear();
    target.slide_size = deckcraft_model::Size::new(100.0, 80.0);
    target.header_footer.footer_text = "old".into();
    let (mut session, uid) = open(target.clone());
    let source = rich_bundle();
    let outcome = session.apply_ai(append_commit(&target, uid, 1, vec![], "AI: 生成 2 页项目汇报")).unwrap();
    assert!(outcome.changed && !outcome.duplicate);
    assert_eq!(outcome.added_slide_ids.len(), 2);
    let state = session.active().unwrap();
    assert_eq!(state.history.undo.len(), 1);
    assert_eq!(state.history.undo[0].label, "AI: 生成 2 页项目汇报");
    assert_eq!(state.selection.slides, outcome.added_slide_ids);
    assert_eq!(state.doc.slide_size, source.slide_size);
    assert_eq!(state.doc.header_footer, source.header_footer);
    assert_eq!(state.doc.notes_size, notes);
    assert_eq!(state.doc.slides.len(), 2);
    let cover = state.doc.slide(outcome.added_slide_ids[0]).unwrap();
    let end = state.doc.slide(outcome.added_slide_ids[1]).unwrap();
    assert_ne!(cover.id, SlideId(100));
    assert_ne!(end.id, SlideId(200));
    assert_eq!(
        cover
            .shapes
            .iter()
            .find(|shape| shape.text.as_ref().is_some_and(|text| text.text() == "新的主张"))
            .unwrap()
            .text
            .as_ref()
            .unwrap()
            .paragraphs[0]
            .props
            .bullet,
        Some(Bullet::Picture { media: cover_media(cover) })
    );
    assert_eq!(
        cover.shapes.iter().find_map(|shape| match &shape.kind {
            ShapeKind::Picture { fill } => Some(fill.media),
            _ => None,
        }),
        Some(cover_media(cover))
    );
    assert!(matches!(cover.shapes.iter().find_map(|shape| shape.click.as_ref()).unwrap().action, Action::Slide { slide } if slide == end.id));
    assert!(
        matches!(cover.shapes.iter().find_map(|shape| shape.hover.as_ref()).unwrap().action, Action::Url { ref url } if url == "https://example.com/source")
    );
    let child = cover
        .shapes
        .iter()
        .find_map(|shape| match &shape.kind {
            ShapeKind::Group { children, .. } => children.first(),
            _ => None,
        })
        .unwrap();
    assert_ne!(child.id, ShapeId(104));
    assert!(
        matches!(child.text.as_ref().unwrap().paragraphs[0].runs[0].props.link.as_ref().unwrap().action, Action::Slide { slide } if slide == end.id)
    );
    let (master, layout) = state.doc.layout(cover.layout).unwrap();
    assert_eq!(master.layouts.len(), 1);
    assert_eq!(layout.id, cover.layout);
    assert_ne!(master.id, source.masters[0].id);
    assert!(matches!(master.body_style.level(0).unwrap().run.link.as_ref().unwrap().action, Action::Slide { slide } if slide == end.id));
    assert!(matches!(master.theme.format.fills.iter().find_map(|fill| match fill {
        Fill::Picture(picture) => Some(picture.media),
        _ => None,
    }).unwrap(), media if media == cover_media(cover)));
    let end_master = state.doc.layout(end.layout).unwrap().0;
    assert_ne!(end_master.id, master.id);
    assert!(matches!(end_master.body_style.level(0).unwrap().run.link.as_ref().unwrap().action, Action::Slide { slide } if slide == end.id));
    assert!(matches!(end.shapes[0].click.as_ref().unwrap().action, Action::Slide { slide } if slide == cover.id));
    assert_eq!(state.doc.media.iter().find(|item| item.id == cover_media(cover)).unwrap().data.as_ref(), png().as_slice());
    assert!(source.slide(SlideId(100)).is_some());

    session.execute("edit.undo", &json!({})).unwrap();
    assert!(session.active().unwrap().doc.slides.is_empty());
    let retry = session.apply_ai(append_commit(&target, uid, 1, vec![], "AI: 生成 2 页项目汇报")).unwrap();
    assert!(retry.duplicate);
    assert!(session.active().unwrap().doc.slides.is_empty());
    assert_eq!(session.active().unwrap().history.undo.len(), 0);
}

fn cover_media(slide: &Slide) -> deckcraft_model::MediaId {
    slide
        .shapes
        .iter()
        .find_map(|shape| match &shape.kind {
            ShapeKind::Picture { fill } => Some(fill.media),
            _ => None,
        })
        .unwrap()
}

#[test]
fn nonempty_append_keeps_existing_pages_and_rejects_a_stale_identity() {
    let mut target = Presentation::default();
    let layout = target.masters[0].layouts[0].id;
    target.slides = vec![Arc::new(Slide {
        id: SlideId(7),
        layout,
        shapes: vec![Shape { id: ShapeId(8), text: Some(TextBody::from_text("已有")), ..Default::default() }],
        ..Default::default()
    })];
    target.fix_next_id();
    let old_slide = Arc::clone(&target.slides[0]);
    let old_master = Arc::clone(&target.masters[0]);
    let footer = target.header_footer.clone();
    let (mut session, uid) = open(target.clone());
    let snapshot = SnapshotIdentity {
        job_id: "append-existing".into(),
        doc_uid: uid,
        base_content_revision: 1,
        target_slide_ids: vec![SlideId(7)],
        plan_version: 1,
        template_version: Some(3),
    };
    let mut native = Presentation::default();
    native.slides = vec![Arc::new(Slide {
        id: SlideId(50),
        layout: native.masters[0].layouts[0].id,
        shapes: vec![Shape { id: ShapeId(51), text: Some(TextBody::from_text("模板")), ..Default::default() }],
        ..Default::default()
    })];
    native.fix_next_id();
    let scope = AppendScope::freeze(
        snapshot.clone(),
        TemplateBundle::freeze(3, native).unwrap(),
        vec![TemplatePage {
            key: "only".into(),
            source_slide_id: SlideId(50),
            slots: vec![TemplateSlot { key: "title".into(), shape_id: ShapeId(51) }],
        }],
        2,
        &target,
    )
    .unwrap();
    let draft = AppendDraft::bind(
        scope,
        AppendResponse::AppendTemplate {
            snapshot: snapshot.clone(),
            pages: vec![PageInstance {
                page_key: "only".into(),
                local_id: LocalId(1),
                replacements: vec![SlotReplacement { slot_key: "title".into(), paragraphs: vec!["填好".into()] }],
                links: vec![],
            }],
        },
    )
    .unwrap();
    let outcome = session.apply_ai(AiCommit::Append { draft, label: "AI: 追加 1 页".into() }).unwrap();
    let state = session.active().unwrap();
    assert!(Arc::ptr_eq(&state.doc.slides[0], &old_slide));
    assert!(Arc::ptr_eq(&state.doc.masters[0], &old_master));
    assert_eq!(state.doc.header_footer, footer);
    assert_eq!(state.doc.slides.len(), 2);
    assert_eq!(state.doc.slide(outcome.added_slide_ids[0]).unwrap().shapes[0].text.as_ref().unwrap().text(), "填好");
    assert_ne!(state.doc.slide(outcome.added_slide_ids[0]).unwrap().layout, old_slide.layout);
}

#[test]
fn append_identity_mismatch_does_not_install() {
    let mut target = Presentation::default();
    let layout = target.masters[0].layouts[0].id;
    target.slides = vec![Arc::new(Slide { id: SlideId(7), layout, ..Default::default() })];
    target.fix_next_id();
    let (mut session, uid) = open(target.clone());
    let before = mark(&session, uid);
    let mut snapshot = SnapshotIdentity {
        job_id: "append-mismatch".into(),
        doc_uid: uid,
        base_content_revision: 1,
        target_slide_ids: vec![SlideId(99)],
        plan_version: 1,
        template_version: Some(3),
    };
    let mut native = Presentation::default();
    native.slides = vec![Arc::new(Slide {
        id: SlideId(50),
        layout: native.masters[0].layouts[0].id,
        shapes: vec![Shape { id: ShapeId(51), text: Some(TextBody::from_text("模板")), ..Default::default() }],
        ..Default::default()
    })];
    native.fix_next_id();
    let frozen = TemplateBundle::freeze(3, native).unwrap();
    let err = AppendScope::freeze(
        snapshot.clone(),
        frozen,
        vec![TemplatePage {
            key: "only".into(),
            source_slide_id: SlideId(50),
            slots: vec![TemplateSlot { key: "title".into(), shape_id: ShapeId(51) }],
        }],
        1,
        &target,
    )
    .unwrap_err();
    assert!(matches!(err, crate::ai_contract::ContractError::SnapshotMismatch));
    // A draft bound against a matching empty document must still be rechecked against the live target.
    let mut empty = Presentation::default();
    empty.slides.clear();
    snapshot.target_slide_ids.clear();
    snapshot.base_content_revision = 1;
    let scope = AppendScope::freeze(
        snapshot.clone(),
        TemplateBundle::freeze(3, {
            let mut native = Presentation::default();
            native.slides = vec![Arc::new(Slide {
                id: SlideId(50),
                layout: native.masters[0].layouts[0].id,
                shapes: vec![Shape { id: ShapeId(51), text: Some(TextBody::from_text("模板")), ..Default::default() }],
                ..Default::default()
            })];
            native.fix_next_id();
            native
        })
        .unwrap(),
        vec![TemplatePage {
            key: "only".into(),
            source_slide_id: SlideId(50),
            slots: vec![TemplateSlot { key: "title".into(), shape_id: ShapeId(51) }],
        }],
        1,
        &empty,
    )
    .unwrap();
    let draft = AppendDraft::bind(
        scope,
        AppendResponse::AppendTemplate {
            snapshot,
            pages: vec![PageInstance {
                page_key: "only".into(),
                local_id: LocalId(1),
                replacements: vec![SlotReplacement { slot_key: "title".into(), paragraphs: vec!["填好".into()] }],
                links: vec![],
            }],
        },
    )
    .unwrap();
    let err = session.apply_ai(AiCommit::Append { draft, label: "AI: 错文档".into() }).unwrap_err();
    assert!(matches!(err, ApplyError::Contract(_)));
    unchanged(&session, uid, &before);
}
