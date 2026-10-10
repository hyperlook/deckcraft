#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use deckcraft_model::style::{Fill, PictureFill};
use deckcraft_model::text::{Action, Hyperlink};
use deckcraft_model::{MediaItem, Slide, TextBody, Xfrm};
use serde_json::json;

fn empty_target() -> Presentation {
    let mut target = Presentation::default();
    target.slides.clear();
    target
}
fn png() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(1, 1).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn link(slide: u32) -> Hyperlink {
    Hyperlink { action: Action::Slide { slide: SlideId(slide) }, tooltip: String::new(), highlight_click: false }
}

fn native_fixture() -> Presentation {
    let mut doc = Presentation::default();
    let layout = doc.masters[0].layouts[0].id;
    doc.slides = vec![
        Arc::new(Slide {
            id: SlideId(100),
            layout,
            shapes: vec![
                Shape { id: ShapeId(101), text: Some(TextBody::from_text("Original title")), ..Default::default() },
                Shape {
                    id: ShapeId(102),
                    kind: ShapeKind::Picture { fill: PictureFill { media: MediaId(300), ..Default::default() } },
                    click: Some(link(200)),
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
    // Bytes are an original synthetic fixture; no image decoding is performed by the contract.
    doc.media.push(MediaItem {
        id: MediaId(300),
        name: "original-test.png".into(),
        content_type: "image/png".into(),
        data: Arc::new(png()),
        link: None,
    });
    doc
}

fn identity() -> SnapshotIdentity {
    SnapshotIdentity {
        job_id: "template-job".into(),
        doc_uid: 1,
        base_content_revision: 1,
        target_slide_ids: vec![],
        plan_version: 1,
        template_version: Some(9),
    }
}

fn permissions() -> Vec<TemplatePage> {
    vec![
        TemplatePage {
            key: "cover".into(),
            source_slide_id: SlideId(100),
            slots: vec![TemplateSlot { key: "title".into(), shape_id: ShapeId(101) }],
        },
        TemplatePage {
            key: "conclusion".into(),
            source_slide_id: SlideId(200),
            slots: vec![TemplateSlot { key: "body".into(), shape_id: ShapeId(201) }],
        },
    ]
}

fn scope() -> AppendScope {
    AppendScope::freeze(identity(), TemplateBundle::freeze(9, native_fixture()).unwrap(), permissions(), 64, &empty_target()).unwrap()
}

fn response() -> AppendResponse {
    AppendResponse::AppendTemplate {
        snapshot: identity(),
        pages: vec![
            PageInstance {
                page_key: "cover".into(),
                local_id: LocalId(1),
                replacements: vec![SlotReplacement { slot_key: "title".into(), paragraphs: vec!["新的主张".into()] }],
                links: vec![PageLink { target_page_key: "conclusion".into(), target_local_id: LocalId(2) }],
            },
            PageInstance {
                page_key: "conclusion".into(),
                local_id: LocalId(2),
                replacements: vec![],
                links: vec![PageLink { target_page_key: "cover".into(), target_local_id: LocalId(1) }],
            },
        ],
    }
}

fn pages_mut(response: &mut AppendResponse) -> &mut Vec<PageInstance> {
    let AppendResponse::AppendTemplate { pages, .. } = response;
    pages
}

#[test]
fn complete_native_bundle_and_mapping_are_read_only_and_deterministic() {
    let scope = scope();
    let before = serde_json::to_vec(scope.bundle().native()).unwrap();
    let first = AppendDraft::bind(scope.clone(), response()).unwrap();
    let second = AppendDraft::bind(scope.clone(), response()).unwrap();
    assert_eq!(first.mappings(), second.mappings());
    for kind in [
        SourceResource::Slide { id: SlideId(100) },
        SourceResource::Layout { id: scope.bundle.native.slides[0].layout },
        SourceResource::Master { id: scope.bundle.native.masters[0].id },
        SourceResource::Media { id: MediaId(300) },
    ] {
        assert!(first.mappings().iter().any(|m| m.source == kind));
    }
    assert_eq!(*scope.bundle().native().media(MediaId(300)).unwrap().data, png());
    assert_eq!(serde_json::to_vec(scope.bundle().native()).unwrap(), before);
    assert!(Arc::ptr_eq(&first.scope.bundle, &scope.bundle));
    first.validate().unwrap();
}

#[test]
fn empty_target_identity_never_authorizes_text() {
    identity().validate_for_append().unwrap();
    assert!(identity().validate().is_err());
    let text_scope = super::super::TextScope { snapshot: identity(), editable: vec![] };
    assert!(text_scope.validate(&empty_target()).is_err());
    let mut target = native_fixture();
    target.slides.truncate(1);
    assert_eq!(scope().validate_target(&target), Err(ContractError::SnapshotMismatch));
}

#[test]
fn nonempty_append_target_freezes_all_existing_ids_and_canvas() {
    let target = native_fixture();
    let mut snapshot = identity();
    snapshot.target_slide_ids = target.slides.iter().map(|s| s.id).collect();
    let scope = AppendScope::freeze(snapshot, TemplateBundle::freeze(9, native_fixture()).unwrap(), permissions(), 2, &target).unwrap();
    scope.validate_target(&target).unwrap();
    let mut altered = target.clone();
    altered.slides.reverse();
    assert_eq!(scope.validate_target(&altered), Err(ContractError::SnapshotMismatch));
    altered = target;
    altered.slide_size.width += 1.0;
    assert_eq!(scope.validate_target(&altered), Err(ContractError::UnsupportedTemplate));
}

#[test]
fn duplicate_instances_get_separate_full_resources_and_explicit_link_destinations() {
    let scope = scope();
    let mut response = response();
    let pages = pages_mut(&mut response);
    let mut copy = pages[0].clone();
    copy.local_id = LocalId(3);
    pages.push(copy);
    pages[1].links[0].target_local_id = LocalId(3);
    let draft = AppendDraft::bind(scope, response).unwrap();
    let originals: BTreeSet<_> = draft.mappings().iter().filter(|m| m.instance == LocalId(1)).map(|m| m.source).collect();
    let copies: BTreeSet<_> = draft.mappings().iter().filter(|m| m.instance == LocalId(3)).map(|m| m.source).collect();
    assert_eq!(originals, copies);
    assert_eq!(draft.mappings().iter().map(|m| m.local).collect::<BTreeSet<_>>().len(), draft.mappings().len());
}

#[test]
fn self_links_bind_to_selected_copy_not_source_or_first_instance() {
    let mut source = native_fixture();
    Arc::make_mut(&mut source.slides[0]).shapes[1].click = Some(link(100));
    let scope = AppendScope::freeze(identity(), TemplateBundle::freeze(9, source).unwrap(), permissions(), 3, &empty_target()).unwrap();
    let mut response = response();
    let pages = pages_mut(&mut response);
    pages[0].links = vec![PageLink { target_page_key: "cover".into(), target_local_id: LocalId(1) }];
    let mut copy = pages[0].clone();
    copy.local_id = LocalId(3);
    copy.links[0].target_local_id = LocalId(3);
    pages.push(copy);
    AppendDraft::bind(scope, response).unwrap();
}

#[test]
fn every_nested_wire_level_denies_unknown_fields_and_arbitrary_commands() {
    let scope = scope();
    for pointer in ["", "/snapshot", "/pages/0", "/pages/0/replacements/0", "/pages/0/links/0"] {
        let mut value = serde_json::to_value(response()).unwrap();
        value.pointer_mut(pointer).unwrap().as_object_mut().unwrap().insert("extra".into(), json!(true));
        assert_eq!(AppendResponse::decode(&serde_json::to_vec(&value).unwrap(), &scope), Err(ContractError::Schema));
    }
    for value in [
        json!([]),
        json!({"slides":[]}),
        json!({"op":"run_command","command":"slide.new"}),
        json!({"op":"append_template","snapshot":identity(),"pages":[],"media":[]}),
    ] {
        assert_eq!(AppendResponse::decode(&serde_json::to_vec(&value).unwrap(), &scope), Err(ContractError::Schema));
    }
}

#[test]
fn malformed_oversized_and_deep_json_fail_without_echoing_payload() {
    let scope = scope();
    for bytes in
        [b"not JSON PRIVATE".to_vec(), vec![b' '; super::super::MAX_WIRE_BYTES + 1], format!("{}0{}", "[".repeat(140), "]".repeat(140)).into_bytes()]
    {
        let error = AppendResponse::decode(&bytes, &scope).unwrap_err();
        assert!(!error.to_string().contains("PRIVATE"));
    }
}

#[test]
fn wrong_identity_versions_are_rejected() {
    let scope = scope();
    for field in ["job_id", "doc_uid", "base_content_revision", "plan_version", "template_version", "target_slide_ids"] {
        let mut value = serde_json::to_value(response()).unwrap();
        value["snapshot"][field] = match field {
            "job_id" => json!("other"),
            "target_slide_ids" => json!([99]),
            _ => json!(10),
        };
        assert_eq!(AppendResponse::decode(&serde_json::to_vec(&value).unwrap(), &scope), Err(ContractError::SnapshotMismatch));
    }
    assert!(AppendScope::freeze(identity(), TemplateBundle::freeze(10, native_fixture()).unwrap(), permissions(), 1, &empty_target()).is_err());
}

#[test]
fn invalid_source_authorization_and_slot_permissions_fail_before_generation() {
    let mut pages = permissions();
    pages[0].source_slide_id = SlideId(999);
    assert_eq!(AppendScope::freeze(identity(), scope().bundle.clone(), pages, 2, &empty_target()).unwrap_err(), ContractError::UnauthorizedTarget);
    let mut pages = permissions();
    pages[0].slots[0].shape_id = ShapeId(999);
    assert!(AppendScope::freeze(identity(), scope().bundle.clone(), pages, 2, &empty_target()).is_err());
    let mut pages = permissions();
    pages[1].key = "cover".into();
    assert_eq!(AppendScope::freeze(identity(), scope().bundle.clone(), pages, 2, &empty_target()).unwrap_err(), ContractError::DuplicateTarget);
    assert_eq!(
        AppendScope::freeze(identity(), scope().bundle.clone(), vec![permissions()[0].clone()], 2, &empty_target()).unwrap_err(),
        ContractError::UnauthorizedTarget
    );
}

#[test]
fn provider_cannot_expand_page_or_slot_authorization() {
    let scope = scope();
    for (key, value) in [("page_key", "other"), ("slot_key", "other")] {
        let mut response = response();
        if key == "page_key" {
            pages_mut(&mut response)[0].page_key = value.into();
        } else {
            pages_mut(&mut response)[0].replacements[0].slot_key = value.into();
        }
        assert_eq!(response.validate(&scope), Err(ContractError::UnauthorizedTarget));
    }
    let mut response = response();
    let duplicate = pages_mut(&mut response)[0].replacements[0].clone();
    pages_mut(&mut response)[0].replacements.push(duplicate);
    assert!(response.validate(&scope).is_err());
}

#[test]
fn missing_duplicate_unknown_and_wrong_copy_links_are_rejected() {
    let scope = scope();
    for case in 0..5 {
        let mut response = response();
        let page = &mut pages_mut(&mut response)[0];
        match case {
            0 => page.links.clear(),
            1 => page.links.push(page.links[0].clone()),
            2 => page.links[0].target_page_key = "unknown".into(),
            3 => page.links[0].target_local_id = LocalId(200), // source ID, not local instance
            _ => page.links[0].target_local_id = LocalId(1),   // wrong family
        }
        assert!(response.validate(&scope).is_err());
    }
}

#[test]
fn zero_duplicate_empty_and_excessive_instances_are_rejected() {
    for case in 0..4 {
        let mut response = response();
        let pages = pages_mut(&mut response);
        match case {
            0 => pages[0].local_id = LocalId(0),
            1 => pages[1].local_id = LocalId(1),
            2 => pages.clear(),
            _ => {
                let p = pages[0].clone();
                *pages = vec![p; 65];
            }
        }
        assert!(response.validate(&scope()).is_err());
    }
    let mut response = response();
    pages_mut(&mut response)[0].local_id = LocalId(u32::MAX);
    pages_mut(&mut response)[1].links[0].target_local_id = LocalId(u32::MAX);
    assert!(matches!(AppendDraft::bind(scope(), response), Err(ContractError::Limit("local IDs"))));
}

#[test]
fn incomplete_extra_duplicate_and_mixed_local_mapping_fail_engine_revalidation() {
    let valid = AppendDraft::bind(scope(), response()).unwrap();
    for case in 0..6 {
        let mut draft = valid.clone();
        match case {
            0 => {
                draft.mappings.pop();
            }
            1 => {
                let mut extra = draft.mappings[0].clone();
                extra.source = SourceResource::Media { id: MediaId(999) };
                extra.local = LocalId(999);
                draft.mappings.push(extra);
            }
            2 => draft.mappings.push(draft.mappings[0].clone()),
            3 => draft.mappings[0].local = draft.mappings[1].local,
            4 => draft.mappings[0].instance = LocalId(999),
            _ => {
                let m = draft.mappings.iter_mut().find(|m| matches!(m.source, SourceResource::Slide { .. })).unwrap();
                m.local = LocalId(999);
            }
        }
        assert_eq!(draft.validate(), Err(ContractError::ResourceGraph));
    }
}

#[test]
fn native_missing_duplicate_and_zero_resource_ids_are_rejected() {
    for case in 0..7 {
        let mut native = native_fixture();
        match case {
            0 => native.media.clear(),
            1 => {
                native.media.push(native.media[0].clone());
            }
            2 => Arc::make_mut(&mut native.slides[0]).layout = LayoutId(999),
            3 => Arc::make_mut(&mut native.slides[0]).id = SlideId(200),
            4 => Arc::make_mut(&mut native.slides[0]).id = SlideId(0),
            5 => {
                let m = Arc::make_mut(&mut native.masters[0]);
                m.layouts.push(m.layouts[0].clone());
            }
            _ => Arc::make_mut(&mut native.slides[0]).shapes[1].click = Some(link(999)),
        }
        assert!(TemplateBundle::freeze(9, native).is_err());
    }
}

#[test]
fn media_in_background_theme_and_text_style_participates_in_dependency_graph() {
    let mut native = native_fixture();
    Arc::make_mut(&mut native.masters[0]).theme.format.bg_fills.push(Fill::Picture(PictureFill { media: MediaId(300), ..Default::default() }));
    Arc::make_mut(&mut native.slides[0]).shapes[1].kind = ShapeKind::Shape;
    let bundle = TemplateBundle::freeze(9, native).unwrap();
    assert!(bundle.graphs[&SlideId(100)].resources.contains(&SourceResource::Media { id: MediaId(300) }));
    assert!(bundle.graphs[&SlideId(200)].resources.contains(&SourceResource::Media { id: MediaId(300) }));
}

#[test]
fn group_shape_ids_are_bounded_and_unique_in_each_owner_namespace() {
    let mut native = native_fixture();
    let sh = Arc::make_mut(&mut native.slides[0]);
    sh.shapes.push(Shape {
        id: ShapeId(103),
        kind: ShapeKind::Group { children: vec![Shape { id: ShapeId(101), ..Default::default() }], child: Xfrm::default() },
        ..Default::default()
    });
    assert_eq!(TemplateBundle::freeze(9, native).unwrap_err(), ContractError::ResourceGraph);
    let mut native = native_fixture();
    let mut group = Shape { id: ShapeId(500), ..Default::default() };
    for n in 0..10 {
        group = Shape { id: ShapeId(501 + n), kind: ShapeKind::Group { children: vec![group], child: Xfrm::default() }, ..Default::default() };
    }
    Arc::make_mut(&mut native.slides[0]).shapes.push(group);
    assert!(TemplateBundle::freeze(9, native).is_err());
}

#[test]
fn native_nonfinite_huge_and_opaque_content_are_bounded_before_allocation() {
    for case in 0..7 {
        let mut native = native_fixture();
        match case {
            0 => native.slide_size.width = f64::NAN,
            1 => Arc::make_mut(&mut native.slides[0]).shapes[0].xfrm = Some(Xfrm::new(0.0, 0.0, f64::INFINITY, 1.0)),
            2 => Arc::make_mut(&mut native.slides[0]).shapes[0].name = "x".repeat(128 * 1024 + 1),
            3 => Arc::make_mut(&mut native.slides[0]).raw_ext = Some("opaque".into()),
            4 => {
                Arc::make_mut(&mut native.slides[0]).shapes[0].kind =
                    ShapeKind::Opaque { xml: "unsafe references".into(), preview: None, label: String::new() }
            }
            5 => native.media[0].link = Some("file:///private".into()),
            _ => native.media[0].data = Arc::new(vec![0; MAX_MEDIA_BYTES + 1]),
        }
        assert!(TemplateBundle::freeze(9, native).is_err());
    }
}

#[test]
fn executable_or_unapproved_fixed_links_are_never_copied() {
    for action in [
        Action::Program { path: "secret".into() },
        Action::NextSlide,
        Action::Url { url: "file:///secret".into() },
        Action::Url { url: "https://user:pass@example.org".into() },
    ] {
        let mut native = native_fixture();
        Arc::make_mut(&mut native.slides[0]).shapes[1].click.as_mut().unwrap().action = action;
        assert_eq!(TemplateBundle::freeze(9, native).unwrap_err(), ContractError::UnsupportedTemplate);
    }
}

#[test]
fn unicode_control_characters_and_paragraph_structure_are_validated() {
    for text in ["长".repeat(MAX_TEXT_CHARS + 1), "bad\nline".into(), "bad\tline".into()] {
        let mut response = response();
        pages_mut(&mut response)[0].replacements[0].paragraphs = vec![text];
        assert!(response.validate(&scope()).is_err());
    }
    let mut response = response();
    pages_mut(&mut response)[0].replacements[0].paragraphs.push("extra".into());
    assert_eq!(response.validate(&scope()), Err(ContractError::ParagraphStructure));
}

#[test]
fn duplicated_embedded_media_must_fit_total_draft_budget() {
    let mut native = native_fixture();
    let mut bytes = png();
    bytes.resize(MAX_MEDIA_BYTES, 0);
    native.media[0].data = Arc::new(bytes);
    let scope = AppendScope::freeze(identity(), TemplateBundle::freeze(9, native).unwrap(), permissions(), 64, &empty_target()).unwrap();
    let mut response = response();
    let pages = pages_mut(&mut response);
    for n in 3..8 {
        let mut copy = pages[0].clone();
        copy.local_id = LocalId(n);
        pages.push(copy);
    }
    assert!(matches!(response.validate(&scope), Err(ContractError::Limit("draft media bytes"))));
}

#[test]
fn total_draft_text_and_mapping_budgets_fail_before_ready() {
    let mut response = response();
    let pages = pages_mut(&mut response);
    pages[0].replacements[0].paragraphs = vec!["字".repeat(8192)];
    for n in 3..10 {
        let mut copy = pages[0].clone();
        copy.local_id = LocalId(n);
        pages.push(copy);
    }
    assert!(matches!(response.validate(&scope()), Err(ContractError::Limit("draft text"))));

    let mut source = native_fixture();
    for n in 400..480 {
        Arc::make_mut(&mut source.slides[0]).shapes.push(Shape { id: ShapeId(n), ..Default::default() });
    }
    let scope = AppendScope::freeze(identity(), TemplateBundle::freeze(9, source).unwrap(), permissions(), 64, &empty_target()).unwrap();
    let mut response = crate::ai_contract::template::tests::response();
    let pages = pages_mut(&mut response);
    for n in 3..65 {
        let mut copy = pages[0].clone();
        copy.local_id = LocalId(n);
        pages.push(copy);
    }
    assert!(matches!(AppendDraft::bind(scope, response), Err(ContractError::Limit("draft mappings"))));
}

#[test]
fn source_counts_image_dimensions_and_invalid_payloads_are_bounded() {
    let mut source = native_fixture();
    for n in 400..1500 {
        Arc::make_mut(&mut source.slides[0]).shapes.push(Shape { id: ShapeId(n), ..Default::default() });
    }
    assert!(matches!(TemplateBundle::freeze(9, source), Err(ContractError::Limit(_))));
    let mut source = native_fixture();
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(MAX_IMAGE_SIDE + 1, 1).write_to(&mut out, image::ImageFormat::Png).unwrap();
    source.media[0].data = Arc::new(out.into_inner());
    assert!(matches!(TemplateBundle::freeze(9, source), Err(ContractError::Limit("media dimensions"))));
    let mut source = native_fixture();
    source.media[0].data = Arc::new(vec![1, 2, 3]);
    assert!(matches!(TemplateBundle::freeze(9, source), Err(ContractError::UnsupportedTemplate)));
}
