//! Offline import review. No model requests and no direct Presentation writes.
use crate::{SlideApp, theme::Tokens};
use deckcraft_model::{ContextDiagnostic as D, ContextDraft, Material, MaterialKind};
use serde_json::json;
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct Assistant {
    editors: HashMap<u64, Editor>,
}
struct Editor {
    draft: ContextDraft,
    limited_ack: bool,
}

pub fn show(app: &mut SlideApp, ui: &mut egui::Ui) {
    crate::ai_pilot::show(app, ui);
    ui.separator();
    ui.collapsing("历史离线提取检查点（摘录 ≠ 大纲；未通过产品验收）", |ui| show_offline(app, ui));
}

fn show_offline(app: &mut SlideApp, ui: &mut egui::Ui) {
    let Some(state) = app.session.active() else {
        ui.label("请先打开自己的 PPTX 或原生文稿。");
        return;
    };
    let uid = state.uid;
    let doc = state.doc.clone();
    let selected = state.current_slide().map(|s| s.id);
    let valid_saved = deckcraft_engine::ai_contract::validate_outline(&doc.outline)
        .and_then(|_| deckcraft_engine::ai_contract::validate_materials(&doc.materials))
        .and_then(|_| doc.context_draft.as_ref().map(deckcraft_engine::ai_contract::validate_context).unwrap_or(Ok(())))
        .and_then(|_| doc.confirmed_context.as_ref().map(deckcraft_engine::ai_contract::validate_context).unwrap_or(Ok(())));
    if let Err(e) = valid_saved {
        ui.label(format!("保存的上下文无效 / 超限，未加载为候选：{e}"));
        if ui.button("清除无效 AI 上下文（不改幻灯片）").clicked() {
            let _ = app.run("ai.setOutline", json!({"docUid":uid,"pages":[],"context":ContextDraft::default()}));
            app.assistant.editors.remove(&uid);
        }
        return;
    }
    // Closed documents cannot leave retained private working copies in the app.
    app.assistant.editors.retain(|id, _| app.session.documents().iter().any(|doc| doc.uid == *id));
    let mut editor = app.assistant.editors.remove(&uid).unwrap_or_else(|| Editor {
        draft: if doc.context_draft.is_some() || doc.confirmed_context.is_some() || !doc.outline.is_empty() {
            deckcraft_ai::review_existing(&doc)
        } else {
            deckcraft_ai::extract_context(&doc)
        },
        limited_ack: false,
    });
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new("导入上下文 · 本地核对").strong());
    ui.label("左侧画布是原页；下方卡片点击定位。提取到文字不等于理解了整稿。此阶段不调用模型、不修改幻灯片。");
    ui.label(if doc.context_draft.is_some() {
        "已恢复暂存草稿 · 仍未确认"
    } else if doc.confirmed_context.is_some() || !doc.outline.is_empty() {
        "默认复用已确认大纲；页面手改不会反写依据"
    } else {
        "页面摘录候选 · 尚未确认"
    });
    ui.label("读取：页内文本及组合文字；不读母版/备注，不做 OCR 或图表解读。未来写回仅限明确标题/正文；组合、图表、图片、页脚及版式不动。");
    if ui.button("显式重新提取（替换当前工作副本）").clicked() {
        editor.draft = deckcraft_ai::extract_context(&doc);
        editor.limited_ack = false;
    }
    let confirmed = doc.outline.pages.iter().filter(|p| p.page_id.is_some_and(|id| doc.slide(id).is_some())).count();
    let covered = editor.draft.pages.iter().filter(|p| !p.unknown && doc.slide(p.page_id).is_some()).count();
    ui.label(format!("已确认依据：{confirmed}/{} 页 · 工作副本拟采用：{covered}/{} 页", doc.slides.len(), doc.slides.len()));
    for d in &editor.draft.diagnostics {
        ui.colored_label(t.text_dim, d.label());
    }
    let mut remove_page = None;
    for page in &mut editor.draft.pages {
        ui.push_id((uid, page.page_id.0), |ui| {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                let index = doc.slide_index(page.page_id);
                let label = index
                    .map(|i| format!("第 {} 页 · ID {}{}", i + 1, page.page_id.0, if selected == Some(page.page_id) { " · 当前原页" } else { "" }))
                    .unwrap_or_else(|| format!("已删除页 · ID {}", page.page_id.0));
                if ui.add_enabled(index.is_some(), egui::Button::new(label)).clicked() && app.session.active().is_some_and(|s| s.uid == uid) {
                    let _ = app.run("slide.go", json!({"id": page.page_id.0}));
                }
                if index.is_none() && ui.button("从草稿移除已删除页").clicked() {
                    remove_page = Some(page.page_id);
                }
                ui.checkbox(&mut page.unknown, "未知 / 本轮不作为事实依据");
                let title_label = ui.label("候选标题（可修正）");
                ui.add(egui::TextEdit::singleline(&mut page.title).hint_text("可修正 / 人工补充").char_limit(500).desired_width(f32::INFINITY))
                    .labelled_by(title_label.id);
                let mut points = page.points.join("\n");
                if ui
                    .add(
                        egui::TextEdit::multiline(&mut points)
                            .hint_text("每行一个要点；未知项不写成事实")
                            .desired_rows(3)
                            .char_limit(64000)
                            .desired_width(f32::INFINITY),
                    )
                    .changed()
                {
                    page.points = points.lines().map(str::to_string).collect();
                }
                let old = doc.outline.pages.iter().find(|p| p.page_id == Some(page.page_id));
                if let Some(old) = old
                    && (old.title != page.title || old.points != page.points || page.unknown)
                {
                    ui.collapsing("将覆盖的已确认依据 / 差异", |ui| {
                        ui.label(format!("原标题：{}", old.title));
                        for p in &old.points {
                            ui.label(format!("• {p}"));
                        }
                        ui.label(if page.unknown { "新状态：未知，移出事实依据" } else { "新内容：上方工作副本" });
                    });
                }
                for d in &page.diagnostics {
                    ui.label(egui::RichText::new(d.label()).small().color(t.text_dim));
                }
                ui.collapsing("候选来源（原页摘录） / 人工修正", |ui| {
                    if page.sources.is_empty() {
                        ui.label("无页面摘录来源；内容来自既有确认 / 人工补充，不是模型归纳。");
                    }
                    for source in &page.sources {
                        let path = source.shape_path.iter().map(|id| id.0.to_string()).collect::<Vec<_>>().join(" / ");
                        ui.label(format!("doc {uid} · page {} · shape {path} · 段 {}：{}", page.page_id.0, source.paragraph + 1, source.text));
                    }
                    ui.label("摘录来源不会被编辑覆盖；与摘录不同的文字须由你核对，是人工修正，不是自动理解。");
                });
            });
        });
    }
    if let Some(id) = remove_page {
        editor.draft.pages.retain(|p| p.page_id != id);
    }
    if selected.is_some_and(|id| !editor.draft.pages.iter().any(|p| p.page_id == id && !p.unknown && !p.title.trim().is_empty())) {
        ui.label("当前目标页缺少确认依据：未来请求改此页前须先补充标题/要点；不要求重写其他页。");
    }
    ui.separator();
    ui.strong("素材 · 用途 / 受众未知可不填");
    ui.label("仅支持粘贴笔记、手填图片说明、宿主选定 UTF-8 txt/md。不会自动抓 URL 或解析 PDF/Office。");
    for name in ["用途", "受众"] {
        let existing = editor.draft.materials.iter().position(|m| m.name == name);
        let mut body = existing.and_then(|i| editor.draft.materials.get(i)).map(|m| m.body.clone()).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label(name);
            let enabled = existing.is_some() || editor.draft.materials.len() < 32;
            if ui.add_enabled(enabled, egui::TextEdit::singleline(&mut body).hint_text("未知（可留空）").char_limit(2000)).changed() {
                if let Some(i) = existing {
                    if body.trim().is_empty() {
                        editor.draft.materials.remove(i);
                    } else if let Some(m) = editor.draft.materials.get_mut(i) {
                        m.body = body;
                    }
                } else if !body.trim().is_empty() {
                    editor.draft.materials.push(Material { name: name.into(), kind: MaterialKind::Text, body });
                }
            }
        });
    }
    if app.services.pick_material.is_none() {
        ui.label("此宿主未提供文本选择器；可粘贴笔记或图片说明。");
    }
    ui.horizontal_wrapped(|ui| {
        for (label, kind) in [("+ 笔记 / 用途 / 受众", MaterialKind::Text), ("+ 图片说明", MaterialKind::Image)] {
            if ui.add_enabled(editor.draft.materials.len() < 32, egui::Button::new(label)).clicked() {
                editor.draft.materials.push(Material { name: format!("素材 {}", editor.draft.materials.len() + 1), kind, body: String::new() });
            }
        }
        if ui.add_enabled(app.services.pick_material.is_some() && editor.draft.materials.len() < 32, egui::Button::new("+ UTF-8 txt/md")).clicked()
            && let Some(pick) = app.services.pick_material.as_mut()
        {
            match pick() {
                Ok(Some((name, body))) => editor.draft.materials.push(Material { name, body, kind: MaterialKind::File }),
                Ok(None) => {}
                Err(e) => app.set_status(e),
            }
        }
    });
    let mut remove = None;
    for (i, material) in editor.draft.materials.iter_mut().enumerate() {
        ui.push_id(("material", uid, i), |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("{:?}", material.kind));
                ui.add(egui::TextEdit::singleline(&mut material.name).char_limit(200).desired_width(160.0));
                if ui.small_button("移除").clicked() {
                    remove = Some(i);
                }
            });
            ui.add(egui::TextEdit::multiline(&mut material.body).desired_rows(3).char_limit(32000).desired_width(f32::INFINITY));
        });
    }
    if let Some(i) = remove {
        editor.draft.materials.remove(i);
    }
    ui.label(
        egui::RichText::new(
            "隐私：确认或暂存后，素材正文与摘录会随 .deckcraft 持久化；不保存源文件路径。PPTX 导出不保证 AI 元数据，重导入需重新核对。",
        )
        .color(t.text_dim),
    );
    let incomplete =
        covered < doc.slides.len()
            || editor.draft.pages.iter().any(|p| {
                p.diagnostics.iter().any(|d| matches!(d, D::Picture | D::Chart | D::Table | D::Group | D::Hidden | D::Limit | D::Unsupported))
            });
    if incomplete {
        ui.checkbox(&mut editor.limited_ack, "我知道仅使用有限上下文，其他页或对象仍有未知");
    }
    let valid = deckcraft_engine::ai_contract::validate_context(&editor.draft);
    if let Err(e) = &valid {
        ui.label(format!("请修正后保存：{e}"));
    }
    let live = editor.draft.pages.iter().all(|p| doc.slide(p.page_id).is_some());
    let mut keep = true;
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(valid.is_ok() && live && (!incomplete || editor.limited_ack), egui::Button::new("统一确认并保存依据")).clicked() {
            let outline = editor.draft.outline();
            if app.run("ai.setOutline", json!({"docUid": uid, "pages": outline.pages, "context": editor.draft})).is_ok() {
                keep = false;
                app.set_status("已确认上下文；幻灯片未修改，可一步撤销。请另存为 .deckcraft 后重开核对。");
            }
        }
        if ui.add_enabled(valid.is_ok(), egui::Button::new("暂存未确认草稿")).clicked() {
            let _ = app.run("ai.setDraft", json!({"docUid": uid, "draft": editor.draft}));
        }
        if ui.button("取消本次编辑").clicked() {
            keep = false;
            app.ui.pane = None;
        }
    });
    if doc.context_draft.is_some()
        && ui.button("丢弃已暂存草稿（保留确认事实）").clicked()
        && app.run("ai.setDraft", json!({"docUid": uid, "draft": null})).is_ok()
    {
        keep = false;
    }
    if keep {
        app.assistant.editors.insert(uid, editor);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use egui_kittest::kittest::Queryable;

    fn imported_app() -> SlideApp {
        let mut s = deckcraft_engine::Session::with_new();
        let shape = s.active().unwrap().doc.slides[0].shapes[0].id;
        s.execute("text.set", &json!({"id":shape, "text":"真实 PPTX 夹具"})).unwrap();
        let pptx = deckcraft_engine::cmd::file::save_bytes(&s.active().unwrap().doc, "pptx").unwrap();
        let mut app = SlideApp::new(deckcraft_engine::Session::new(), Default::default());
        app.open_or_insert_bytes("fixture.pptx", &pptx);
        app.ui.pane = Some("ai".into());
        app
    }

    fn harness(app: SlideApp) -> egui_kittest::Harness<'static, SlideApp> {
        egui_kittest::Harness::builder().with_size(egui::vec2(650.0, 1600.0)).build_ui_state(|ui, app: &mut SlideApp| show_offline(app, ui), app)
    }

    #[test]
    fn imported_pptx_can_be_corrected_confirmed_cancelled_and_undone_in_the_ui() {
        let mut h = harness(imported_app());
        h.run_steps(2);
        let uid = h.state().session.active().unwrap().uid;
        let slides = h.state().session.active().unwrap().doc.slides.clone();
        let title = &h.state().assistant.editors.get(&uid).unwrap().draft.pages[0].title;
        assert_eq!(title, "真实 PPTX 夹具");
        h.get_by_label("候选标题（可修正）").click();
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.get_by_label("候选标题（可修正）").type_text("人工修正");
        h.run_steps(2);
        h.get_by_label("统一确认并保存依据").click();
        h.run_steps(2);
        assert_eq!(h.state().session.active().unwrap().doc.outline.pages[0].title, "人工修正");
        assert_eq!(h.state().session.active().unwrap().doc.slides, slides);
        h.state_mut().run("edit.undo", json!({})).unwrap();
        assert!(h.state().session.active().unwrap().doc.outline.is_empty());
        h.state_mut().run("edit.redo", json!({})).unwrap();
        h.run_steps(2);
        h.get_by_label("候选标题（可修正）").click();
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.get_by_label("候选标题（可修正）").type_text("取消此改动");
        h.run_steps(2);
        h.get_by_label("取消本次编辑").click();
        h.run_steps(1);
        assert_eq!(h.state().session.active().unwrap().doc.outline.pages[0].title, "人工修正");
        assert_eq!(h.state().session.active().unwrap().doc.slides, slides);
    }

    #[test]
    fn confirming_unknown_does_not_reextract_it_on_the_next_frame() {
        let mut h = harness(imported_app());
        h.run_steps(2);
        let uid = h.state().session.active().unwrap().uid;
        let editor = h.state_mut().assistant.editors.get_mut(&uid).unwrap();
        editor.draft.pages[0].unknown = true;
        editor.limited_ack = true;
        h.run_steps(2);
        h.get_by_label("统一确认并保存依据").click();
        h.run_steps(2);
        assert!(h.state().session.active().unwrap().doc.outline.is_empty());
        assert!(h.state().session.active().unwrap().doc.confirmed_context.is_some());
        assert!(h.state().assistant.editors.get(&uid).unwrap().draft.pages[0].unknown);
    }

    #[test]
    fn working_copies_are_bound_to_document_not_active_tab() {
        let mut h = harness(imported_app());
        h.run_steps(2);
        let first_uid = h.state().session.active().unwrap().uid;
        h.state_mut().assistant.editors.get_mut(&first_uid).unwrap().draft.pages[0].title = "第一文稿草稿".into();
        h.state_mut().run("file.new", json!({})).unwrap();
        h.run_steps(2);
        let second_uid = h.state().session.active().unwrap().uid;
        assert_ne!(first_uid, second_uid);
        assert_eq!(h.state().assistant.editors.get(&first_uid).unwrap().draft.pages[0].title, "第一文稿草稿");
        assert!(h.state().assistant.editors.get(&second_uid).unwrap().draft.pages[0].unknown);
        h.state_mut().session.set_active(0);
        h.run_steps(2);
        h.get_by_label("暂存未确认草稿").click();
        h.run_steps(2);
        assert_eq!(h.state().session.active().unwrap().doc.context_draft.as_ref().unwrap().pages[0].title, "第一文稿草稿");
        assert!(h.state().session.active().unwrap().doc.outline.is_empty());
    }

    /// Explicit opt-in evidence capture; the source is exported then truly imported PPTX.
    #[test]
    #[ignore = "writes /tmp/deckcraft-p1-evidence.png using the GPU test renderer"]
    fn render_import_review_evidence() {
        let mut app = imported_app();
        app.ui.pane_width = 400.0;
        app.ui.notes = false;
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 1000.0)).build_ui_state(
            |ui, app: &mut SlideApp| {
                app.logic(ui.ctx());
                app.ui(ui);
            },
            app,
        );
        h.run_steps(3);
        h.render().unwrap().save("/tmp/deckcraft-p1-evidence.png").unwrap();
        let uid = h.state().session.active().unwrap().uid;
        let mut d = deckcraft_ai::extract_context(&h.state().session.active().unwrap().doc);
        d.pages[0].title = "已核对：原生保存恢复".into();
        h.state_mut().run("ai.setOutline", json!({"docUid":uid,"pages":d.outline().pages,"context":d})).unwrap();
        let native = deckcraft_engine::cmd::file::save_bytes(&h.state().session.active().unwrap().doc, "deckcraft").unwrap();
        h.state_mut().open_or_insert_bytes("reviewed.deckcraft", &native);
        h.run_steps(3);
        assert_eq!(h.state().session.active().unwrap().doc.outline.pages[0].title, "已核对：原生保存恢复");
        h.render().unwrap().save("/tmp/deckcraft-p1-restored.png").unwrap();
        let mut sample = deckcraft_engine::Session::new();
        deckcraft_engine::sample::open_sample(&mut sample).unwrap();
        let pptx = deckcraft_engine::cmd::file::save_bytes(&sample.active().unwrap().doc, "pptx").unwrap();
        h.state_mut().open_or_insert_bytes("aurora-import.pptx", &pptx);
        h.run_steps(3);
        h.render().unwrap().save("/tmp/deckcraft-p1-multipage.png").unwrap();
    }

    #[test]
    fn native_review_labels_have_glyphs_when_a_cjk_font_is_available() {
        let db = deckcraft_fonts::FontDb::global();
        // No installation is required on CI; assert when its existing fonts support CJK.
        if !db.has_family("Arial Unicode MS") {
            return;
        }
        let mut h = egui_kittest::Harness::builder().build_ui(|ui| {
            crate::theme::install_fonts(ui.ctx());
            ui.label("核对大纲上下文");
        });
        h.run_steps(2);
        assert!(h.ctx.fonts_mut(|fonts| fonts.has_glyphs(&egui::FontId::proportional(13.0), "核对大纲上下文")));
    }

    #[test]
    fn no_document_is_a_safe_review_screen() {
        let mut harness = egui_kittest::Harness::builder()
            .build_ui_state(|ui, app: &mut SlideApp| show(app, ui), SlideApp::new(deckcraft_engine::Session::new(), Default::default()));
        harness.run_steps(2);
        assert!(harness.state().session.active().is_none());
    }
}
