#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use deckcraft_engine::ai_contract::*;
use deckcraft_model::text::{RunKind, RunProps};
use deckcraft_model::{Presentation, Run, ShapeId, SlideId};
use serde_json::{Value, json};

use crate::fake::{FakeError, FakeProvider, FakeReply, MAX_SCRIPT_REPLIES};
use crate::request::{MAX_CONTEXT_CHARS, MAX_FACTS, TextRequest};
use crate::task::*;

const DOCUMENT: &[u8] = include_bytes!("../tests/fixtures/document.json");
const REQUEST: &[u8] = include_bytes!("../tests/fixtures/request.json");
const RESPONSE: &[u8] = include_bytes!("../tests/fixtures/response.json");

fn fixture() -> (Presentation, TextRequest) {
    let doc: Presentation = serde_json::from_slice(DOCUMENT).unwrap();
    let request = TextRequest::decode(REQUEST, &doc).unwrap();
    (doc, request)
}

fn response() -> Value {
    serde_json::from_slice(RESPONSE).unwrap()
}

fn decode(value: &Value, request: &TextRequest, doc: &Presentation) -> ContractResult<TextPatch> {
    TextPatch::decode(&serde_json::to_vec(value).unwrap(), &request.scope, doc)
}

fn event(task: &Task, action: TaskAction) -> TaskEvent {
    TaskEvent { snapshot: task.snapshot().clone(), action }
}

fn advance(task: &mut Task, action: TaskAction) {
    let e = event(task, action);
    assert!(matches!(task.accept(&e), Ok(EventOutcome::Transitioned { .. })));
}

#[test]
fn fake_multi_page_result_is_deterministic_and_read_only() {
    let (doc, request) = fixture();
    let before = serde_json::to_vec(&doc).unwrap();
    let mut fake = FakeProvider::new(vec![FakeReply::Json(RESPONSE.to_vec()), FakeReply::Json(RESPONSE.to_vec())]).unwrap();
    let first = fake.respond(&request, &doc).unwrap();
    let second = fake.respond(&request, &doc).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.operations.len(), 2);
    assert_eq!(serde_json::to_vec(&doc).unwrap(), before);
    assert_eq!(fake.remaining(), 0);
    assert_eq!(fake.respond(&request, &doc), Err(FakeError::Exhausted));
}

#[test]
fn request_uploads_only_approved_text_not_whole_document() {
    let (_, request) = fixture();
    let wire = serde_json::to_string(&request).unwrap();
    assert!(!wire.contains("PRIVATE_UNSELECTED"));
    assert!(!wire.contains("masters"));
    assert!(!wire.contains("media"));
    assert!(!wire.contains("credentials"));
}

#[test]
fn unknown_fields_are_rejected_at_every_patch_level() {
    let (doc, request) = fixture();
    for pointer in ["", "/snapshot", "/operations/0", "/operations/0/target"] {
        let mut v = response();
        v.pointer_mut(pointer).unwrap().as_object_mut().unwrap().insert("extra".into(), json!(true));
        assert_eq!(decode(&v, &request, &doc), Err(ContractError::Schema), "{pointer}");
    }
}

#[test]
fn unknown_fields_are_rejected_at_every_request_level() {
    let (doc, _) = fixture();
    for pointer in ["", "/scope", "/scope/snapshot", "/scope/editable/0", "/intent", "/intent/confirmed_facts/0", "/inputs/0", "/inputs/0/target"] {
        let mut v: Value = serde_json::from_slice(REQUEST).unwrap();
        v.pointer_mut(pointer).unwrap().as_object_mut().unwrap().insert("extra".into(), json!(true));
        assert_eq!(TextRequest::decode(&serde_json::to_vec(&v).unwrap(), &doc), Err(ContractError::Schema), "{pointer}");
    }
}

#[test]
fn arbitrary_commands_and_template_operations_are_not_supported() {
    let (doc, request) = fixture();
    for op in ["execute", "shape_delete", "append_slide", "replace_rich_text"] {
        let mut v = response();
        v["operations"][0]["op"] = json!(op);
        assert_eq!(decode(&v, &request, &doc), Err(ContractError::Schema));
    }
}

