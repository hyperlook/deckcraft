//! Thin assistant projection and host-main-loop adapter. Task state lives in deckcraft-ai.
use crate::SlideApp;
use deckcraft_ai::workspace::{Coordinator, Document, Phase};
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct Pilot {
    pub coordinator: Coordinator,
    inputs: HashMap<u64, String>,
    other: HashMap<u64, String>,
}
fn active(app: &SlideApp) -> Result<Document, String> {
    let d = app.session.active().ok_or("请先创建或打开文稿。")?;
    Ok(Document { uid: d.uid, revision: d.revision, interacting: d.interaction.is_some(), doc: d.doc.clone() })
}
pub fn action(app: &mut SlideApp, id: &str, params: &Value) -> Result<Value, String> {
    if id == "ai.cancelRequest" {
        let host = app.services.ai.as_mut().map(|h| h.as_mut() as &mut dyn deckcraft_ai::request::Host);
        app.ai_pilot.coordinator.cancel(host);
        return Ok(Value::Null);
    }
    if id == "ai.loadModels" {
        if app.services.ai.is_none() {
            return Err("此宿主没有本地模型适配器；普通编辑仍可用。".into());
        }
        app.ai_pilot.coordinator.reload_models();
        return Ok(Value::Null);
    }
    let d = active(app)?;
    let c = &mut app.ai_pilot.coordinator;
    c.observe(&d);
    match id {
        "ai.chat" => c.submit(d, params.get("text").and_then(Value::as_str).ok_or("缺少需求。")?)?,
        "ai.reply" => c.reply(
            d.uid,
            params.get("field").and_then(Value::as_str).ok_or("缺少问题。")?,
            params.get("value").and_then(Value::as_str).ok_or("缺少回答。")?,
        )?,
        "ai.authorize" => {
            let on = params.get("on").and_then(Value::as_bool).unwrap_or(false);
            if on && c.choice().is_none() {
                return Err("请等待可用模型加载后再授权。".into());
            }
            c.authorize(d.uid, on);
        }
        "ai.retry" => c.retry(d.uid),
        _ => return Err("未知 AI 动作。".into()),
    }
    Ok(Value::Null)
}
pub fn tick(app: &mut SlideApp, ctx: &egui::Context) {
    let documents: Vec<_> = app
        .session
        .documents()
        .iter()
        .map(|d| Document { uid: d.uid, revision: d.revision, interacting: d.interaction.is_some(), doc: d.doc.clone() })
        .collect();
    app.ai_pilot.inputs.retain(|uid, _| documents.iter().any(|d| d.uid == *uid));
    app.ai_pilot.other.retain(|uid, _| documents.iter().any(|d| d.uid == *uid));
    let host = app.services.ai.as_mut().map(|h| h.as_mut() as &mut dyn deckcraft_ai::request::Host);
    let effects = app.ai_pilot.coordinator.tick(&documents, host);
    for effect in effects {
        let result = app.session.execute("ai.installWorkspace", &effect.params).map(|_| ()).map_err(|e| e.to_string());
        app.ai_pilot.coordinator.acknowledge(effect.uid, result, effect.notify);
    }
    if app.ai_pilot.coordinator.life.busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}
