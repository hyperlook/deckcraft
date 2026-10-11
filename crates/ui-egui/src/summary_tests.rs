//! M1 integration evidence: fake transport is offline, not model-quality acceptance.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use deckcraft_ai::request::{ENDPOINT, Event, Failure, Host, ModelChoice, Request};
use deckcraft_model::{DeckSummary, PlanDraft, PlannedContent, SummaryPage};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

#[derive(Clone, Default)]
struct Wire {
    requests: Rc<RefCell<Vec<Request>>>,
    events: Rc<RefCell<VecDeque<Event>>>,
    cancelled: Rc<RefCell<usize>>,
}
impl Host for Wire {
    fn start(&mut self, request: Request) -> Result<(), Failure> {
        if let Request::Models { id } = &request {
            self.events.borrow_mut().push_back(Event::Models {
                id: *id,
                models: vec![ModelChoice { provider: "openai-codex".into(), model: "fixture".into(), endpoint: ENDPOINT.into() }],
                sdk_version: "offline".into(),
            });
        }
        self.requests.borrow_mut().push(request);
        Ok(())
    }
    fn poll(&mut self) -> Option<Event> {
        self.events.borrow_mut().pop_front()
    }
    fn cancel(&mut self) {
        *self.cancelled.borrow_mut() += 1;
    }
}
fn app() -> (SlideApp, Wire) {
    let wire = Wire::default();
    let app = SlideApp::new(deckcraft_engine::Session::with_new(), crate::Services { ai: Some(Box::new(wire.clone())), ..Default::default() });
    (app, wire)
}
fn poll(app: &mut SlideApp) {
    tick(app, &egui::Context::default());
}
fn ready(app: &mut SlideApp) -> u64 {
    poll(app);
    poll(app);
    assert_eq!(app.ai_pilot.coordinator.selected, Some(0));
    app.session.active().unwrap().uid
}
fn authorize(app: &mut SlideApp) {
    app.run("ai.authorize", json!({"on":true})).unwrap();
}
fn clarify(app: &mut SlideApp) {
    app.run("ai.chat", json!({"text":"社区花园介绍"})).unwrap();
    for (field, value) in [("purpose", "科普与介绍"), ("audience", "社区居民"), ("pageCount", "3 页")] {
        app.run("ai.reply", json!({"field":field,"value":value})).unwrap();
    }
}
fn respond(wire: &Wire) {
    match wire.requests.borrow().last().unwrap().clone() {
        Request::Plan { id, input, .. } => {
            let r: deckcraft_model::Requirements = serde_json::from_str(&input).unwrap();
            wire.events.borrow_mut().push_back(Event::Plan {
                id,
                result: PlanDraft {
                    mainline: "从问题引出社区参与路径".into(),
                    pages: (0..r.page_count)
                        .map(|i| PlannedContent {
                            title: format!("页职责 {}", i + 1), intent: "连接现状与行动；案例和数字待补资料".into()
                        })
                        .collect(),
                },
            });
        }
        Request::Summarize { id, input, .. } => {
            let input: Value = serde_json::from_str(&input).unwrap();
            let pages = input["pages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| {
                    let unknown = p["sources"].as_array().unwrap().is_empty();
                    SummaryPage {
                        page_id: deckcraft_model::SlideId(p["pageId"].as_u64().unwrap() as u32),
                        role: if unknown { "未知" } else { "连接问题与行动" }.into(),
                        intent: if unknown { "没有可读原文，无法判断" } else { "交代条件并引出社区行动路径" }.into(),
                        unknown,
                    }
                })
                .collect();
            wire.events
                .borrow_mut()
                .push_back(Event::Summary { id, result: DeckSummary { mainline: "从问题引出社区参与路径".into(), pages } });
        }
        _ => panic!("not a content request"),
    }
}
fn planned(app: &mut SlideApp, wire: &Wire) {
    ready(app);
    clarify(app);
    authorize(app);
    poll(app);
    respond(wire);
    poll(app);
    assert!(app.session.active().unwrap().doc.has_planned_pages());
}
fn imported(app: &mut SlideApp) -> std::sync::Arc<deckcraft_model::Presentation> {
    app.run("file.new", json!({})).unwrap();
    app.run("slide.new", json!({"title":"垃圾分类困难","body":"每周 42 千克\n回收条件有限"})).unwrap();
    app.run("slide.new", json!({"title":"社区行动","body":"周六开放堆肥课堂\n免费入场"})).unwrap();
    let bytes = deckcraft_engine::cmd::file::save_bytes(&app.session.active().unwrap().doc, "pptx").unwrap();
    app.run("file.openBytes", json!({"name":"public.pptx","data":deckcraft_engine::cmd::base64_encode(&bytes)})).unwrap();
    app.session.active().unwrap().doc.clone()
}
#[test]
fn empty_clarification_and_import_converge_in_the_same_project_workspace() {
    let (mut app, wire) = app();
    let uid = ready(&mut app);
    assert_eq!(wire.requests.borrow().len(), 1); // Auto models; never auto upload.
    clarify(&mut app);
    poll(&mut app);
    assert_eq!(wire.requests.borrow().len(), 1);
    assert!(app.session.active().unwrap().doc.workspace.is_none());
    let history = app.session.active().unwrap().history.undo.len();
    authorize(&mut app);
    poll(&mut app);
    assert!(matches!(wire.requests.borrow().last(), Some(Request::Plan { .. })));
    respond(&wire);
    poll(&mut app);
    let state = app.session.active().unwrap();
    assert_eq!(state.uid, uid);
    assert_eq!(app.session.documents().len(), 1); // No separate generator tab.
    assert_eq!(state.doc.slides.len(), 3);
    assert!(state.doc.slides.iter().all(|s| s.is_planned() && s.shapes.is_empty()));
    assert_eq!(state.doc.workspace.as_ref().unwrap().requirements.as_ref().unwrap().audience, "社区居民");
    assert_eq!(state.history.undo.len(), history + 1);
    assert!(state.doc.slides.windows(2).all(|s| s[0].id != s[1].id));
    let original = imported(&mut app);
    let import_uid = ready(&mut app);
    let requests = wire.requests.borrow().len();
    poll(&mut app);
    assert_eq!(wire.requests.borrow().len(), requests);
    authorize(&mut app);
    poll(&mut app);
    let Request::Summarize { input, .. } = wire.requests.borrow().last().unwrap().clone() else { panic!("wrong request") };
    assert!(input.contains("42") && input.contains("免费入场"));
    respond(&wire);
    poll(&mut app);
    let state = app.session.active().unwrap();
    assert_eq!(state.uid, import_uid);
    assert!(!state.doc.has_planned_pages());
    assert!(state.doc.workspace.is_some());
    for (before, after) in original.slides.iter().zip(&state.doc.slides) {
        assert_eq!(before.shapes, after.shapes);
        assert_eq!(before.id, after.id);
    }
    assert_eq!(app.ai_pilot.coordinator.conversations.get(&import_uid).unwrap().phase, Phase::Ready);
}
#[test]
fn planned_slots_reorder_adjust_undo_redo_save_and_reopen_without_model_calls() {
    let (mut app, wire) = app();
    planned(&mut app, &wire);
    let a = app.session.active().unwrap().doc.slides[0].id;
    app.run("slide.move", json!({"from":0,"to":2})).unwrap();
    app.run("ai.adjustPageIntent", json!({"pageId":a.0,"title":"用户调整","intent":"先说明边界，再介绍行动"})).unwrap();
    poll(&mut app);
    let doc = app.session.active().unwrap().doc.clone();
    assert_eq!(doc.slides[2].id, a);
    assert_eq!(doc.slides[2].ai_plan.as_ref().unwrap().title, "用户调整");
    app.run("edit.undo", json!({})).unwrap();
    poll(&mut app);
    assert_ne!(app.session.active().unwrap().doc.slides[2].ai_plan.as_ref().unwrap().title, "用户调整");
    app.run("edit.redo", json!({})).unwrap();
    poll(&mut app);
    assert_eq!(app.session.active().unwrap().doc, doc);
    let bytes = deckcraft_engine::cmd::file::save_bytes(&doc, "deckcraft").unwrap();
    let requests = wire.requests.borrow().len();
    app.run("file.openBytes", json!({"name":"project.deckcraft","data":deckcraft_engine::cmd::base64_encode(&bytes)})).unwrap();
    poll(&mut app);
    assert_eq!(app.session.active().unwrap().doc, doc);
    assert_eq!(wire.requests.borrow().len(), requests);
    assert!(deckcraft_engine::cmd::file::save_bytes(&doc, "pptx").is_err());
    assert!(deckcraft_engine::cmd::file::save_bytes(&doc, "pdf").is_err());
    assert!(app.run("file.render", json!({"slide":0})).is_err());
    app.run("slide.go", json!({"id":a.0})).unwrap();
    app.run("slide.delete", json!({})).unwrap();
    poll(&mut app);
    assert!(app.session.active().unwrap().doc.slide(a).is_none());
}
#[test]
fn one_plan_task_is_one_undo_and_undo_does_not_restart_it() {
    let (mut app, wire) = app();
    ready(&mut app);
    let original = app.session.active().unwrap().doc.clone();
    clarify(&mut app);
    authorize(&mut app);
    poll(&mut app);
    respond(&wire);
    poll(&mut app);
    let count = wire.requests.borrow().len();
    app.run("edit.undo", json!({})).unwrap();
    poll(&mut app);
    poll(&mut app);
    assert_eq!(app.session.active().unwrap().doc, original);
    assert_eq!(wire.requests.borrow().len(), count);
    app.run("edit.redo", json!({})).unwrap();
    poll(&mut app);
    assert!(app.session.active().unwrap().doc.has_planned_pages());
}
#[test]
fn hand_edits_refresh_sources_without_replacing_direction_or_adding_undo() {
    let (mut app, wire) = app();
    imported(&mut app);
    ready(&mut app);
    authorize(&mut app);
    poll(&mut app);
    respond(&wire);
    poll(&mut app);
    let intent = app.session.active().unwrap().doc.slides[1].ai_plan.clone();
    let uid = app.session.active().unwrap().uid;
    let shape = app.session.active().unwrap().doc.slides[1].shapes[0].id;
    app.run("slide.go", json!({"index":1})).unwrap();
    app.run("text.set", json!({"id":shape.0,"text":"手改后的新现状 99"})).unwrap();
    let undo = app.session.active().unwrap().history.undo.len();
    let count = wire.requests.borrow().len();
    let messages = app.ai_pilot.coordinator.conversations.get(&uid).unwrap().messages.len();
    poll(&mut app);
    assert_eq!(app.ai_pilot.coordinator.conversations.get(&uid).unwrap().messages.len(), messages);
    let state = app.session.active().unwrap();
    assert_eq!(state.doc.slides[1].ai_plan, intent);
    assert!(serde_json::to_string(&state.doc.workspace.as_ref().unwrap().evidence).unwrap().contains("99"));
    assert_eq!(state.history.undo.len(), undo);
    assert_eq!(wire.requests.borrow().len(), count);
    assert_eq!(app.ai_pilot.coordinator.conversations.get(&uid).unwrap().phase, Phase::Ready);
}
#[test]
fn cancel_close_tab_switch_and_late_results_never_retarget() {
    let (mut app, wire) = app();
    let uid = ready(&mut app);
    clarify(&mut app);
    authorize(&mut app);
    poll(&mut app);
    respond(&wire);
    app.run("ai.cancelRequest", json!({})).unwrap();
    poll(&mut app);
    assert!(app.session.active().unwrap().doc.workspace.is_none());
    app.run("ai.retry", json!({})).unwrap();
    poll(&mut app);
    app.run("file.new", json!({})).unwrap();
    let other_uid = app.session.active().unwrap().uid;
    let other = app.session.active().unwrap().doc.clone();
    respond(&wire);
    poll(&mut app);
    assert_eq!(app.session.active().unwrap().uid, other_uid);
    assert_eq!(app.session.active().unwrap().doc, other);
    assert!(app.session.documents().iter().find(|d| d.uid == uid).unwrap().doc.has_planned_pages());
    clarify(&mut app);
    authorize(&mut app);
    poll(&mut app);
    respond(&wire);
    app.run("file.close", json!({"discard":true})).unwrap();
    poll(&mut app);
    assert!(!app.ai_pilot.coordinator.conversations.contains_key(&other_uid));
    assert!(*wire.cancelled.borrow() >= 2);
}
#[test]
fn conflicting_manual_changes_preserve_an_isolated_candidate_and_wrong_output_writes_nothing() {
    let (mut app, wire) = app();
    ready(&mut app);
    clarify(&mut app);
    authorize(&mut app);
    poll(&mut app);
    app.run("slide.new", json!({"title":"用户临时加入"})).unwrap();
    let changed = app.session.active().unwrap().doc.clone();
    respond(&wire);
    poll(&mut app);
    assert_eq!(app.session.active().unwrap().doc, changed);
    assert!(!app.ai_pilot.coordinator.conflict_candidates.is_empty());
    let (mut app, wire) = self::app();
    ready(&mut app);
    clarify(&mut app);
    authorize(&mut app);
    poll(&mut app);
    respond(&wire);
    if let Some(Event::Plan { result, .. }) = wire.events.borrow_mut().back_mut() {
        result.pages.pop();
    }
    poll(&mut app);
    assert!(app.session.active().unwrap().doc.workspace.is_none());
}
#[test]
fn missing_bridge_and_oversized_sources_do_not_report_success() {
    let mut app = SlideApp::new(deckcraft_engine::Session::with_new(), Default::default());
    poll(&mut app);
    clarify(&mut app);
    poll(&mut app);
    assert!(app.session.active().unwrap().doc.workspace.is_none());
    assert!(app.run("ai.loadModels", json!({})).is_err());
    let (mut app, wire) = self::app();
    for _ in 0..64 {
        app.run("slide.new", json!({"title":"public"})).unwrap();
    }
    ready(&mut app);
    authorize(&mut app);
    poll(&mut app);
    assert!(app.session.active().unwrap().doc.workspace.is_none());
    assert_eq!(wire.requests.borrow().len(), 1);
}
#[test]
fn normal_chat_auto_loads_models_and_shows_choices_not_an_outline_console() {
    use egui_kittest::kittest::Queryable;
    let (app, wire) = app();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(680.0, 1200.0)).build_ui_state(
        |ui, app: &mut SlideApp| {
            tick(app, ui.ctx());
            show(app, ui);
        },
        app,
    );
    h.run_steps(3);
    assert!(h.query_all_by_label("加载 Pi 可用模型（不发送原文）").next().is_none());
    assert!(h.query_all_by_label("确认大纲（不改页面）").next().is_none());
    h.state_mut().run("ai.chat", json!({"text":"社区花园介绍"})).unwrap();
    h.run_steps(2);
    h.get_by_label("介绍与科普（推荐）").click();
    h.run_steps(2);
    h.get_by_label("一般受众（推荐）").click();
    h.run_steps(2);
    h.get_by_label("3 页").click();
    h.run_steps(2);
    h.get_by_label("允许此文稿使用所选模型").click();
    h.run_steps(2);
    respond(&wire);
    h.run_steps(3);
    assert!(h.state().session.active().unwrap().doc.has_planned_pages());
    assert!(h.query_all_by_label("上下文可用 · 页计划在原分页列表中").next().is_some());
}

#[path = "m1_visual_tests.rs"]
mod visual;
