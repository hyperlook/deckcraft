//! Minimal model entry: display state and dispatch host actions. No prompt/network/deck writes.
use crate::SlideApp;
use deckcraft_ai::request::{FIXTURE, Failure, INPUT_LIMIT, Lifecycle, Request, Status};
use serde_json::{Value, json};

pub(crate) struct Pilot {
    life: Lifecycle,
    input: String,
    selected: Option<usize>,
    authorized: bool,
    origin: Option<u64>,
    snapshot: Option<Request>,
}
impl Default for Pilot {
    fn default() -> Self {
        Self { life: Lifecycle::default(), input: FIXTURE.into(), selected: None, authorized: false, origin: None, snapshot: None }
    }
}

pub fn action(app: &mut SlideApp, id: &str) -> Result<Value, String> {
    if id == "ai.cancelRequest" {
        if let Some(host) = app.services.ai.as_mut() {
            host.cancel();
        }
        app.ai_pilot.life.cancel();
        return Ok(Value::Null);
    }
    let pilot = &mut app.ai_pilot;
    if pilot.life.busy() {
        return Err(Failure::Busy.label().into());
    }
    let choice = if id == "ai.sendMaterial" {
        if !pilot.authorized {
            return Err("请先核对素材和模型，并明确授权发送。".into());
        }
        Some(pilot.selected.and_then(|i| pilot.life.models.get(i)).cloned().ok_or("请先加载并选择模型。")?)
    } else {
        None
    };
    if choice.is_none() {
        pilot.authorized = false;
        pilot.selected = None;
        pilot.life.models.clear();
        pilot.origin = None;
        pilot.snapshot = None;
    }
    let request = match pilot.life.begin(choice.as_ref(), &pilot.input) {
        Ok(request) => request,
        Err(e) => {
            pilot.life.fail(e);
            return Err(e.label().into());
        }
    };
    let Some(host) = app.services.ai.as_mut() else {
        pilot.life.fail(Failure::Unavailable);
        return Err("此宿主不提供本地 Pi 桥；Web 仍可正常编辑。".into());
    };
    if let Err(e) = host.start(request.clone()) {
        pilot.life.fail(e);
        return Err(e.label().into());
    }
    if choice.is_some() {
        pilot.origin = app.session.active().map(|d| d.uid);
        pilot.snapshot = Some(request);
        pilot.authorized = false;
    }
    Ok(Value::Null)
}

pub fn tick(app: &mut SlideApp, ctx: &egui::Context) {
    let pilot = &mut app.ai_pilot;
    // Switching tabs does not rebind. Closing the originating document invalidates the request.
    if pilot.origin.is_some_and(|uid| !app.session.documents().iter().any(|d| d.uid == uid)) {
        if let Some(host) = app.services.ai.as_mut() {
            host.cancel();
        }
        pilot.life.cancel();
        pilot.life.status = Status::Cancelled;
        pilot.origin = None;
        pilot.snapshot = None;
        pilot.input = FIXTURE.into();
        pilot.authorized = false;
    }
    if let Some(host) = app.services.ai.as_mut() {
        // Bounded even if a defective host supplies an endless event stream.
        for _ in 0..deckcraft_ai::request::QUEUE_LIMIT {
            let Some(event) = host.poll() else { break };
            pilot.life.receive(event);
        }
    }
    if pilot.life.busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}