pub fn show(app: &mut SlideApp, ui: &mut egui::Ui) {
    let Ok(d) = active(app) else {
        ui.label("创建或打开文稿，即可开始共同工作。");
        return;
    };
    let uid = d.uid;
    let mut command = None;
    let c = &mut app.ai_pilot.coordinator;
    let busy = c.life.busy();
    ui.strong("一起组织这份演示");
    ui.small("聊天澄清需求，页面列表展示成果。当前支持页计划，尚不生成真实页面。");
    if let Some(workspace) = &d.doc.workspace {
        ui.label(format!("当前方向：{}", workspace.mainline));
    }
    if app.services.ai.is_none() {
        ui.label("此宿主没有本地模型适配器；仍可澄清需求、调整已保存计划和编辑文稿。");
    }
    let mut selected = c.selected;
    ui.add_enabled_ui(!busy, |ui| {
        egui::ComboBox::from_id_salt("workspace-model").selected_text(c.choice().map(|m| m.model.as_str()).unwrap_or("正在加载模型…")).show_ui(
            ui,
            |ui| {
                for (i, m) in c.life.models.iter().enumerate() {
                    ui.selectable_value(&mut selected, Some(i), &m.model);
                }
            },
        );
    });
    if !busy
        && selected != c.selected
        && let Some(index) = selected
    {
        c.select(index);
    }
    if matches!(c.life.status, deckcraft_ai::request::Status::ModelsReady) && c.life.models.is_empty() {
        ui.label("没有可用模型，请检查 Pi 登录。");
        if ui.button("重试模型连接").clicked() {
            command = Some(("ai.loadModels", json!({})));
        }
    }
    if let deckcraft_ai::request::Status::Failed(e) = c.life.status {
        ui.label(e.label());
        if ui.button("重试模型连接").clicked() {
            command = Some(("ai.loadModels", json!({})));
        }
    }
    if let Some(conversation) = c.conversations.get(&uid) {
        let mut authorized = conversation.authorized;
        ui.small("启用后，仅将此文稿的可读文字、需求与素材发送给所选模型；不上传原文件和图片。图片/图表/OCR 尚未理解。换模型需重新授权。");
        if ui
            .add_enabled(!busy && app.services.ai.is_some() && c.choice().is_some(), egui::Checkbox::new(&mut authorized, "允许此文稿使用所选模型"))
            .changed()
        {
            command = Some(("ai.authorize", json!({"on":authorized})));
        }
        egui::ScrollArea::vertical().id_salt(("chat", uid)).max_height(360.0).stick_to_bottom(true).show(ui, |ui| {
            for message in &conversation.messages {
                ui.label(egui::RichText::new(if message.user { "你" } else { "助手" }).strong());
                ui.label(&message.text);
                ui.add_space(6.0);
            }
        });
        if !conversation.actions.is_empty() {
            ui.collapsing("工作状态", |ui| {
                for a in &conversation.actions {
                    ui.small(a);
                }
            });
        }
        if conversation.phase == Phase::Preparing {
            ui.spinner();
            ui.label("正在工作，可取消；切换标签不会换目标。");
        }
        if let Some(question) = conversation.question() {
            ui.separator();
            ui.strong(question.title);
            for choice in question.choices {
                if ui.button(choice).clicked() {
                    command = Some(("ai.reply", json!({"field":question.field,"value":choice})));
                }
            }
            let other = app.ai_pilot.other.entry(uid).or_default();
            ui.add(
                egui::TextEdit::singleline(other)
                    .hint_text(if question.field == "pageCount" { "其他页数（填 3–12）" } else { "其他 / 补充" })
                    .char_limit(200),
            );
            if ui.button("采用补充").clicked() {
                command = Some(("ai.reply", json!({"field":question.field,"value":other})));
            }
        }
        if matches!(conversation.phase, Phase::Failed | Phase::Cancelled | Phase::Conflict) && ui.button("按当前文稿重新组织").clicked() {
            command = Some(("ai.retry", json!({})));
        }
        if conversation.phase == Phase::Ready {
            ui.small("上下文可用 · 页计划在原分页列表中");
        }
    }
    ui.separator();
    let input = app.ai_pilot.inputs.entry(uid).or_default();
    ui.add(egui::TextEdit::singleline(input).hint_text("例如：做一份社区花园介绍").char_limit(2000).desired_width(f32::INFINITY));
    ui.horizontal(|ui| {
        if ui.add_enabled(!busy && !input.trim().is_empty(), egui::Button::new("发送")).clicked() {
            command = Some(("ai.chat", json!({"text":input})));
        }
        if ui.add_enabled(busy, egui::Button::new("取消")).clicked() {
            command = Some(("ai.cancelRequest", json!({})));
        }
    });
    if let Some((id, params)) = command {
        if app.run(id, params).is_ok() {
            if id == "ai.chat" {
                app.ai_pilot.inputs.remove(&uid);
            }
            if id == "ai.reply" {
                app.ai_pilot.other.remove(&uid);
            }
        }
        ui.ctx().request_repaint();
    }
}

#[cfg(test)]
#[path = "summary_tests.rs"]
mod tests;
