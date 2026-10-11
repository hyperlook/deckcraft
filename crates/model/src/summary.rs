//! Versioned expression plan. Original excerpts are kept separately from model inference.
use crate::{ContextDraft, SlideId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SummaryPage {
    pub page_id: SlideId,
    pub role: String,
    pub intent: String,
    pub unknown: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeckSummary {
    pub mainline: String,
    pub pages: Vec<SummaryPage>,
}
impl DeckSummary {
    /// Short expression intent, not the slide's full title/body contract.
    pub fn validate(&self) -> bool {
        fn line(s: &str, limit: usize) -> bool {
            !s.trim().is_empty() && s.chars().count() <= limit && !s.chars().any(char::is_control)
        }
        line(&self.mainline, 2000)
            && !self.pages.is_empty()
            && self.pages.len() <= 64
            && self.pages.iter().enumerate().all(|(at, p)| {
                p.page_id.0 != 0 && line(&p.role, 200) && line(&p.intent, 500) && !self.pages.iter().take(at).any(|other| other.page_id == p.page_id)
            })
    }
}

/// Version 1 means semantic synthesis, unlike legacy Outline/ContextDraft excerpts.
/// Draft and confirmed slots use the same payload but have different acceptance states.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SummaryReview {
    pub version: u32,
    pub summary: DeckSummary,
    pub evidence: ContextDraft,
}
