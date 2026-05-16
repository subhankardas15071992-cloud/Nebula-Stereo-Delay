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
    self, vec2, Button, Color32, Context, CornerRadius, Frame, Painter, Pos2, Rect, Response,
    Sense, Shape, Stroke, Ui,
};
use nih_plug_egui::EguiState;

use crate::parameters::{
    InputModeParam, NebulaStereoDelayParams, NoteValueParam, ParamSnapshot, RoutingModeParam,
};

// ═══════════════════════════════════════════════════════════════════════════
// Theme constants
// ═══════════════════════════════════════════════════════════════════════════

/// Main background — very dark gray.
const BG: Color32 = Color32::from_rgb(0x1A, 0x1A, 0x1A);
/// Panel / section background — slightly lighter.
const PANEL_BG: Color32 = Color32::from_rgb(0x22, 0x22, 0x22);
/// Inset / recessed area background.
const INSET_BG: Color32 = Color32::from_rgb(0x1E, 0x1E, 0x1E);
/// Widget surface (knob body, button face).
const WIDGET_BG: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x2A);
/// Accent colour — cyan / teal.
const ACCENT: Color32 = Color32::from_rgb(0x00, 0xCC, 0xCC);
/// Accent at reduced opacity for highlight backgrounds.
const ACCENT_DIM: Color32 = Color32::from_rgba_premultiplied(0x00, 0x80, 0x80, 0x55);
/// Primary text — white.
const TEXT_PRI: Color32 = Color32::from_rgb(0xEE, 0xEE, 0xEE);
/// Secondary / label text — mid gray.
const TEXT_SEC: Color32 = Color32::from_rgb(0x99, 0x99, 0x99);
/// Knob track (unfilled arc).
const KNOB_TRACK: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3A);
/// Active / "on" button fill.
const BTN_ON: Color32 = ACCENT;
/// Inactive / "off" button fill.
const BTN_OFF: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);
/// Bypass / danger indicator.
const DANGER: Color32 = Color32::from_rgb(0xCC, 0x33, 0x33);
/// Border / separator.
const BORDER: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3A);

/// Arc start angle: −135° (7-o'clock position).
const ARC_START: f32 = -3.0 * std::f32::consts::FRAC_PI_4;
/// Arc end angle: +135° (5-o'clock position).
const ARC_END: f32 = 3.0 * std::f32::consts::FRAC_PI_4;
/// Total sweep = 270°.
const ARC_SWEEP: f32 = ARC_END - ARC_START;

/// Default window width in logical pixels.
const WIN_W: u32 = 1200;
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
}

// ═══════════════════════════════════════════════════════════════════════════
// Public entry point
// ═══════════════════════════════════════════════════════════════════════════

/// Create the egui editor for the Nebula Stereo Delay plugin.
///
/// Returns `Option<Box<dyn Editor>>` suitable for use in the plugin's
/// `editor()` method. The window starts at 1200 × 700 logical pixels and
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
                    draw_root(ui, state, setter);
                });
        },
    )
}

