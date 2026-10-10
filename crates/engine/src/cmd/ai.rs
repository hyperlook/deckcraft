//! `ai.applyPatch` dispatches a text commit. Template append uses [`crate::Session::apply_ai`]
//! directly because the draft owns native bytes and is not a JSON command.
//!
//! The command is not undoable by the registry: `apply_ai` writes the one named history entry.
//! The raw patch is not journaled; it can be up to the contract wire limit.

use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{CommandSpec, has_doc};
use crate::ai_apply::{AiCommit, ApplyError};
use crate::ai_contract::{TextPatch, TextScope, decode_bounded};
use crate::{EngineError, Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "ai.applyPatch",
        label: "Apply AI Patch",
        menu: &[],
        shortcut: None,
        params: "{label?, scope, patch} — one text commit; scope.snapshot.doc_uid selects the document",
        enabled: has_doc,
        run: apply_patch,
        journal: false,
        undoable: false,
    }]
}

fn apply_patch(session: &mut Session, params: &Value) -> Result<Value> {
    let commit = parse_text_commit(params)?;
    let outcome = session.apply_ai(commit)?;
    serde_json::to_value(&outcome).map_err(|err| EngineError::Other(err.to_string()))
}

fn parse_text_commit(params: &Value) -> std::result::Result<AiCommit, ApplyError> {
    let obj = params.as_object().ok_or(ApplyError::Contract("parameters".into()))?;
    if obj.keys().any(|key| key != "label" && key != "scope" && key != "patch") {
        return Err(ApplyError::Contract("unknown field".into()));
    }
    let label = match obj.get("label") {
        None => String::new(),
        Some(Value::String(label)) => label.clone(),
        Some(_) => return Err(ApplyError::BadLabel),
    };
    let scope = decode_value::<TextScope>(obj.get("scope").ok_or(ApplyError::Contract("scope".into()))?)?;
    let patch = decode_value::<TextPatch>(obj.get("patch").ok_or(ApplyError::Contract("patch".into()))?)?;
    Ok(AiCommit::Text { scope, patch, label })
}

fn decode_value<T: DeserializeOwned>(value: &Value) -> std::result::Result<T, ApplyError> {
    let bytes = serde_json::to_vec(value).map_err(|_| ApplyError::Contract("schema".into()))?;
    decode_bounded(&bytes).map_err(ApplyError::from)
}
