//! Intent projection in the existing thumbnail/sorter/canvas positions, not a second outline.
use crate::{
    SlideApp,
    theme::{self, Tokens},
};
use deckcraft_model::Slide;
use egui::{Align2, CornerRadius, Rect, pos2};
use serde_json::json;

/// Returns true only for a planned slot, so existing slides retain their actual thumbnail.
pub fn paint(ui: &egui::Ui, slide: &Slide, rect: Rect) -> bool {
    let Some(plan) = &slide.ai_plan else { return false };
    if !slide.is_planned() {
        return false;
    }
    let t = Tokens::get(ui.ctx());
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    p.rect_filled(rect, CornerRadius::same(3), t.card);
    p.text(pos2(rect.min.x + 8.0, rect.min.y + 8.0), Align2::LEFT_TOP, "计划页 · 尚未展开", theme::font(10.0), t.accent);
    let w = (rect.width() - 16.0).max(1.0);
    let title = p.layout(plan.title.clone(), theme::bold(13.0), t.text, w);
    let y = rect.min.y + 28.0;
    let title_h = title.size().y;
    p.galley(pos2(rect.min.x + 8.0, y), title, t.text);
    let intent = p.layout(plan.intent.clone(), theme::font(10.5), t.text_dim, w);
    p.galley(pos2(rect.min.x + 8.0, y + title_h + 6.0), intent, t.text_dim);
    true
}
pub fn editor(app: &mut SlideApp, ui: &mut egui::Ui, slide: &Slide) {
    let Some(plan) = &slide.ai_plan else { return };
    let Some(uid) = app.session.active().map(|d| d.uid) else { return };
    let key = egui::Id::new(("page-intent", uid, slide.id.0));
    let mut text: (String, String) = ui.data_mut(|d| d.get_temp(key)).unwrap_or_else(|| (plan.title.clone(), plan.intent.clone()));
    ui.strong(if slide.is_planned() { "计划页 · 尚未生成真实页面" } else { "页面概括 / 表达意图（推断）" });
    if plan.unknown {
        ui.small("此页包含读取缺口，意图未知；编辑意图不补造原文事实。");
    }
    ui.add(egui::TextEdit::singleline(&mut text.0).hint_text("页标题 / 职责").char_limit(200).desired_width(f32::INFINITY));
    ui.add(
        egui::TextEdit::multiline(&mut text.1).hint_text("简要表达意图，不是正文全文").char_limit(500).desired_width(f32::INFINITY).desired_rows(3),
    );
    if ui.button("保存页面意图").clicked() {
        let _ = app.run("ai.adjustPageIntent", json!({"docUid":uid,"pageId":slide.id.0,"title":text.0,"intent":text.1}));
        ui.data_mut(|d| d.remove::<(String, String)>(key));
    } else {
        ui.data_mut(|d| d.insert_temp(key, text));
    }
    if slide.is_planned() {
        ui.small("可排序或删除；请保存 .deckcraft。计划未展开时不导出为空白成品。");
    }
}