#[test]
fn malformed_wire_is_rejected_without_echoing_sensitive_content() {
    let (doc, request) = fixture();
    for bytes in [b"{invalid PRIVATE_TEXT}".as_slice(), b"null", b"{}", b"\xff", b"{\"snapshot\":null,\"snapshot\":null}"] {
        let err = TextPatch::decode(bytes, &request.scope, &doc).unwrap_err();
        assert_eq!(err, ContractError::Schema);
        assert!(!err.to_string().contains("PRIVATE"));
    }
    let nested = format!("{}0{}", "[".repeat(200), "]".repeat(200));
    assert_eq!(decode_bounded::<Value>(nested.as_bytes()), Err(ContractError::Schema));
}

#[test]
fn invalid_numeric_ids_fail_schema_or_identity_validation() {
    let (doc, request) = fixture();
    for id in [json!(-1), json!(1.5), json!("7"), json!(null), json!(4_294_967_296u64)] {
        let mut v = response();
        v["operations"][0]["target"]["shape_id"] = id;
        assert_eq!(decode(&v, &request, &doc), Err(ContractError::Schema));
    }
    let mut scope = request.scope.clone();
    scope.editable[0].shape_id = ShapeId(0);
    assert_eq!(scope.validate(&doc), Err(ContractError::Identity("shape")));
    scope = request.scope.clone();
    scope.snapshot.target_slide_ids[0] = SlideId(0);
    assert_eq!(scope.validate(&doc), Err(ContractError::Identity("slide")));
}

#[test]
fn zero_versions_bad_keys_and_duplicate_snapshot_slides_are_rejected() {
    let (_, request) = fixture();
    for key in ["", "../../tmp/x", "job id", "中文", &"x".repeat(65)] {
        let mut snapshot = request.scope.snapshot.clone();
        snapshot.job_id = key.into();
        assert_eq!(snapshot.validate(), Err(ContractError::Identity("key")));
    }
    for field in ["doc_uid", "base_content_revision", "plan_version", "template_version"] {
        let mut v = serde_json::to_value(&request.scope.snapshot).unwrap();
        v[field] = json!(0);
        let snapshot: SnapshotIdentity = serde_json::from_value(v).unwrap();
        assert_eq!(snapshot.validate(), Err(ContractError::Identity("version or document")));
    }
    let mut snapshot = request.scope.snapshot.clone();
    snapshot.target_slide_ids.push(SlideId(101));
    assert_eq!(snapshot.validate(), Err(ContractError::DuplicateTarget));
}

#[test]
fn every_snapshot_identity_component_is_frozen() {
    let (doc, request) = fixture();
    for (field, replacement) in [
        ("job_id", json!("other-job")),
        ("doc_uid", json!(18)),
        ("base_content_revision", json!(5)),
        ("target_slide_ids", json!([202, 101])),
        ("plan_version", json!(2)),
        ("template_version", json!(1)),
    ] {
        let mut v = response();
        v["snapshot"][field] = replacement;
        assert_eq!(decode(&v, &request, &doc), Err(ContractError::SnapshotMismatch), "{field}");
    }
}

#[test]
fn unknown_document_targets_fail_before_any_patch_is_accepted() {
    let (doc, request) = fixture();
    let mut scope = request.scope.clone();
    scope.snapshot.target_slide_ids[0] = SlideId(999);
    assert_eq!(scope.validate(&doc), Err(ContractError::UnknownSlide(SlideId(999))));
    scope = request.scope.clone();
    scope.editable[0].shape_id = ShapeId(999);
    assert_eq!(scope.validate(&doc), Err(ContractError::UnknownShape(scope.editable[0])));
}

#[test]
fn existing_but_unauthorized_shapes_and_slides_are_rejected() {
    let (doc, request) = fixture();
    for target in [json!({"slide_id":101,"shape_id":8}), json!({"slide_id":303,"shape_id":7})] {
        let mut v = response();
        v["operations"][1]["target"] = target;
        assert_eq!(decode(&v, &request, &doc), Err(ContractError::UnauthorizedTarget));
    }
}

