//! Bounded, read-only page excerpts. This is not semantic understanding.
use deckcraft_engine::ai_contract::{MAX_OUTLINE_PAGES, MAX_POINT_CHARS, MAX_POINTS, MAX_TITLE_CHARS};
use deckcraft_model::{ContextDiagnostic as D, ContextDraft, ContextPage, ContextSource, PhType, Presentation, Shape, ShapeKind};

const MAX_SHAPES: usize = 4096;
const MAX_DEPTH: usize = 16;
const MAX_SOURCES: usize = 64;

pub fn extract_context(doc: &Presentation) -> ContextDraft {
    let mut draft = ContextDraft::default();
    if deckcraft_engine::ai_contract::validate_materials(&doc.materials).is_ok() {
        draft.materials = doc.materials.clone();
    } else {
        draft.diagnostics.push(D::Limit);
    }
    if doc.slides.len() > MAX_OUTLINE_PAGES {
        draft.diagnostics.push(D::Limit);
    }
    for slide in doc.slides.iter().take(MAX_OUTLINE_PAGES) {
        let mut page = ContextPage { page_id: slide.id, title: String::new(), points: vec![], unknown: false, sources: vec![], diagnostics: vec![] };
        if slide.shapes.is_empty() {
            diagnostic(&mut page, D::Empty);
        }
        if slide.hidden {
            diagnostic(&mut page, D::Hidden);
        }
        let mut budget = MAX_SHAPES;
        visit(&slide.shapes, &mut vec![], &mut budget, &mut page);
        if page.title.is_empty() && !page.points.is_empty() {
            let first = page.points.remove(0);
            if first.chars().count() > MAX_TITLE_CHARS {
                diagnostic(&mut page, D::Limit);
            }
            page.title = first.chars().take(MAX_TITLE_CHARS).collect();
            diagnostic(&mut page, D::OrdinaryText);
        }
        page.unknown = page.title.trim().is_empty();
        if page.sources.is_empty() {
            diagnostic(&mut page, D::NoText);
        }
        // Explicitly disclose the reading scope, including inheritance and notes.
        diagnostic(&mut page, D::ScopeExcluded);
        draft.pages.push(page);
    }
    draft
}

/// Reuse saved facts rather than re-reading the slide after hand edits.
pub fn review_existing(doc: &Presentation) -> ContextDraft {
    use deckcraft_engine::ai_contract::{validate_context, validate_materials, validate_outline};
    let valid = validate_outline(&doc.outline)
        .and_then(|_| validate_materials(&doc.materials))
        .and_then(|_| doc.context_draft.as_ref().map(validate_context).unwrap_or(Ok(())))
        .and_then(|_| doc.confirmed_context.as_ref().map(validate_context).unwrap_or(Ok(())));
    if valid.is_err() {
        return ContextDraft { diagnostics: vec![D::Limit], ..Default::default() };
    }
    if let Some(draft) = &doc.context_draft {
        return draft.clone();
    }
    let mut draft = doc.confirmed_context.clone().unwrap_or_default();
    draft.materials = doc.materials.clone();
    draft.pages.retain(|p| doc.slide(p.page_id).is_some());
    for slide in doc.slides.iter().take(MAX_OUTLINE_PAGES) {
        let agreed = doc.outline.pages.iter().find(|p| p.page_id == Some(slide.id));
        if let Some(page) = draft.pages.iter_mut().find(|p| p.page_id == slide.id) {
            if let Some(agreed) = agreed {
                page.title = agreed.title.clone();
                page.points = agreed.points.clone();
                page.unknown = false;
            }
        } else {
            if draft.pages.len() >= MAX_OUTLINE_PAGES {
                diagnostic_global(&mut draft, D::Limit);
                continue;
            }
            draft.pages.push(ContextPage {
                page_id: slide.id,
                title: agreed.map(|p| p.title.clone()).unwrap_or_default(),
                points: agreed.map(|p| p.points.clone()).unwrap_or_default(),
                unknown: agreed.is_none(),
                sources: vec![],
                diagnostics: vec![],
            });
        }
    }
    if doc.slides.len() > MAX_OUTLINE_PAGES {
        diagnostic_global(&mut draft, D::Limit);
    }
    draft
}

fn diagnostic_global(draft: &mut ContextDraft, d: D) {
    if !draft.diagnostics.contains(&d) {
        draft.diagnostics.push(d);
    }
}
fn diagnostic(page: &mut ContextPage, d: D) {
    if !page.diagnostics.contains(&d) {
        page.diagnostics.push(d);
    }
}

