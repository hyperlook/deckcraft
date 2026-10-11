//! Tool-free pilot contract and host-independent request lifecycle. No deck writes.
use serde::{Deserialize, Serialize};

pub const INPUT_LIMIT: usize = 512 * 1024;
pub const OUTPUT_LIMIT: usize = 1024 * 1024;
pub const DEADLINE_SECS: u64 = 120;
pub const QUEUE_LIMIT: usize = 64;
pub const ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
pub const FIXTURE: &str = "非敏感连接素材：虚构的社区花园将在周六举办开放日。访客可以了解堆肥和植物养护。免费入场。";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelChoice {
    pub provider: String,
    pub model: String,
    pub endpoint: String,
}
impl ModelChoice {
    pub fn validate(&self) -> bool {
        self.provider == "openai-codex" && !self.model.is_empty() && self.model.len() <= 200 && self.endpoint == ENDPOINT
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
pub enum Request {
    Models { id: u64 },
    Generate { id: u64, provider: String, model: String, input: String },
}
impl Request {
    pub fn id(&self) -> u64 {
        match self {
            Self::Models { id } | Self::Generate { id, .. } => *id,
        }
    }
    pub fn encode(&self) -> Result<Vec<u8>, Failure> {
        // JS identity must remain an exact safe integer. Check size before allocating JSON.
        if self.id() == 0 || self.id() > 9_007_199_254_740_991 {
            return Err(Failure::Protocol);
        }
        if let Self::Generate { provider, model, input, .. } = self {
            if provider != "openai-codex" || model.is_empty() || model.len() > 200 || input.trim().is_empty() {
                return Err(Failure::Protocol);
            }
            if input.len() > INPUT_LIMIT {
                return Err(Failure::Limit);
            }
        }
        let mut bytes = serde_json::to_vec(self).map_err(|_| Failure::Protocol)?;
        if bytes.len() >= INPUT_LIMIT {
            return Err(Failure::Limit);
        }
        bytes.push(b'\n');
        Ok(bytes)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Content {
    pub title: String,
    pub points: Vec<String>,
}
impl Content {
    pub fn validate(&self) -> bool {
        !self.title.trim().is_empty()
            && self.title.chars().count() <= 500
            && self.points.len() <= 32
            && self.points.iter().all(|p| p.chars().count() <= 2000)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    Auth,
    RateLimit,
    InvalidOutput,
    Limit,
    Cancelled,
    Timeout,
    Network,
    Unavailable,
    Unsupported,
    Protocol,
    Transport,
    Exited,
    Busy,
}
impl Failure {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auth => "Pi 认证不可用或刷新失败：请在 Pi /login openai-codex 后重试。",
            Self::RateLimit => "服务商限流 / 额度不足；未自动重试，请稍后手动重试。",
            Self::InvalidOutput => "模型返回了坏 JSON、工具调用或不符合标题/要点契约；没有写稿。",
            Self::Limit => "输入或输出超限（512 KiB / 1 MiB）；请缩短素材后重试。",
            Self::Cancelled => "已取消；迟到结果会被丢弃，没有写稿。",
            Self::Timeout => "请求超过 120 秒；已终止连接，可手动重试。",
            Self::Network => "连接或模型响应失败；请检查网络及 Pi 登录后手动重试。",
            Self::Unavailable => "本地 Bun / Pi SDK 桥不可用。请核对开发环境；不会自动安装。",
            Self::Unsupported => "此先导只支持内置 openai-codex 模型；扩展及其他 Provider 尚未接入。",
            Self::Protocol | Self::Transport => "桥协议或受限传输校验失败；请求已终止。",
            Self::Exited => "本地桥提前退出；请检查 Bun / Pi SDK 开发环境。",
            Self::Busy => "上一请求仍在清理，请稍后重试。",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Event {
    Models {
        id: u64,
        models: Vec<ModelChoice>,
        #[serde(rename = "sdkVersion")]
        sdk_version: String,
    },
    Result {
        id: u64,
        result: Content,
    },
    Error {
        id: u64,
        code: Failure,
    },
}
impl Event {
    pub fn id(&self) -> u64 {
        match self {
            Self::Models { id, .. } | Self::Result { id, .. } | Self::Error { id, .. } => *id,
        }
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Failure> {
        if bytes.len() > OUTPUT_LIMIT {
            return Err(Failure::Limit);
        }
        let event: Self = serde_json::from_slice(bytes).map_err(|_| Failure::Protocol)?;
        match &event {
            Self::Models { models, sdk_version, .. }
                if models.len() <= 256 && sdk_version.len() <= 80 && models.iter().all(ModelChoice::validate) => {}
            Self::Result { result, .. } if result.validate() => {}
            Self::Error { .. } => {}
            _ => return Err(Failure::InvalidOutput),
        }
        Ok(event)
    }
}

/// Optional platform service. Desktop implementation owns all process/I/O work; web has none.
pub trait Host {
    fn start(&mut self, request: Request) -> Result<(), Failure>;
    fn poll(&mut self) -> Option<Event>;
    fn cancel(&mut self);
}

#[derive(Clone, Debug, Default)]
pub enum Status {
    #[default]
    Idle,
    Running(Request),
    Candidate(Content),
    ModelsReady,
    Failed(Failure),
    Cancelled,
}
#[derive(Default)]
pub struct Lifecycle {
    pub status: Status,
    pub models: Vec<ModelChoice>,
    pub sdk_version: String,
    next_id: u64,
}
impl Lifecycle {
    pub fn busy(&self) -> bool {
        matches!(self.status, Status::Running(_))
    }
    pub fn begin(&mut self, choice: Option<&ModelChoice>, input: &str) -> Result<Request, Failure> {
        if self.busy() {
            return Err(Failure::Busy);
        }
        if choice.is_some() && input.len() > INPUT_LIMIT {
            return Err(Failure::Limit);
        }
        self.next_id = self.next_id.checked_add(1).ok_or(Failure::Limit)?;
        let request = match choice {
            Some(choice) if choice.validate() => {
                Request::Generate { id: self.next_id, provider: choice.provider.clone(), model: choice.model.clone(), input: input.to_owned() }
            }
            Some(_) => return Err(Failure::Unsupported),
            None => Request::Models { id: self.next_id },
        };
        request.encode()?;
        self.status = Status::Running(request.clone());
        Ok(request)
    }
    pub fn receive(&mut self, event: Event) {
        let Status::Running(request) = &self.status else { return };
        if event.id() != request.id() {
            return; // Wrong identity and late terminal events are never activated.
        }
        self.status = match (request, event) {
            (Request::Models { .. }, Event::Models { models, sdk_version, .. })
                if models.len() <= 256 && sdk_version.len() <= 80 && models.iter().all(ModelChoice::validate) =>
            {
                self.models = models;
                self.sdk_version = sdk_version;
                Status::ModelsReady
            }
            (Request::Generate { .. }, Event::Result { result, .. }) if result.validate() => Status::Candidate(result),
            (_, Event::Error { code, .. }) => Status::Failed(code),
            _ => Status::Failed(Failure::Protocol),
        };
    }
    pub fn fail(&mut self, failure: Failure) {
        self.status = Status::Failed(failure);
    }
    pub fn cancel(&mut self) {
        if self.busy() {
            self.status = Status::Cancelled;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    fn choice() -> ModelChoice {
        ModelChoice { provider: "openai-codex".into(), model: "gpt-6.1-sol".into(), endpoint: ENDPOINT.into() }
    }
    #[test]
    fn cancelled_wrong_and_duplicate_responses_cannot_reactivate() {
        let mut life = Lifecycle::default();
        let first = life.begin(Some(&choice()), FIXTURE).unwrap();
        assert!(matches!(life.begin(None, ""), Err(Failure::Busy)));
        life.cancel();
        let result = Content { title: "Fixture".into(), points: vec![] };
        life.receive(Event::Result { id: first.id(), result: result.clone() });
        assert!(matches!(life.status, Status::Cancelled));
        let second = life.begin(Some(&choice()), FIXTURE).unwrap();
        life.receive(Event::Result { id: first.id(), result: result.clone() });
        assert!(life.busy());
        life.receive(Event::Result { id: second.id(), result });
        assert!(matches!(life.status, Status::Candidate(_)));
        life.receive(Event::Error { id: second.id(), code: Failure::Auth });
        assert!(matches!(life.status, Status::Candidate(_)));
    }
    #[test]
    fn strict_bounds_and_whitelist() {
        assert!(Event::decode(br#"{"id":1,"type":"result","result":{"title":"x","points":[],"command":"file.save"}}"#).is_err());
        assert!(Event::decode(br#"{"id":1,"type":"error","code":"token=secret"}"#).is_err());
        let mut life = Lifecycle::default();
        assert!(life.begin(Some(&choice()), &"x".repeat(INPUT_LIMIT)).is_err());
        assert!(!life.busy());
        assert!(life.begin(Some(&choice()), &"\n".repeat(INPUT_LIMIT / 2)).is_err());
        // Catalog lookup sends no material and remains usable with an oversized editor input.
        let request = life.begin(None, &"x".repeat(INPUT_LIMIT + 1)).unwrap();
        life.receive(Event::Result { id: request.id(), result: Content { title: "x".into(), points: vec![] } });
        assert!(matches!(life.status, Status::Failed(Failure::Protocol)));
    }
}
