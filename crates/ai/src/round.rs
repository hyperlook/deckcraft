//! Assemble the only context a model is allowed to see for one round.

use deckcraft_engine::ai_contract::{self, ContractError, ContractResult, PageProposal};
use deckcraft_model::{Material, Outline, Presentation, SlideId};
use serde::Serialize;

/// Told to the model with every round. Facts come from the outline and the materials.
pub const FACT_RULE: &str = "Rewrite the cited page using the outline and the materials as the only facts. The comment says what feels wrong. The number of points may change. Do not keep the old paragraph count. Return a proposal for this page only.";

/// One round of context. Hand edits on the slide are not part of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Round {
    pub doc_uid: u64,
    pub page_id: SlideId,
    pub comment: String,
    pub outline: Outline,
    pub materials: Vec<Material>,
}

/// What is sent for this round. It carries no text-box map and no paragraph quota.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelBrief {
    pub instruction: &'static str,
    pub doc_uid: u64,
    pub page_id: SlideId,
    pub comment: String,
    pub outline: Outline,
    pub materials: Vec<Material>,
}

impl Round {
    pub fn assemble(doc_uid: u64, page_id: SlideId, comment: impl Into<String>, outline: Outline, materials: Vec<Material>) -> ContractResult<Self> {
        let comment = comment.into();
        if doc_uid == 0 || page_id.0 == 0 {
            return Err(ContractError::Schema);
        }
        ai_contract::validate_comment(&comment)?;
        ai_contract::validate_outline(&outline)?;
        ai_contract::validate_materials(&materials)?;
        Ok(Self { doc_uid, page_id, comment, outline, materials })
    }

    /// P1 readiness boundary: only saved facts, never the unconfirmed draft or live slide text.
    pub fn from_confirmed(doc_uid: u64, page_id: SlideId, comment: impl Into<String>, doc: &Presentation) -> ContractResult<Self> {
        ai_contract::validate_outline(&doc.outline)?;
        ai_contract::validate_materials(&doc.materials)?;
        if doc.slide(page_id).is_none() {
            return Err(ContractError::Schema);
        }
        if !doc.outline.pages.iter().any(|p| p.page_id == Some(page_id) && !p.title.trim().is_empty()) {
            return Err(ContractError::MissingContext);
        }
        Self::assemble(doc_uid, page_id, comment, doc.outline.clone(), doc.materials.clone())
    }

    pub fn brief(&self) -> ModelBrief {
        ModelBrief {
            instruction: FACT_RULE,
            doc_uid: self.doc_uid,
            page_id: self.page_id,
            comment: self.comment.clone(),
            outline: self.outline.clone(),
            materials: self.materials.clone(),
        }
    }

    /// A proposal is only for the page this round cited.
    pub fn proposal(&self, title: impl Into<String>, points: Vec<String>) -> ContractResult<PageProposal> {
        let proposal = PageProposal { doc_uid: self.doc_uid, page_id: self.page_id, title: title.into(), points, label: String::new() };
        proposal.validate()?;
        Ok(proposal)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use deckcraft_model::{MaterialKind, OutlinePage};

    fn round() -> Round {
        Round::assemble(
            7,
            SlideId(12),
            "这页看着平，重点没出来",
            Outline {
                pages: vec![OutlinePage {
                    page_id: Some(SlideId(12)), title: "旧主张".into(), points: vec!["甲".into(), "乙".into(), "丙".into()]
                }],
            },
            vec![Material { name: "访谈".into(), kind: MaterialKind::File, body: "用户说要先验证场景".into() }],
        )
        .unwrap()
    }

    #[test]
    fn the_brief_is_the_outline_the_comment_and_the_materials() {
        let brief = round().brief();
        assert_eq!(brief.page_id, SlideId(12));
        assert_eq!(brief.comment, "这页看着平，重点没出来");
        assert_eq!(brief.materials[0].body, "用户说要先验证场景");
        assert_eq!(brief.outline.pages[0].points.len(), 3);
        let wire = serde_json::to_value(&brief).unwrap();
        assert!(wire.get("paragraphs").is_none());
        assert!(wire.get("baseContentRevision").is_none());
        assert!(wire["instruction"].as_str().unwrap().contains("may change"));
    }

    #[test]
    fn a_proposal_may_have_fewer_points_than_the_outline() {
        let proposal = round().proposal("新主张", vec!["先验证场景".into()]).unwrap();
        assert_eq!(proposal.points.len(), 1);
        assert_eq!(proposal.page_id, SlideId(12));
    }

    #[test]
    fn drafts_and_live_text_are_not_target_facts() {
        let mut doc = Presentation::default();
        let page = doc.slides[0].id;
        doc.context_draft = Some(deckcraft_model::ContextDraft {
            pages: vec![deckcraft_model::ContextPage {
                page_id: page,
                title: "草稿不是事实".into(),
                points: vec![],
                unknown: false,
                sources: vec![],
                diagnostics: vec![],
            }],
            ..Default::default()
        });
        assert_eq!(Round::from_confirmed(1, page, "", &doc), Err(ContractError::MissingContext));
        doc.outline.upsert(page, "已确认".into(), vec![]);
        let r = Round::from_confirmed(1, page, "", &doc).unwrap();
        assert_eq!(r.brief().outline.pages[0].title, "已确认");
    }

    #[test]
    fn a_newline_inside_one_point_is_rejected() {
        let err = round().proposal("标题", vec!["一行\n另一行".into()]).unwrap_err();
        assert!(matches!(err, ContractError::Limit(_)));
    }
}
