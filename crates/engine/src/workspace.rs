//! Atomic installation of project intent and explicit planned page slots.
use crate::ai_apply::{ApplyError, StoreOutcome};
use crate::{EngineError, Result, Session};
use deckcraft_model::{PageIntent, PageState, PlanDraft, Presentation, Slide, SlideId, WorkspaceContext};
use serde_json::Value;
use std::sync::Arc;

impl Session {
    pub fn install_workspace(
        &mut self,
        uid: u64,
        expected_revision: u64,
        context: WorkspaceContext,
        plan: Option<PlanDraft>,
        derived: bool,
    ) -> Result<StoreOutcome> {
        let state = self.documents().iter().find(|s| s.uid == uid).ok_or(EngineError::NoDocument)?;
        if state.revision != expected_revision {
            return Err(EngineError::Other("文稿已变化；候选没有覆盖当前页面，请重新组织。".into()));
        }
        validate(&context, plan.as_ref())?;
        if derived && plan.is_some() {
            return Err(EngineError::Other("后台更新不能创建计划页。".into()));
        }
        if derived {
            // A derived source refresh is not a user undo step. Preserve the adopted directions.
            let index = self.doc_index_by_uid(uid).ok_or(EngineError::NoDocument)?;
            let state = self.doc_at_mut(index).ok_or(EngineError::NoDocument)?;
            if state.interaction.is_some() {
                return Err(EngineError::Other("请先结束当前拖动。".into()));
            }
            if let Some(previous) = &state.doc.workspace
                && (previous.mainline != context.mainline || previous.requirements != context.requirements)
            {
                return Err(EngineError::Other("后台刷新不能替换已采用的表达方向。".into()));
            }
            let mut candidate = (*state.doc).clone();
            install(&mut candidate, context, None)?;
            let changed = candidate != *state.doc;
            if changed {
                state.doc = Arc::new(candidate);
                state.revision = state.revision.saturating_add(1);
            }
            return Ok(StoreOutcome { doc_uid: uid, changed });
        }
        self.guarded_edit("ai.installWorkspace", uid, "AI: 组织页计划".into(), move |doc| install(doc, context, plan))
            .map(|changed| StoreOutcome { doc_uid: uid, changed })
            .map_err(EngineError::from)
    }
}
fn validate(context: &WorkspaceContext, plan: Option<&PlanDraft>) -> Result<()> {
    if context.version != 1
        || !deckcraft_model::workspace::line(&context.mainline, 2000)
        || context.requirements.as_ref().is_some_and(|r| !r.validate())
    {
        return Err(EngineError::Other("项目上下文无效。".into()));
    }
    crate::ai_contract::validate_context(&context.evidence).map_err(|e| EngineError::Other(e.to_string()))?;
    use deckcraft_model::ContextDiagnostic::Limit;
    if context.evidence.diagnostics.contains(&Limit) || context.evidence.pages.iter().any(|p| p.diagnostics.contains(&Limit)) {
        return Err(EngineError::Other("不能将不完整提取标记为可用上下文。".into()));
    }
    if let Some(p) = plan
        && (!p.validate() || context.requirements.as_ref().is_none_or(|r| r.page_count != p.pages.len()) || context.mainline != p.mainline)
    {
        return Err(EngineError::Other("页计划与需求不匹配。".into()));
    }
    Ok(())
}
fn install(doc: &mut Presentation, context: WorkspaceContext, plan: Option<PlanDraft>) -> std::result::Result<(), ApplyError> {
    let expected: Vec<_> = if doc.is_empty_start() { vec![] } else { doc.slides.iter().filter(|s| !s.is_planned()).map(|s| s.id).collect() };
    if !expected.iter().copied().eq(context.evidence.pages.iter().map(|p| p.page_id)) {
        return Err(ApplyError::PageGone);
    }
    if let Some(plan) = plan {
        if !doc.is_empty_start() {
            return Err(ApplyError::PageGone);
        }
        doc.fix_next_id();
        let count = u32::try_from(plan.pages.len()).map_err(|_| ApplyError::PageGone)?;
        doc.next_id.checked_add(count).ok_or(ApplyError::PageGone)?;
        let layout = deckcraft_model::defaults::layout_of_kind(doc, deckcraft_model::LayoutType::Blank)
            .or_else(|| doc.masters.first().and_then(|m| m.layouts.first()).map(|l| l.id))
            .ok_or(ApplyError::PageGone)?;
        let mut slots = Vec::with_capacity(plan.pages.len());
        for p in plan.pages {
            doc.next_id = doc.next_id.checked_add(1).ok_or(ApplyError::PageGone)?;
            slots.push(Arc::new(Slide {
                id: SlideId(doc.next_id),
                layout,
                ai_plan: Some(PageIntent { title: p.title, intent: p.intent, unknown: false, state: PageState::Planned }),
                ..Default::default()
            }));
        }
        doc.slides = slots;
    }
    for slide in &mut doc.slides {
        if slide.ai_plan.is_none() {
            let title = slide.title();
            Arc::make_mut(slide).ai_plan = Some(PageIntent {
                title: if title.trim().is_empty() { "新增页".into() } else { title.chars().filter(|c| !c.is_control()).take(200).collect() },
                intent: "此页尚未建立表达意图；请直接调整，原文依据已更新。".into(),
                unknown: true,
                state: PageState::Existing,
            });
        }
    }
    doc.workspace = Some(context);
    Ok(())
}

