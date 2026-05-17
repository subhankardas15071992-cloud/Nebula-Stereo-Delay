//! GUI module for **Nebula Stereo Delay** by Nebula Audio.
//!
//! Implements a DPI-aware, freely-scalable egui editor with a dark professional
//! theme matching the Nebula Audio family style. All elements resize
//! proportionally with the window.
//!
//! # Architecture
//!
//! ```text
//! create_egui_editor()
//!   └─ EditorState (holds params + GUI-only state)
//!       ├─ build()   — one-time theme setup
//!       └─ update()  — per-frame draw calls
//!           ├─ draw_top_bar()
//!           ├─ draw_channel_panel(Left)  | draw_center_section() | draw_channel_panel(Right)
//!           └─ draw_bottom_bar()
//! ```
//!
//! # Layout
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────┐
//! │ Top Bar: Name v1.0 | Preset | A/B | ← → | FX ON | FREE | UNLINKED │
//! ├───────────────────────┬────────┬───────────────────────────────┤
//! │  LEFT Channel Panel   │ Center │  RIGHT Channel Panel          │
//! │  Input Mode           │Routing │  Input Mode                   │
//! │  Delay Time (large)   │CF Phase│  Delay Time (large)           │
//! │  :2  x2               │       │  :2  x2                       │
//! │  Note / Deviation*    │       │  Note / Deviation*             │
//! │  LOW CUT   HIGH CUT   │       │  LOW CUT   HIGH CUT           │
//! │  FEEDBACK  FB PHASE   │       │  FEEDBACK  FB PHASE           │
//! │  Crossfeed L→R        │       │  Crossfeed R→L                │
//! ├───────────────────────┴────────┴───────────────────────────────┤
//! │ Bottom Bar: MIX L  MIX R                                       │
//! └──────────────────────────────────────────────────────────────────┘
//! ```
//!
//! \* Visible only when Tempo Sync is enabled.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use nih_plug::params::enums::Enum;
use nih_plug::params::Param;
use nih_plug::prelude::{Editor, ParamSetter};
use nih_plug_egui::egui::{
    self, vec2, Align, Align2, Button, Color32, Context, CornerRadius, FontId, Frame, Layout,
    Painter, Pos2, Rect, Response, ScrollArea, Sense, Shape, Stroke, Ui, UiBuilder,
};
use nih_plug_egui::EguiState;

use crate::parameters::{
    InputModeParam, NebulaStereoDelayParams, NoteValueParam, ParamSnapshot, RoutingModeParam,
};
use crate::preset::{PresetManager, PresetValues};

// ═══════════════════════════════════════════════════════════════════════════
// Theme constants
// ═══════════════════════════════════════════════════════════════════════════

/// Main background — deep midnight navy.
const BG: Color32 = Color32::from_rgb(0x06, 0x05, 0x16);
/// Panel / section background — slightly lighter.
const PANEL_BG: Color32 = Color32::from_rgb(0x0D, 0x0A, 0x22);
/// Inset / recessed area background.
const INSET_BG: Color32 = Color32::from_rgb(0x08, 0x07, 0x18);
/// Widget surface (knob body, button face).
const WIDGET_BG: Color32 = Color32::from_rgb(0x13, 0x10, 0x2D);
/// Accent colour — cyan / teal.
const ACCENT: Color32 = Color32::from_rgb(0x00, 0xD8, 0xFF);
/// Accent at reduced opacity for highlight backgrounds.
const ACCENT_DIM: Color32 = Color32::from_rgba_premultiplied(0x00, 0xA8, 0xFF, 0x44);
const MAGENTA: Color32 = Color32::from_rgb(0xFF, 0x28, 0xC7);
const ORANGE: Color32 = Color32::from_rgb(0xFF, 0xA8, 0x00);
const PURPLE: Color32 = Color32::from_rgb(0x9A, 0x5C, 0xFF);
/// Primary text — white.
const TEXT_PRI: Color32 = Color32::from_rgb(0xEE, 0xEE, 0xEE);
/// Secondary / label text — mid gray.
const TEXT_SEC: Color32 = Color32::from_rgb(0x63, 0x86, 0xC7);
/// Knob track (unfilled arc).
const KNOB_TRACK: Color32 = Color32::from_rgb(0x23, 0x1B, 0x4C);
/// Active / "on" button fill.
const BTN_ON: Color32 = ACCENT;
/// Inactive / "off" button fill.
const BTN_OFF: Color32 = Color32::from_rgb(0x16, 0x12, 0x35);
/// Bypass / danger indicator.
const DANGER: Color32 = Color32::from_rgb(0xCC, 0x33, 0x33);
/// Border / separator.
const BORDER: Color32 = Color32::from_rgb(0x34, 0x25, 0x72);

/// Arc start angle: 135° (7-o'clock position in egui's y-down coordinates).
const ARC_START: f32 = 3.0 * std::f32::consts::FRAC_PI_4;
/// Arc end angle: 405° (5-o'clock position).
const ARC_END: f32 = ARC_START + 3.0 * std::f32::consts::FRAC_PI_2;
/// Total sweep = 270°.
const ARC_SWEEP: f32 = ARC_END - ARC_START;

/// Default window width in logical pixels.
const WIN_W: u32 = 900;
/// Default window height in logical pixels.
const WIN_H: u32 = 700;

// ═══════════════════════════════════════════════════════════════════════════
// Editor state
// ═══════════════════════════════════════════════════════════════════════════

/// GUI-specific state that persists across frames and editor sessions.
struct EditorState {
    params: Arc<NebulaStereoDelayParams>,
    /// Shared reference to the `EguiState` for window-size coordination.
    egui_state: Arc<EguiState>,
    /// Whether a MIDI-learn listening session is active.
    midi_learn_active: bool,
    /// The parameter ID currently targeted for MIDI learn, if any.
    midi_learn_target: Option<String>,
    /// Factory/user preset IO.
    preset_manager: PresetManager,
    /// Name used when saving a user preset.
    preset_name: String,
    /// Short status shown in the preset menu after save/load actions.
    preset_status: Option<String>,
}

// ═══════════════════════════════════════════════════════════════════════════
// Public entry point
// ═══════════════════════════════════════════════════════════════════════════

/// Create the egui editor for the Nebula Stereo Delay plugin.
///
/// Returns `Option<Box<dyn Editor>>` suitable for use in the plugin's
/// `editor()` method. The window starts at 900 × 700 logical pixels and
/// is freely resizable via the corner drag handle; all elements scale
/// proportionally with the window size and system DPI.
pub fn create_egui_editor(params: Arc<NebulaStereoDelayParams>) -> Option<Box<dyn Editor>> {
    let egui_state = EguiState::from_size(WIN_W, WIN_H);
    let egui_state_for_closure = egui_state.clone();

    nih_plug_egui::create_egui_editor(
        egui_state,
        EditorState {
            params,
            egui_state: egui_state_for_closure,
            midi_learn_active: false,
            midi_learn_target: None,
            preset_manager: PresetManager::new(),
            preset_name: "User Preset".to_string(),
            preset_status: None,
        },
        |ctx, _state| {
            apply_dark_theme(ctx);
        },
        |ctx, setter, state| {
            // Use ResizableWindow so the user can drag the corner to resize.
            // The egui_state reference is shared with the EguiEditor wrapper
            // for window-size coordination.
            let egui_state = state.egui_state.clone();
            nih_plug_egui::resizable_window::ResizableWindow::new("nebula-stereo-delay")
                .min_size(vec2(600.0, 400.0))
                .show(ctx, egui_state.as_ref(), |ui| {
                    let root_rect = ui.max_rect();
                    let mut root_ui = ui.new_child(
                        UiBuilder::new()
                            .max_rect(root_rect)
                            .layout(Layout::top_down(Align::Min)),
                    );
                    draw_root(&mut root_ui, state, setter);
                });
        },
    )
}

/// Draw the entire plugin UI into the given `Ui`.
fn draw_root(ui: &mut Ui, state: &mut EditorState, setter: &ParamSetter<'_>) {
    let root_rect = ui.max_rect();

    // Derive a proportional scale factor from the available width.
    // Reference width = 900 logical px; all sizes are multiplied by `s`.
    let s = (root_rect.width() / 900.0).clamp(0.72, 1.35);

    // Full-background fill.
    ui.painter().rect_filled(root_rect, 0.0, BG);

    ui.set_min_size(root_rect.size());

    ScrollArea::vertical()
        .id_salt("nebula_editor_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(root_rect.width());
            draw_editor_contents(ui, state, setter, s);
        });
}

fn draw_editor_contents(ui: &mut Ui, state: &mut EditorState, setter: &ParamSetter<'_>, s: f32) {
    ui.with_layout(Layout::top_down(Align::Min), |ui| {
        // ── Top Bar ─────────────────────────────────────────
        draw_top_bar(ui, state, setter, s);

        ui.add_space(6.0 * s);

        // ── Main content row: Left | Center | Right ─────────
        let content_w = ui.available_width().max(320.0);
        if content_w < 760.0 {
            draw_channel_panel(ui, state, setter, s, Channel::Left, content_w);
            ui.add_space(6.0 * s);
            draw_center_section(ui, state, setter, s, content_w);
            ui.add_space(6.0 * s);
            draw_channel_panel(ui, state, setter, s, Channel::Right, content_w);
        } else {
            ui.horizontal(|ui| {
                let center_w = 116.0 * s;
                let side_w = ((ui.available_width() - center_w - 12.0 * s) / 2.0).max(240.0 * s);

                draw_channel_panel(ui, state, setter, s, Channel::Left, side_w);
                ui.add_space(6.0 * s);
                draw_center_section(ui, state, setter, s, center_w);
                ui.add_space(6.0 * s);
                draw_channel_panel(ui, state, setter, s, Channel::Right, side_w);
            });
        }

        ui.add_space(6.0 * s);

        // ── Bottom Bar ──────────────────────────────────────
        draw_bottom_bar(ui, state, setter, s);
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// Dark theme
// ═══════════════════════════════════════════════════════════════════════════

fn apply_dark_theme(ctx: &Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = vec2(6.0, 4.0);
    style.spacing.button_padding = vec2(6.0, 3.0);
    style.visuals.dark_mode = true;
    style.visuals.panel_fill = PANEL_BG;
    style.visuals.extreme_bg_color = BG;
    style.visuals.window_fill = PANEL_BG;
    style.visuals.widgets.inactive.bg_fill = WIDGET_BG;
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_PRI);
    style.visuals.widgets.inactive.weak_bg_fill = WIDGET_BG;
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(0x36, 0x36, 0x36);
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT_PRI);
    style.visuals.widgets.active.bg_fill = ACCENT_DIM;
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.5, TEXT_PRI);
    style.visuals.selection.bg_fill = ACCENT_DIM;
    style.visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    style.visuals.window_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_SEC);
    ctx.set_style(style);
}

