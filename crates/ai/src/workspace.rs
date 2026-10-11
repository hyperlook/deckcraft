//! Shared, host-independent M1 coordinator. UI reads projections, the host performs I/O,
//! and every persistent effect is installed by an engine command on the originating document.
use crate::{
    request::{Failure, Host, Lifecycle, ModelChoice, Status},
    synthesis::Snapshot,
};
use deckcraft_model::{ContextDraft, Presentation, Requirements, WorkspaceContext};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};

#[derive(Clone)]
pub struct Document {
    pub uid: u64,
    pub revision: u64,
    pub interacting: bool,
    pub doc: Arc<Presentation>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Waiting,
    Clarifying,
    Preparing,
    Ready,
    Failed,
    Cancelled,
    Conflict,
}
#[derive(Clone, Debug)]
pub struct Message {
    pub user: bool,
    pub text: String,
}
#[derive(Clone, Debug)]
pub struct Question {
    pub field: &'static str,
    pub title: &'static str,
    pub choices: Vec<&'static str>,
}
#[derive(Clone)]
pub struct Conversation {
    pub phase: Phase,
    pub messages: Vec<Message>,
    pub actions: Vec<String>,
    pub requirements: Option<Requirements>,
    /// Consent is runtime-only and scoped to this document and the selected model.
    pub authorized: bool,
    attempted: bool,
    observed: Arc<Presentation>,
}
impl Conversation {
    fn new(doc: Arc<Presentation>) -> Self {
        Self {
            phase: Phase::Waiting,
            messages: vec![],
            actions: vec![],
            requirements: doc.workspace.as_ref().and_then(|w| w.requirements.clone()),
            authorized: false,
            attempted: false,
            observed: doc,
        }
    }
    fn message(&mut self, user: bool, text: impl Into<String>) {
        if self.messages.len() >= 64 {
            self.messages.remove(0);
        }
        self.messages.push(Message { user, text: text.into() });
    }
    fn action(&mut self, text: &str) {
        if self.actions.len() >= 32 {
            self.actions.remove(0);
        }
        self.actions.push(text.into());
    }
    pub fn question(&self) -> Option<Question> {
        if self.phase != Phase::Clarifying {
            return None;
        }
        let r = self.requirements.as_ref()?;
        if r.purpose.is_empty() {
            return Some(Question {
                field: "purpose",
                title: "这份演示主要用来做什么？",
                choices: vec!["介绍与科普（推荐）", "汇报进展", "说服与提案"],
            });
        }
        if r.audience.is_empty() {
            return Some(Question {
                field: "audience", title: "主要讲给谁听？", choices: vec!["一般受众（推荐）", "团队同事", "客户或决策者"]
            });
        }
        Some(Question { field: "pageCount", title: "希望多大规模？", choices: vec!["6 页（推荐）", "3 页", "12 页"] })
    }
}
struct Pending {
    uid: u64,
    baseline: Arc<Presentation>,
    snapshot: Option<Snapshot>,
    requirements: Option<Requirements>,
}
#[derive(Clone)]
pub struct Effect {
    pub uid: u64,
    pub params: Value,
    pub notify: bool,
}
#[derive(Default)]
pub struct Coordinator {
    pub life: Lifecycle,
    pub selected: Option<usize>,
    pub conversations: HashMap<u64, Conversation>,
    pending: Option<Pending>,
    models_started: bool,
    /// Isolated results survive a target conflict, but are never silently installed.
    pub conflict_candidates: HashMap<u64, Effect>,
}
impl Coordinator {
    pub fn choice(&self) -> Option<&ModelChoice> {
        self.selected.and_then(|i| self.life.models.get(i))
    }
    pub fn select(&mut self, index: usize) {
        if self.selected != Some(index) && self.life.models.get(index).is_some() {
            self.selected = Some(index);
            for c in self.conversations.values_mut() {
                c.authorized = false;
            }
        }
    }
    pub fn observe(&mut self, d: &Document) {
        self.conversations.entry(d.uid).or_insert_with(|| Conversation::new(d.doc.clone()));
    }
    pub fn authorize(&mut self, uid: u64, on: bool) {
        if on && self.choice().is_none() {
            return;
        }
        if let Some(c) = self.conversations.get_mut(&uid) {
            c.authorized = on;
        }
    }
    pub fn submit(&mut self, document: Document, text: &str) -> Result<(), String> {
        if self.life.busy() {
            return Err(Failure::Busy.label().into());
        }
        if !deckcraft_model::workspace::line(text, 2000) {
            return Err("需求为空、包含换行或超过 2000 字。".into());
        }
        let c = self.conversations.entry(document.uid).or_insert_with(|| Conversation::new(document.doc.clone()));
        c.message(true, text);
        c.observed = document.doc.clone();
        if document.doc.is_empty_start() {
            c.requirements = Some(Requirements { topic: text.into(), purpose: String::new(), audience: String::new(), page_count: 6 });
            c.phase = Phase::Clarifying;
            c.attempted = false;
            c.message(false, "先选几个影响页面结构的需求；也可以填写其他选项。之后页计划会出现在左侧页面列表。");
        } else if document.doc.workspace.is_some() && c.phase == Phase::Ready {
            c.message(false, "当前切片支持页计划：请在页面列表右键调整意图和顺序。生成真实页面和按聊天改页将在共享执行切片接入；本轮没有改页。");
        } else {
            // Treat user text as requirements, never source facts. Model summary still reads original sources.
            c.requirements =
                Some(Requirements { topic: text.into(), purpose: "理解现有文稿".into(), audience: "沿用原稿受众".into(), page_count: 6 });
            c.attempted = false;
            c.phase = Phase::Waiting;
        }
        Ok(())
    }
    pub fn reply(&mut self, uid: u64, field: &str, value: &str) -> Result<(), String> {
        let c = self.conversations.get_mut(&uid).ok_or("对话不存在。")?;
        let q = c.question().ok_or("当前没有需要回答的问题。")?;
        if field != q.field || !deckcraft_model::workspace::line(value, 200) {
            return Err("澄清回答不匹配或超限。".into());
        }
        let r = c.requirements.as_mut().ok_or("需求不存在。")?;
        match field {
            "purpose" => r.purpose = value.into(),
            "audience" => r.audience = value.into(),
            "pageCount" => {
                r.page_count =
                    value.split_whitespace().next().and_then(|s| s.parse().ok()).filter(|n| (3..=12).contains(n)).ok_or("页数应为 3–12。")?;
                c.phase = Phase::Waiting;
            }
            _ => return Err("未知问题。".into()),
        }
        c.message(true, value);
        Ok(())
    }
    pub fn retry(&mut self, uid: u64) {
        if !self.life.busy()
            && let Some(c) = self.conversations.get_mut(&uid)
        {
            c.phase = Phase::Waiting;
            c.attempted = false;
            self.conflict_candidates.remove(&uid);
        }
    }
    pub fn cancel(&mut self, host: Option<&mut dyn Host>) {
        if let Some(host) = host {
            host.cancel();
        }
        self.life.cancel();
        if let Some(p) = self.pending.take()
            && let Some(c) = self.conversations.get_mut(&p.uid)
        {
            c.phase = Phase::Cancelled;
            c.message(false, "已取消，未提交的页计划已撤去；原稿保留。");
        }
    }
    pub fn reload_models(&mut self) {
        if !self.life.busy() {
            self.models_started = false;
            self.selected = None;
            self.life.models.clear();
            for c in self.conversations.values_mut() {
                c.authorized = false;
            }
        }
    }
    pub fn acknowledge(&mut self, uid: u64, result: Result<(), String>, notify: bool) {
        if let Some(c) = self.conversations.get_mut(&uid) {
            match result {
                Ok(()) => {
                    c.phase = Phase::Ready;
                    if notify {
                        c.action("页计划已进入页面列表");
                        c.message(false, "页计划已就绪，可在原页面列表扫看、调整意图和顺序。计划页尚未生成真实 PPT；原有页面内容未改动。请保存 .deckcraft 继续工作。");
                    }
                }
                Err(e) => {
                    c.phase = Phase::Failed;
                    c.message(false, e);
                }
            }
        }
    }
    /// Called by the host main loop, independent of whether the assistant panel is visible.
    pub fn tick(&mut self, documents: &[Document], mut host: Option<&mut dyn Host>) -> Vec<Effect> {
        self.conversations.retain(|uid, _| documents.iter().any(|d| d.uid == *uid));
        self.conflict_candidates.retain(|uid, _| documents.iter().any(|d| d.uid == *uid));
        if self.pending.as_ref().is_some_and(|p| !documents.iter().any(|d| d.uid == p.uid)) {
            if let Some(h) = host.as_deref_mut() {
                h.cancel();
            }
            self.life.cancel();
            self.pending = None;
        }
        for d in documents {
            self.conversations.entry(d.uid).or_insert_with(|| Conversation::new(d.doc.clone()));
        }
        if let Some(h) = host.as_deref_mut() {
            for _ in 0..crate::request::QUEUE_LIMIT {
                let Some(event) = h.poll() else { break };
                self.life.receive(event);
            }
        }
        if matches!(self.life.status, Status::ModelsReady) && self.selected.is_none() && !self.life.models.is_empty() {
            self.selected = Some(0);
        }
        let mut effects = Vec::new();
        if !self.life.busy()
            && let Some(pending) = self.pending.take()
        {
            let result = self.result(&pending, documents);
            if let Some(c) = self.conversations.get_mut(&pending.uid) {
                match result {
                    Ok(Some(effect)) => {
                        c.action("页面计划校验通过");
                        effects.push(effect);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        c.phase = if self.conflict_candidates.contains_key(&pending.uid) { Phase::Conflict } else { Phase::Failed };
                        c.message(false, error);
                    }
                }
            }
        }
        if !self.models_started && !self.life.busy() && host.is_some() {
            self.models_started = true;
            if let Ok(request) = self.life.begin(None, "")
                && let Some(h) = host.as_deref_mut()
                && let Err(e) = h.start(request)
            {
                self.life.fail(e);
            }
            return effects;
        }
        if self.life.busy() || !effects.is_empty() {
            return effects;
        }
        for d in documents {
            if d.interacting {
                continue;
            }
            let Some(c) = self.conversations.get_mut(&d.uid) else { continue };
            // Arc identity avoids re-extracting every open deck on every UI frame.
            if c.phase == Phase::Ready && Arc::ptr_eq(&c.observed, &d.doc) {
                continue;
            }
            // Undo/redo and native reopen are projected from current persistent state, not runtime flags.
            if let Some(w) = &d.doc.workspace
                && valid_context(w)
                && d.doc.slides.iter().all(|s| s.ai_plan.as_ref().is_none_or(|p| p.validate()))
            {
                let current = evidence(&d.doc);
                if w.evidence == current && d.doc.slides.iter().all(|s| s.ai_plan.is_some()) {
                    c.phase = Phase::Ready;
                    c.observed = d.doc.clone();
                    continue;
                }
                // Source updates do not replace the user's expression direction or infer new facts.
                if deckcraft_engine::ai_contract::validate_context(&current).is_ok() && !limited(&current) {
                    let mut updated = w.clone();
                    updated.evidence = current;
                    effects.push(Effect {
                        uid: d.uid,
                        notify: false,
                        params: json!({"docUid":d.uid,"expectedRevision":d.revision,"context":updated,"derived":true}),
                    });
                    c.observed = d.doc.clone();
                    continue;
                }
                if c.phase != Phase::Failed {
                    c.message(false, "当前文字依据超限或不完整；保留原有表达方向，请减少内容后重试。");
                }
                c.phase = Phase::Failed;
                continue;
            }
            if c.phase == Phase::Ready {
                c.phase = Phase::Waiting;
                c.attempted = true;
            }
            if c.attempted || !c.authorized || c.phase == Phase::Clarifying || host.is_none() {
                continue;
            }
            if d.doc.is_empty_start() && c.requirements.as_ref().is_none_or(|r| !r.validate()) {
                continue;
            }
            let Some(choice) = self.selected.and_then(|i| self.life.models.get(i)).cloned() else { continue };
            c.attempted = true;
            let prepared = if d.doc.is_empty_start() {
                serde_json::to_string(&c.requirements)
                    .map_err(|_| Failure::Protocol)
                    .and_then(|input| self.life.begin_plan(&choice, &input))
                    .map(|r| (r, None))
            } else {
                Snapshot::extract(&d.doc).and_then(|mut snapshot| {
                    if let Some(requirements) = &c.requirements {
                        let mut e = snapshot.evidence.clone();
                        e.materials.push(deckcraft_model::Material {
                            name: "本轮需求（非原文事实）".into(),
                            body: requirements.topic.clone(),
                            kind: Default::default(),
                        });
                        snapshot = Snapshot::from_evidence(e)?;
                    }
                    self.life.begin_summary(&choice, &snapshot).map(|r| (r, Some(snapshot)))
                })
            };
            match prepared {
                Ok((request, snapshot)) => {
                    if let Some(h) = host.as_deref_mut() {
                        match h.start(request) {
                            Ok(()) => {
                                c.phase = Phase::Preparing;
                                c.action(if snapshot.is_some() { "理解原稿" } else { "组织页面" });
                                self.pending = Some(Pending { uid: d.uid, baseline: d.doc.clone(), snapshot, requirements: c.requirements.clone() });
                            }
                            Err(e) => {
                                self.life.fail(e);
                                c.phase = Phase::Failed;
                                c.message(false, e.label());
                            }
                        }
                    }
                }
                Err(e) => {
                    c.phase = Phase::Failed;
                    c.message(false, e.label());
                }
            }
            break; // One active host request, never per-page agents.
        }
        effects
    }
    fn result(&mut self, p: &Pending, documents: &[Document]) -> Result<Option<Effect>, String> {
        let Some(current) = documents.iter().find(|d| d.uid == p.uid) else { return Ok(None) };
        let params = match (&self.life.status, &p.snapshot) {
            (Status::Summary(summary), Some(snapshot)) => {
                snapshot.review(summary.clone()).map_err(|e| e.label().to_string())?;
                // User requirements stay separate from source evidence/materials.
                json!({"docUid":p.uid,"expectedRevision":current.revision,"context":WorkspaceContext { version:1, requirements:p.requirements.clone(), mainline:summary.mainline.clone(), evidence:evidence(&p.baseline) },"summary":summary,"derived":true})
            }
            (Status::Plan(plan), None) if p.requirements.as_ref().is_some_and(|r| r.page_count == plan.pages.len()) => {
                json!({"docUid":p.uid,"expectedRevision":current.revision,"context":WorkspaceContext { version:1, requirements:p.requirements.clone(), mainline:plan.mainline.clone(), evidence:evidence(&p.baseline) },"plan":plan})
            }
            (Status::Failed(e), _) => return Err(e.label().into()),
            (Status::Cancelled, _) => return Ok(None),
            _ => return Err(Failure::InvalidOutput.label().into()),
        };
        let effect = Effect { uid: p.uid, params, notify: true };
        if current.doc != p.baseline || current.interacting {
            self.conflict_candidates.insert(p.uid, effect);
            return Err("页面在任务期间已变化；已保留隔离候选，未覆盖手改。请选择按当前文稿重新组织。".into());
        }
        Ok(Some(effect))
    }
}
pub fn evidence(doc: &Presentation) -> ContextDraft {
    if doc.is_empty_start() {
        return ContextDraft { materials: doc.materials.clone(), ..Default::default() };
    }
    let mut source = doc.clone();
    source.slides.retain(|s| !s.is_planned());
    crate::extract_context(&source)
}
fn valid_context(w: &WorkspaceContext) -> bool {
    w.version == 1
        && deckcraft_model::workspace::line(&w.mainline, 2000)
        && w.requirements.as_ref().is_none_or(Requirements::validate)
        && deckcraft_engine::ai_contract::validate_context(&w.evidence).is_ok()
        && !limited(&w.evidence)
}
#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;

fn limited(e: &ContextDraft) -> bool {
    use deckcraft_model::ContextDiagnostic::Limit;
    e.diagnostics.contains(&Limit) || e.pages.iter().any(|p| p.diagnostics.contains(&Limit))
}