#[test]
fn shape_ids_are_scoped_to_slides_not_globally_deduplicated() {
    let (doc, request) = fixture();
    assert_eq!(request.scope.editable[0].shape_id, request.scope.editable[1].shape_id);
    assert!(decode(&response(), &request, &doc).is_ok());
}

#[test]
fn duplicate_scope_and_patch_targets_fail() {
    let (doc, request) = fixture();
    let mut scope = request.scope.clone();
    scope.editable.push(scope.editable[0]);
    assert_eq!(scope.validate(&doc), Err(ContractError::DuplicateTarget));
    let mut v = response();
    let first = v["operations"][0].clone();
    v["operations"].as_array_mut().unwrap().push(first);
    assert_eq!(decode(&v, &request, &doc), Err(ContractError::DuplicateTarget));
}

#[test]
fn explicit_paragraph_structure_and_control_characters_are_checked() {
    let (doc, request) = fixture();
    for paragraphs in [json!([]), json!(["one"]), json!(["one", "two", "three"])] {
        let mut v = response();
        v["operations"][0]["paragraphs"] = paragraphs;
        assert_eq!(decode(&v, &request, &doc), Err(ContractError::ParagraphStructure));
    }
    for text in ["换行\n不隐式拆段", "\r", "\0", "\u{b}", "\t"] {
        let mut v = response();
        v["operations"][0]["paragraphs"] = json!([text, ""]);
        assert_eq!(decode(&v, &request, &doc), Err(ContractError::ControlCharacter));
    }
    let mut v = response();
    v["operations"][0]["paragraphs"] = json!(["", ""]);
    assert!(decode(&v, &request, &doc).is_ok());
}

#[test]
fn chinese_character_limit_is_not_a_byte_limit() {
    let (doc, request) = fixture();
    let mut v = response();
    v["operations"][0]["paragraphs"] = json!(["中".repeat(MAX_TEXT_CHARS), ""]);
    assert!(decode(&v, &request, &doc).is_ok());
    v["operations"][0]["paragraphs"] = json!(["中".repeat(MAX_TEXT_CHARS), "多"]);
    assert_eq!(decode(&v, &request, &doc), Err(ContractError::Limit("text characters")));
}

#[test]
fn total_text_budget_is_enforced_for_typed_patches() {
    let (mut doc, mut request) = fixture();
    let slide = Arc::make_mut(doc.slides.get_mut(0).unwrap());
    let prototype = slide.shapes[0].clone();
    for id in 20..24 {
        let mut shape = prototype.clone();
        shape.id = ShapeId(id);
        slide.shapes.push(shape);
        request.scope.editable.push(TextTarget { slide_id: SlideId(101), shape_id: ShapeId(id) });
    }
    let operations = request
        .scope
        .editable
        .iter()
        .filter(|t| t.slide_id == SlideId(101))
        .map(|target| TextPatchOp::ReplaceText { target: *target, paragraphs: vec!["中".repeat(MAX_TEXT_CHARS), String::new()] })
        .collect();
    let patch = TextPatch { snapshot: request.scope.snapshot.clone(), operations };
    assert_eq!(patch.validate(&request.scope, &doc), Err(ContractError::Limit("text bytes")));
}

#[test]
fn wire_and_collection_limits_are_enforced() {
    let (doc, request) = fixture();
    assert_eq!(TextPatch::decode(&vec![b' '; MAX_WIRE_BYTES + 1], &request.scope, &doc), Err(ContractError::Limit("wire bytes")));
    let mut patch: TextPatch = serde_json::from_slice(RESPONSE).unwrap();
    patch.operations = vec![patch.operations[0].clone(); MAX_PATCH_OPS + 1];
    assert_eq!(patch.validate(&request.scope, &doc), Err(ContractError::Limit("patch operations")));
    let mut scope = request.scope.clone();
    scope.editable = vec![scope.editable[0]; MAX_TEXT_TARGETS + 1];
    assert_eq!(scope.validate(&doc), Err(ContractError::Limit("text targets")));
    let mut identity = request.scope.snapshot.clone();
    identity.target_slide_ids = (1..=(MAX_TARGET_SLIDES as u32 + 1)).map(SlideId).collect();
    assert_eq!(identity.validate(), Err(ContractError::Limit("target slides")));
}

