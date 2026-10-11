//! Opt-in offline visual evidence; no model I/O and no user document.
use super::*;
#[test]
#[ignore = "writes offline workspace PNG evidence; run explicitly for visual inspection"]
fn render_two_start_workspace_evidence() {
    let out = std::env::var("DECKCRAFT_M1_EVIDENCE_DIR").unwrap_or_else(|_| "/tmp/deckcraft-m1-evidence".into());
    std::fs::create_dir_all(&out).unwrap();
    let (mut app, wire) = app();
    ready(&mut app);
    app.run("ai.chat", json!({"text":"给社区居民做一份花园参与介绍"})).unwrap();
    app.ui.pane = Some("ai".into());
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 1000.0)).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h.render().unwrap().save(format!("{out}/01-clarification.png")).unwrap();
    for (field, value) in [("purpose", "科普介绍"), ("audience", "社区居民"), ("pageCount", "6 页")] {
        h.state_mut().run("ai.reply", json!({"field":field,"value":value})).unwrap();
    }
    authorize(h.state_mut());
    h.run_steps(2);
    respond(&wire);
    h.run_steps(4);
    h.render().unwrap().save(format!("{out}/02-planned-normal.png")).unwrap();
    std::fs::write(
        format!("{out}/public-planned.deckcraft"),
        deckcraft_engine::cmd::file::save_bytes(&h.state().session.active().unwrap().doc, "deckcraft").unwrap(),
    )
    .unwrap();
    h.state_mut().ui.view = crate::ViewMode::Sorter;
    h.run_steps(4);
    h.render().unwrap().save(format!("{out}/03-planned-sorter.png")).unwrap();
    imported(h.state_mut());
    h.state_mut().ui.view = crate::ViewMode::Normal;
    h.run_steps(3);
    authorize(h.state_mut());
    h.run_steps(2);
    respond(&wire);
    h.run_steps(5);
    h.state_mut().run("slide.go", json!({"index":1})).unwrap();
    h.run_steps(2);
    h.render().unwrap().save(format!("{out}/04-imported-normal.png")).unwrap();
    let doc = &h.state().session.active().unwrap().doc;
    std::fs::write(format!("{out}/public-context.deckcraft"), deckcraft_engine::cmd::file::save_bytes(doc, "deckcraft").unwrap()).unwrap();
}