// ═══════════════════════════════════════════════════════════════════════════
// Channel discriminator
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, PartialEq, Eq)]
enum Channel {
    Left,
    Right,
}

// ═══════════════════════════════════════════════════════════════════════════
// Top Bar
// ═══════════════════════════════════════════════════════════════════════════

fn draw_top_bar(ui: &mut Ui, state: &mut EditorState, setter: &ParamSetter<'_>, s: f32) {
    Frame::NONE
        .fill(PANEL_BG)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(corner_radius(4.0 * s))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.set_min_height(36.0 * s);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(12.0 * s);

                // Plugin name
                ui.label(rich("NEBULA STEREO DELAY", 14.0 * s).color(ACCENT).strong());
                ui.label(rich("v1.0", 9.0 * s).color(TEXT_SEC));
                ui.add_space(14.0 * s);

                // Preset button
                draw_preset_button(ui, state, setter, s);
                ui.add_space(6.0 * s);

                // A/B toggle
                draw_ab_button(ui, state, setter, s);
                ui.add_space(4.0 * s);

                // Undo / Redo
                draw_undo_btn(ui, state, setter, s);
                ui.add_space(2.0 * s);
                draw_redo_btn(ui, state, setter, s);

                ui.add_space(10.0 * s);

                // MIDI Learn
                draw_midi_learn_btn(ui, state, s);
                ui.add_space(4.0 * s);

                // FX Bypass
                draw_bypass_btn(ui, state, setter, s);
                ui.add_space(4.0 * s);

                // Tempo Sync
                draw_sync_btn(ui, state, setter, s);
                ui.add_space(4.0 * s);

                // Stereo Link
                draw_link_btn(ui, state, setter, s);

                ui.add_space(12.0 * s);
            });
        });
}

// ── Top-bar helpers ──────────────────────────────────────────────────────

fn draw_preset_button(ui: &mut Ui, state: &mut EditorState, setter: &ParamSetter<'_>, s: f32) {
    let resp = ui.add(
        Button::new(rich("Preset", 11.0 * s).color(TEXT_PRI))
            .fill(WIDGET_BG)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(corner_radius(3.0 * s)),
    );
    let popup_id = ui.id().with("preset_menu");
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup_id));
    }

    egui::popup::popup_above_or_below_widget(
        ui,
        popup_id,
        &resp,
        egui::AboveOrBelow::Below,
        egui::popup::PopupCloseBehavior::CloseOnClickOutside,
        |ui| {
            Frame::NONE
                .fill(PANEL_BG)
                .stroke(Stroke::new(1.0, BORDER))
                .show(ui, |ui| {
                    ui.set_min_width(250.0 * s);

                    ui.label(rich("Save", 10.0 * s).color(ACCENT).strong());
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut state.preset_name)
                                .desired_width(138.0 * s)
                                .font(egui::TextStyle::Small),
                        );
                        if ui
                            .add(Button::new(rich("Current", 9.0 * s).color(TEXT_PRI)))
                            .clicked()
                        {
                            let values = preset_values_from_params(&state.params);
                            state.preset_status = match state.preset_manager.save_user_preset(
                                &state.preset_name,
                                "Nebula User",
                                &values,
                            ) {
                                Ok(()) => Some(format!("Saved {}", state.preset_name)),
                                Err(err) => Some(err),
                            };
                        }
                    });

                    ui.horizontal(|ui| {
                        if ui
                            .add(Button::new(rich("Save A", 9.0 * s).color(TEXT_PRI)))
                            .clicked()
                        {
                            if let Ok(snapshots) = state.params.ab_snapshots.read() {
                                let values = preset_values_from_snapshot(&snapshots.a);
                                state.preset_status = match state.preset_manager.save_user_preset(
                                    &format!("{} A", state.preset_name),
                                    "Nebula User",
                                    &values,
                                ) {
                                    Ok(()) => Some(format!("Saved {} A", state.preset_name)),
                                    Err(err) => Some(err),
                                };
                            }
                        }
                        if ui
                            .add(Button::new(rich("Save B", 9.0 * s).color(TEXT_PRI)))
                            .clicked()
                        {
                            if let Ok(snapshots) = state.params.ab_snapshots.read() {
                                let values = preset_values_from_snapshot(&snapshots.b);
                                state.preset_status = match state.preset_manager.save_user_preset(
                                    &format!("{} B", state.preset_name),
                                    "Nebula User",
                                    &values,
                                ) {
                                    Ok(()) => Some(format!("Saved {} B", state.preset_name)),
                                    Err(err) => Some(err),
                                };
                            }
                        }
                    });

                    if let Some(status) = &state.preset_status {
                        ui.label(rich(status, 8.0 * s).color(TEXT_SEC));
                    }

                    ui.separator();
                    ui.label(rich("Factory", 10.0 * s).color(ACCENT).strong());
                    let factory = state.preset_manager.factory_presets().to_vec();
                    for preset in factory {
                        if ui
                            .add(Button::new(rich(&preset.name, 9.0 * s).color(TEXT_PRI)))
                            .clicked()
                        {
                            state.params.push_undo();
                            state
                                .preset_manager
                                .load_preset(&preset, &state.params, setter);
                            state.preset_status = Some(format!("Loaded {}", preset.name));
                            ui.memory_mut(|m| m.close_popup());
                        }
                    }

                    ui.separator();
                    ui.label(rich("User", 10.0 * s).color(ACCENT).strong());
                    match state.preset_manager.user_presets() {
                        Ok(user_presets) if user_presets.is_empty() => {
                            ui.label(rich("No user presets", 8.0 * s).color(TEXT_SEC));
                        }
                        Ok(user_presets) => {
                            for preset in user_presets {
                                ui.horizontal(|ui| {
                                    if ui
                                        .add(Button::new(
                                            rich(&preset.name, 9.0 * s).color(TEXT_PRI),
                                        ))
                                        .clicked()
                                    {
                                        state.params.push_undo();
                                        state.preset_manager.load_preset(
                                            &preset,
                                            &state.params,
                                            setter,
                                        );
                                        state.preset_status =
                                            Some(format!("Loaded {}", preset.name));
                                        ui.memory_mut(|m| m.close_popup());
                                    }
                                    if ui
                                        .add(Button::new(rich("Delete", 8.0 * s).color(TEXT_SEC)))
                                        .clicked()
                                    {
                                        state.preset_status = match state
                                            .preset_manager
                                            .delete_user_preset(&preset.name)
                                        {
                                            Ok(()) => Some(format!("Deleted {}", preset.name)),
                                            Err(err) => Some(err),
                                        };
                                    }
                                });
                            }
                        }
                        Err(err) => {
                            ui.label(rich(err, 8.0 * s).color(DANGER));
                        }
                    }
                });
        },
    );
    resp.on_hover_text("Preset management: save, load, and recall");
}

fn draw_ab_button(ui: &mut Ui, state: &mut EditorState, setter: &ParamSetter<'_>, s: f32) {
    let slot = state.params.ab_state.load(Ordering::Relaxed);
    let label = if slot == 0 { "A" } else { "B" };
    let active = slot == 1;

    let resp = ui.add(
        Button::new(
            rich(label, 13.0 * s)
                .color(if active { TEXT_PRI } else { TEXT_SEC })
                .strong(),
        )
        .fill(if active { BTN_ON } else { BTN_OFF })
        .stroke(Stroke::new(1.0, if active { ACCENT } else { BORDER }))
        .corner_radius(corner_radius(3.0 * s)),
    );

    if resp.clicked() {
        state.params.push_undo();
        let snapshot = state.params.ab_toggle();
        apply_snapshot(&state.params, setter, &snapshot);
    }
    resp.on_hover_text("Toggle A/B comparison");
}

fn draw_undo_btn(ui: &mut Ui, state: &mut EditorState, setter: &ParamSetter<'_>, s: f32) {
    let resp = ui.add(
        Button::new(rich("\u{2190}", 14.0 * s).color(TEXT_SEC))
            .fill(WIDGET_BG)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(corner_radius(3.0 * s)),
    );
    if resp.clicked() {
        let current = take_snapshot(&state.params);
        if let Ok(mut stack) = state.params.undo_stack.write() {
            if let Some(prev) = stack.undo(current) {
                apply_snapshot(&state.params, setter, &prev);
            }
        }
    }
    resp.on_hover_text("Undo");
}

fn draw_redo_btn(ui: &mut Ui, state: &mut EditorState, setter: &ParamSetter<'_>, s: f32) {
    let resp = ui.add(
        Button::new(rich("\u{2192}", 14.0 * s).color(TEXT_SEC))
            .fill(WIDGET_BG)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(corner_radius(3.0 * s)),
    );
    if resp.clicked() {
        let current = take_snapshot(&state.params);
        if let Ok(mut stack) = state.params.undo_stack.write() {
            if let Some(next) = stack.redo(current) {
                apply_snapshot(&state.params, setter, &next);
            }
        }
    }
    resp.on_hover_text("Redo");
}

fn draw_midi_learn_btn(ui: &mut Ui, state: &mut EditorState, s: f32) {
    let learning = state.midi_learn_active
        || state
            .params
            .midi_learn
            .read()
            .map(|ml| ml.is_learning())
            .unwrap_or(false);
    let global_on = state
        .params
        .midi_learn
        .read()
        .map(|ml| ml.is_global_enabled())
        .unwrap_or(true);
    let label = if learning { "LEARN..." } else { "MIDI" };

    let resp = ui.add(
        Button::new(rich(label, 11.0 * s).color(if global_on { TEXT_PRI } else { TEXT_SEC }))
            .fill(if learning { BTN_ON } else { WIDGET_BG })
            .stroke(Stroke::new(
                1.0,
                if learning {
                    ACCENT
                } else if global_on {
                    BORDER
                } else {
                    DANGER
                },
            ))
            .corner_radius(corner_radius(3.0 * s)),
    );

    if resp.clicked() {
        state.midi_learn_active = !state.midi_learn_active;
        if !state.midi_learn_active {
            state.midi_learn_target = None;
            if let Ok(mut ml) = state.params.midi_learn.write() {
                ml.stop_learn();
            }
        }
    }

    resp.context_menu(|ui| {
        let mut close = false;
        if let Ok(mut ml) = state.params.midi_learn.write() {
            if ui
                .button(if ml.is_global_enabled() {
                    "MIDI Off"
                } else {
                    "MIDI On"
                })
                .clicked()
            {
                ml.toggle_global_enabled();
                close = true;
            }

            ui.separator();
            ui.label(rich("Clean Up", 10.0).color(ACCENT).strong());
            let mappings = ml.mappings().to_vec();
            for mapping in mappings {
                let label = format!(
                    "{}  Ch {} CC {}",
                    mapping.param_id, mapping.channel, mapping.cc
                );
                if ui.button(label).clicked() {
                    ml.clean_up(&mapping.param_id);
                    close = true;
                }
            }
            if ui.button("Clear All").clicked() {
                ml.clear_all();
                close = true;
            }

            ui.separator();
            if ui.button("Roll Back").clicked() {
                ml.roll_back();
                close = true;
            }
            if ui.button("Save").clicked() {
                ml.save_for_rollback();
                close = true;
            }
        }
        if close {
            ui.close_menu();
        }
    });

    resp.on_hover_text("MIDI learn");
}