#[test]
fn unsafe_text_objects_fail_closed() {
    let (doc, request) = fixture();
    for kind in ["mixed", "field", "math", "link", "break", "locked", "hidden", "no_text", "no_paragraphs", "too_many_paragraphs", "duplicate_id"] {
        let mut doc = doc.clone();
        let slide = Arc::make_mut(doc.slides.get_mut(0).unwrap());
        let shape = slide.shapes.get_mut(0).unwrap();
        match kind {
            "locked" => shape.locked = true,
            "hidden" => shape.hidden = true,
            "no_text" => shape.text = None,
            "duplicate_id" => {
                let duplicate = shape.clone();
                slide.shapes.push(duplicate);
            }
            _ => {
                let body = shape.text.as_mut().unwrap();
                match kind {
                    "no_paragraphs" => body.paragraphs.clear(),
                    "too_many_paragraphs" => body.paragraphs = vec![body.paragraphs[0].clone(); MAX_PARAGRAPHS + 1],
                    _ => {
                        let p = body.paragraphs.get_mut(0).unwrap();
                        match kind {
                            "mixed" => p.runs.push(Run::with("bold", RunProps { bold: Some(true), ..Default::default() })),
                            "field" => p.runs[0].kind = RunKind::Field { field: "slidenum".into() },
                            "math" => p.runs[0].kind = RunKind::Math { omml: "<xml/>".into() },
                            "break" => p.runs[0].kind = RunKind::Break,
                            "link" => {
                                p.runs[0].props.link = Some(deckcraft_model::text::Hyperlink {
                                    action: deckcraft_model::text::Action::NextSlide,
                                    tooltip: String::new(),
                                    highlight_click: false,
                                });
                            }
                            _ => panic!("unexpected test case"),
                        }
                    }
                }
            }
        }
        assert_eq!(request.scope.validate(&doc), Err(ContractError::UnsupportedText(request.scope.editable[0])), "{kind}");
    }
}

#[test]
fn duplicate_document_slide_ids_are_ambiguous_and_rejected() {
    let (mut doc, request) = fixture();
    doc.slides.push(doc.slides[0].clone());
    assert_eq!(request.scope.validate(&doc), Err(ContractError::DuplicateTarget));
}

#[test]
fn groups_tables_and_connectors_are_not_implicitly_authorized() {
    let (doc, request) = fixture();
    for kind in [
        deckcraft_model::ShapeKind::Group { children: vec![], child: Default::default() },
        deckcraft_model::ShapeKind::Table(Default::default()),
        deckcraft_model::ShapeKind::Connector { start: None, end: None },
    ] {
        let mut doc = doc.clone();
        Arc::make_mut(doc.slides.get_mut(0).unwrap()).shapes.get_mut(0).unwrap().kind = kind;
        assert_eq!(request.scope.validate(&doc), Err(ContractError::UnsupportedText(request.scope.editable[0])));
    }
}

#[test]
fn oversized_source_text_and_excessive_run_counts_are_bounded() {
    let (mut doc, request) = fixture();
    let shape = Arc::make_mut(doc.slides.get_mut(0).unwrap()).shapes.get_mut(0).unwrap();
    shape.text.as_mut().unwrap().paragraphs[0].runs[0].text = "x".repeat(MAX_TOTAL_TEXT_BYTES + 1);
    assert_eq!(request.scope.validate(&doc), Err(ContractError::Limit("source text bytes")));
    let (mut doc, request) = fixture();
    let p = &mut Arc::make_mut(doc.slides.get_mut(0).unwrap()).shapes[0].text.as_mut().unwrap().paragraphs[0];
    p.runs = vec![Run::new(""); MAX_RUNS_PER_PARAGRAPH + 1];
    assert_eq!(request.scope.validate(&doc), Err(ContractError::UnsupportedText(request.scope.editable[0])));
}

