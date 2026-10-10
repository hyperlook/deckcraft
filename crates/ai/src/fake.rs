//! Bounded, deterministic provider fixture. Enabled only in tests or with `test-support`.
//! Scripts contain raw responses so malformed model output exercises the real decoder.

use std::collections::VecDeque;

use deckcraft_engine::ai_contract::{ContractError, MAX_WIRE_BYTES, TextPatch};
use deckcraft_model::Presentation;

use crate::request::TextRequest;
use crate::template::TemplateRequest;
use deckcraft_engine::ai_contract::template::{AppendDraft, AppendResponse, AppendScope};

pub const MAX_SCRIPT_REPLIES: usize = 64;

#[derive(Clone, Debug)]
pub enum FakeReply {
    Json(Vec<u8>),
    ProviderFailure,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FakeError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error("fake provider script exhausted")]
    Exhausted,
    #[error("scripted provider failure")]
    ProviderFailure,
}

pub struct FakeProvider {
    replies: VecDeque<FakeReply>,
}

impl FakeProvider {
    pub fn new(replies: Vec<FakeReply>) -> Result<Self, FakeError> {
        if replies.len() > MAX_SCRIPT_REPLIES {
            return Err(ContractError::Limit("script replies").into());
        }
        if replies.iter().any(|r| matches!(r, FakeReply::Json(bytes) if bytes.len() > MAX_WIRE_BYTES)) {
            return Err(ContractError::Limit("wire bytes").into());
        }
        Ok(Self { replies: replies.into() })
    }

    pub fn remaining(&self) -> usize {
        self.replies.len()
    }

    pub fn respond_template(&mut self, request: &TemplateRequest, scope: &AppendScope) -> Result<AppendDraft, FakeError> {
        request.validate(scope)?;
        match self.replies.pop_front().ok_or(FakeError::Exhausted)? {
            FakeReply::Json(bytes) => Ok(AppendDraft::bind(scope.clone(), AppendResponse::decode(&bytes, scope)?)?),
            FakeReply::ProviderFailure => Err(FakeError::ProviderFailure),
        }
    }

    pub fn respond(&mut self, request: &TextRequest, doc: &Presentation) -> Result<TextPatch, FakeError> {
        // Invalid requests do not consume a reply. No prompts/bodies are logged or persisted.
        request.validate(doc)?;
        match self.replies.pop_front().ok_or(FakeError::Exhausted)? {
            FakeReply::Json(bytes) => Ok(TextPatch::decode(&bytes, &request.scope, doc)?),
            FakeReply::ProviderFailure => Err(FakeError::ProviderFailure),
        }
    }
}