fn draw_bypass_btn(ui: &mut Ui, state: &mut EditorState, _setter: &ParamSetter<'_>, s: f32) {
    let bypassed = state.params.bypass.load(Ordering::Relaxed);
    let (label, fg, bg, st) = if bypassed {
        ("FX OFF", TEXT_PRI, DANGER, DANGER)
    } else {
        ("FX ON", TEXT_PRI, BTN_ON, ACCENT)
    };

    let resp = ui.add(
        Button::new(rich(label, 11.0 * s).color(fg).strong())
            .fill(bg)
            .stroke(Stroke::new(1.0, st))
            .corner_radius(corner_radius(3.0 * s)),
    );
    if resp.clicked() {
        state.params.bypass.store(!bypassed, Ordering::Relaxed);
    }
    resp.on_hover_text(if bypassed {
        "Effect bypassed \u{2014} click to enable"
    } else {
        "Effect active \u{2014} click to bypass"
    });
}

fn draw_sync_btn(ui: &mut Ui, state: &mut EditorState, setter: &ParamSetter<'_>, s: f32) {
    let synced = state.params.tempo_sync.value();
    let (label, fg, bg, st) = if synced {
        ("SYNC", TEXT_PRI, BTN_ON, ACCENT)
    } else {
        ("FREE", TEXT_SEC, BTN_OFF, BORDER)
    };

    let resp = ui.add(
        Button::new(rich(label, 11.0 * s).color(fg).strong())
            .fill(bg)
            .stroke(Stroke::new(1.0, st))
            .corner_radius(corner_radius(3.0 * s)),
    );
    if resp.clicked() {
        state.params.push_undo();
        setter.begin_set_parameter(&state.params.tempo_sync);
        setter.set_parameter(&state.params.tempo_sync, !synced);
        setter.end_set_parameter(&state.params.tempo_sync);
    }
    resp.on_hover_text(if synced {
        "Tempo sync on \u{2014} delay follows host tempo"
    } else {
        "Free mode \u{2014} delay time in seconds"
    });
}

fn draw_link_btn(ui: &mut Ui, state: &mut EditorState, setter: &ParamSetter<'_>, s: f32) {
    let linked = state.params.stereo_link.value();
    let (label, fg, bg, st) = if linked {
        ("LINKED", TEXT_PRI, BTN_ON, ACCENT)
    } else {
        ("UNLINKED", TEXT_SEC, BTN_OFF, BORDER)
    };

    let resp = ui.add(
        Button::new(rich(label, 11.0 * s).color(fg).strong())
            .fill(bg)
            .stroke(Stroke::new(1.0, st))
            .corner_radius(corner_radius(3.0 * s)),
    );
    if resp.clicked() {
        state.params.push_undo();
        setter.begin_set_parameter(&state.params.stereo_link);
        setter.set_parameter(&state.params.stereo_link, !linked);
        setter.end_set_parameter(&state.params.stereo_link);
    }
    resp.on_hover_text(if linked {
        "Stereo linked \u{2014} L mirrored to R"
    } else {
        "Stereo unlinked \u{2014} independent L/R"
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// Channel Panel (Left / Right)
// ═══════════════════════════════════════════════════════════════════════════

fn draw_channel_panel(
    ui: &mut Ui,
    state: &mut EditorState,
    setter: &ParamSetter<'_>,
    s: f32,
    ch: Channel,
    width: f32,
) {
    Frame::NONE
        .fill(PANEL_BG)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(corner_radius(6.0 * s))
        .show(ui, |ui| {
            let params = state.params.clone();

            ui.set_min_width(width);
            ui.set_max_width(width);
            ui.vertical(|ui| {
                ui.add_space(8.0 * s);

                // Channel label
                let ch_name = if ch == Channel::Left { "LEFT" } else { "RIGHT" };
                ui.label(rich(ch_name, 13.0 * s).color(ACCENT).strong());
                ui.add_space(4.0 * s);

                // Input mode popup
                draw_input_popup(ui, state, setter, s, ch);
                ui.add_space(6.0 * s);

                // Delay time (large, prominent)
                draw_delay_section(ui, state, setter, s, ch);
                ui.add_space(8.0 * s);

                // Low Cut / High Cut side-by-side
                ui.horizontal(|ui| {
                    draw_knob_field(
                        ui,
                        state,
                        setter,
                        s,
                        ch_knob_param!(params, ch, low_cut_l, low_cut_r),
                        "HPF",
                        KnobSize::Small,
                    );
                    draw_knob_field(
                        ui,
                        state,
                        setter,
                        s,
                        ch_knob_param!(params, ch, high_cut_l, high_cut_r),
                        "LPF",
                        KnobSize::Small,
                    );
                });
                ui.horizontal(|ui| {
                    draw_knob_field(
                        ui,
                        state,
                        setter,
                        s,
                        ch_knob_param!(params, ch, low_cut_slope_l, low_cut_slope_r),
                        "HPFS",
                        KnobSize::Small,
                    );
                    draw_knob_field(
                        ui,
                        state,
                        setter,
                        s,
                        ch_knob_param!(params, ch, high_cut_slope_l, high_cut_slope_r),
                        "LPFS",
                        KnobSize::Small,
                    );
                });
                ui.add_space(6.0 * s);

                // Feedback + Phase
                ui.horizontal(|ui| {
                    draw_knob_field(
                        ui,
                        state,
                        setter,
                        s,
                        ch_knob_param!(params, ch, feedback_l, feedback_r),
                        "FEEDBACK",
                        KnobSize::Small,
                    );
                    let phase_params = state.params.clone();
                    let phase_param = if ch == Channel::Left {
                        &phase_params.feedback_phase_l
                    } else {
                        &phase_params.feedback_phase_r
                    };
                    draw_phase_btn(ui, state, setter, phase_param, s, "FB \u{00d8}");
                });
                ui.add_space(6.0 * s);

                // Crossfeed + per-direction phase
                let cf_label = if ch == Channel::Left {
                    "L\u{2192}R"
                } else {
                    "R\u{2192}L"
                };
                ui.horizontal(|ui| {
                    draw_knob_field(
                        ui,
                        state,
                        setter,
                        s,
                        ch_knob_param!(params, ch, crossfeed_lr, crossfeed_rl),
                        cf_label,
                        KnobSize::Small,
                    );
                    let cf_phase_param = if ch == Channel::Left {
                        &params.crossfeed_phase_lr
                    } else {
                        &params.crossfeed_phase_rl
                    };
                    draw_phase_btn(ui, state, setter, cf_phase_param, s, "CF \u{00d8}");
                });

                ui.add_space(6.0 * s);
            });
        });
}

/// Macro to select the L or R variant of a parameter pair.
macro_rules! ch_knob_param {
    ($params:expr, $ch:expr, $l:ident, $r:ident) => {
        if $ch == Channel::Left {
            &$params.$l
        } else {
            &$params.$r
        }
    };
}
use ch_knob_param;

// ── Input mode popup ────────────────────────────────────────────────────

fn draw_input_popup(
    ui: &mut Ui,
    state: &mut EditorState,
    setter: &ParamSetter<'_>,
    s: f32,
    ch: Channel,
) {
    let param = if ch == Channel::Left {
        &state.params.input_mode_l
    } else {
        &state.params.input_mode_r
    };

    let current_name = enum_name(param.value());
    let label = format!("Input: {current_name}");

    let resp = ui.add(
        Button::new(rich(&label, 10.0 * s).color(TEXT_PRI))
            .fill(WIDGET_BG)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(corner_radius(3.0 * s)),
    );

    let popup_id = ui.id().with(if ch == Channel::Left {
        "input_l"
    } else {
        "input_r"
    });
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup_id));
    }

    let variants: [(InputModeParam, &str); 5] = [
        (InputModeParam::Off, "Off"),
        (InputModeParam::Left, "Left"),
        (InputModeParam::Right, "Right"),
        (InputModeParam::LeftPlusRight, "L+R"),
        (InputModeParam::LeftMinusRight, "L-R"),
    ];

    egui::popup::popup_above_or_below_widget(
        ui,
        popup_id,
        &resp,
        egui::AboveOrBelow::Below,
        egui::popup::PopupCloseBehavior::CloseOnClick,
        |ui| {
            Frame::NONE
                .fill(PANEL_BG)
                .stroke(Stroke::new(1.0, BORDER))
                .show(ui, |ui| {
                    for (variant, name) in variants {
                        let sel = enum_name(variant) == current_name;
                        let btn = Button::new(rich(name, 10.0 * s).color(if sel {
                            ACCENT
                        } else {
                            TEXT_PRI
                        }))
                        .fill(if sel { ACCENT_DIM } else { PANEL_BG })
                        .corner_radius(corner_radius(2.0));
                        if ui.add(btn).clicked() {
                            state.params.push_undo();
                            setter.begin_set_parameter(param);
                            setter.set_parameter(param, variant);
                            setter.end_set_parameter(param);
                            if stereo_link_active(ui, &state.params) {
                                let other = if ch == Channel::Left {
                                    &state.params.input_mode_r
                                } else {
                                    &state.params.input_mode_l
                                };
                                setter.begin_set_parameter(other);
                                setter.set_parameter(other, variant);
                                setter.end_set_parameter(other);
                            }
                            ui.memory_mut(|m| m.close_popup());
                        }
                    }
                });
        },
    );

    add_midi_learn_menu(ui, &resp, &param_id_for(param.name()), state);
}

// ── Delay time section ──────────────────────────────────────────────────