#[test]
fn uniform_split_runs_and_independent_paragraph_formats_are_supported() {
    let (mut doc, request) = fixture();
    let p = &mut Arc::make_mut(doc.slides.get_mut(0).unwrap()).shapes[0].text.as_mut().unwrap().paragraphs[0];
    p.runs = vec![Run::new("本轮只验证文本契约，"), Run::new("不改真实文档。")];
    for run in &mut p.runs {
        run.props.bold = Some(true);
    }
    assert!(request.validate(&doc).is_ok());
    assert!(decode(&response(), &request, &doc).is_ok());
}

#[test]
fn empty_patch_is_valid_for_future_noop_commit() {
    let (doc, request) = fixture();
    let patch = TextPatch { snapshot: request.scope.snapshot.clone(), operations: vec![] };
    assert!(patch.validate(&request.scope, &doc).is_ok());
}

#[test]
fn stale_or_duplicate_request_slices_fail() {
    let (doc, mut request) = fixture();
    request.inputs[0].paragraphs[0] = "old conversation".into();
    assert_eq!(request.validate(&doc), Err(ContractError::SnapshotMismatch));
    let (_, mut request) = fixture();
    request.inputs[1] = request.inputs[0].clone();
    assert_eq!(request.validate(&doc), Err(ContractError::UnauthorizedTarget));
}

#[test]
fn request_instruction_and_context_budgets_are_checked() {
    let (doc, mut request) = fixture();
    request.intent.instruction = " ".into();
    assert_eq!(request.validate(&doc), Err(ContractError::Identity("instruction")));
    request.intent.instruction = "中".repeat(MAX_CONTEXT_CHARS + 1);
    assert_eq!(request.validate(&doc), Err(ContractError::Limit("request text")));
    let (_, mut request) = fixture();
    request.intent.confirmed_facts = vec![request.intent.confirmed_facts[0].clone(); MAX_FACTS + 1];
    assert_eq!(request.validate(&doc), Err(ContractError::Limit("intent entries")));
}

#[test]
fn fake_provider_rejects_invalid_requests_and_exposes_scripted_errors() {
    let (doc, mut request) = fixture();
    let mut fake = FakeProvider::new(vec![FakeReply::ProviderFailure, FakeReply::Json(b"{}".to_vec())]).unwrap();
    request.intent.instruction.clear();
    assert!(matches!(fake.respond(&request, &doc), Err(FakeError::Contract(_))));
    assert_eq!(fake.remaining(), 2);
    request.intent.instruction = "rewrite".into();
    assert_eq!(fake.respond(&request, &doc), Err(FakeError::ProviderFailure));
    assert_eq!(fake.respond(&request, &doc), Err(FakeError::Contract(ContractError::Schema)));
    assert!(FakeProvider::new(vec![FakeReply::ProviderFailure; MAX_SCRIPT_REPLIES + 1]).is_err());
    assert!(FakeProvider::new(vec![FakeReply::Json(vec![b' '; MAX_WIRE_BYTES + 1])]).is_err());
}

#[test]
fn text_lifecycle_requires_validation_and_explicit_application() {
    let (_, request) = fixture();
    let mut task = Task::text_edit(request.scope.snapshot).unwrap();
    assert_eq!(task.state(), TaskState::Generating);
    let e = event(&task, TaskAction::Applied {});
    assert_eq!(task.accept(&e), Err(TaskError::InvalidTransition(TaskState::Generating)));
    assert_eq!(task.state(), TaskState::Generating);
    advance(&mut task, TaskAction::GenerationFinished {});
    advance(&mut task, TaskAction::RetryPages {});
    advance(&mut task, TaskAction::GenerationFinished {});
    advance(&mut task, TaskAction::ValidationPassed {});
    advance(&mut task, TaskAction::ApplyRequested {});
    advance(&mut task, TaskAction::Applied {});
    assert_eq!(task.state(), TaskState::Applied);
    let cancel = event(&task, TaskAction::Cancel {});
    assert_eq!(task.accept(&cancel), Ok(EventOutcome::IgnoredTerminal));
    assert_eq!(task.state(), TaskState::Applied);
}

