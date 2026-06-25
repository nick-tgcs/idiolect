//! egui view layer: renders the DashboardModel into the eframe window.
//! Pure rendering — all decisions come from the model; gestures are
//! mapped by DashboardModel::on_gesture and forwarded to the Backend.

use eframe::egui::{self, Color32, RichText, Ui};

use crate::model::{DashboardModel, DashboardScreen, Gesture};
use crate::theme::{PERIWINKLE, SLATE, TEXT};

pub(crate) fn render(ui: &mut Ui, model: &DashboardModel) -> Vec<Gesture> {
    let mut gestures = Vec::new();

    ui.vertical(|ui| {
        ui.add_space(8.0);
        match &model.screen {
            DashboardScreen::SyncDisabled => render_sync_disabled(ui, &mut gestures),
            DashboardScreen::NoPhones => render_no_phones(ui, &mut gestures),
            DashboardScreen::Phones => render_phones(ui, model, &mut gestures),
            DashboardScreen::PairingQr => render_pairing_qr(ui, model, &mut gestures),
            DashboardScreen::Training => render_training(ui, model),
            DashboardScreen::Prefs => render_prefs(ui, model, &mut gestures),
        }
    });

    gestures
}

fn render_sync_disabled(ui: &mut Ui, gestures: &mut Vec<Gesture>) {
    ui.label(RichText::new("Sync is disabled").color(SLATE));
    ui.add_space(8.0);
    ui.label(
        RichText::new("Enable sync so your phone can send corrections to this PC.").color(TEXT),
    );
    ui.add_space(12.0);
    if ui
        .add(egui::Button::new(RichText::new("Enable Sync").color(Color32::WHITE)).fill(PERIWINKLE))
        .clicked()
    {
        gestures.push(Gesture::EnableSync);
    }
}

fn render_no_phones(ui: &mut Ui, gestures: &mut Vec<Gesture>) {
    ui.label(RichText::new("No phones paired").color(SLATE));
    ui.add_space(8.0);
    ui.label(RichText::new("Pair your Android phone to start sending corrections.").color(TEXT));
    ui.add_space(12.0);
    if ui
        .add(
            egui::Button::new(RichText::new("Pair a phone…").color(Color32::WHITE))
                .fill(PERIWINKLE),
        )
        .clicked()
    {
        gestures.push(Gesture::PairPhone);
    }
}

fn render_phones(ui: &mut Ui, model: &DashboardModel, gestures: &mut Vec<Gesture>) {
    ui.label(RichText::new("Paired phones").color(TEXT).strong());
    ui.add_space(4.0);
    for phone in &model.phones {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(if phone.name.is_empty() {
                    &phone.device_id
                } else {
                    &phone.name
                })
                .color(TEXT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button(RichText::new("Unpair").color(SLATE))
                    .clicked()
                {
                    gestures.push(Gesture::UnpairPhone(phone.device_id.clone()));
                }
            });
        });
    }
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);

    // Corrections / training panel
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} new corrections", model.new_corrections)).color(TEXT));
    });
    if model.new_corrections > 0 {
        ui.add_space(4.0);
        if ui
            .add(
                egui::Button::new(
                    RichText::new(format!("Train now ({})", model.new_corrections))
                        .color(Color32::WHITE),
                )
                .fill(PERIWINKLE),
            )
            .clicked()
        {
            gestures.push(Gesture::TrainNow);
        }
    }
    if let Some(ts) = &model.last_trained_at {
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("Last trained: {ts}"))
                .color(SLATE)
                .small(),
        );
    }

    ui.add_space(8.0);
    if ui
        .add(
            egui::Button::new(RichText::new("Pair another phone…").color(Color32::WHITE))
                .fill(PERIWINKLE),
        )
        .clicked()
    {
        gestures.push(Gesture::PairPhone);
    }
}