fn draw_delay_section(
    ui: &mut Ui,
    state: &mut EditorState,
    setter: &ParamSetter<'_>,
    s: f32,
    ch: Channel,
) {
    let synced = state.params.tempo_sync.value();

    Frame::NONE
        .fill(INSET_BG)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(corner_radius(4.0 * s))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(6.0 * s);
                ui.label(rich("DELAY", 9.0 * s).color(TEXT_SEC).strong());
                ui.add_space(4.0 * s);

                ui.horizontal_centered(|ui| {
                    draw_delay_scale_button(ui, state, setter, ch, 0.5, ":2", s);
                    ui.add_space(4.0 * s);
                    draw_delay_knob(ui, state, setter, s, ch, synced);
                    ui.add_space(4.0 * s);
                    draw_delay_scale_button(ui, state, setter, ch, 2.0, "x2", s);
                });

                if synced {
                    ui.add_space(5.0 * s);
                    draw_note_popup(ui, state, setter, s, ch);
                    ui.add_space(3.0 * s);
                    draw_deviation_field(ui, state, setter, s, ch);
                }
                ui.add_space(6.0 * s);
            });
        });
}

fn draw_delay_knob(
    ui: &mut Ui,
    state: &mut EditorState,
    setter: &ParamSetter<'_>,
    s: f32,
    ch: Channel,
    synced: bool,
) {
    let params = state.params.clone();
    let delay_param = if ch == Channel::Left {
        &params.delay_time_l
    } else {
        &params.delay_time_r
    };
    let note_param = if ch == Channel::Left {
        &params.note_l
    } else {
        &params.note_r
    };
    let deviation_param = if ch == Channel::Left {
        &params.deviation_l
    } else {
        &params.deviation_r
    };

    let knob_d = KnobSize::Large.diameter(s);
    let ring_pad = if synced { 50.0 * s } else { 4.0 * s };
    let total = knob_d + ring_pad * 2.0;
    let (rect, response) = ui.allocate_exact_size(vec2(total, total), Sense::click_and_drag());
    let knob_rect = Rect::from_center_size(rect.center(), vec2(knob_d, knob_d));
    let normalized = if synced {
        sync_knob_normalized(note_param.value(), deviation_param.value())
    } else {
        delay_param.modulated_normalized_value()
    };
    let mut display_norm = normalized;

    if response.drag_started() {
        state.params.push_undo();
        if synced {
            setter.begin_set_parameter(note_param);
            setter.begin_set_parameter(deviation_param);
            if stereo_link_active(ui, &state.params) {
                let other_note = if ch == Channel::Left {
                    &state.params.note_r
                } else {
                    &state.params.note_l
                };
                let other_dev = if ch == Channel::Left {
                    &state.params.deviation_r
                } else {
                    &state.params.deviation_l
                };
                setter.begin_set_parameter(other_note);
                setter.begin_set_parameter(other_dev);
            }
        } else {
            setter.begin_set_parameter(delay_param);
            if stereo_link_active(ui, &state.params) {
                if let Some(other) = linked_float_counterpart(&state.params, delay_param.name()) {
                    setter.begin_set_parameter(other);
                }
            }
        }
    }

    if response.dragged() {
        let pointer_delta = ui.input(|i| i.pointer.delta().y);
        let delta = -pointer_delta / (knob_d * 1.8);
        let new_norm = (normalized + delta).clamp(0.0, 1.0);
        display_norm = new_norm;
        if synced {
            let link_active = stereo_link_active(ui, &state.params);
            set_sync_from_norm(state, setter, ch, new_norm, false, link_active);
        } else {
            let plain = delay_param.preview_plain(new_norm);
            setter.set_parameter(delay_param, plain);
            if stereo_link_active(ui, &state.params) {
                if let Some(other) = linked_float_counterpart(&state.params, delay_param.name()) {
                    setter.set_parameter(other, other.preview_plain(new_norm));
                }
            }
        }
    }

    if response.drag_stopped() {
        if synced {
            setter.end_set_parameter(note_param);
            setter.end_set_parameter(deviation_param);
            if stereo_link_active(ui, &state.params) {
                let other_note = if ch == Channel::Left {
                    &state.params.note_r
                } else {
                    &state.params.note_l
                };
                let other_dev = if ch == Channel::Left {
                    &state.params.deviation_r
                } else {
                    &state.params.deviation_l
                };
                setter.end_set_parameter(other_note);
                setter.end_set_parameter(other_dev);
            }
        } else {
            setter.end_set_parameter(delay_param);
            if stereo_link_active(ui, &state.params) {
                if let Some(other) = linked_float_counterpart(&state.params, delay_param.name()) {
                    setter.end_set_parameter(other);
                }
            }
        }
    }

    if response.double_clicked() {
        state.params.push_undo();
        if synced {
            let link_active = stereo_link_active(ui, &state.params);
            set_note_deviation(
                state,
                setter,
                ch,
                NoteValueParam::Quarter,
                0.0,
                true,
                link_active,
            );
        } else {
            setter.begin_set_parameter(delay_param);
            setter.set_parameter(delay_param, delay_param.default_plain_value());
            setter.end_set_parameter(delay_param);
            display_norm = delay_param.default_normalized_value();
            if stereo_link_active(ui, &state.params) {
                if let Some(other) = linked_float_counterpart(&state.params, delay_param.name()) {
                    setter.begin_set_parameter(other);
                    setter.set_parameter(other, other.default_plain_value());
                    setter.end_set_parameter(other);
                }
            }
        }
    }

    if ui.is_rect_visible(rect) {
        if synced {
            draw_note_ring(ui, state, setter, ch, rect, s);
        }
        draw_knob_visual(
            &ui.painter_at(knob_rect),
            knob_rect,
            display_norm,
            KnobSize::Large,
            s,
            ACCENT,
        );
        let center = knob_rect.center();
        let value = if synced {
            format!(
                "{}\n{:+.1} ct",
                enum_name(note_param.value()),
                deviation_param.value()
            )
        } else {
            delay_param.to_string()
        };
        ui.painter().text(
            center,
            Align2::CENTER_CENTER,
            value,
            FontId::proportional(10.0 * s),
            TEXT_PRI,
        );
    }

    let response = response.on_hover_text(if synced {
        "Tempo-synced delay: drag to sweep note values and deviation"
    } else {
        "Delay time in milliseconds"
    });
    add_midi_learn_menu(ui, &response, &param_id_for(delay_param.name()), state);
}

fn draw_delay_scale_button(
    ui: &mut Ui,
    state: &mut EditorState,
    setter: &ParamSetter<'_>,
    ch: Channel,
    factor: f32,
    label: &str,
    s: f32,
) {
    let resp = ui.add(
        Button::new(rich(label, 12.0 * s).color(TEXT_PRI).strong())
            .fill(WIDGET_BG)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(corner_radius(3.0 * s))
            .min_size(vec2(32.0 * s, 28.0 * s)),
    );
    if resp.clicked() {
        state.params.push_undo();
        apply_delay_scale(ui, state, setter, ch, factor);
    }
}

fn apply_delay_scale(
    ui: &Ui,
    state: &mut EditorState,
    setter: &ParamSetter<'_>,
    ch: Channel,
    factor: f32,
) {
    let synced = state.params.tempo_sync.value();
    clear_delay_scale_flags(ui, state, setter, ch);
    if synced {
        let note_param = if ch == Channel::Left {
            &state.params.note_l
        } else {
            &state.params.note_r
        };
        let deviation = if ch == Channel::Left {
            state.params.deviation_l.value()
        } else {
            state.params.deviation_r.value()
        };
        let idx = note_index(note_param.value()) as isize;
        let step = if factor < 1.0 { -1 } else { 1 };
        let next_idx = (idx + step).clamp(0, (note_variants().len() - 1) as isize) as usize;
        let link_active = stereo_link_active(ui, &state.params);
        set_note_deviation(
            state,
            setter,
            ch,
            note_variants()[next_idx].0,
            deviation,
            true,
            link_active,
        );
    } else {
        let delay_param = if ch == Channel::Left {
            &state.params.delay_time_l
        } else {
            &state.params.delay_time_r
        };
        let next = (delay_param.value() * factor).clamp(0.005, 2.0);
        setter.begin_set_parameter(delay_param);
        setter.set_parameter(delay_param, next);
        setter.end_set_parameter(delay_param);
        if stereo_link_active(ui, &state.params) {
            if let Some(other) = linked_float_counterpart(&state.params, delay_param.name()) {
                let other_next = (other.value() * factor).clamp(0.005, 2.0);
                setter.begin_set_parameter(other);
                setter.set_parameter(other, other_next);
                setter.end_set_parameter(other);
            }
        }
    }
}

fn clear_delay_scale_flags(
    ui: &Ui,
    state: &mut EditorState,
    setter: &ParamSetter<'_>,
    ch: Channel,
) {
    let params = state.params.clone();
    let clear_one = |param: &nih_plug::params::BoolParam| {
        if param.value() {
            setter.begin_set_parameter(param);
            setter.set_parameter(param, false);
            setter.end_set_parameter(param);
        }
    };

    match ch {
        Channel::Left => {
            clear_one(&params.halve_l);
            clear_one(&params.double_l);
        }
        Channel::Right => {
            clear_one(&params.halve_r);
            clear_one(&params.double_r);
        }
    }

    if stereo_link_active(ui, &state.params) {
        match ch {
            Channel::Left => {
                clear_one(&params.halve_r);
                clear_one(&params.double_r);
            }
            Channel::Right => {
                clear_one(&params.halve_l);
                clear_one(&params.double_l);
            }
        }
    }
}