/// Imported inference uses the same command boundary but never modifies source shapes.
pub fn imported_intents(doc: &mut Presentation, summary: &deckcraft_model::DeckSummary) -> std::result::Result<(), ApplyError> {
    if !summary.validate() || !doc.slides.iter().map(|s| s.id).eq(summary.pages.iter().map(|p| p.page_id)) {
        return Err(ApplyError::PageGone);
    }
    for (slide, page) in doc.slides.iter_mut().zip(&summary.pages) {
        if slide.ai_plan.is_none() {
            Arc::make_mut(slide).ai_plan =
                Some(PageIntent { title: page.role.clone(), intent: page.intent.clone(), unknown: page.unknown, state: PageState::Existing });
        }
    }
    Ok(())
}

pub fn command(session: &mut Session, p: &Value) -> Result<Value> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Params {
        doc_uid: u64,
        expected_revision: u64,
        context: WorkspaceContext,
        plan: Option<PlanDraft>,
        summary: Option<deckcraft_model::DeckSummary>,
        #[serde(default)]
        derived: bool,
    }
    let p: Params = serde_json::from_value(p.clone()).map_err(|_| EngineError::Other("无效的项目参数。".into()))?;
    if let Some(summary) = p.summary {
        if p.plan.is_some() || p.context.mainline != summary.mainline {
            return Err(EngineError::Other("不能混合两个起点。".into()));
        }
        validate(&p.context, None)?;
        let state = session.documents().iter().find(|s| s.uid == p.doc_uid).ok_or(EngineError::NoDocument)?;
        if state.revision != p.expected_revision {
            return Err(EngineError::Other("文稿已变化；未覆盖当前页面。".into()));
        }
        let review = deckcraft_model::SummaryReview { version: 1, summary: summary.clone(), evidence: p.context.evidence.clone() };
        crate::ai_contract::validate_summary(&review).map_err(|e| EngineError::Other(e.to_string()))?;
        // First import understanding is derived metadata, not a change to slide content.
        let index = session.doc_index_by_uid(p.doc_uid).ok_or(EngineError::NoDocument)?;
        let state = session.doc_at_mut(index).ok_or(EngineError::NoDocument)?;
        if state.interaction.is_some() {
            return Err(EngineError::Other("请先结束当前拖动。".into()));
        }
        let mut candidate = (*state.doc).clone();
        imported_intents(&mut candidate, &summary)?;
        candidate.workspace = Some(p.context);
        state.doc = Arc::new(candidate);
        state.revision = state.revision.saturating_add(1);
        return Ok(serde_json::json!({"docUid": p.doc_uid,"changed":true}));
    }
    serde_json::to_value(session.install_workspace(p.doc_uid, p.expected_revision, p.context, p.plan, p.derived)?)
        .map_err(|e| EngineError::Other(e.to_string()))
}

pub fn adjust(session: &mut Session, p: &Value) -> Result<Value> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Params {
        doc_uid: Option<u64>,
        page_id: SlideId,
        title: String,
        intent: String,
    }
    let p: Params = serde_json::from_value(p.clone()).map_err(|_| EngineError::Other("无效的页面意图。".into()))?;
    if !deckcraft_model::workspace::line(&p.title, 200) || !deckcraft_model::workspace::line(&p.intent, 500) {
        return Err(EngineError::Other("页面意图为空或超限。".into()));
    }
    let uid = p.doc_uid.or_else(|| session.active().map(|d| d.uid)).ok_or(EngineError::NoDocument)?;
    let changed = session.guarded_edit("ai.adjustPageIntent", uid, "调整页面意图".into(), move |doc| {
        let plan = doc.slide_mut(p.page_id).and_then(|s| s.ai_plan.as_mut()).ok_or(ApplyError::PageGone)?;
        plan.title = p.title;
        plan.intent = p.intent;
        // Editing inferred intent does not promote it to a source fact.
        Ok(())
    })?;
    Ok(serde_json::json!({"changed":changed}))
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