fn render_pairing_qr(ui: &mut Ui, model: &DashboardModel, gestures: &mut Vec<Gesture>) {
    ui.label(RichText::new("Scan to pair").color(TEXT).strong());
    ui.add_space(4.0);

    // QR render: dark modules as filled squares, light as gaps
    let qr = &model.pairing;
    if qr.qr_width > 0 && !qr.qr_matrix.is_empty() {
        let module_px = 6.0f32;
        let size = module_px * qr.qr_width as f32;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, Color32::WHITE);
        for row in 0..qr.qr_width {
            for col in 0..qr.qr_width {
                if qr.qr_matrix[row * qr.qr_width + col] {
                    let x = rect.left() + col as f32 * module_px;
                    let y = rect.top() + row as f32 * module_px;
                    painter.rect_filled(
                        egui::Rect::from_min_size(
                            egui::pos2(x, y),
                            egui::vec2(module_px, module_px),
                        ),
                        0.0,
                        Color32::BLACK,
                    );
                }
            }
        }
    } else {
        ui.label(RichText::new("Generating QR…").color(SLATE));
    }

    ui.add_space(6.0);
    if !qr.code.is_empty() {
        ui.label(
            RichText::new(format!("Code: {}", qr.code))
                .color(TEXT)
                .monospace(),
        );
    }
    ui.label(
        RichText::new(format!("Expires in {}s", qr.expires_in_secs))
            .color(SLATE)
            .small(),
    );

    ui.add_space(8.0);
    if ui.button(RichText::new("Cancel").color(SLATE)).clicked() {
        gestures.push(Gesture::CancelPair);
    }
}

fn render_training(ui: &mut Ui, model: &DashboardModel) {
    ui.label(RichText::new("Training…").color(PERIWINKLE).strong());
    if let Some(p) = &model.training_progress {
        ui.label(
            RichText::new(format!(
                "Epoch {}/{} — sample {}/{} — loss {:.3}→{:.3}",
                p.epoch, p.epochs, p.sample, p.total, p.loss_before, p.loss_now
            ))
            .color(TEXT),
        );
        let ratio = if p.total > 0 {
            p.sample as f32 / p.total as f32
        } else {
            0.0
        };
        ui.add(egui::ProgressBar::new(ratio));
    } else {
        ui.spinner();
    }
}

fn render_prefs(ui: &mut Ui, model: &DashboardModel, gestures: &mut Vec<Gesture>) {
    ui.label(RichText::new("Preferences").color(TEXT).strong());
    ui.add_space(8.0);

    ui.label(RichText::new("Phone-facing URL").color(SLATE).small());
    let mut url = model.sync_url.clone();
    if ui.text_edit_singleline(&mut url).changed() {
        gestures.push(Gesture::SetReachableUrl(url.clone()));
    }
    ui.add_space(8.0);

    let mut auto = model.auto_train;
    if ui.checkbox(&mut auto, "Auto-train").changed() {
        gestures.push(Gesture::SetAutoTrain(auto));
    }
    if model.auto_train {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Threshold:").color(TEXT));
            let mut n = model.auto_threshold;
            if ui
                .add(egui::DragValue::new(&mut n).range(1..=200))
                .changed()
            {
                gestures.push(Gesture::SetAutoThreshold(n));
            }
            ui.label(RichText::new("corrections").color(SLATE));
        });
    }
    ui.add_space(4.0);

    let tls_label = if model.sync_tls {
        "TLS: on"
    } else {
        "TLS: off"
    };
    ui.label(RichText::new(tls_label).color(SLATE).small());

    if !model.model_name.is_empty() {
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!(
                "Model: {} ({})",
                model.model_name, model.model_device
            ))
            .color(SLATE)
            .small(),
        );
    }

    ui.add_space(8.0);
    if ui
        .button(RichText::new("Disable Sync").color(SLATE))
        .clicked()
    {
        gestures.push(Gesture::DisableSync);
        gestures.push(Gesture::ClosePrefs);
    }
    ui.add_space(4.0);
    if ui
        .add(egui::Button::new(RichText::new("Done").color(Color32::WHITE)).fill(PERIWINKLE))
        .clicked()
    {
        gestures.push(Gesture::ClosePrefs);
    }
}