pub fn show(app: &mut SlideApp, ui: &mut egui::Ui) {
    tick(app, ui.ctx());
    ui.strong("AI · 最小真实连接先导");
    ui.label("使用 Pi 已有认证；只发送下方素材，不读取会话、仓库、项目指令或当前幻灯片。不调用工具，不写稿。结果不是全稿大纲。");
    if app.services.ai.is_none() {
        ui.label("此宿主没有本地 Pi 适配器（Web 不支持）；编辑功能不受影响。");
        return;
    }
    ui.label("开发先导需本地 Bun / Pi SDK；目前只接入内置 openai-codex SSE。其他 Provider / 扩展不自动加载。认证问题请在 Pi /login openai-codex。");
    let mut command = None;
    let pilot = &mut app.ai_pilot;
    let busy = pilot.life.busy();
    if ui.add_enabled(!busy, egui::Button::new("加载 Pi 可用模型（不发送素材）")).clicked() {
        command = Some("ai.loadModels");
    }
    if !pilot.life.sdk_version.is_empty() {
        ui.label(format!("Pi SDK {} · {} 个可用模型", pilot.life.sdk_version, pilot.life.models.len()));
        if pilot.life.models.is_empty() {
            ui.label("没有可用的内置 Codex 模型；请检查 Pi 登录。专用扩展 Provider 不在此先导内。");
        }
    }
    let before = pilot.selected;
    ui.add_enabled_ui(!busy, |ui| {
        egui::ComboBox::from_id_salt("ai-model")
            .selected_text(
                pilot
                    .selected
                    .and_then(|i| pilot.life.models.get(i))
                    .map(|m| format!("{} / {}", m.provider, m.model))
                    .unwrap_or_else(|| "请选择模型（不继承当前 coding 会话）".into()),
            )
            .show_ui(ui, |ui| {
                for (i, model) in pilot.life.models.iter().enumerate() {
                    ui.selectable_value(&mut pilot.selected, Some(i), format!("{} / {}", model.provider, model.model));
                }
            });
    });
    if pilot.selected != before {
        pilot.authorized = false;
    }
    if let Some(choice) = pilot.selected.and_then(|i| pilot.life.models.get(i)) {
        ui.label(format!("服务商：{}\n模型：{}\n端点：{}", choice.provider, choice.model, choice.endpoint));
    }
    ui.label("实际发送范围：固定的标题/要点 JSON 契约 + 以下素材全文；不上传文稿文件、路径、图片、认证或其他上下文。素材仅留内存，不随项目保存。");
    ui.add_enabled_ui(!busy, |ui| {
        if ui.add(egui::TextEdit::multiline(&mut pilot.input).desired_rows(6).char_limit(INPUT_LIMIT).desired_width(f32::INFINITY)).changed() {
            pilot.authorized = false;
        }
        if ui.button("恢复非敏感连接素材").clicked() {
            pilot.input = FIXTURE.into();
            pilot.authorized = false;
        }
        ui.checkbox(&mut pilot.authorized, "我已核对服务商、端点和全文，同意发送此素材");
    });
    ui.label(format!("素材 {} 字节 · 输入总上限 512 KiB / 输出 1 MiB / 120 秒 · 无自动重试", pilot.input.len()));
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !busy && pilot.authorized && pilot.selected.and_then(|i| pilot.life.models.get(i)).is_some(),
                egui::Button::new("发送素材（只返回候选）"),
            )
            .clicked()
        {
            command = Some("ai.sendMaterial");
        }
        if ui.add_enabled(busy, egui::Button::new("取消请求")).clicked() {
            command = Some("ai.cancelRequest");
        }
    });
    if let Some(Request::Generate { id, provider, model, input }) = &pilot.snapshot {
        ui.collapsing(format!("本轮不可变发送快照 · 请求 {id}"), |ui| {
            ui.label(format!("{provider} / {model}"));
            ui.label(input);
        });
    }
    match &pilot.life.status {
        Status::Idle => {
            ui.label("尚未连接；加载模型不会发送素材。");
        }
        Status::ModelsReady => {
            ui.label("模型已加载；请选择并授权后发送。");
        }
        Status::Running(_) => {
            ui.spinner();
            ui.label("请求中；可取消，编辑器不阻塞。");
        }
        Status::Cancelled => {
            ui.label(Failure::Cancelled.label());
        }
        Status::Failed(e) => {
            ui.label(e.label());
        }
        Status::Candidate(content) => {
            ui.strong("结构已校验 · 仅候选，未确认/未写稿");
            ui.heading(&content.title);
            for point in &content.points {
                ui.label(format!("• {point}"));
            }
        }
    }
    if let Some(id) = command {
        let _ = app.run(id, json!({}));
        ui.ctx().request_repaint();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use deckcraft_ai::request::{Content, ENDPOINT, Event, Host, ModelChoice};
    struct Fake {
        event: Option<Event>,
    }
    impl Host for Fake {
        fn start(&mut self, request: Request) -> Result<(), Failure> {
            self.event = Some(match request {
                Request::Models { id } => Event::Models {
                    id,
                    models: vec![ModelChoice { provider: "openai-codex".into(), model: "fixture".into(), endpoint: ENDPOINT.into() }],
                    sdk_version: "offline".into(),
                },
                Request::Generate { id, .. } => Event::Result { id, result: Content { title: "候选".into(), points: vec!["离线回归".into()] } },
            });
            Ok(())
        }
        fn poll(&mut self) -> Option<Event> {
            self.event.take()
        }
        fn cancel(&mut self) {} // Deliberately leaves a late event for lifecycle regression.
    }
    fn app() -> SlideApp {
        SlideApp::new(deckcraft_engine::Session::with_new(), crate::Services { ai: Some(Box::new(Fake { event: None })), ..Default::default() })
    }
    #[test]
    fn consent_cancel_and_closure_never_write_deck() {
        let mut app = app();
        let ctx = egui::Context::default();
        let original = serde_json::to_value(&app.session.active().unwrap().doc).unwrap();
        app.run("ai.loadModels", json!({})).unwrap();
        tick(&mut app, &ctx);
        app.ai_pilot.selected = Some(0);
        assert!(app.run("ai.sendMaterial", json!({})).is_err());
        app.ai_pilot.authorized = true;
        app.run("ai.sendMaterial", json!({})).unwrap();
        app.run("ai.cancelRequest", json!({})).unwrap();
        tick(&mut app, &ctx);
        assert!(matches!(app.ai_pilot.life.status, Status::Cancelled));
        app.ai_pilot.authorized = true;
        app.run("ai.sendMaterial", json!({})).unwrap();
        tick(&mut app, &ctx);
        assert!(matches!(app.ai_pilot.life.status, Status::Candidate(_)));
        assert_eq!(original, serde_json::to_value(&app.session.active().unwrap().doc).unwrap());
        app.ai_pilot.authorized = true;
        app.run("ai.sendMaterial", json!({})).unwrap();
        app.run("file.close", json!({"discard":true})).unwrap();
        tick(&mut app, &ctx);
        assert!(matches!(app.ai_pilot.life.status, Status::Cancelled));
    }
    #[test]
    fn visible_model_consent_and_result_path() {
        use egui_kittest::kittest::Queryable;
        let mut h =
            egui_kittest::Harness::builder().with_size(egui::vec2(700.0, 1400.0)).build_ui_state(|ui, app: &mut SlideApp| show(app, ui), app());
        h.run_steps(2);
        h.get_by_label("加载 Pi 可用模型（不发送素材）").click();
        h.run_steps(3);
        h.get_by_value("请选择模型（不继承当前 coding 会话）").click();
        h.run_steps(2);
        h.get_by_label("openai-codex / fixture").click();
        h.run_steps(2);
        assert!(!h.state().ai_pilot.authorized);
        h.get_by_label("我已核对服务商、端点和全文，同意发送此素材").click();
        h.run_steps(2);
        h.get_by_label("发送素材（只返回候选）").click();
        h.run_steps(3);
        assert!(matches!(h.state().ai_pilot.life.status, Status::Candidate(_)));
        assert!(!h.state().ai_pilot.authorized);
        assert!(h.query_all_by_value("结构已校验 · 仅候选，未确认/未写稿").next().is_some());
    }
    #[test]
    fn web_has_an_explicit_unavailable_service() {
        let mut app = SlideApp::new(deckcraft_engine::Session::with_new(), Default::default());
        assert!(app.run("ai.loadModels", json!({})).is_err());
        assert!(matches!(app.ai_pilot.life.status, Status::Failed(Failure::Unavailable)));
    }
}