#[test]
fn outline_lifecycle_freezes_identity() {
    let (_, request) = fixture();
    let snapshot = request.scope.snapshot;
    let mut task = Task::outline_generation(snapshot.clone()).unwrap();
    for action in [TaskAction::OutlineAvailable {}, TaskAction::OutlineConfirmed {}, TaskAction::TemplateConfirmed {}] {
        advance(&mut task, action);
    }
    assert_eq!(task.state(), TaskState::Generating);
    assert_eq!(task.snapshot(), &snapshot);
}

#[test]
fn every_terminal_state_ignores_late_events() {
    let (_, request) = fixture();
    for terminal in [TaskState::Applied, TaskState::Cancelled, TaskState::Failed, TaskState::Conflicted] {
        let mut task = Task::text_edit(request.scope.snapshot.clone()).unwrap();
        match terminal {
            TaskState::Cancelled => advance(&mut task, TaskAction::Cancel {}),
            TaskState::Failed => advance(&mut task, TaskAction::Fail { reason: FailureReason::Deadline }),
            _ => {
                advance(&mut task, TaskAction::GenerationFinished {});
                advance(&mut task, TaskAction::ValidationPassed {});
                if terminal == TaskState::Conflicted {
                    advance(&mut task, TaskAction::Conflict {});
                } else {
                    advance(&mut task, TaskAction::ApplyRequested {});
                    advance(&mut task, TaskAction::Applied {});
                }
            }
        }
        for action in [
            TaskAction::GenerationFinished {},
            TaskAction::ValidationPassed {},
            TaskAction::RetryPages {},
            TaskAction::ApplyRequested {},
            TaskAction::Applied {},
            TaskAction::Cancel {},
            TaskAction::DocumentClosed {},
            TaskAction::Fail { reason: FailureReason::Provider },
        ] {
            let e = event(&task, action);
            assert_eq!(task.accept(&e), Ok(EventOutcome::IgnoredTerminal));
            assert_eq!(task.state(), terminal);
        }
    }
}

#[test]
fn document_close_and_failure_do_not_resurrect_tasks() {
    let (_, request) = fixture();
    let mut task = Task::text_edit(request.scope.snapshot.clone()).unwrap();
    advance(&mut task, TaskAction::DocumentClosed {});
    assert_eq!(task.state(), TaskState::Cancelled);
    assert_eq!(task.failure(), None);
    let mut task = Task::text_edit(request.scope.snapshot).unwrap();
    advance(&mut task, TaskAction::Fail { reason: FailureReason::Budget });
    assert_eq!(task.failure(), Some(FailureReason::Budget));
    let e = event(&task, TaskAction::Fail { reason: FailureReason::Provider });
    assert_eq!(task.accept(&e), Ok(EventOutcome::IgnoredTerminal));
    assert_eq!(task.failure(), Some(FailureReason::Budget));
}

#[test]
fn mismatched_events_leave_task_unchanged() {
    let (_, request) = fixture();
    let mut task = Task::text_edit(request.scope.snapshot).unwrap();
    let mut e = event(&task, TaskAction::Cancel {});
    e.snapshot.plan_version += 1;
    assert_eq!(task.accept(&e), Err(TaskError::SnapshotMismatch));
    assert_eq!(task.state(), TaskState::Generating);
    assert_eq!(task.failure(), None);
}