/// Draw the entire plugin UI into the given `Ui`.
fn draw_root(ui: &mut Ui, state: &mut EditorState, setter: &ParamSetter<'_>) {
    // Derive a proportional scale factor from the available width.
    // Reference width = 1200 logical px; all sizes are multiplied by `s`.
    let s = (ui.available_width() / 1200.0).clamp(0.4, 2.5);

    // Full-background fill.
    ui.painter().rect_filled(ui.max_rect(), 0.0, BG);

    ui.vertical(|ui| {
        // ── Top Bar ─────────────────────────────────────────
        draw_top_bar(ui, state, setter, s);

        ui.add_space(6.0 * s);

        // ── Main content row: Left | Center | Right ─────────
        ui.horizontal(|ui| {
            let center_w = 88.0 * s;
            let side_w = (ui.available_width() - center_w) / 2.0;

            draw_channel_panel(ui, state, setter, s, Channel::Left, side_w);
            draw_center_section(ui, state, setter, s, center_w);
            draw_channel_panel(ui, state, setter, s, Channel::Right, side_w);
        });

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
            ui.set_min_height(36.0 * s);
            ui.horizontal_centered(|ui| {
                ui.add_space(12.0 * s);

                // Plugin name
                ui.label(rich("NEBULA STEREO DELAY", 14.0 * s).color(ACCENT).strong());
                ui.label(rich("v1.0", 9.0 * s).color(TEXT_SEC));
                ui.add_space(14.0 * s);

                // Preset button
                draw_preset_button(ui, s);
                ui.add_space(6.0 * s);

                // A/B toggle
                draw_ab_button(ui, state, setter, s);
                ui.add_space(4.0 * s);

                // Undo / Redo
                draw_undo_btn(ui, state, setter, s);
                ui.add_space(2.0 * s);
                draw_redo_btn(ui, state, setter, s);

                ui.add_space(10.0 * s);

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

fn draw_preset_button(ui: &mut Ui, s: f32) {
    let resp = ui.add(
        Button::new(rich("Preset", 11.0 * s).color(TEXT_PRI))
            .fill(WIDGET_BG)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(corner_radius(3.0 * s)),
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
        // Save current state to the old slot, then load the new slot.
        let new_slot = if slot == 0 { 1u8 } else { 0u8 };
        state.params.ab_state.store(new_slot, Ordering::Relaxed);
        if let Ok(snapshots) = state.params.ab_snapshots.read() {
            let snapshot = if new_slot == 0 {
                &snapshots.a
            } else {
                &snapshots.b
            };
            apply_snapshot(&state.params, setter, snapshot);
        }
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
                        "LOW CUT",
                        KnobSize::Small,
                    );
                    draw_knob_field(
                        ui,
                        state,
                        setter,
                        s,
                        ch_knob_param!(params, ch, high_cut_l, high_cut_r),
                        "HIGH CUT",
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
                    let phase_param = if ch == Channel::Left {
                        &state.params.feedback_phase_l
                    } else {
                        &state.params.feedback_phase_r
                    };
                    draw_phase_btn(ui, setter, phase_param, s, "FB \u{00d8}");
                });
                ui.add_space(6.0 * s);

                // Crossfeed
                let cf_label = if ch == Channel::Left {
                    "L\u{2192}R"
                } else {
                    "R\u{2192}L"
                };
                draw_knob_field(
                    ui,
                    state,
                    setter,
                    s,
                    ch_knob_param!(params, ch, crossfeed_lr, crossfeed_rl),
                    cf_label,
                    KnobSize::Small,
                );

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
                            setter.begin_set_parameter(param);
                            setter.set_parameter(param, variant);
                            setter.end_set_parameter(param);
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
            let params = state.params.clone();

            ui.vertical_centered(|ui| {
                ui.add_space(4.0 * s);
                ui.label(rich("DELAY TIME", 9.0 * s).color(TEXT_SEC));
                ui.add_space(2.0 * s);

                // Large delay-time knob
                let delay_param = ch_knob_param!(params, ch, delay_time_l, delay_time_r);
                draw_knob_field(ui, state, setter, s, delay_param, "", KnobSize::Large);

                // :2  x2 buttons
                ui.horizontal(|ui| {
                    let halve = ch_knob_param!(params, ch, halve_l, halve_r);
                    let double = ch_knob_param!(params, ch, double_l, double_r);

                    draw_toggle_btn(ui, setter, halve, ":2", s);
                    ui.add_space(4.0 * s);
                    draw_toggle_btn(ui, setter, double, "x2", s);
                });

                // Sync-only controls
                if synced {
                    ui.add_space(4.0 * s);
                    draw_note_popup(ui, state, setter, s, ch);
                    ui.add_space(4.0 * s);
                    draw_deviation_field(ui, state, setter, s, ch);
                }
            });
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

    let notes: [(NoteValueParam, &str); 12] = [
        (NoteValueParam::Whole, "1/1"),
        (NoteValueParam::Half, "1/2"),
        (NoteValueParam::HalfTriplet, "1/2T"),
        (NoteValueParam::Quarter, "1/4"),
        (NoteValueParam::QuarterTriplet, "1/4T"),
        (NoteValueParam::Eighth, "1/8"),
        (NoteValueParam::EighthTriplet, "1/8T"),
        (NoteValueParam::Sixteenth, "1/16"),
        (NoteValueParam::SixteenthTriplet, "1/16T"),
        (NoteValueParam::ThirtySecond, "1/32"),
        (NoteValueParam::ThirtySecondTriplet, "1/32T"),
        (NoteValueParam::SixtyFourth, "1/64"),
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
                    ui.set_max_width(90.0 * s);
                    for (variant, name) in notes {
                        let sel = enum_name(variant) == current_name;
                        let btn = Button::new(rich(name, 10.0 * s).color(if sel {
                            ACCENT
                        } else {
                            TEXT_PRI
                        }))
                        .fill(if sel { ACCENT_DIM } else { PANEL_BG })
                        .corner_radius(corner_radius(2.0));
                        if ui.add(btn).clicked() {
                            setter.begin_set_parameter(param);
                            setter.set_parameter(param, variant);
                            setter.end_set_parameter(param);
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
            setter.begin_set_parameter(param);
        }
        if resp.dragged() {
            let delta = -resp.drag_delta().y * 0.5;
            let new = (val + delta).clamp(-100.0, 100.0);
            setter.set_parameter(param, new);
        }
        if resp.drag_stopped() {
            setter.end_set_parameter(param);
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

                // Crossfeed phase
                draw_phase_btn(ui, setter, &state.params.crossfeed_phase, s, "CF \u{00d8}");

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
        (RoutingModeParam::PingPong, "Ping Pong"),
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
                            setter.begin_set_parameter(param);
                            setter.set_parameter(param, variant);
                            setter.end_set_parameter(param);
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
            Self::Large => 74.0 * s,
            Self::Medium => 54.0 * s,
            Self::Small => 44.0 * s,
        }
    }
    fn track_w(self, s: f32) -> f32 {
        match self {
            Self::Large => 4.5 * s,
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

        if response.drag_started() {
            setter.begin_set_parameter(param);
        }
        if response.dragged() {
            let speed = 1.0 / (diameter * 2.5);
            let delta = -response.drag_delta().y * speed;
            new_norm = (normalized + delta).clamp(0.0, 1.0);

            // Snap through preview_plain → preview_normalized so stepped
            // parameters land on exact values.
            let plain = param.preview_plain(new_norm);
            setter.set_parameter(param, plain);
        }
        if response.drag_stopped() {
            setter.end_set_parameter(param);
        }

        // Double-click → reset to default
        if response.double_clicked() {
            setter.begin_set_parameter(param);
            setter.set_parameter(param, param.default_plain_value());
            setter.end_set_parameter(param);
        }

        // Ctrl/Cmd-click → also reset
        if response.clicked() && ui.input(|i| i.modifiers.command || i.modifiers.ctrl) {
            setter.begin_set_parameter(param);
            setter.set_parameter(param, param.default_plain_value());
            setter.end_set_parameter(param);
        }

        // ── Render ───────────────────────────────────────────
        if ui.is_rect_visible(rect) {
            draw_knob_visual(&ui.painter_at(rect), rect, new_norm, size, s);
        }

        // ── Tooltip ──────────────────────────────────────────
        let response = response.on_hover_text(format!("{}: {}", param.name(), param));

        // ── MIDI Learn ───────────────────────────────────────
        add_midi_learn_menu(ui, &response, &param_id_for(param.name()), state);

        // ── Numeric field ────────────────────────────────────
        let field_w = diameter * 0.72;
        let field_resp = ui.add(
            Button::new(rich(&value_text, size.font_size(s)).color(TEXT_PRI))
                .fill(WIDGET_BG)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(corner_radius(3.0 * s))
                .min_size(vec2(field_w, 0.0)),
        );
        field_resp.on_hover_text(format!("{} (drag knob to adjust)", param.name()));
    });
}

/// Render the knob: body circle, background arc, filled arc, indicator line, center dot.
fn draw_knob_visual(painter: &Painter, rect: Rect, normalized: f32, size: KnobSize, s: f32) {
    let center = rect.center();
    let radius = rect.width() / 2.0;
    let tw = size.track_w(s);
    let body_r = radius - tw;

    // ── Body ─────────────────────────────────────────────
    painter.circle_filled(center, body_r, WIDGET_BG);
    painter.circle_stroke(center, body_r, Stroke::new(0.5 * s, BORDER));

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
            Stroke::new(tw, ACCENT),
        );
    }

    // ── Value indicator line ─────────────────────────────
    let angle = ARC_START + ARC_SWEEP * norm;
    let inner = radius * 0.32;
    let outer = body_r - 1.5 * s;
    let p1 = center + vec2(inner * angle.cos(), inner * angle.sin());
    let p2 = center + vec2(outer * angle.cos(), outer * angle.sin());
    painter.line_segment([p1, p2], Stroke::new(2.0 * s, TEXT_PRI));

    // ── Centre dot ───────────────────────────────────────
    painter.circle_filled(center, 2.5 * s, ACCENT);
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
            .corner_radius(corner_radius(3.0 * s)),
    );
    if resp.clicked() {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, !inverted);
        setter.end_set_parameter(param);
    }
    resp.on_hover_text(if inverted {
        format!("{label}: Inverted \u{2014} click for Normal")
    } else {
        format!("{label}: Normal \u{2014} click for Inverted")
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// Bool toggle button (used for :2 / x2)
// ═══════════════════════════════════════════════════════════════════════════

fn draw_toggle_btn(
    ui: &mut Ui,
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
        setter.begin_set_parameter(param);
        setter.set_parameter(param, !on);
        setter.end_set_parameter(param);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// MIDI Learn Right-Click Menu
// ═══════════════════════════════════════════════════════════════════════════

/// Attach a right-click MIDI Learn context menu to a widget response.
fn add_midi_learn_menu(_ui: &mut Ui, response: &Response, param_id: &str, state: &mut EditorState) {
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
                ml.mappings.retain(|m| m.param_id != param_id);
            }
            ui.close_menu();
        }

        // Roll Back
        if ui.button("Roll Back").clicked() {
            if let Ok(mut ml) = state.params.midi_learn.write() {
                if let Some(pos) = ml.mappings.iter().rposition(|m| m.param_id == param_id) {
                    ml.mappings.remove(pos);
                }
            }
            ui.close_menu();
        }

        // Save
        if ui.button("Save").clicked() {
            ui.close_menu();
        }

        ui.separator();

        // Current mapping info
        if let Ok(ml) = state.params.midi_learn.read() {
            if let Some(mapping) = ml.mappings.iter().find(|m| m.param_id == param_id) {
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
        high_cut_l: params.high_cut_l.value(),
        high_cut_r: params.high_cut_r.value(),
        feedback_l: params.feedback_l.value(),
        feedback_r: params.feedback_r.value(),
        feedback_phase_l: params.feedback_phase_l.value(),
        feedback_phase_r: params.feedback_phase_r.value(),
        crossfeed_lr: params.crossfeed_lr.value(),
        crossfeed_rl: params.crossfeed_rl.value(),
        crossfeed_phase: params.crossfeed_phase.value(),
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
    set_f!(&params.high_cut_l, snap.high_cut_l);
    set_f!(&params.high_cut_r, snap.high_cut_r);
    set_f!(&params.feedback_l, snap.feedback_l);
    set_f!(&params.feedback_r, snap.feedback_r);
    set_b!(&params.feedback_phase_l, snap.feedback_phase_l);
    set_b!(&params.feedback_phase_r, snap.feedback_phase_r);
    set_f!(&params.crossfeed_lr, snap.crossfeed_lr);
    set_f!(&params.crossfeed_rl, snap.crossfeed_rl);
    set_b!(&params.crossfeed_phase, snap.crossfeed_phase);
    set_routing!(&params.routing, snap.routing);
    set_b!(&params.tempo_sync, snap.tempo_sync);
    set_b!(&params.stereo_link, snap.stereo_link);
    set_f!(&params.output_mix_l, snap.output_mix_l);
    set_f!(&params.output_mix_r, snap.output_mix_r);
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