fn note_variants() -> [(NoteValueParam, &'static str); 17] {
    [
        (NoteValueParam::SixtyFourth, "1/64"),
        (NoteValueParam::ThirtySecondTriplet, "1/32T"),
        (NoteValueParam::ThirtySecond, "1/32"),
        (NoteValueParam::SixteenthTriplet, "1/16T"),
        (NoteValueParam::Sixteenth, "1/16"),
        (NoteValueParam::ThirtySecondDotted, "1/32."),
        (NoteValueParam::EighthTriplet, "1/8T"),
        (NoteValueParam::Eighth, "1/8"),
        (NoteValueParam::SixteenthDotted, "1/16."),
        (NoteValueParam::QuarterTriplet, "1/4T"),
        (NoteValueParam::EighthDotted, "1/8."),
        (NoteValueParam::Quarter, "1/4"),
        (NoteValueParam::HalfTriplet, "1/2T"),
        (NoteValueParam::QuarterDotted, "1/4."),
        (NoteValueParam::Half, "1/2"),
        (NoteValueParam::HalfDotted, "1/2."),
        (NoteValueParam::Whole, "1/1"),
    ]
}

fn note_index(value: NoteValueParam) -> usize {
    note_variants()
        .iter()
        .position(|(variant, _)| *variant == value)
        .unwrap_or(11)
}

fn sync_knob_normalized(note: NoteValueParam, deviation: f32) -> f32 {
    let max = (note_variants().len() - 1) as f32;
    let pos = note_index(note) as f32 + (deviation.clamp(-100.0, 100.0) / 200.0);
    (pos / max).clamp(0.0, 1.0)
}

fn set_sync_from_norm(
    state: &mut EditorState,
    setter: &ParamSetter<'_>,
    ch: Channel,
    normalized: f32,
    end_gesture: bool,
    link_active: bool,
) {
    let max = (note_variants().len() - 1) as f32;
    let pos = normalized.clamp(0.0, 1.0) * max;
    let idx = pos.round().clamp(0.0, max) as usize;
    let deviation = ((pos - idx as f32) * 200.0).clamp(-100.0, 100.0);
    set_note_deviation(
        state,
        setter,
        ch,
        note_variants()[idx].0,
        deviation,
        end_gesture,
        link_active,
    );
}

fn set_note_deviation(
    state: &mut EditorState,
    setter: &ParamSetter<'_>,
    ch: Channel,
    note: NoteValueParam,
    deviation: f32,
    full_gesture: bool,
    link_active: bool,
) {
    let note_param = if ch == Channel::Left {
        &state.params.note_l
    } else {
        &state.params.note_r
    };
    let deviation_param = if ch == Channel::Left {
        &state.params.deviation_l
    } else {
        &state.params.deviation_r
    };
    if full_gesture {
        setter.begin_set_parameter(note_param);
        setter.begin_set_parameter(deviation_param);
        if link_active {
            let other_note = if ch == Channel::Left {
                &state.params.note_r
            } else {
                &state.params.note_l
            };
            let other_dev = if ch == Channel::Left {
                &state.params.deviation_r
            } else {
                &state.params.deviation_l
            };
            setter.begin_set_parameter(other_note);
            setter.begin_set_parameter(other_dev);
        }
    }
    setter.set_parameter(note_param, note);
    setter.set_parameter(deviation_param, deviation.clamp(-100.0, 100.0));
    if link_active {
        let other_note = if ch == Channel::Left {
            &state.params.note_r
        } else {
            &state.params.note_l
        };
        let other_dev = if ch == Channel::Left {
            &state.params.deviation_r
        } else {
            &state.params.deviation_l
        };
        setter.set_parameter(other_note, note);
        setter.set_parameter(other_dev, deviation.clamp(-100.0, 100.0));
    }
    if full_gesture {
        setter.end_set_parameter(note_param);
        setter.end_set_parameter(deviation_param);
        if link_active {
            let other_note = if ch == Channel::Left {
                &state.params.note_r
            } else {
                &state.params.note_l
            };
            let other_dev = if ch == Channel::Left {
                &state.params.deviation_r
            } else {
                &state.params.deviation_l
            };
            setter.end_set_parameter(other_note);
            setter.end_set_parameter(other_dev);
        }
    }
}

fn draw_note_ring(
    ui: &mut Ui,
    state: &mut EditorState,
    setter: &ParamSetter<'_>,
    ch: Channel,
    rect: Rect,
    s: f32,
) {
    let painter = ui.painter();
    let center = rect.center();
    let radius = rect.width() * 0.41;
    let current = if ch == Channel::Left {
        state.params.note_l.value()
    } else {
        state.params.note_r.value()
    };

    let variants = note_variants();
    let denom = (variants.len() - 1) as f32;
    for (idx, (variant, label)) in variants.into_iter().enumerate() {
        let t = idx as f32 / denom;
        let angle = ARC_START + ARC_SWEEP * t;
        let dir = vec2(angle.cos(), angle.sin());
        let tick_pos = center + vec2(radius * dir.x, radius * dir.y);
        let text_pos = center + vec2((radius + 11.0 * s) * dir.x, (radius + 11.0 * s) * dir.y);
        let hit_rect = Rect::from_center_size(tick_pos, vec2(28.0 * s, 24.0 * s));
        let id = ui.id().with((
            "note_ring",
            if ch == Channel::Left { "l" } else { "r" },
            idx,
        ));
        let resp = ui
            .interact(hit_rect, id, Sense::click())
            .on_hover_text(format!("Set note to {label}"));
        let selected = variant == current;
        let color = if selected {
            ACCENT
        } else if resp.hovered() {
            TEXT_PRI
        } else if label.ends_with('.') {
            ORANGE
        } else if label.ends_with('T') {
            PURPLE
        } else {
            TEXT_SEC
        };

        let tick_r = if selected || resp.hovered() {
            3.1 * s
        } else if label.ends_with('.') {
            2.4 * s
        } else {
            1.8 * s
        };
        painter.circle_filled(tick_pos, tick_r, color);

        let show_label = selected
            || resp.hovered()
            || matches!(
                variant,
                NoteValueParam::Whole
                    | NoteValueParam::Half
                    | NoteValueParam::Quarter
                    | NoteValueParam::Eighth
                    | NoteValueParam::Sixteenth
                    | NoteValueParam::ThirtySecond
                    | NoteValueParam::SixtyFourth
            );
        let label_rect = Rect::from_center_size(text_pos, vec2(30.0 * s, 14.0 * s));
        if selected || resp.hovered() {
            painter.rect_filled(label_rect, corner_radius(2.0 * s), ACCENT_DIM);
        }
        if show_label {
            painter.text(
                label_rect.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(7.0 * s),
                color,
            );
        }
        if resp.clicked() {
            state.params.push_undo();
            let link_active = stereo_link_active(ui, &state.params);
            set_note_deviation(state, setter, ch, variant, 0.0, true, link_active);
        }
    }
}

#[allow(dead_code)]
fn draw_note_value_buttons(
    ui: &mut Ui,
    state: &mut EditorState,
    setter: &ParamSetter<'_>,
    s: f32,
    ch: Channel,
) {
    let note_param = if ch == Channel::Left {
        &state.params.note_l
    } else {
        &state.params.note_r
    };
    let deviation_param = if ch == Channel::Left {
        &state.params.deviation_l
    } else {
        &state.params.deviation_r
    };
    let current = note_param.value().to_index();

    egui::Grid::new(if ch == Channel::Left {
        "note_buttons_l"
    } else {
        "note_buttons_r"
    })
    .num_columns(6)
    .spacing(vec2(2.0 * s, 2.0 * s))
    .show(ui, |ui| {
        for (idx, (variant, label)) in note_variants().into_iter().enumerate() {
            let selected = variant.to_index() == current;
            let resp = ui.add(
                Button::new(rich(label, 8.0 * s).color(if selected { TEXT_PRI } else { TEXT_SEC }))
                    .fill(if selected { BTN_ON } else { WIDGET_BG })
                    .stroke(Stroke::new(1.0, if selected { ACCENT } else { BORDER }))
                    .corner_radius(corner_radius(2.0 * s))
                    .min_size(vec2(28.0 * s, 18.0 * s)),
            );
            if resp.clicked() {
                state.params.push_undo();
                setter.begin_set_parameter(note_param);
                setter.set_parameter(note_param, variant);
                setter.end_set_parameter(note_param);
                if stereo_link_active(ui, &state.params) {
                    let other_note = if ch == Channel::Left {
                        &state.params.note_r
                    } else {
                        &state.params.note_l
                    };
                    setter.begin_set_parameter(other_note);
                    setter.set_parameter(other_note, variant);
                    setter.end_set_parameter(other_note);
                }
                setter.begin_set_parameter(deviation_param);
                setter.set_parameter(deviation_param, 0.0);
                setter.end_set_parameter(deviation_param);
                if stereo_link_active(ui, &state.params) {
                    let other_deviation = if ch == Channel::Left {
                        &state.params.deviation_r
                    } else {
                        &state.params.deviation_l
                    };
                    setter.begin_set_parameter(other_deviation);
                    setter.set_parameter(other_deviation, 0.0);
                    setter.end_set_parameter(other_deviation);
                }
            }
            if idx % 6 == 5 {
                ui.end_row();
            }
        }
    });
}

fn draw_note_popup(
    ui: &mut Ui,
    state: &mut EditorState,
    setter: &ParamSetter<'_>,
    s: f32,
    ch: Channel,
) {
    let param = if ch == Channel::Left {
        &state.params.note_l
    } else {
        &state.params.note_r
    };

    let current_name = enum_name(param.value());
    let label = format!("Note: {current_name}");

    let resp = ui.add(
        Button::new(rich(&label, 10.0 * s).color(TEXT_PRI))
            .fill(WIDGET_BG)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(corner_radius(3.0 * s)),
    );

    let popup_id = ui.id().with(if ch == Channel::Left {
        "note_l"
    } else {
        "note_r"
    });
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup_id));
    }

    egui::popup::popup_above_or_below_widget(
        ui,
        popup_id,
        &resp,
        egui::AboveOrBelow::Below,
        egui::popup::PopupCloseBehavior::CloseOnClick,
        |ui| {
            Frame::NONE
                .fill(PANEL_BG)
                .stroke(Stroke::new(1.0, BORDER))
                .show(ui, |ui| {
                    ui.set_max_width(90.0 * s);
                    for (variant, name) in note_variants() {
                        let sel = enum_name(variant) == current_name;
                        let btn = Button::new(rich(name, 10.0 * s).color(if sel {
                            ACCENT
                        } else {
                            TEXT_PRI
                        }))
                        .fill(if sel { ACCENT_DIM } else { PANEL_BG })
                        .corner_radius(corner_radius(2.0));
                        if ui.add(btn).clicked() {
                            state.params.push_undo();
                            setter.begin_set_parameter(param);
                            setter.set_parameter(param, variant);
                            setter.end_set_parameter(param);
                            if stereo_link_active(ui, &state.params) {
                                let other = if ch == Channel::Left {
                                    &state.params.note_r
                                } else {
                                    &state.params.note_l
                                };
                                setter.begin_set_parameter(other);
                                setter.set_parameter(other, variant);
                                setter.end_set_parameter(other);
                            }
                            ui.memory_mut(|m| m.close_popup());
                        }
                    }
                });
        },
    );
}