fn visit(shapes: &[Shape], path: &mut Vec<deckcraft_model::ShapeId>, budget: &mut usize, page: &mut ContextPage) {
    for shape in shapes {
        if *budget == 0 || path.len() >= MAX_DEPTH {
            diagnostic(page, D::Limit);
            return;
        }
        *budget -= 1;
        if shape.hidden {
            diagnostic(page, D::Hidden);
            continue;
        }
        if shape.ph_type().is_some_and(PhType::is_footer_kind) {
            diagnostic(page, D::FooterExcluded);
            continue;
        }
        path.push(shape.id);
        match &shape.kind {
            ShapeKind::Group { children, .. } => {
                diagnostic(page, D::Group);
                visit(children, path, budget, page);
            }
            ShapeKind::Picture { .. } => diagnostic(page, D::Picture),
            ShapeKind::Chart(_) => diagnostic(page, D::Chart),
            ShapeKind::Table(_) => diagnostic(page, D::Table),
            ShapeKind::Opaque { .. } | ShapeKind::Media(_) | ShapeKind::Ink { .. } => diagnostic(page, D::Unsupported),
            ShapeKind::Shape | ShapeKind::Connector { .. } => {
                if let Some(text) = &shape.text {
                    for (at, paragraph) in text.paragraphs.iter().take(MAX_SOURCES + 1).enumerate() {
                        if page.sources.len() >= MAX_SOURCES {
                            diagnostic(page, D::Limit);
                            break;
                        }
                        let title = shape.ph_type().is_some_and(PhType::is_title) && page.title.is_empty();
                        let limit = if title { MAX_TITLE_CHARS } else { MAX_POINT_CHARS };
                        // Do not allocate the complete input paragraph first.
                        let mut chars = paragraph.runs.iter().take(4096).flat_map(|r| r.content().chars());
                        let excerpt: String = chars.by_ref().take(limit).map(|c| if c.is_control() { ' ' } else { c }).collect();
                        if chars.next().is_some() || paragraph.runs.len() > 4096 {
                            diagnostic(page, D::Limit);
                        }
                        let excerpt = excerpt.trim().to_string();
                        if excerpt.is_empty() {
                            continue;
                        }
                        if !title && page.points.len() >= MAX_POINTS {
                            diagnostic(page, D::Limit);
                            break;
                        }
                        page.sources.push(ContextSource { shape_path: path.clone(), paragraph: at, text: excerpt.clone(), title });
                        if title {
                            page.title = excerpt;
                        } else {
                            page.points.push(excerpt);
                        }
                    }
                    if text.paragraphs.len() > MAX_SOURCES {
                        diagnostic(page, D::Limit);
                    }
                }
            }
        }
        path.pop();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use deckcraft_model::{Placeholder, ShapeId, Slide, SlideId, text::TextBody};
    use std::sync::Arc;
    fn text(id: u32, title: bool, value: &str) -> Shape {
        Shape {
            id: ShapeId(id),
            text: Some(TextBody::from_text(value)),
            ph: title.then_some(Placeholder { kind: PhType::Title, ..Default::default() }),
            ..Default::default()
        }
    }
    #[test]
    fn excerpts_have_stable_ids_and_do_not_write_the_deck() {
        let mut doc = Presentation::default();
        doc.slides =
            vec![Arc::new(Slide {
                id: SlideId(99), shapes: vec![text(1, true, "标题"), text(2, false, "要点一\n要点二")], ..Default::default()
            })];
        let before = doc.clone();
        let draft = extract_context(&doc);
        assert_eq!(doc, before);
        assert_eq!(draft.pages[0].page_id, SlideId(99));
        assert_eq!(draft.pages[0].title, "标题");
        assert_eq!(draft.pages[0].points.len(), 2);
        assert_eq!(draft.pages[0].sources[1].shape_path, vec![ShapeId(2)]);
    }
    #[test]
    fn ordinary_empty_and_unsupported_pages_are_not_understood() {
        let mut doc = Presentation::default();
        doc.slides = vec![
            Arc::new(Slide { id: SlideId(1), shapes: vec![text(7, false, "首段\n正文")], ..Default::default() }),
            Arc::new(Slide { id: SlideId(2), ..Default::default() }),
            Arc::new(Slide {
                id: SlideId(3),
                shapes: vec![Shape { kind: ShapeKind::Opaque { xml: String::new(), preview: None, label: "object".into() }, ..Default::default() }],
                ..Default::default()
            }),
        ];
        let draft = extract_context(&doc);
        assert_eq!(draft.pages[0].title, "首段");
        assert!(draft.pages[0].diagnostics.contains(&D::OrdinaryText));
        assert!(draft.pages[1].unknown && draft.pages[2].unknown);
        assert_eq!(draft.outline().pages.len(), 1);
    }
    #[test]
    fn groups_pictures_charts_and_footer_have_explicit_scope() {
        let mut doc = Presentation::default();
        let mut footer = text(6, false, "not a fact");
        footer.ph = Some(Placeholder { kind: PhType::Footer, ..Default::default() });
        doc.slides = vec![Arc::new(Slide {
            id: SlideId(9),
            shapes: vec![
                Shape {
                    id: ShapeId(1),
                    kind: ShapeKind::Group { children: vec![text(2, false, "组合标题")], child: Default::default() },
                    ..Default::default()
                },
                Shape { id: ShapeId(3), kind: ShapeKind::Picture { fill: Default::default() }, ..Default::default() },
                Shape { id: ShapeId(4), kind: ShapeKind::Chart(Box::default()), ..Default::default() },
                Shape { id: ShapeId(5), kind: ShapeKind::Table(Default::default()), ..Default::default() },
                footer,
            ],
            ..Default::default()
        })];
        let draft = extract_context(&doc);
        let p = &draft.pages[0];
        assert_eq!(p.title, "组合标题");
        assert_eq!(p.sources[0].shape_path, vec![ShapeId(1), ShapeId(2)]);
        for d in [D::Group, D::Picture, D::Chart, D::Table, D::FooterExcluded] {
            assert!(p.diagnostics.contains(&d));
        }
        assert!(!p.sources.iter().any(|s| s.text.contains("not a fact")));
    }

    #[test]
    fn reuse_never_silently_reextracts_hand_edits_and_restores_draft_as_draft() {
        let mut doc = Presentation::default();
        doc.slides = vec![Arc::new(Slide { id: SlideId(4), shapes: vec![text(2, true, "手改标题")], ..Default::default() })];
        doc.outline.upsert(SlideId(4), "已确认标题".into(), vec![]);
        assert_eq!(review_existing(&doc).pages[0].title, "已确认标题");
        assert_eq!(extract_context(&doc).pages[0].title, "手改标题");
        let mut draft = review_existing(&doc);
        draft.pages[0].title = "未确认标题".into();
        doc.context_draft = Some(draft);
        assert_eq!(review_existing(&doc).pages[0].title, "未确认标题");
        assert_eq!(doc.outline.pages[0].title, "已确认标题");
    }

    #[test]
    fn image_only_pages_and_deep_groups_remain_unknown() {
        let mut deep = text(2, false, "too deep");
        for id in 10..28 {
            deep = Shape { id: ShapeId(id), kind: ShapeKind::Group { children: vec![deep], child: Default::default() }, ..Default::default() };
        }
        let mut doc = Presentation::default();
        doc.slides = vec![
            Arc::new(Slide {
                id: SlideId(1),
                shapes: vec![Shape { id: ShapeId(3), kind: ShapeKind::Picture { fill: Default::default() }, ..Default::default() }],
                ..Default::default()
            }),
            Arc::new(Slide { id: SlideId(2), shapes: vec![deep], ..Default::default() }),
        ];
        let draft = extract_context(&doc);
        assert!(draft.pages[0].unknown && draft.pages[1].unknown);
        assert!(draft.pages[0].diagnostics.contains(&D::Picture));
        assert!(draft.pages[1].diagnostics.contains(&D::Limit));
        assert!(draft.outline().is_empty());
    }

    #[test]
    fn limits_are_explicit_not_silent() {
        let mut doc = Presentation::default();
        doc.slides =
            (1..=65).map(|id| Arc::new(Slide { id: SlideId(id), shapes: vec![text(1, true, &"字".repeat(501))], ..Default::default() })).collect();
        let draft = extract_context(&doc);
        assert_eq!(draft.pages.len(), 64);
        assert!(draft.diagnostics.contains(&D::Limit));
        assert_eq!(draft.pages[0].title.chars().count(), 500);
        assert!(draft.pages[0].diagnostics.contains(&D::Limit));
    }
}
