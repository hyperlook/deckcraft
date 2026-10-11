//! Persistent project intent. Requirements and inferred plans are not source facts.
use crate::{ContextDraft, Presentation};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Requirements {
    pub topic: String,
    pub purpose: String,
    pub audience: String,
    pub page_count: usize,
}
impl Requirements {
    pub fn validate(&self) -> bool {
        line(&self.topic, 2000) && line(&self.purpose, 200) && line(&self.audience, 200) && (3..=12).contains(&self.page_count)
    }
}
pub fn line(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.chars().count() <= max && !s.chars().any(char::is_control)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PageState {
    Planned,
    Existing,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageIntent {
    pub title: String,
    pub intent: String,
    pub unknown: bool,
    pub state: PageState,
}
impl PageIntent {
    pub fn validate(&self) -> bool {
        line(&self.title, 200) && line(&self.intent, 500)
    }
}
/// Model returns content only; slot IDs and state are always assigned by the engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannedContent {
    pub title: String,
    pub intent: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanDraft {
    pub mainline: String,
    pub pages: Vec<PlannedContent>,
}
impl PlanDraft {
    pub fn validate(&self) -> bool {
        line(&self.mainline, 2000) && (3..=12).contains(&self.pages.len()) && self.pages.iter().all(|p| line(&p.title, 200) && line(&p.intent, 500))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceContext {
    pub version: u32,
    pub requirements: Option<Requirements>,
    pub mainline: String,
    /// Current extracted sources, not the inferred expression direction.
    pub evidence: ContextDraft,
}
impl Presentation {
    pub fn has_planned_pages(&self) -> bool {
        self.slides.iter().any(|s| s.is_planned())
    }
    /// Only an untouched empty starter may be replaced with planned slots.
    pub fn is_empty_start(&self) -> bool {
        self.slides.len() <= 1
            && self.sections.is_empty()
            && self.custom_shows.is_empty()
            && self.slides.iter().all(|s| {
                s.ai_plan.is_none()
                    && !s.hidden
                    && s.background.is_none()
                    && s.transition.is_none()
                    && s.animations.is_empty()
                    && s.raw_ext.is_none()
                    && s.notes.is_empty()
                    && s.comments.is_empty()
                    && s.shapes.iter().all(|sh| {
                        matches!(sh.kind, crate::ShapeKind::Shape) && sh.ph_type().is_some() && sh.text.as_ref().is_none_or(|t| t.is_empty())
                    })
            })
    }
}
