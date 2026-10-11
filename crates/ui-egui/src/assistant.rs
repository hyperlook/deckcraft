//! Chat projection over the shared project coordinator; no separate outline console.
use crate::SlideApp;

pub fn show(app: &mut SlideApp, ui: &mut egui::Ui) {
    crate::ai_pilot::show(app, ui);
}