#[test]
fn all_nonterminal_states_follow_the_transition_allowlist() {
    let (_, request) = fixture();
    let paths: Vec<(TaskState, Vec<TaskAction>)> = vec![
        (TaskState::Planning, vec![]),
        (TaskState::AwaitingOutline, vec![TaskAction::OutlineAvailable {}]),
        (TaskState::AwaitingTemplate, vec![TaskAction::OutlineAvailable {}, TaskAction::OutlineConfirmed {}]),
        (TaskState::Generating, vec![TaskAction::OutlineAvailable {}, TaskAction::OutlineConfirmed {}, TaskAction::TemplateConfirmed {}]),
        (
            TaskState::Validating,
            vec![
                TaskAction::OutlineAvailable {},
                TaskAction::OutlineConfirmed {},
                TaskAction::TemplateConfirmed {},
                TaskAction::GenerationFinished {},
            ],
        ),
        (
            TaskState::Ready,
            vec![
                TaskAction::OutlineAvailable {},
                TaskAction::OutlineConfirmed {},
                TaskAction::TemplateConfirmed {},
                TaskAction::GenerationFinished {},
                TaskAction::ValidationPassed {},
            ],
        ),
        (
            TaskState::Applying,
            vec![
                TaskAction::OutlineAvailable {},
                TaskAction::OutlineConfirmed {},
                TaskAction::TemplateConfirmed {},
                TaskAction::GenerationFinished {},
                TaskAction::ValidationPassed {},
                TaskAction::ApplyRequested {},
            ],
        ),
    ];
    let actions = [
        TaskAction::OutlineAvailable {},
        TaskAction::OutlineConfirmed {},
        TaskAction::TemplateConfirmed {},
        TaskAction::GenerationFinished {},
        TaskAction::ValidationPassed {},
        TaskAction::RetryPages {},
        TaskAction::ApplyRequested {},
        TaskAction::Applied {},
        TaskAction::Conflict {},
        TaskAction::Cancel {},
        TaskAction::DocumentClosed {},
        TaskAction::Fail { reason: FailureReason::Validation },
    ];
    for (state, path) in paths {
        let mut task = Task::outline_generation(request.scope.snapshot.clone()).unwrap();
        for action in path {
            advance(&mut task, action);
        }
        assert_eq!(task.state(), state);
        for action in &actions {
            let mut candidate = task.clone();
            let valid = match action {
                TaskAction::OutlineAvailable {} => state == TaskState::Planning,
                TaskAction::OutlineConfirmed {} => state == TaskState::AwaitingOutline,
                TaskAction::TemplateConfirmed {} => state == TaskState::AwaitingTemplate,
                TaskAction::GenerationFinished {} => state == TaskState::Generating,
                TaskAction::ValidationPassed {} | TaskAction::RetryPages {} => state == TaskState::Validating,
                TaskAction::ApplyRequested {} => state == TaskState::Ready,
                TaskAction::Applied {} => state == TaskState::Applying,
                TaskAction::Conflict {} => matches!(state, TaskState::Ready | TaskState::Applying),
                TaskAction::Cancel {} | TaskAction::DocumentClosed {} | TaskAction::Fail { .. } => true,
            };
            let e = event(&candidate, action.clone());
            let outcome = candidate.accept(&e);
            assert_eq!(outcome.is_ok(), valid, "{state:?}, {action:?}");
            if !valid {
                assert_eq!(candidate.state(), state);
                assert_eq!(candidate.failure(), None);
            }
        }
    }
}

#[test]
fn event_schema_rejects_unknown_controls_and_fields() {
    let (_, request) = fixture();
    let task = Task::text_edit(request.scope.snapshot).unwrap();
    let original = serde_json::to_value(event(&task, TaskAction::Cancel {})).unwrap();
    for pointer in ["", "/snapshot", "/action"] {
        let mut v = original.clone();
        v.pointer_mut(pointer).unwrap().as_object_mut().unwrap().insert("extra".into(), json!("no"));
        assert_eq!(decode_bounded::<TaskEvent>(&serde_json::to_vec(&v).unwrap()), Err(ContractError::Schema), "{pointer}");
    }
    let mut v = original;
    v["action"]["event"] = json!("run_command");
    assert_eq!(decode_bounded::<TaskEvent>(&serde_json::to_vec(&v).unwrap()), Err(ContractError::Schema));
}
