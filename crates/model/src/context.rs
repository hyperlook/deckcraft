//! Review metadata is separate from the agreed facts. Drafts never enter model briefs.
use crate::{Material, Outline, OutlinePage, ShapeId, SlideId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContextDiagnostic {
    Empty,
    NoText,
    OrdinaryText,
    Group,
    Picture,
    Chart,
    Table,
    Unsupported,
    ScopeExcluded,
    Hidden,
    FooterExcluded,
    Limit,
}

impl ContextDiagnostic {
    pub fn label(self) -> &'static str {
        match self {
            Self::Empty => "空页：请补充或保留未知",
            Self::NoText => "没有可读文字：未做 OCR / 图片理解",
            Self::OrdinaryText => "普通文本框：标题仅为首段候选，需核对",
            Self::Group => "组合文字只作摘录；组合不在写回范围",
            Self::Picture => "图片未理解；可手填图片说明",
            Self::Chart => "图表未理解（不推断趋势或数字）",
            Self::Table => "表格未提取",
            Self::Unsupported => "不支持理解的对象",
            Self::ScopeExcluded => "读取范围不含母版 / 布局文字和演讲者备注",
            Self::Hidden => "包含隐藏页 / 对象：隐藏对象文字未读，隐藏页仍需人工核对",
            Self::FooterExcluded => "页脚、日期与页码已排除",
            Self::Limit => "提取达到上限：候选不完整",
        }
    }
}

/// Original excerpt, including group ancestry. Edits do not overwrite this provenance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextSource {
    pub shape_path: Vec<ShapeId>,
    pub paragraph: usize,
    pub text: String,
    pub title: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextPage {
    pub page_id: SlideId,
    pub title: String,
    pub points: Vec<String>,
    /// An unknown page is coverage metadata, not an outline fact.
    pub unknown: bool,
    pub sources: Vec<ContextSource>,
    pub diagnostics: Vec<ContextDiagnostic>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextDraft {
    pub pages: Vec<ContextPage>,
    pub materials: Vec<Material>,
    pub diagnostics: Vec<ContextDiagnostic>,
}

impl ContextDraft {
    /// Only explicitly confirmed, non-unknown pages can be installed as facts.
    pub fn outline(&self) -> Outline {
        Outline {
            pages: self
                .pages
                .iter()
                .filter(|p| !p.unknown)
                .map(|p| OutlinePage { page_id: Some(p.page_id), title: p.title.clone(), points: p.points.clone() })
                .collect(),
        }
    }
}
