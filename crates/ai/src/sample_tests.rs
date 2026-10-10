#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;

const CATALOG_FIXTURE: &str = include_str!("../tests/fixtures/samples-catalog.json");

#[test]
fn sample_catalog_fixture_decodes_and_validates_successfully() {
    let catalog = SampleTemplateCatalog::decode(CATALOG_FIXTURE.as_bytes()).expect("catalog fixture should decode");
    assert_eq!(catalog.version, 1);
    assert_eq!(catalog.templates.len(), 3);

    let garyvee = catalog.find("garyvee_content_strategy_framework").expect("garyvee template must exist");
    assert_eq!(garyvee.slide_count, 17);
    assert_eq!(garyvee.width, 720);
    assert_eq!(garyvee.height, 405);
    assert_eq!(garyvee.image_slots, 0);
    assert_eq!(garyvee.text_slots, 185);
    assert_eq!(garyvee.source_sha256, "45a1619662039f67bd8f830f16f992ce735996a60458fb6af596703fa8c53cfa");
    assert_eq!(garyvee.native_sha256, "f72d5f76a8327f430d95da1638eb3217d475c2d1d1e978679e741ea96f1acc81");

    let crypto = catalog.find("crypto_quantum_risk_briefing").expect("crypto template must exist");
    assert_eq!(crypto.slide_count, 15);
    assert_eq!(crypto.width, 960);
    assert_eq!(crypto.height, 540);
    assert_eq!(crypto.image_slots, 0);
    assert_eq!(crypto.text_slots, 225);

    let macro_outlook = catalog.find("fixed_income_macro_outlook").expect("fixed income template must exist");
    assert_eq!(macro_outlook.slide_count, 15);
    assert_eq!(macro_outlook.width, 960);
    assert_eq!(macro_outlook.height, 540);
    assert_eq!(macro_outlook.image_slots, 0);
    assert_eq!(macro_outlook.text_slots, 312);
}

#[test]
fn sample_catalog_denies_unknown_fields() {
    let bad_json = r#"{
        "version": 1,
        "description": "test",
        "license_policy": "test",
        "unknown_extra_field": "disallowed",
        "templates": []
    }"#;
    let err = SampleTemplateCatalog::decode(bad_json.as_bytes()).unwrap_err();
    assert!(matches!(err, ContractError::Schema));
}

#[test]
fn sample_catalog_rejects_invalid_version_and_empty_list() {
    let bad_version = CATALOG_FIXTURE.replace("\"version\": 1", "\"version\": 2");
    assert!(matches!(SampleTemplateCatalog::decode(bad_version.as_bytes()).unwrap_err(), ContractError::SnapshotMismatch));

    let empty = r#"{
        "version": 1,
        "description": "empty",
        "license_policy": "none",
        "templates": []
    }"#;
    assert!(matches!(SampleTemplateCatalog::decode(empty.as_bytes()).unwrap_err(), ContractError::Limit("sample template count")));
}

#[test]
fn sample_catalog_rejects_duplicate_templates_or_invalid_roles() {
    let mut catalog = SampleTemplateCatalog::decode(CATALOG_FIXTURE.as_bytes()).expect("catalog fixture should decode");
    let duplicate = catalog.templates[0].clone();
    catalog.templates.push(duplicate);
    assert!(matches!(catalog.validate().unwrap_err(), ContractError::DuplicateTarget));

    let mut catalog2 = SampleTemplateCatalog::decode(CATALOG_FIXTURE.as_bytes()).expect("catalog fixture should decode");
    catalog2.templates[0].roles.push(SamplePageRole { slide_index: 99, role: "invalid".to_string(), description: "out of bounds".to_string() });
    assert!(matches!(catalog2.validate().unwrap_err(), ContractError::UnauthorizedTarget));
}