fn draw_deviation_field(
    ui: &mut Ui,
    state: &mut EditorState,
    setter: &ParamSetter<'_>,
    s: f32,
    ch: Channel,
) {
    let param = if ch == Channel::Left {
        &state.params.deviation_l
    } else {
        &state.params.deviation_r
    };

    let val = param.value();
    let text = format!("{val:.1} ct");

    ui.horizontal(|ui| {
        ui.label(rich("Dev:", 9.0 * s).color(TEXT_SEC));
        let resp = ui.add(
            Button::new(rich(&text, 10.0 * s).color(TEXT_PRI))
                .fill(WIDGET_BG)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(corner_radius(3.0 * s)),
        );

        if resp.drag_started() {
            state.params.push_undo();
            setter.begin_set_parameter(param);
            if stereo_link_active(ui, &state.params) {
                if let Some(other) = linked_float_counterpart(&state.params, param.name()) {
                    setter.begin_set_parameter(other);
                }
            }
        }
        if resp.dragged() {
            let delta = -ui.input(|i| i.pointer.delta().y) * 0.5;
            let new = (param.value() + delta).clamp(-100.0, 100.0);
            setter.set_parameter(param, new);
            if stereo_link_active(ui, &state.params) {
                if let Some(other) = linked_float_counterpart(&state.params, param.name()) {
                    let other_new = (other.value() + delta).clamp(-100.0, 100.0);
                    setter.set_parameter(other, other_new);
                }
            }
        }
        if resp.drag_stopped() {
            setter.end_set_parameter(param);
            if stereo_link_active(ui, &state.params) {
                if let Some(other) = linked_float_counterpart(&state.params, param.name()) {
                    setter.end_set_parameter(other);
                }
            }
        }
        resp.on_hover_text("Deviation in cents (drag to adjust)");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// Center Section
// ═══════════════════════════════════════════════════════════════════════════

fn draw_center_section(
    ui: &mut Ui,
    state: &mut EditorState,
    setter: &ParamSetter<'_>,
    s: f32,
    width: f32,
) {
    Frame::NONE
        .fill(INSET_BG)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(corner_radius(6.0 * s))
        .show(ui, |ui| {
            ui.set_min_width(width);
            ui.set_max_width(width);
            ui.vertical_centered(|ui| {
                ui.add_space(10.0 * s);

                // Routing
                ui.label(rich("ROUTING", 8.0 * s).color(TEXT_SEC));
                draw_routing_popup(ui, state, setter, s);

                ui.add_space(14.0 * s);

                ui.add_space(10.0 * s);
            });
        });
}

fn draw_routing_popup(ui: &mut Ui, state: &mut EditorState, setter: &ParamSetter<'_>, s: f32) {
    let param = &state.params.routing;
    let current_name = enum_name(param.value());

    let resp = ui.add(
        Button::new(rich(current_name, 9.0 * s).color(TEXT_PRI))
            .fill(WIDGET_BG)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(corner_radius(3.0 * s)),
    );

    let popup_id = ui.id().with("routing");
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup_id));
    }

    let modes: [(RoutingModeParam, &str); 8] = [
        (RoutingModeParam::Customized, "Customized"),
        (RoutingModeParam::Straight, "Straight"),
        (RoutingModeParam::Crossfeed, "Crossfeed"),
        (RoutingModeParam::NinetyTen, "90/10"),
        (RoutingModeParam::TenNinety, "10/90"),
        (RoutingModeParam::PingPong, "Ping Pong L/R"),
        (RoutingModeParam::Pan, "Pan L/R"),
        (RoutingModeParam::Rotate, "Rotate L/R"),
    ];

    egui::popup::popup_above_or_below_widget(
        ui,
        popup_id,
        &resp,
        egui::AboveOrBelow::Below,
        egui::popup::PopupCloseBehavior::CloseOnClick,
        |ui| {
            Frame::NONE
                .fill(PANEL_BG)
                .stroke(Stroke::new(1.0, BORDER))
                .show(ui, |ui| {
                    ui.set_max_width(110.0 * s);
                    for (variant, name) in modes {
                        let sel = enum_name(variant) == current_name;
                        let btn = Button::new(rich(name, 9.0 * s).color(if sel {
                            ACCENT
                        } else {
                            TEXT_PRI
                        }))
                        .fill(if sel { ACCENT_DIM } else { PANEL_BG })
                        .corner_radius(corner_radius(2.0));
                        if ui.add(btn).clicked() {
                            state.params.push_undo();
                            setter.begin_set_parameter(param);
                            setter.set_parameter(param, variant);
                            setter.end_set_parameter(param);
                            if state.params.stereo_link.value() {
                                setter.begin_set_parameter(&state.params.stereo_link);
                                setter.set_parameter(&state.params.stereo_link, false);
                                setter.end_set_parameter(&state.params.stereo_link);
                            }
                            ui.memory_mut(|m| m.close_popup());
                        }
                    }
                });
        },
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Bottom Bar
// ═══════════════════════════════════════════════════════════════════════════

fn draw_bottom_bar(ui: &mut Ui, state: &mut EditorState, setter: &ParamSetter<'_>, s: f32) {
    Frame::NONE
        .fill(PANEL_BG)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(corner_radius(4.0 * s))
        .show(ui, |ui| {
            let params = state.params.clone();

            ui.horizontal_centered(|ui| {
                ui.add_space(24.0 * s);
                draw_knob_field(
                    ui,
                    state,
                    setter,
                    s,
                    &params.output_mix_l,
                    "MIX L",
                    KnobSize::Medium,
                );
                ui.add_space(28.0 * s);
                draw_knob_field(
                    ui,
                    state,
                    setter,
                    s,
                    &params.output_mix_r,
                    "MIX R",
                    KnobSize::Medium,
                );
                ui.add_space(24.0 * s);
            });
        });
}

// ═══════════════════════════════════════════════════════════════════════════
// Custom Knob Widget
// ═══════════════════════════════════════════════════════════════════════════

/// Knob size variants.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KnobSize {
    Large,  // main delay time
    Medium, // output mix
    Small,  // filters, feedback, crossfeed
}

impl KnobSize {
    fn diameter(self, s: f32) -> f32 {
        match self {
            Self::Large => 86.0 * s,
            Self::Medium => 54.0 * s,
            Self::Small => 44.0 * s,
        }
    }
    fn track_w(self, s: f32) -> f32 {
        match self {
            Self::Large => 5.0 * s,
            Self::Medium => 3.5 * s,
            Self::Small => 2.8 * s,
        }
    }
    fn font_size(self, s: f32) -> f32 {
        match self {
            Self::Large => 12.0 * s,
            Self::Medium => 10.0 * s,
            Self::Small => 9.0 * s,
        }
    }
}

/// Draw a labelled knob + numeric value field.
fn draw_knob_field(
    ui: &mut Ui,
    state: &mut EditorState,
    setter: &ParamSetter<'_>,
    s: f32,
    param: &nih_plug::params::FloatParam,
    label: &str,
    size: KnobSize,
) {
    let diameter = size.diameter(s);
    let normalized = param.modulated_normalized_value();
    let value_text = param.to_string();

    ui.vertical_centered(|ui| {
        // Label above
        if !label.is_empty() {
            ui.label(rich(label, 8.0 * s).color(TEXT_SEC));
        }

        // ── Knob allocation + interaction ────────────────────
        let (rect, response) =
            ui.allocate_exact_size(vec2(diameter, diameter), Sense::click_and_drag());

        let mut new_norm = normalized;
        let linked_counterpart = linked_float_counterpart(&state.params, param.name());
        let link_active = stereo_link_active(ui, &state.params);

        if response.drag_started() {
            state.params.push_undo();
            setter.begin_set_parameter(param);
            if link_active {
                if let Some(other) = linked_counterpart {
                    setter.begin_set_parameter(other);
                }
            }
        }
        if response.dragged() {
            let speed = 1.0 / (diameter * 2.5);
            let delta = -ui.input(|i| i.pointer.delta().y) * speed;
            new_norm = (normalized + delta).clamp(0.0, 1.0);

            // Snap through preview_plain → preview_normalized so stepped
            // parameters land on exact values.
            let plain = param.preview_plain(new_norm);
            setter.set_parameter(param, plain);
            if link_active {
                if let Some(other) = linked_counterpart {
                    let other_norm = (other.modulated_normalized_value() + (new_norm - normalized))
                        .clamp(0.0, 1.0);
                    setter.set_parameter(other, other.preview_plain(other_norm));
                }
            }
        }
        if response.drag_stopped() {
            setter.end_set_parameter(param);
            if link_active {
                if let Some(other) = linked_counterpart {
                    setter.end_set_parameter(other);
                }
            }
        }

        // Double-click → reset to default
        if response.double_clicked() {
            state.params.push_undo();
            setter.begin_set_parameter(param);
            setter.set_parameter(param, param.default_plain_value());
            setter.end_set_parameter(param);
            if link_active {
                if let Some(other) = linked_counterpart {
                    setter.begin_set_parameter(other);
                    setter.set_parameter(other, other.default_plain_value());
                    setter.end_set_parameter(other);
                }
            }
        }

        // Ctrl/Cmd-click → also reset
        if response.clicked() && ui.input(|i| i.modifiers.command || i.modifiers.ctrl) {
            state.params.push_undo();
            setter.begin_set_parameter(param);
            setter.set_parameter(param, param.default_plain_value());
            setter.end_set_parameter(param);
            if link_active {
                if let Some(other) = linked_counterpart {
                    setter.begin_set_parameter(other);
                    setter.set_parameter(other, other.default_plain_value());
                    setter.end_set_parameter(other);
                }
            }
        }

        // ── Render ───────────────────────────────────────────
        if ui.is_rect_visible(rect) {
            draw_knob_visual(
                &ui.painter_at(rect),
                rect,
                new_norm,
                size,
                s,
                knob_accent(label, param.name()),
            );
        }

        // ── Tooltip ──────────────────────────────────────────
        let response = response.on_hover_text(format!("{}: {}", param.name(), param));

        // ── MIDI Learn ───────────────────────────────────────
        add_midi_learn_menu(ui, &response, &param_id_for(param.name()), state);

        // ── Numeric field ────────────────────────────────────
        let field_w = match size {
            KnobSize::Large => diameter * 0.82,
            KnobSize::Medium => (diameter * 1.22).max(70.0 * s),
            KnobSize::Small => (diameter * 1.55).max(72.0 * s),
        };
        let field_resp = ui.add(
            Button::new(rich(&value_text, size.font_size(s)).color(TEXT_PRI))
                .fill(WIDGET_BG)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(corner_radius(3.0 * s))
                .min_size(vec2(field_w, 16.0 * s)),
        );
        field_resp.on_hover_text(format!("{} (drag knob to adjust)", param.name()));
    });
}

/// Render the knob: body circle, background arc, filled arc, indicator line, center dot.
fn draw_knob_visual(
    painter: &Painter,
    rect: Rect,
    normalized: f32,
    size: KnobSize,
    s: f32,
    accent: Color32,
) {
    let center = rect.center();
    let radius = rect.width() / 2.0;
    let tw = size.track_w(s);
    let body_r = radius - tw;

    // ── Body ─────────────────────────────────────────────
    painter.circle_filled(
        center + vec2(1.5 * s, 2.0 * s),
        body_r,
        Color32::from_black_alpha(90),
    );
    painter.circle_filled(center, body_r, Color32::from_rgb(0x16, 0x13, 0x2F));
    painter.circle_stroke(center, body_r, Stroke::new(1.0 * s, BORDER));
    painter.circle_stroke(
        center,
        body_r * 0.72,
        Stroke::new(0.8 * s, Color32::from_rgb(0x2B, 0x22, 0x5E)),
    );

    // ── Background arc (full sweep, dim) ─────────────────
    draw_arc_line(
        painter,
        center,
        radius,
        ARC_START,
        ARC_END,
        Stroke::new(tw, KNOB_TRACK),
    );

    // ── Filled arc ───────────────────────────────────────
    let norm = normalized.clamp(0.0, 1.0);
    if norm > 0.001 {
        let fill_end = ARC_START + ARC_SWEEP * norm;
        draw_arc_line(
            painter,
            center,
            radius,
            ARC_START,
            fill_end,
            Stroke::new(tw, accent),
        );
    }

    // ── Value indicator line ─────────────────────────────
    let angle = ARC_START + ARC_SWEEP * norm;
    let inner = radius * 0.32;
    let outer = body_r - 1.5 * s;
    let p1 = center + vec2(inner * angle.cos(), inner * angle.sin());
    let p2 = center + vec2(outer * angle.cos(), outer * angle.sin());
    painter.line_segment([p1, p2], Stroke::new(2.2 * s, TEXT_PRI));
    painter.circle_filled(p2, 2.8 * s, accent);

    // ── Centre dot ───────────────────────────────────────
    painter.circle_filled(center, 2.5 * s, Color32::from_rgb(0xCF, 0xD6, 0xFF));
}

fn knob_accent(label: &str, name: &str) -> Color32 {
    if label.contains("LP")
        || label.contains("HP")
        || name.contains("Cut")
        || name.contains("Slope")
    {
        ORANGE
    } else if label.contains("FEED") || label.contains("MIX") || name.contains("Feedback") {
        MAGENTA
    } else if label.contains("R") && label.contains("L") {
        PURPLE
    } else {
        ACCENT
    }
}

/// Draw a circular arc as a polyline with the given stroke.
fn draw_arc_line(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    start: f32,
    end: f32,
    stroke: Stroke,
) {
    let span = end - start;
    if span.abs() < 0.001 {
        return;
    }
    let segments = (span.abs() * radius / 2.0).ceil() as u32;
    let segments = segments.clamp(12, 256);
    let step = span / segments as f32;

    let pts: Vec<Pos2> = (0..=segments)
        .map(|i| {
            let a = start + step * i as f32;
            center + vec2(radius * a.cos(), radius * a.sin())
        })
        .collect();

    if pts.len() >= 2 {
        painter.add(Shape::line(pts, stroke));
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase Toggle Button
// ═══════════════════════════════════════════════════════════════════════════

fn draw_phase_btn(
    ui: &mut Ui,
    state: &mut EditorState,
    setter: &ParamSetter<'_>,
    param: &nih_plug::params::BoolParam,
    s: f32,
    label: &str,
) {
    let inverted = param.value();
    let display = format!("{label}\n{}", if inverted { "\u{00d8}" } else { "0\u{b0}" });

    let resp = ui.add(
        Button::new(rich(&display, 8.0 * s).color(if inverted { TEXT_PRI } else { TEXT_SEC }))
            .fill(if inverted { BTN_ON } else { WIDGET_BG })
            .stroke(Stroke::new(1.0, if inverted { ACCENT } else { BORDER }))
            .corner_radius(corner_radius(3.0 * s))
            .min_size(vec2(48.0 * s, 42.0 * s)),
    );
    if resp.clicked() {
        let link_active = stereo_link_active(ui, &state.params);
        let linked_counterpart = linked_bool_counterpart(&state.params, param.name());
        state.params.push_undo();
        setter.begin_set_parameter(param);
        setter.set_parameter(param, !inverted);
        setter.end_set_parameter(param);
        if link_active {
            if let Some(other) = linked_counterpart {
                setter.begin_set_parameter(other);
                setter.set_parameter(other, !inverted);
                setter.end_set_parameter(other);
            }
        }
    }
    let resp = resp.on_hover_text(if inverted {
        format!("{label}: Inverted \u{2014} click for Normal")
    } else {
        format!("{label}: Normal \u{2014} click for Inverted")
    });
    add_midi_learn_menu(ui, &resp, &param_id_for(param.name()), state);
}

// ═══════════════════════════════════════════════════════════════════════════
// Bool toggle button (used for :2 / x2)
// ═══════════════════════════════════════════════════════════════════════════

#[allow(dead_code)]
fn draw_toggle_btn(
    ui: &mut Ui,
    state: &mut EditorState,
    setter: &ParamSetter<'_>,
    param: &nih_plug::params::BoolParam,
    label: &str,
    s: f32,
) {
    let on = param.value();
    let resp = ui.add(
        Button::new(
            rich(label, 10.0 * s)
                .color(if on { TEXT_PRI } else { TEXT_SEC })
                .strong(),
        )
        .fill(if on { BTN_ON } else { WIDGET_BG })
        .stroke(Stroke::new(1.0, if on { ACCENT } else { BORDER }))
        .corner_radius(corner_radius(3.0 * s)),
    );
    if resp.clicked() {
        let link_active = stereo_link_active(ui, &state.params);
        let linked_counterpart = linked_bool_counterpart(&state.params, param.name());
        state.params.push_undo();
        setter.begin_set_parameter(param);
        setter.set_parameter(param, !on);
        setter.end_set_parameter(param);
        if link_active {
            if let Some(other) = linked_counterpart {
                setter.begin_set_parameter(other);
                setter.set_parameter(other, !on);
                setter.end_set_parameter(other);
            }
        }
    }
    add_midi_learn_menu(ui, &resp, &param_id_for(param.name()), state);
}

// ═══════════════════════════════════════════════════════════════════════════
// MIDI Learn Right-Click Menu
// ═══════════════════════════════════════════════════════════════════════════

/// Attach a right-click MIDI Learn context menu to a widget response.
fn add_midi_learn_menu(_ui: &mut Ui, response: &Response, param_id: &str, state: &mut EditorState) {
    if state.midi_learn_active && response.clicked() {
        if let Ok(mut ml) = state.params.midi_learn.write() {
            ml.start_learn(param_id);
            state.midi_learn_target = Some(param_id.to_string());
            state.midi_learn_active = false;
        }
    }

    response.context_menu(|ui| {
        ui.label(rich("MIDI Learn", 11.0).color(ACCENT).strong());
        ui.separator();

        // On / Off toggle
        let is_active = state.midi_learn_active;
        if ui
            .button(if is_active {
                "MIDI Learn: ON"
            } else {
                "MIDI Learn: OFF"
            })
            .clicked()
        {
            state.midi_learn_active = !is_active;
            state.midi_learn_target = if !is_active {
                Some(param_id.to_string())
            } else {
                None
            };
            ui.close_menu();
        }

        // Clean Up
        if ui.button("Clean Up").clicked() {
            if let Ok(mut ml) = state.params.midi_learn.write() {
                ml.clean_up(param_id);
            }
            ui.close_menu();
        }

        // Roll Back
        if ui.button("Roll Back").clicked() {
            if let Ok(mut ml) = state.params.midi_learn.write() {
                ml.roll_back();
            }
            ui.close_menu();
        }

        // Save
        if ui.button("Save").clicked() {
            if let Ok(mut ml) = state.params.midi_learn.write() {
                ml.save_for_rollback();
            }
            ui.close_menu();
        }

        ui.separator();

        // Current mapping info
        if let Ok(ml) = state.params.midi_learn.read() {
            if let Some(mapping) = ml.get_mapping(param_id) {
                ui.label(
                    rich(
                        format!("Mapped: CC {} Ch {}", mapping.cc, mapping.channel),
                        9.0,
                    )
                    .color(TEXT_SEC),
                );
            } else {
                ui.label(rich("No mapping", 9.0).color(TEXT_SEC));
            }
        }
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// Snapshot capture & apply
// ═══════════════════════════════════════════════════════════════════════════

/// Capture the current parameter state as a `ParamSnapshot`.
fn take_snapshot(params: &NebulaStereoDelayParams) -> ParamSnapshot {
    ParamSnapshot {
        input_mode_l: input_mode_to_index(params.input_mode_l.value()),
        input_mode_r: input_mode_to_index(params.input_mode_r.value()),
        delay_time_l: params.delay_time_l.value(),
        delay_time_r: params.delay_time_r.value(),
        note_l: note_to_index(params.note_l.value()),
        note_r: note_to_index(params.note_r.value()),
        deviation_l: params.deviation_l.value(),
        deviation_r: params.deviation_r.value(),
        halve_l: params.halve_l.value(),
        halve_r: params.halve_r.value(),
        double_l: params.double_l.value(),
        double_r: params.double_r.value(),
        low_cut_l: params.low_cut_l.value(),
        low_cut_r: params.low_cut_r.value(),
        low_cut_slope_l: params.low_cut_slope_l.value(),
        low_cut_slope_r: params.low_cut_slope_r.value(),
        high_cut_l: params.high_cut_l.value(),
        high_cut_r: params.high_cut_r.value(),
        high_cut_slope_l: params.high_cut_slope_l.value(),
        high_cut_slope_r: params.high_cut_slope_r.value(),
        feedback_l: params.feedback_l.value(),
        feedback_r: params.feedback_r.value(),
        feedback_phase_l: params.feedback_phase_l.value(),
        feedback_phase_r: params.feedback_phase_r.value(),
        crossfeed_lr: params.crossfeed_lr.value(),
        crossfeed_rl: params.crossfeed_rl.value(),
        crossfeed_phase_lr: params.crossfeed_phase_lr.value(),
        crossfeed_phase_rl: params.crossfeed_phase_rl.value(),
        routing: routing_to_index(params.routing.value()),
        tempo_sync: params.tempo_sync.value(),
        stereo_link: params.stereo_link.value(),
        output_mix_l: params.output_mix_l.value(),
        output_mix_r: params.output_mix_r.value(),
    }
}

/// Apply a `ParamSnapshot` to the live parameters, notifying the host.
fn apply_snapshot(
    params: &NebulaStereoDelayParams,
    setter: &ParamSetter<'_>,
    snap: &ParamSnapshot,
) {
    // FloatParams — plain value matches snapshot value directly.
    macro_rules! set_f {
        ($param:expr, $val:expr) => {
            setter.set_parameter($param, $val)
        };
    }

    // BoolParams
    macro_rules! set_b {
        ($param:expr, $val:expr) => {
            setter.set_parameter($param, $val)
        };
    }

    // EnumParams — convert stored usize index to the enum variant.
    macro_rules! set_input {
        ($param:expr, $idx:expr) => {
            setter.set_parameter($param, input_mode_from_index($idx))
        };
    }
    macro_rules! set_note {
        ($param:expr, $idx:expr) => {
            setter.set_parameter($param, note_from_index($idx))
        };
    }
    macro_rules! set_routing {
        ($param:expr, $idx:expr) => {
            setter.set_parameter($param, routing_from_index($idx))
        };
    }

    set_input!(&params.input_mode_l, snap.input_mode_l);
    set_input!(&params.input_mode_r, snap.input_mode_r);
    set_f!(&params.delay_time_l, snap.delay_time_l);
    set_f!(&params.delay_time_r, snap.delay_time_r);
    set_note!(&params.note_l, snap.note_l);
    set_note!(&params.note_r, snap.note_r);
    set_f!(&params.deviation_l, snap.deviation_l);
    set_f!(&params.deviation_r, snap.deviation_r);
    set_b!(&params.halve_l, snap.halve_l);
    set_b!(&params.halve_r, snap.halve_r);
    set_b!(&params.double_l, snap.double_l);
    set_b!(&params.double_r, snap.double_r);
    set_f!(&params.low_cut_l, snap.low_cut_l);
    set_f!(&params.low_cut_r, snap.low_cut_r);
    set_f!(&params.low_cut_slope_l, snap.low_cut_slope_l);
    set_f!(&params.low_cut_slope_r, snap.low_cut_slope_r);
    set_f!(&params.high_cut_l, snap.high_cut_l);
    set_f!(&params.high_cut_r, snap.high_cut_r);
    set_f!(&params.high_cut_slope_l, snap.high_cut_slope_l);
    set_f!(&params.high_cut_slope_r, snap.high_cut_slope_r);
    set_f!(&params.feedback_l, snap.feedback_l);
    set_f!(&params.feedback_r, snap.feedback_r);
    set_b!(&params.feedback_phase_l, snap.feedback_phase_l);
    set_b!(&params.feedback_phase_r, snap.feedback_phase_r);
    set_f!(&params.crossfeed_lr, snap.crossfeed_lr);
    set_f!(&params.crossfeed_rl, snap.crossfeed_rl);
    set_b!(&params.crossfeed_phase_lr, snap.crossfeed_phase_lr);
    set_b!(&params.crossfeed_phase_rl, snap.crossfeed_phase_rl);
    set_routing!(&params.routing, snap.routing);
    set_b!(&params.tempo_sync, snap.tempo_sync);
    set_b!(&params.stereo_link, snap.stereo_link);
    set_f!(&params.output_mix_l, snap.output_mix_l);
    set_f!(&params.output_mix_r, snap.output_mix_r);
}

fn preset_values_from_params(params: &NebulaStereoDelayParams) -> PresetValues {
    preset_values_from_snapshot(&take_snapshot(params))
}

fn preset_values_from_snapshot(snap: &ParamSnapshot) -> PresetValues {
    PresetValues {
        input_mode_l: snap.input_mode_l as u8,
        input_mode_r: snap.input_mode_r as u8,
        delay_time_l: snap.delay_time_l,
        delay_time_r: snap.delay_time_r,
        note_l: snap.note_l as u8,
        note_r: snap.note_r as u8,
        deviation_l: snap.deviation_l,
        deviation_r: snap.deviation_r,
        halve_l: snap.halve_l,
        halve_r: snap.halve_r,
        double_l: snap.double_l,
        double_r: snap.double_r,
        low_cut_l: snap.low_cut_l,
        low_cut_r: snap.low_cut_r,
        low_cut_slope_l: snap.low_cut_slope_l,
        low_cut_slope_r: snap.low_cut_slope_r,
        high_cut_l: snap.high_cut_l,
        high_cut_r: snap.high_cut_r,
        high_cut_slope_l: snap.high_cut_slope_l,
        high_cut_slope_r: snap.high_cut_slope_r,
        feedback_l: snap.feedback_l,
        feedback_r: snap.feedback_r,
        feedback_phase_l: snap.feedback_phase_l,
        feedback_phase_r: snap.feedback_phase_r,
        crossfeed_lr: snap.crossfeed_lr,
        crossfeed_rl: snap.crossfeed_rl,
        crossfeed_phase_lr: snap.crossfeed_phase_lr,
        crossfeed_phase_rl: snap.crossfeed_phase_rl,
        routing: snap.routing as u8,
        tempo_sync: snap.tempo_sync,
        stereo_link: snap.stereo_link,
        output_mix_l: snap.output_mix_l,
        output_mix_r: snap.output_mix_r,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Enum ↔ index conversion helpers
// ═══════════════════════════════════════════════════════════════════════════

fn input_mode_from_index(idx: usize) -> InputModeParam {
    match idx {
        0 => InputModeParam::Off,
        1 => InputModeParam::Left,
        2 => InputModeParam::Right,
        3 => InputModeParam::LeftPlusRight,
        4 => InputModeParam::LeftMinusRight,
        _ => InputModeParam::Left,
    }
}

fn input_mode_to_index(val: InputModeParam) -> usize {
    match val {
        InputModeParam::Off => 0,
        InputModeParam::Left => 1,
        InputModeParam::Right => 2,
        InputModeParam::LeftPlusRight => 3,
        InputModeParam::LeftMinusRight => 4,
    }
}

fn note_from_index(idx: usize) -> NoteValueParam {
    match idx {
        0 => NoteValueParam::Whole,
        1 => NoteValueParam::Half,
        2 => NoteValueParam::HalfTriplet,
        3 => NoteValueParam::Quarter,
        4 => NoteValueParam::QuarterTriplet,
        5 => NoteValueParam::Eighth,
        6 => NoteValueParam::EighthTriplet,
        7 => NoteValueParam::Sixteenth,
        8 => NoteValueParam::SixteenthTriplet,
        9 => NoteValueParam::ThirtySecond,
        10 => NoteValueParam::ThirtySecondTriplet,
        11 => NoteValueParam::SixtyFourth,
        12 => NoteValueParam::HalfDotted,
        13 => NoteValueParam::QuarterDotted,
        14 => NoteValueParam::EighthDotted,
        15 => NoteValueParam::SixteenthDotted,
        16 => NoteValueParam::ThirtySecondDotted,
        _ => NoteValueParam::Quarter,
    }
}

fn note_to_index(val: NoteValueParam) -> usize {
    match val {
        NoteValueParam::Whole => 0,
        NoteValueParam::Half => 1,
        NoteValueParam::HalfTriplet => 2,
        NoteValueParam::Quarter => 3,
        NoteValueParam::QuarterTriplet => 4,
        NoteValueParam::Eighth => 5,
        NoteValueParam::EighthTriplet => 6,
        NoteValueParam::Sixteenth => 7,
        NoteValueParam::SixteenthTriplet => 8,
        NoteValueParam::ThirtySecond => 9,
        NoteValueParam::ThirtySecondTriplet => 10,
        NoteValueParam::SixtyFourth => 11,
        NoteValueParam::HalfDotted => 12,
        NoteValueParam::QuarterDotted => 13,
        NoteValueParam::EighthDotted => 14,
        NoteValueParam::SixteenthDotted => 15,
        NoteValueParam::ThirtySecondDotted => 16,
    }
}

fn routing_from_index(idx: usize) -> RoutingModeParam {
    match idx {
        0 => RoutingModeParam::Customized,
        1 => RoutingModeParam::Straight,
        2 => RoutingModeParam::Crossfeed,
        3 => RoutingModeParam::NinetyTen,
        4 => RoutingModeParam::TenNinety,
        5 => RoutingModeParam::PingPong,
        6 => RoutingModeParam::Pan,
        7 => RoutingModeParam::Rotate,
        _ => RoutingModeParam::Customized,
    }
}

fn routing_to_index(val: RoutingModeParam) -> usize {
    match val {
        RoutingModeParam::Customized => 0,
        RoutingModeParam::Straight => 1,
        RoutingModeParam::Crossfeed => 2,
        RoutingModeParam::NinetyTen => 3,
        RoutingModeParam::TenNinety => 4,
        RoutingModeParam::PingPong => 5,
        RoutingModeParam::Pan => 6,
        RoutingModeParam::Rotate => 7,
    }
}

fn stereo_link_active(ui: &Ui, params: &NebulaStereoDelayParams) -> bool {
    let flip = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
    params.stereo_link.value() ^ flip
}

fn linked_float_counterpart<'a>(
    params: &'a NebulaStereoDelayParams,
    name: &str,
) -> Option<&'a nih_plug::params::FloatParam> {
    match name {
        "Delay Time L" => Some(&params.delay_time_r),
        "Delay Time R" => Some(&params.delay_time_l),
        "Deviation L" => Some(&params.deviation_r),
        "Deviation R" => Some(&params.deviation_l),
        "Low Cut L" => Some(&params.low_cut_r),
        "Low Cut R" => Some(&params.low_cut_l),
        "Low Cut Slope L" => Some(&params.low_cut_slope_r),
        "Low Cut Slope R" => Some(&params.low_cut_slope_l),
        "High Cut L" => Some(&params.high_cut_r),
        "High Cut R" => Some(&params.high_cut_l),
        "High Cut Slope L" => Some(&params.high_cut_slope_r),
        "High Cut Slope R" => Some(&params.high_cut_slope_l),
        "Feedback L" => Some(&params.feedback_r),
        "Feedback R" => Some(&params.feedback_l),
        "Crossfeed L-R" => Some(&params.crossfeed_rl),
        "Crossfeed R-L" => Some(&params.crossfeed_lr),
        "Output Mix L" => Some(&params.output_mix_r),
        "Output Mix R" => Some(&params.output_mix_l),
        _ => None,
    }
}

fn linked_bool_counterpart<'a>(
    params: &'a NebulaStereoDelayParams,
    name: &str,
) -> Option<&'a nih_plug::params::BoolParam> {
    match name {
        "Halve L" => Some(&params.halve_r),
        "Halve R" => Some(&params.halve_l),
        "Double L" => Some(&params.double_r),
        "Double R" => Some(&params.double_l),
        "Feedback Phase L" => Some(&params.feedback_phase_r),
        "Feedback Phase R" => Some(&params.feedback_phase_l),
        "Crossfeed Phase L-R" => Some(&params.crossfeed_phase_rl),
        "Crossfeed Phase R-L" => Some(&params.crossfeed_phase_lr),
        _ => None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Shorthand to create a `RichText` with the given content and size.
fn rich(text: impl ToString, size: f32) -> egui::RichText {
    egui::RichText::new(text.to_string()).size(size)
}

/// Derive a stable parameter ID string from the parameter's display name.
fn param_id_for(name: &str) -> String {
    name.to_lowercase().replace([' ', '-', '/', '(', ')'], "_")
}

fn enum_name<T: Enum>(value: T) -> &'static str {
    T::variants()
        .get(value.to_index())
        .copied()
        .unwrap_or_default()
}

fn corner_radius(radius: f32) -> CornerRadius {
    CornerRadius::same(radius.round().clamp(0.0, 255.0) as u8)
}
