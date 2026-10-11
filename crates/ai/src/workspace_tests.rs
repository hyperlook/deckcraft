#![allow(clippy::unwrap_used)]
use super::*;
use crate::request::{ENDPOINT, Event, ModelChoice, Request};
use std::collections::VecDeque;
#[derive(Default)]
struct Wire {
    requests: Vec<Request>,
    events: VecDeque<Event>,
}
impl Host for Wire {
    fn start(&mut self, r: Request) -> Result<(), Failure> {
        self.requests.push(r);
        Ok(())
    }
    fn poll(&mut self) -> Option<Event> {
        self.events.pop_front()
    }
    fn cancel(&mut self) {}
}
fn document() -> Document {
    Document { uid: 11, revision: 1, interacting: false, doc: Arc::new(Presentation::default()) }
}
fn ready(c: &mut Coordinator, w: &mut Wire, d: &Document) {
    c.tick(std::slice::from_ref(d), Some(w));
    let id = w.requests.last().unwrap().id();
    w.events.push_back(Event::Models {
        id,
        models: vec![ModelChoice { provider: "openai-codex".into(), model: "fixture".into(), endpoint: ENDPOINT.into() }],
        sdk_version: "offline".into(),
    });
    c.tick(std::slice::from_ref(d), Some(w));
}
#[test]
fn coordinator_needs_no_egui_and_never_sends_sources_without_document_consent() {
    let mut c = Coordinator::default();
    let mut w = Wire::default();
    let d = document();
    ready(&mut c, &mut w, &d);
    c.submit(d.clone(), "社区花园").unwrap();
    assert!(c.reply(d.uid, "audience", "公众").is_err()); // Questions have identities, not visible indices.
    c.reply(d.uid, "purpose", "介绍").unwrap();
    c.reply(d.uid, "audience", "公众").unwrap();
    c.reply(d.uid, "pageCount", "3 页").unwrap();
    c.tick(std::slice::from_ref(&d), Some(&mut w));
    assert_eq!(w.requests.len(), 1);
    c.authorize(d.uid, true);
    c.tick(std::slice::from_ref(&d), Some(&mut w));
    let Request::Plan { id, input, .. } = w.requests.last().unwrap() else { panic!("wrong op") };
    let requirements: Requirements = serde_json::from_str(input).unwrap();
    assert_eq!(requirements.page_count, 3);
    w.events.push_back(Event::Plan {
        id: *id,
        result: deckcraft_model::PlanDraft {
            mainline: "问题到行动".into(),
            pages: (0..3).map(|_| deckcraft_model::PlannedContent { title: "职责".into(), intent: "案例待补，连接行动".into() }).collect(),
        },
    });
    let effects = c.tick(std::slice::from_ref(&d), Some(&mut w));
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].uid, d.uid);
    assert!(d.doc.workspace.is_none()); // Coordinator is read-only.
    assert_eq!(effects[0].params["docUid"], 11);
}
#[test]
fn wrong_result_type_and_wrong_count_never_emit_engine_effects() {
    for wrong_type in [true, false] {
        let mut c = Coordinator::default();
        let mut w = Wire::default();
        let d = document();
        ready(&mut c, &mut w, &d);
        c.submit(d.clone(), "社区花园").unwrap();
        c.reply(d.uid, "purpose", "介绍").unwrap();
        c.reply(d.uid, "audience", "公众").unwrap();
        c.reply(d.uid, "pageCount", "3 页").unwrap();
        c.authorize(d.uid, true);
        c.tick(std::slice::from_ref(&d), Some(&mut w));
        let id = w.requests.last().unwrap().id();
        w.events.push_back(if wrong_type {
            Event::Result { id, result: crate::request::Content { title: "旧内容".into(), points: vec![] } }
        } else {
            Event::Plan {
                id,
                result: deckcraft_model::PlanDraft {
                    mainline: "不匹配规模".into(),
                    pages: (0..4).map(|_| deckcraft_model::PlannedContent { title: "职责".into(), intent: "案例待补".into() }).collect(),
                },
            }
        });
        assert!(c.tick(std::slice::from_ref(&d), Some(&mut w)).is_empty());
        assert_eq!(c.conversations.get(&d.uid).unwrap().phase, Phase::Failed);
    }
}
#[test]
fn cancellation_never_reactivates_on_a_late_model_response() {
    let mut c = Coordinator::default();
    let mut w = Wire::default();
    let d = document();
    ready(&mut c, &mut w, &d);
    c.submit(d.clone(), "主题").unwrap();
    c.reply(d.uid, "purpose", "介绍").unwrap();
    c.reply(d.uid, "audience", "公众").unwrap();
    c.reply(d.uid, "pageCount", "3 页").unwrap();
    c.authorize(d.uid, true);
    c.tick(std::slice::from_ref(&d), Some(&mut w));
    let id = w.requests.last().unwrap().id();
    c.cancel(Some(&mut w));
    w.events.push_back(Event::Plan {
        id,
        result: deckcraft_model::PlanDraft {
            mainline: "迟到".into(),
            pages: (0..3).map(|_| deckcraft_model::PlannedContent { title: "迟到".into(), intent: "不得写入".into() }).collect(),
        },
    });
    assert!(c.tick(std::slice::from_ref(&d), Some(&mut w)).is_empty());
    assert_eq!(c.conversations.get(&d.uid).unwrap().phase, Phase::Cancelled);
}
#[test]
fn pointer_interaction_defers_start_and_prevents_install_of_completed_results() {
    let mut c = Coordinator::default();
    let mut w = Wire::default();
    let mut d = document();
    ready(&mut c, &mut w, &d);
    c.submit(d.clone(), "主题").unwrap();
    c.reply(d.uid, "purpose", "介绍").unwrap();
    c.reply(d.uid, "audience", "公众").unwrap();
    c.reply(d.uid, "pageCount", "3 页").unwrap();
    c.authorize(d.uid, true);
    d.interacting = true;
    c.tick(std::slice::from_ref(&d), Some(&mut w));
    assert_eq!(w.requests.len(), 1);
    d.interacting = false;
    c.tick(std::slice::from_ref(&d), Some(&mut w));
    let id = w.requests.last().unwrap().id();
    d.interacting = true;
    w.events.push_back(Event::Plan {
        id,
        result: deckcraft_model::PlanDraft {
            mainline: "任务期间正在手改".into(),
            pages: (0..3).map(|_| deckcraft_model::PlannedContent { title: "角色".into(), intent: "不得覆盖编辑".into() }).collect(),
        },
    });
    assert!(c.tick(std::slice::from_ref(&d), Some(&mut w)).is_empty());
    assert_eq!(c.conversations.get(&d.uid).unwrap().phase, Phase::Conflict);
    assert!(c.conflict_candidates.contains_key(&d.uid));
}
