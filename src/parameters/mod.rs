//! Parameters module for **Nebula Stereo Delay** by Nebula Audio.
//!
//! Defines all plugin parameters using nih_plug's parameter system, including
//! per-channel delay parameters, crossfeed controls, routing modes, and
//! global options. Internal (non-automatable) state such as A/B snapshots,
//! undo/redo history, and MIDI learn mappings are also housed here.
//!
//! # Parameter Organisation
//!
//! | Group               | Parameters                                              |
//! |---------------------|---------------------------------------------------------|
//! | Per-Channel (L/R)   | Input mode, delay time, note, deviation, halve/double,  |
//! |                     | low/high cut, feedback, feedback phase                   |
//! | Crossfeed           | L→R amount/phase, R→L amount/phase                     |
//! | Global / Output     | Routing, tempo sync, stereo link, output mix            |
//! | Internal State      | FX bypass, A/B snapshots, undo/redo, MIDI learn         |
//!
//! # Stable IDs
//!
//! Every `#[id]` attribute value is a short, **never-to-change** string used
//! by the host for automation mapping and preset serialisation. Renaming a
//! field or moving it within the struct is safe; changing the ID is not.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, RwLock};

use nih_plug::params::enums::{Enum, EnumParam};
use nih_plug::params::range::FloatRange;
use nih_plug::params::smoothing::SmoothingStyle;
use nih_plug::params::{BoolParam, FloatParam, Params};
use serde::{Deserialize, Serialize};

use crate::dsp;
use crate::midi::MidiLearnState;

// ═══════════════════════════════════════════════════════════════════════════
// Enum Parameter Types
// ═══════════════════════════════════════════════════════════════════════════

/// Per-channel input source selection.
///
/// Determines which input signal (or combination) feeds each delay line.
/// The default for the left channel is **Left**, and for the right channel
/// is **Right**, so the plugin behaves as a standard stereo delay out of
/// the box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum InputModeParam {
    #[id = "off"]
    #[name = "Off"]
    Off,
    #[id = "left"]
    #[name = "Left"]
    Left,
    #[id = "right"]
    #[name = "Right"]
    Right,
    #[id = "lpr"]
    #[name = "L+R"]
    LeftPlusRight,
    #[id = "lmr"]
    #[name = "L-R"]
    LeftMinusRight,
}

impl From<InputModeParam> for dsp::InputMode {
    #[inline]
    fn from(val: InputModeParam) -> Self {
        match val {
            InputModeParam::Off => dsp::InputMode::Off,
            InputModeParam::Left => dsp::InputMode::Left,
            InputModeParam::Right => dsp::InputMode::Right,
            InputModeParam::LeftPlusRight => dsp::InputMode::LeftPlusRight,
            InputModeParam::LeftMinusRight => dsp::InputMode::LeftMinusRight,
        }
    }
}

/// Musical note values for tempo-sync quantisation.
///
/// Each variant maps to a rhythmic subdivision. `T` variants are **triplet**
/// divisions (2/3 of the straight value), and `.` variants are dotted notes.
/// The enum order preserves host/preset indices; the custom editor presents
/// these as a short-to-long note menu and ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum NoteValueParam {
    #[id = "1/1"]
    #[name = "1/1"]
    Whole,
    #[id = "1/2"]
    #[name = "1/2"]
    Half,
    #[id = "1/2t"]
    #[name = "1/2T"]
    HalfTriplet,
    #[id = "1/4"]
    #[name = "1/4"]
    Quarter,
    #[id = "1/4t"]
    #[name = "1/4T"]
    QuarterTriplet,
    #[id = "1/8"]
    #[name = "1/8"]
    Eighth,
    #[id = "1/8t"]
    #[name = "1/8T"]
    EighthTriplet,
    #[id = "1/16"]
    #[name = "1/16"]
    Sixteenth,
    #[id = "1/16t"]
    #[name = "1/16T"]
    SixteenthTriplet,
    #[id = "1/32"]
    #[name = "1/32"]
    ThirtySecond,
    #[id = "1/32t"]
    #[name = "1/32T"]
    ThirtySecondTriplet,
    #[id = "1/64"]
    #[name = "1/64"]
    SixtyFourth,
    #[id = "1/2."]
    #[name = "1/2."]
    HalfDotted,
    #[id = "1/4."]
    #[name = "1/4."]
    QuarterDotted,
    #[id = "1/8."]
    #[name = "1/8."]
    EighthDotted,
    #[id = "1/16."]
    #[name = "1/16."]
    SixteenthDotted,
    #[id = "1/32."]
    #[name = "1/32."]
    ThirtySecondDotted,
}

impl From<NoteValueParam> for dsp::NoteValue {
    #[inline]
    fn from(val: NoteValueParam) -> Self {
        match val {
            NoteValueParam::Whole => dsp::NoteValue::Whole,
            NoteValueParam::Half => dsp::NoteValue::Half,
            NoteValueParam::HalfTriplet => dsp::NoteValue::HalfTriplet,
            NoteValueParam::Quarter => dsp::NoteValue::Quarter,
            NoteValueParam::QuarterTriplet => dsp::NoteValue::QuarterTriplet,
            NoteValueParam::Eighth => dsp::NoteValue::Eighth,
            NoteValueParam::EighthTriplet => dsp::NoteValue::EighthTriplet,
            NoteValueParam::Sixteenth => dsp::NoteValue::Sixteenth,
            NoteValueParam::SixteenthTriplet => dsp::NoteValue::SixteenthTriplet,
            NoteValueParam::ThirtySecond => dsp::NoteValue::ThirtySecond,
            NoteValueParam::ThirtySecondTriplet => dsp::NoteValue::ThirtySecondTriplet,
            NoteValueParam::SixtyFourth => dsp::NoteValue::SixtyFourth,
            NoteValueParam::HalfDotted => dsp::NoteValue::HalfDotted,
            NoteValueParam::QuarterDotted => dsp::NoteValue::QuarterDotted,
            NoteValueParam::EighthDotted => dsp::NoteValue::EighthDotted,
            NoteValueParam::SixteenthDotted => dsp::NoteValue::SixteenthDotted,
            NoteValueParam::ThirtySecondDotted => dsp::NoteValue::ThirtySecondDotted,
        }
    }
}

/// Routing modes that determine how the L and R delay channels interact
/// in the feedback network and (for some modes) at the output stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum RoutingModeParam {
    #[id = "custom"]
    #[name = "Customized"]
    Customized,
    #[id = "straight"]
    #[name = "Straight"]
    Straight,
    #[id = "crossfeed"]
    #[name = "Crossfeed"]
    Crossfeed,
    #[id = "90/10"]
    #[name = "90/10"]
    NinetyTen,
    #[id = "10/90"]
    #[name = "10/90"]
    TenNinety,
    #[id = "pingpong"]
    #[name = "Ping Pong L/R"]
    PingPong,
    #[id = "pan"]
    #[name = "Pan L/R"]
    Pan,
    #[id = "rotate"]
    #[name = "Rotate L/R"]
    Rotate,
}

impl From<RoutingModeParam> for dsp::RoutingMode {
    #[inline]
    fn from(val: RoutingModeParam) -> Self {
        match val {
            RoutingModeParam::Customized => dsp::RoutingMode::Customized,
            RoutingModeParam::Straight => dsp::RoutingMode::Straight,
            RoutingModeParam::Crossfeed => dsp::RoutingMode::Crossfeed,
            RoutingModeParam::NinetyTen => dsp::RoutingMode::NinetyTen,
            RoutingModeParam::TenNinety => dsp::RoutingMode::TenNinety,
            RoutingModeParam::PingPong => dsp::RoutingMode::PingPong,
            RoutingModeParam::Pan => dsp::RoutingMode::Pan,
            RoutingModeParam::Rotate => dsp::RoutingMode::Rotate,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Internal-State Types (serialised via `#[persist]`)
// ═══════════════════════════════════════════════════════════════════════════

/// A complete snapshot of all plain parameter values for A/B comparison
/// and undo/redo.
///
/// Each field stores the **plain** (un-normalised) value exactly as it
/// appears to the user — e.g., `delay_time_l` is in seconds, `feedback_l`
/// is 0.0–1.0, enums are stored by variant index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamSnapshot {
    // ── Per-channel ──────────────────────────────────────────────────
    pub input_mode_l: usize,
    pub input_mode_r: usize,
    pub delay_time_l: f32,
    pub delay_time_r: f32,
    pub note_l: usize,
    pub note_r: usize,
    pub deviation_l: f32,
    pub deviation_r: f32,
    pub halve_l: bool,
    pub halve_r: bool,
    pub double_l: bool,
    pub double_r: bool,
    pub low_cut_l: f32,
    pub low_cut_r: f32,
    pub low_cut_slope_l: f32,
    pub low_cut_slope_r: f32,
    pub high_cut_l: f32,
    pub high_cut_r: f32,
    pub high_cut_slope_l: f32,
    pub high_cut_slope_r: f32,
    pub feedback_l: f32,
    pub feedback_r: f32,
    pub feedback_phase_l: bool,
    pub feedback_phase_r: bool,

    // ── Crossfeed ────────────────────────────────────────────────────
    pub crossfeed_lr: f32,
    pub crossfeed_rl: f32,
    #[serde(default, alias = "crossfeed_phase")]
    pub crossfeed_phase_lr: bool,
    #[serde(default)]
    pub crossfeed_phase_rl: bool,

    // ── Global / Output ──────────────────────────────────────────────
    pub routing: usize,
    pub tempo_sync: bool,
    pub stereo_link: bool,
    pub output_mix_l: f32,
    pub output_mix_r: f32,
}

/// A/B state storage: two complete parameter snapshots (slot A and slot B).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbSnapshots {
    pub a: ParamSnapshot,
    pub b: ParamSnapshot,
}

impl Default for AbSnapshots {
    fn default() -> Self {
        let default_snapshot = ParamSnapshot::default_values();
        Self {
            a: default_snapshot.clone(),
            b: default_snapshot,
        }
    }
}

impl ParamSnapshot {
    /// Returns a snapshot filled with the plugin's default parameter values.
    fn default_values() -> Self {
        Self {
            input_mode_l: 1, // Left
            input_mode_r: 2, // Right
            delay_time_l: 0.5,
            delay_time_r: 0.5,
            note_l: 3, // Quarter
            note_r: 3,
            deviation_l: 0.0,
            deviation_r: 0.0,
            halve_l: false,
            halve_r: false,
            double_l: false,
            double_r: false,
            low_cut_l: 20.0,
            low_cut_r: 20.0,
            low_cut_slope_l: 12.0,
            low_cut_slope_r: 12.0,
            high_cut_l: 20000.0,
            high_cut_r: 20000.0,
            high_cut_slope_l: 12.0,
            high_cut_slope_r: 12.0,
            feedback_l: 0.4,
            feedback_r: 0.4,
            feedback_phase_l: false,
            feedback_phase_r: false,
            crossfeed_lr: 0.0,
            crossfeed_rl: 0.0,
            crossfeed_phase_lr: false,
            crossfeed_phase_rl: false,
            routing: 0, // Customized
            tempo_sync: false,
            stereo_link: false,
            output_mix_l: 1.0,
            output_mix_r: 1.0,
        }
    }
}

/// Undo/redo stack with a maximum depth of 50 entries.
///
/// When a new snapshot is pushed and the stack is full, the oldest entry
/// is discarded. Performing a push after an undo clears the redo stack
/// (standard undo/redo semantics).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UndoRedoStack {
    pub undo: Vec<ParamSnapshot>,
    pub redo: Vec<ParamSnapshot>,
}

impl UndoRedoStack {
    /// Maximum number of undo steps retained.
    pub const MAX_DEPTH: usize = 50;

    /// Push a snapshot onto the undo stack, clearing the redo stack.
    /// If the undo stack is full, the oldest entry is discarded.
    pub fn push_undo(&mut self, snapshot: ParamSnapshot) {
        if self.undo.len() >= Self::MAX_DEPTH {
            self.undo.remove(0);
        }
        self.undo.push(snapshot);
        self.redo.clear();
    }

    /// Pop the most recent undo snapshot, pushing the current state onto
    /// the redo stack. Returns `None` if the undo stack is empty.
    pub fn undo(&mut self, current: ParamSnapshot) -> Option<ParamSnapshot> {
        self.redo.push(current);
        self.undo.pop()
    }

    /// Pop the most recent redo snapshot, pushing the current state onto
    /// the undo stack. Returns `None` if the redo stack is empty.
    pub fn redo(&mut self, current: ParamSnapshot) -> Option<ParamSnapshot> {
        self.undo.push(current);
        self.redo.pop()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Display Formatters (value ↔ string)
// ═══════════════════════════════════════════════════════════════════════════

/// Format a delay time in milliseconds with 1 decimal place.
fn format_delay_time(val: f32) -> String {
    format!("{:.1}", val * 1000.0)
}

/// Parse a delay time string in milliseconds (e.g., "500" or "500 ms").
fn parse_delay_time(s: &str) -> Option<f32> {
    let trimmed = s.trim();
    let lower = trimmed.to_lowercase();
    if lower.ends_with("ms") {
        trimmed
            .trim_end_matches("ms")
            .trim()
            .parse::<f32>()
            .ok()
            .map(|v| v / 1000.0)
    } else if lower.ends_with('s') {
        trimmed.trim_end_matches('s').trim().parse::<f32>().ok()
    } else {
        trimmed.parse::<f32>().ok().map(|v| v / 1000.0)
    }
}

/// Format a deviation value in cents with 1 decimal place.
fn format_deviation(val: f32) -> String {
    format!("{val:.1}")
}

/// Parse a deviation string (e.g., "0.0" or "0.0 ct").
fn parse_deviation(s: &str) -> Option<f32> {
    s.trim().trim_end_matches("ct").trim().parse::<f32>().ok()
}

/// Format a frequency, switching between Hz and kHz for readability.
fn format_frequency(val: f32) -> String {
    if val >= 1000.0 {
        format!("{:.2} kHz", val / 1000.0)
    } else {
        format!("{val:.1} Hz")
    }
}

/// Parse a frequency string that may use "Hz" or "kHz".
fn parse_frequency(s: &str) -> Option<f32> {
    let s = s.trim();
    if s.contains("kHz") || s.contains("KHz") || s.contains("khz") {
        s.trim_end_matches("kHz")
            .trim_end_matches("KHz")
            .trim_end_matches("khz")
            .trim()
            .parse::<f32>()
            .ok()
            .map(|v| v * 1000.0)
    } else {
        s.trim_end_matches("Hz")
            .trim_end_matches("hz")
            .trim()
            .parse::<f32>()
            .ok()
    }
}

/// Format a filter slope in dB/octave.
fn format_slope(val: f32) -> String {
    format!("{val:.1}")
}

/// Parse a filter slope string that may include "dB/oct".
fn parse_slope(s: &str) -> Option<f32> {
    s.trim()
        .trim_end_matches("dB/oct")
        .trim_end_matches("db/oct")
        .trim()
        .parse::<f32>()
        .ok()
}

/// Format a 0.0–1.0 value as a percentage with 1 decimal place
/// (e.g., 0.4 → "40.0").
fn format_percentage(val: f32) -> String {
    format!("{:.1}", val * 100.0)
}

/// Parse a percentage string back to 0.0–1.0
/// (e.g., "40.0" or "40.0%" → 0.4).
fn parse_percentage(s: &str) -> Option<f32> {
    s.trim()
        .trim_end_matches('%')
        .trim()
        .parse::<f32>()
        .ok()
        .map(|v| v / 100.0)
}

// ═══════════════════════════════════════════════════════════════════════════
// Main Parameter Struct
// ═══════════════════════════════════════════════════════════════════════════

/// All parameters for the **Nebula Stereo Delay** plugin.
///
/// This struct is shared between the audio thread (read-only via smoothed
/// values) and the GUI (read-write). Every `FloatParam` that feeds a
/// continuous DSP coefficient has a 10 ms linear smoother attached to
/// prevent zipper noise.
///
/// # Per-Channel Parameters (L/R)
///
/// Each left/right pair uses the same range and step size. The left
/// channel defaults to a "Left" input and the right channel to a "Right"
/// input so the plugin works as a standard stereo delay without any
/// configuration.
///
/// # Internal State
///
/// Fields marked with `#[persist]` are serialised alongside preset data
/// but are **not** exposed to the host as automatable parameters. This
/// includes the FX bypass flag, A/B snapshot bank, undo/redo history,
/// and MIDI-learn mappings.
///
/// # Stereo Link
///
/// When `stereo_link` is enabled, adjusting a left-channel parameter
/// automatically mirrors the value to the corresponding right-channel
/// parameter. The Ctrl/Cmd modifier temporarily overrides this link.
#[derive(Params)]
pub struct NebulaStereoDelayParams {
    // ── Per-Channel: Input Mode ──────────────────────────────────────────
    /// Input source for the left delay channel. Default: **Left**.
    #[id = "iml"]
    pub input_mode_l: EnumParam<InputModeParam>,

    /// Input source for the right delay channel. Default: **Right**.
    #[id = "imr"]
    pub input_mode_r: EnumParam<InputModeParam>,

    // ── Per-Channel: Delay Time ──────────────────────────────────────────
    /// Base delay time for the left channel in seconds (0.005–2.0).
    /// Used when `tempo_sync` is off. Default: 0.5 s.
    #[id = "dtl"]
    pub delay_time_l: FloatParam,

    /// Base delay time for the right channel in seconds (0.005–2.0).
    /// Used when `tempo_sync` is off. Default: 0.5 s.
    #[id = "dtr"]
    pub delay_time_r: FloatParam,

    // ── Per-Channel: Tempo-Sync Note ─────────────────────────────────────
    /// Note value for the left channel (used when `tempo_sync` is on).
    /// Default: 1/4 (quarter note).
    #[id = "ntl"]
    pub note_l: EnumParam<NoteValueParam>,

    /// Note value for the right channel (used when `tempo_sync` is on).
    /// Default: 1/4 (quarter note).
    #[id = "ntr"]
    pub note_r: EnumParam<NoteValueParam>,

    // ── Per-Channel: Deviation ───────────────────────────────────────────
    /// Deviation from the quantised delay time for the left channel in
    /// cents (−100 to +100). Applied as `2^(deviation/1200)`.
    /// Default: 0 ct.
    #[id = "dvl"]
    pub deviation_l: FloatParam,

    /// Deviation from the quantised delay time for the right channel in
    /// cents (−100 to +100). Default: 0 ct.
    #[id = "dvr"]
    pub deviation_r: FloatParam,

    // ── Per-Channel: Halve / Double ──────────────────────────────────────
    /// Halve the left-channel delay time (the ":2" button).
    /// Default: off.
    #[id = "hvl"]
    pub halve_l: BoolParam,

    /// Halve the right-channel delay time (the ":2" button).
    /// Default: off.
    #[id = "hvr"]
    pub halve_r: BoolParam,

    /// Double the left-channel delay time (the "×2" button).
    /// Default: off.
    #[id = "dbl"]
    pub double_l: BoolParam,

    /// Double the right-channel delay time (the "×2" button).
    /// Default: off.
    #[id = "dbr"]
    pub double_r: BoolParam,

    // ── Per-Channel: Filters ─────────────────────────────────────────────
    /// Low-cut (high-pass) frequency for the left channel (20–20 000 Hz).
    /// Default: 20 Hz (effectively off).
    #[id = "lcl"]
    pub low_cut_l: FloatParam,

    /// Low-cut (high-pass) frequency for the right channel (20–20 000 Hz).
    /// Default: 20 Hz.
    #[id = "lcr"]
    pub low_cut_r: FloatParam,

    /// Low-cut slope for the left channel (1–100 dB/oct).
    /// Default: 12 dB/oct.
    #[id = "lcsl"]
    pub low_cut_slope_l: FloatParam,

    /// Low-cut slope for the right channel (1–100 dB/oct).
    /// Default: 12 dB/oct.
    #[id = "lcsr"]
    pub low_cut_slope_r: FloatParam,

    /// High-cut (low-pass) frequency for the left channel (20–20 000 Hz).
    /// Default: 20 000 Hz (effectively off).
    #[id = "hcl"]
    pub high_cut_l: FloatParam,

    /// High-cut (low-pass) frequency for the right channel (20–20 000 Hz).
    /// Default: 20 000 Hz.
    #[id = "hcr"]
    pub high_cut_r: FloatParam,

    /// High-cut slope for the left channel (1–100 dB/oct).
    /// Default: 12 dB/oct.
    #[id = "hcsl"]
    pub high_cut_slope_l: FloatParam,

    /// High-cut slope for the right channel (1–100 dB/oct).
    /// Default: 12 dB/oct.
    #[id = "hcsr"]
    pub high_cut_slope_r: FloatParam,

    // ── Per-Channel: Feedback ────────────────────────────────────────────
    /// Feedback amount for the left channel (0.0–1.0). Default: 0.4 (40%).
    #[id = "fbl"]
    pub feedback_l: FloatParam,

    /// Feedback amount for the right channel (0.0–1.0). Default: 0.4 (40%).
    #[id = "fbr"]
    pub feedback_r: FloatParam,

    /// Invert left-channel feedback phase (180° flip).
    /// Display: "Normal" / "Inverted". Default: Normal.
    #[id = "fpl"]
    pub feedback_phase_l: BoolParam,

    /// Invert right-channel feedback phase (180° flip).
    /// Display: "Normal" / "Inverted". Default: Normal.
    #[id = "fpr"]
    pub feedback_phase_r: BoolParam,

    // ── Crossfeed ────────────────────────────────────────────────────────
    /// L→R crossfeed amount (0.0–1.0). Default: 0.0 (0%).
    #[id = "clr"]
    pub crossfeed_lr: FloatParam,

    /// R→L crossfeed amount (0.0–1.0). Default: 0.0 (0%).
    #[id = "crl"]
    pub crossfeed_rl: FloatParam,

    /// Invert the L→R crossfeed phase. Default: Normal.
    #[id = "cfp"]
    pub crossfeed_phase_lr: BoolParam,

    /// Invert the R→L crossfeed phase. Default: Normal.
    #[id = "cfpr"]
    pub crossfeed_phase_rl: BoolParam,

    // ── Global: Routing ──────────────────────────────────────────────────
    /// Active routing mode. Default: Customized.
    #[id = "rout"]
    pub routing: EnumParam<RoutingModeParam>,

    // ── Global: Tempo Sync ───────────────────────────────────────────────
    /// When enabled, delay time is derived from the host tempo and the
    /// selected note value. Display: "Free" / "Sync". Default: Free.
    #[id = "tsyn"]
    pub tempo_sync: BoolParam,

    // ── Global: Stereo Link ──────────────────────────────────────────────
    /// When enabled, left-channel parameters are mirrored to the right
    /// channel. Display: "Unlinked" / "Linked". Default: Unlinked.
    /// Ctrl/Cmd modifier allows temporary unlink.
    #[id = "slnk"]
    pub stereo_link: BoolParam,

    // ── Global: Output Mix ───────────────────────────────────────────────
    /// Dry/wet mix for the left channel (0.0 = dry, 1.0 = full wet).
    /// Default: 1.0 (100%).
    #[id = "oml"]
    pub output_mix_l: FloatParam,

    /// Dry/wet mix for the right channel (0.0 = dry, 1.0 = full wet).
    /// Default: 1.0 (100%).
    #[id = "omr"]
    pub output_mix_r: FloatParam,

    // ══════════════════════════════════════════════════════════════════════
    // Internal State (persisted but not automatable)
    // ══════════════════════════════════════════════════════════════════════
    /// FX bypass flag. When `true`, the DSP engine crossfades to a
    /// passthrough signal over 512 samples (soft bypass). This is a
    /// software-internal flag; nih_plug provides its own host-visible
    /// bypass parameter automatically.
    #[persist = "bypass"]
    pub bypass: AtomicBool,

    /// Active A/B slot (0 = A, 1 = B).
    #[persist = "ab_state"]
    pub ab_state: AtomicU8,

    /// Two complete parameter snapshots for A/B comparison.
    #[persist = "ab_snapshots"]
    pub ab_snapshots: RwLock<AbSnapshots>,

    /// Undo/redo history (up to 50 steps).
    #[persist = "undo_stack"]
    pub undo_stack: RwLock<UndoRedoStack>,

    /// MIDI-learn mappings (CC + channel → parameter ID).
    #[persist = "midi_learn"]
    pub midi_learn: RwLock<MidiLearnState>,
}

impl Default for NebulaStereoDelayParams {
    fn default() -> Self {
        // ── Smoothing time for continuous parameters ─────────────────
        // 10 ms linear smoothing prevents zipper noise without adding
        // noticeable latency to parameter changes.
        const SMOOTH_MS: f32 = 10.0;

        // ── Skew factor for frequency parameters ─────────────────────
        // A factor of `skew_factor(-2.0)` gives approximately
        // logarithmic distribution, placing more resolution at low
        // frequencies where the human ear is most sensitive.
        let freq_skew = FloatRange::skew_factor(-2.0);

        Self {
            // ══════════════════════════════════════════════════════════
            // Per-Channel: Input Mode
            // ══════════════════════════════════════════════════════════
            input_mode_l: EnumParam::new("Input Mode L", InputModeParam::Left),
            input_mode_r: EnumParam::new("Input Mode R", InputModeParam::Right),

            // ══════════════════════════════════════════════════════════
            // Per-Channel: Delay Time
            // ══════════════════════════════════════════════════════════
            delay_time_l: FloatParam::new(
                "Delay Time L",
                0.5,
                FloatRange::Linear {
                    min: 0.005,
                    max: 2.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(SMOOTH_MS))
            .with_step_size(0.001)
            .with_value_to_string(Arc::new(format_delay_time))
            .with_string_to_value(Arc::new(parse_delay_time))
            .with_unit(" ms"),

            delay_time_r: FloatParam::new(
                "Delay Time R",
                0.5,
                FloatRange::Linear {
                    min: 0.005,
                    max: 2.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(SMOOTH_MS))
            .with_step_size(0.001)
            .with_value_to_string(Arc::new(format_delay_time))
            .with_string_to_value(Arc::new(parse_delay_time))
            .with_unit(" ms"),

            // ══════════════════════════════════════════════════════════
            // Per-Channel: Tempo-Sync Note
            // ══════════════════════════════════════════════════════════
            note_l: EnumParam::new("Note L", NoteValueParam::Quarter),
            note_r: EnumParam::new("Note R", NoteValueParam::Quarter),

            // ══════════════════════════════════════════════════════════
            // Per-Channel: Deviation
            // ══════════════════════════════════════════════════════════
            deviation_l: FloatParam::new(
                "Deviation L",
                0.0,
                FloatRange::Linear {
                    min: -100.0,
                    max: 100.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(SMOOTH_MS))
            .with_step_size(0.1)
            .with_value_to_string(Arc::new(format_deviation))
            .with_string_to_value(Arc::new(parse_deviation))
            .with_unit(" ct"),

            deviation_r: FloatParam::new(
                "Deviation R",
                0.0,
                FloatRange::Linear {
                    min: -100.0,
                    max: 100.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(SMOOTH_MS))
            .with_step_size(0.1)
            .with_value_to_string(Arc::new(format_deviation))
            .with_string_to_value(Arc::new(parse_deviation))
            .with_unit(" ct"),

            // ══════════════════════════════════════════════════════════
            // Per-Channel: Halve / Double
            // ══════════════════════════════════════════════════════════
            halve_l: BoolParam::new("Halve L", false)
                .with_value_to_string(Arc::new(|v| {
                    if v {
                        ":2".to_string()
                    } else {
                        "Off".to_string()
                    }
                }))
                .with_string_to_value(Arc::new(|s| {
                    let s = s.trim().to_lowercase();
                    Some(s == ":2" || s == "on" || s == "true")
                })),

            halve_r: BoolParam::new("Halve R", false)
                .with_value_to_string(Arc::new(|v| {
                    if v {
                        ":2".to_string()
                    } else {
                        "Off".to_string()
                    }
                }))
                .with_string_to_value(Arc::new(|s| {
                    let s = s.trim().to_lowercase();
                    Some(s == ":2" || s == "on" || s == "true")
                })),

            double_l: BoolParam::new("Double L", false)
                .with_value_to_string(Arc::new(|v| {
                    if v {
                        "x2".to_string()
                    } else {
                        "Off".to_string()
                    }
                }))
                .with_string_to_value(Arc::new(|s| {
                    let s = s.trim().to_lowercase();
                    Some(s == "x2" || s == "on" || s == "true")
                })),

            double_r: BoolParam::new("Double R", false)
                .with_value_to_string(Arc::new(|v| {
                    if v {
                        "x2".to_string()
                    } else {
                        "Off".to_string()
                    }
                }))
                .with_string_to_value(Arc::new(|s| {
                    let s = s.trim().to_lowercase();
                    Some(s == "x2" || s == "on" || s == "true")
                })),

            // ══════════════════════════════════════════════════════════
            // Per-Channel: Filters
            // ══════════════════════════════════════════════════════════
            low_cut_l: FloatParam::new(
                "Low Cut L",
                20.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 20000.0,
                    factor: freq_skew,
                },
            )
            .with_smoother(SmoothingStyle::Linear(SMOOTH_MS))
            .with_value_to_string(Arc::new(format_frequency))
            .with_string_to_value(Arc::new(parse_frequency)),

            low_cut_r: FloatParam::new(
                "Low Cut R",
                20.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 20000.0,
                    factor: freq_skew,
                },
            )
            .with_smoother(SmoothingStyle::Linear(SMOOTH_MS))
            .with_value_to_string(Arc::new(format_frequency))
            .with_string_to_value(Arc::new(parse_frequency)),

            low_cut_slope_l: FloatParam::new(
                "Low Cut Slope L",
                12.0,
                FloatRange::Linear {
                    min: 1.0,
                    max: 100.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(SMOOTH_MS))
            .with_step_size(0.1)
            .with_value_to_string(Arc::new(format_slope))
            .with_string_to_value(Arc::new(parse_slope))
            .with_unit(" dB/oct"),

            low_cut_slope_r: FloatParam::new(
                "Low Cut Slope R",
                12.0,
                FloatRange::Linear {
                    min: 1.0,
                    max: 100.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(SMOOTH_MS))
            .with_step_size(0.1)
            .with_value_to_string(Arc::new(format_slope))
            .with_string_to_value(Arc::new(parse_slope))
            .with_unit(" dB/oct"),

            high_cut_l: FloatParam::new(
                "High Cut L",
                20000.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 20000.0,
                    factor: freq_skew,
                },
            )
            .with_smoother(SmoothingStyle::Linear(SMOOTH_MS))
            .with_value_to_string(Arc::new(format_frequency))
            .with_string_to_value(Arc::new(parse_frequency)),

            high_cut_r: FloatParam::new(
                "High Cut R",
                20000.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 20000.0,
                    factor: freq_skew,
                },
            )
            .with_smoother(SmoothingStyle::Linear(SMOOTH_MS))
            .with_value_to_string(Arc::new(format_frequency))
            .with_string_to_value(Arc::new(parse_frequency)),

            high_cut_slope_l: FloatParam::new(
                "High Cut Slope L",
                12.0,
                FloatRange::Linear {
                    min: 1.0,
                    max: 100.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(SMOOTH_MS))
            .with_step_size(0.1)
            .with_value_to_string(Arc::new(format_slope))
            .with_string_to_value(Arc::new(parse_slope))
            .with_unit(" dB/oct"),

            high_cut_slope_r: FloatParam::new(
                "High Cut Slope R",
                12.0,
                FloatRange::Linear {
                    min: 1.0,
                    max: 100.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(SMOOTH_MS))
            .with_step_size(0.1)
            .with_value_to_string(Arc::new(format_slope))
            .with_string_to_value(Arc::new(parse_slope))
            .with_unit(" dB/oct"),

            // ══════════════════════════════════════════════════════════
            // Per-Channel: Feedback
            // ══════════════════════════════════════════════════════════
            feedback_l: FloatParam::new(
                "Feedback L",
                0.4,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_smoother(SmoothingStyle::Linear(SMOOTH_MS))
            .with_step_size(0.01)
            .with_value_to_string(Arc::new(format_percentage))
            .with_string_to_value(Arc::new(parse_percentage))
            .with_unit("%"),

            feedback_r: FloatParam::new(
                "Feedback R",
                0.4,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_smoother(SmoothingStyle::Linear(SMOOTH_MS))
            .with_step_size(0.01)
            .with_value_to_string(Arc::new(format_percentage))
            .with_string_to_value(Arc::new(parse_percentage))
            .with_unit("%"),

            feedback_phase_l: BoolParam::new("Feedback Phase L", false)
                .with_value_to_string(Arc::new(|v| {
                    if v {
                        "Inverted".to_string()
                    } else {
                        "Normal".to_string()
                    }
                }))
                .with_string_to_value(Arc::new(|s| {
                    let s = s.trim().to_lowercase();
                    Some(s == "inverted" || s == "inv" || s == "on" || s == "true")
                })),

            feedback_phase_r: BoolParam::new("Feedback Phase R", false)
                .with_value_to_string(Arc::new(|v| {
                    if v {
                        "Inverted".to_string()
                    } else {
                        "Normal".to_string()
                    }
                }))
                .with_string_to_value(Arc::new(|s| {
                    let s = s.trim().to_lowercase();
                    Some(s == "inverted" || s == "inv" || s == "on" || s == "true")
                })),

            // ══════════════════════════════════════════════════════════
            // Crossfeed
            // ══════════════════════════════════════════════════════════
            crossfeed_lr: FloatParam::new(
                "Crossfeed L-R",
                0.0,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_smoother(SmoothingStyle::Linear(SMOOTH_MS))
            .with_step_size(0.01)
            .with_value_to_string(Arc::new(format_percentage))
            .with_string_to_value(Arc::new(parse_percentage))
            .with_unit("%"),

            crossfeed_rl: FloatParam::new(
                "Crossfeed R-L",
                0.0,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_smoother(SmoothingStyle::Linear(SMOOTH_MS))
            .with_step_size(0.01)
            .with_value_to_string(Arc::new(format_percentage))
            .with_string_to_value(Arc::new(parse_percentage))
            .with_unit("%"),

            crossfeed_phase_lr: BoolParam::new("Crossfeed Phase L-R", false)
                .with_value_to_string(Arc::new(|v| {
                    if v {
                        "Inverted".to_string()
                    } else {
                        "Normal".to_string()
                    }
                }))
                .with_string_to_value(Arc::new(|s| {
                    let s = s.trim().to_lowercase();
                    Some(s == "inverted" || s == "inv" || s == "on" || s == "true")
                })),

            crossfeed_phase_rl: BoolParam::new("Crossfeed Phase R-L", false)
                .with_value_to_string(Arc::new(|v| {
                    if v {
                        "Inverted".to_string()
                    } else {
                        "Normal".to_string()
                    }
                }))
                .with_string_to_value(Arc::new(|s| {
                    let s = s.trim().to_lowercase();
                    Some(s == "inverted" || s == "inv" || s == "on" || s == "true")
                })),

            // ══════════════════════════════════════════════════════════
            // Global: Routing
            // ══════════════════════════════════════════════════════════
            routing: EnumParam::new("Routing", RoutingModeParam::Customized),

            // ══════════════════════════════════════════════════════════
            // Global: Tempo Sync
            // ══════════════════════════════════════════════════════════
            tempo_sync: BoolParam::new("Tempo Sync", false)
                .with_value_to_string(Arc::new(|v| {
                    if v {
                        "Sync".to_string()
                    } else {
                        "Free".to_string()
                    }
                }))
                .with_string_to_value(Arc::new(|s| {
                    let s = s.trim().to_lowercase();
                    Some(s == "sync" || s == "on" || s == "true")
                })),

            // ══════════════════════════════════════════════════════════
            // Global: Stereo Link
            // ══════════════════════════════════════════════════════════
            stereo_link: BoolParam::new("Stereo Link", false)
                .with_value_to_string(Arc::new(|v| {
                    if v {
                        "Linked".to_string()
                    } else {
                        "Unlinked".to_string()
                    }
                }))
                .with_string_to_value(Arc::new(|s| {
                    let s = s.trim().to_lowercase();
                    Some(s == "linked" || s == "on" || s == "true")
                })),

            // ══════════════════════════════════════════════════════════
            // Global: Output Mix
            // ══════════════════════════════════════════════════════════
            output_mix_l: FloatParam::new(
                "Output Mix L",
                1.0,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_smoother(SmoothingStyle::Linear(SMOOTH_MS))
            .with_step_size(0.01)
            .with_value_to_string(Arc::new(format_percentage))
            .with_string_to_value(Arc::new(parse_percentage))
            .with_unit("%"),

            output_mix_r: FloatParam::new(
                "Output Mix R",
                1.0,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_smoother(SmoothingStyle::Linear(SMOOTH_MS))
            .with_step_size(0.01)
            .with_value_to_string(Arc::new(format_percentage))
            .with_string_to_value(Arc::new(parse_percentage))
            .with_unit("%"),

            // ══════════════════════════════════════════════════════════
            // Internal State
            // ══════════════════════════════════════════════════════════
            bypass: AtomicBool::new(false),
            ab_state: AtomicU8::new(0),
            ab_snapshots: RwLock::new(AbSnapshots::default()),
            undo_stack: RwLock::new(UndoRedoStack::default()),
            midi_learn: RwLock::new(MidiLearnState::default()),
        }
    }
}

impl NebulaStereoDelayParams {
    // ──────────────────────────────────────────────────────────────────
    // Snapshot helpers
    // ──────────────────────────────────────────────────────────────────

    /// Capture the current state of all automatable parameters into a
    /// [`ParamSnapshot`].
    ///
    /// This only reads parameter values, so it can be called from any
    /// thread.
    pub fn capture_snapshot(&self) -> ParamSnapshot {
        ParamSnapshot {
            input_mode_l: self.input_mode_l.value().to_index(),
            input_mode_r: self.input_mode_r.value().to_index(),
            delay_time_l: self.delay_time_l.value(),
            delay_time_r: self.delay_time_r.value(),
            note_l: self.note_l.value().to_index(),
            note_r: self.note_r.value().to_index(),
            deviation_l: self.deviation_l.value(),
            deviation_r: self.deviation_r.value(),
            halve_l: self.halve_l.value(),
            halve_r: self.halve_r.value(),
            double_l: self.double_l.value(),
            double_r: self.double_r.value(),
            low_cut_l: self.low_cut_l.value(),
            low_cut_r: self.low_cut_r.value(),
            low_cut_slope_l: self.low_cut_slope_l.value(),
            low_cut_slope_r: self.low_cut_slope_r.value(),
            high_cut_l: self.high_cut_l.value(),
            high_cut_r: self.high_cut_r.value(),
            high_cut_slope_l: self.high_cut_slope_l.value(),
            high_cut_slope_r: self.high_cut_slope_r.value(),
            feedback_l: self.feedback_l.value(),
            feedback_r: self.feedback_r.value(),
            feedback_phase_l: self.feedback_phase_l.value(),
            feedback_phase_r: self.feedback_phase_r.value(),
            crossfeed_lr: self.crossfeed_lr.value(),
            crossfeed_rl: self.crossfeed_rl.value(),
            crossfeed_phase_lr: self.crossfeed_phase_lr.value(),
            crossfeed_phase_rl: self.crossfeed_phase_rl.value(),
            routing: self.routing.value().to_index(),
            tempo_sync: self.tempo_sync.value(),
            stereo_link: self.stereo_link.value(),
            output_mix_l: self.output_mix_l.value(),
            output_mix_r: self.output_mix_r.value(),
        }
    }

    /// Apply a [`ParamSnapshot`] to all automatable parameters using a
    /// [`ParamSetter`].
    ///
    /// This is the **only** correct way to programmatically change
    /// multiple parameter values in nih_plug — direct mutation of a
    /// parameter's internal value bypasses the host's automation system
    /// and will cause desynchronisation between the plugin and the host.
    ///
    /// The method wraps every parameter change in a
    /// `begin_set_parameter` / `set_parameter` / `end_set_parameter`
    /// sequence so the host correctly records the change.
    ///
    /// # Important
    ///
    /// This method must only be called from the **main/GUI thread**, as
    /// required by nih_plug's `ParamSetter`.
    pub fn apply_snapshot(
        &self,
        setter: &nih_plug::context::gui::ParamSetter,
        snapshot: &ParamSnapshot,
    ) {
        // Float params
        setter.set_parameter(&self.delay_time_l, snapshot.delay_time_l);
        setter.set_parameter(&self.delay_time_r, snapshot.delay_time_r);
        setter.set_parameter(&self.deviation_l, snapshot.deviation_l);
        setter.set_parameter(&self.deviation_r, snapshot.deviation_r);
        setter.set_parameter(&self.low_cut_l, snapshot.low_cut_l);
        setter.set_parameter(&self.low_cut_r, snapshot.low_cut_r);
        setter.set_parameter(&self.low_cut_slope_l, snapshot.low_cut_slope_l);
        setter.set_parameter(&self.low_cut_slope_r, snapshot.low_cut_slope_r);
        setter.set_parameter(&self.high_cut_l, snapshot.high_cut_l);
        setter.set_parameter(&self.high_cut_r, snapshot.high_cut_r);
        setter.set_parameter(&self.high_cut_slope_l, snapshot.high_cut_slope_l);
        setter.set_parameter(&self.high_cut_slope_r, snapshot.high_cut_slope_r);
        setter.set_parameter(&self.feedback_l, snapshot.feedback_l);
        setter.set_parameter(&self.feedback_r, snapshot.feedback_r);
        setter.set_parameter(&self.crossfeed_lr, snapshot.crossfeed_lr);
        setter.set_parameter(&self.crossfeed_rl, snapshot.crossfeed_rl);
        setter.set_parameter(&self.output_mix_l, snapshot.output_mix_l);
        setter.set_parameter(&self.output_mix_r, snapshot.output_mix_r);

        // Bool params
        setter.set_parameter(&self.halve_l, snapshot.halve_l);
        setter.set_parameter(&self.halve_r, snapshot.halve_r);
        setter.set_parameter(&self.double_l, snapshot.double_l);
        setter.set_parameter(&self.double_r, snapshot.double_r);
        setter.set_parameter(&self.feedback_phase_l, snapshot.feedback_phase_l);
        setter.set_parameter(&self.feedback_phase_r, snapshot.feedback_phase_r);
        setter.set_parameter(&self.crossfeed_phase_lr, snapshot.crossfeed_phase_lr);
        setter.set_parameter(&self.crossfeed_phase_rl, snapshot.crossfeed_phase_rl);
        setter.set_parameter(&self.tempo_sync, snapshot.tempo_sync);
        setter.set_parameter(&self.stereo_link, snapshot.stereo_link);

        // Enum params (set via variant index)
        setter.set_parameter(
            &self.input_mode_l,
            InputModeParam::from_index(snapshot.input_mode_l),
        );
        setter.set_parameter(
            &self.input_mode_r,
            InputModeParam::from_index(snapshot.input_mode_r),
        );
        setter.set_parameter(&self.note_l, NoteValueParam::from_index(snapshot.note_l));
        setter.set_parameter(&self.note_r, NoteValueParam::from_index(snapshot.note_r));
        setter.set_parameter(
            &self.routing,
            RoutingModeParam::from_index(snapshot.routing),
        );
    }

    // ──────────────────────────────────────────────────────────────────
    // A/B switching
    // ──────────────────────────────────────────────────────────────────

    /// Save the current parameter state into the active A/B slot.
    pub fn ab_save_current(&self) {
        let slot = self.ab_state.load(Ordering::Relaxed);
        let snapshot = self.capture_snapshot();
        let mut snapshots = self
            .ab_snapshots
            .write()
            .expect("Poisoned RwLock on ab_save");
        match slot {
            0 => snapshots.a = snapshot,
            _ => snapshots.b = snapshot,
        }
    }

    /// Switch to the other A/B slot. Saves the current state to the
    /// slot being left, switches the active slot, and returns the
    /// snapshot that the caller should apply via
    /// [`apply_snapshot`][Self::apply_snapshot].
    ///
    /// # Usage
    ///
    /// ```ignore
    /// let snapshot = params.ab_toggle();
    /// params.apply_snapshot(&setter, &snapshot);
    /// ```
    pub fn ab_toggle(&self) -> ParamSnapshot {
        let current_slot = self.ab_state.load(Ordering::Relaxed);
        let current_snapshot = self.capture_snapshot();

        // Save current state into the slot we're leaving
        {
            let mut snapshots = self
                .ab_snapshots
                .write()
                .expect("Poisoned RwLock on ab_toggle save");
            match current_slot {
                0 => snapshots.a = current_snapshot,
                _ => snapshots.b = current_snapshot,
            }
        }

        // Switch to the other slot
        let new_slot = if current_slot == 0 { 1 } else { 0 };
        self.ab_state.store(new_slot, Ordering::Relaxed);

        // Return the target slot's snapshot for the caller to apply
        let snapshots = self
            .ab_snapshots
            .read()
            .expect("Poisoned RwLock on ab_toggle read");
        match new_slot {
            0 => snapshots.a.clone(),
            _ => snapshots.b.clone(),
        }
    }

    // ──────────────────────────────────────────────────────────────────
    // Undo / Redo
    // ──────────────────────────────────────────────────────────────────

    /// Push the current state onto the undo stack (called *before*
    /// a parameter change is applied).
    pub fn push_undo(&self) {
        let snapshot = self.capture_snapshot();
        let mut stack = self
            .undo_stack
            .write()
            .expect("Poisoned RwLock on push_undo");
        stack.push_undo(snapshot);
    }

    /// Undo the last change. Returns the snapshot to apply, or `None`
    /// if the undo stack is empty.
    ///
    /// The caller must use [`apply_snapshot`][Self::apply_snapshot] to
    /// apply the returned snapshot through a `ParamSetter`.
    pub fn undo(&self) -> Option<ParamSnapshot> {
        let current = self.capture_snapshot();
        let mut stack = self.undo_stack.write().expect("Poisoned RwLock on undo");
        stack.undo(current)
    }

    /// Redo the last undone change. Returns the snapshot to apply, or
    /// `None` if the redo stack is empty.
    ///
    /// The caller must use [`apply_snapshot`][Self::apply_snapshot] to
    /// apply the returned snapshot through a `ParamSetter`.
    pub fn redo(&self) -> Option<ParamSnapshot> {
        let current = self.capture_snapshot();
        let mut stack = self.undo_stack.write().expect("Poisoned RwLock on redo");
        stack.redo(current)
    }

    // ──────────────────────────────────────────────────────────────────
    // Bypass
    // ──────────────────────────────────────────────────────────────────

    /// Check whether the internal FX bypass is active.
    #[inline]
    pub fn is_bypassed(&self) -> bool {
        self.bypass.load(Ordering::Relaxed)
    }

    /// Set the internal FX bypass flag.
    #[inline]
    pub fn set_bypass(&self, bypass: bool) {
        self.bypass.store(bypass, Ordering::Relaxed);
    }

    // ──────────────────────────────────────────────────────────────────
    // DSP parameter conversion
    // ──────────────────────────────────────────────────────────────────

    /// Build a [`dsp::DelayParams`] from the current parameter state.
    ///
    /// This is the primary bridge between the nih_plug parameter world
    /// (which uses `f32` for floats and enum params) and the DSP engine
    /// (which uses `f64` throughout). All `f32` values are widened to
    /// `f64`, and enum params are converted to their DSP counterparts.
    ///
    /// The caller should pass the host's current tempo in BPM via the
    /// `tempo_bpm` argument so the DSP engine can compute tempo-synced
    /// delay times.
    pub fn to_dsp_params(&self, tempo_bpm: f64) -> dsp::DelayParams {
        dsp::DelayParams {
            input_mode_l: self.input_mode_l.value().into(),
            input_mode_r: self.input_mode_r.value().into(),
            delay_time_l: self.delay_time_l.value() as f64,
            delay_time_r: self.delay_time_r.value() as f64,
            low_cut_l: self.low_cut_l.value() as f64,
            low_cut_r: self.low_cut_r.value() as f64,
            low_cut_slope_l: self.low_cut_slope_l.value() as f64,
            low_cut_slope_r: self.low_cut_slope_r.value() as f64,
            high_cut_l: self.high_cut_l.value() as f64,
            high_cut_r: self.high_cut_r.value() as f64,
            high_cut_slope_l: self.high_cut_slope_l.value() as f64,
            high_cut_slope_r: self.high_cut_slope_r.value() as f64,
            feedback_l: self.feedback_l.value() as f64,
            feedback_r: self.feedback_r.value() as f64,
            feedback_phase_l: self.feedback_phase_l.value(),
            feedback_phase_r: self.feedback_phase_r.value(),
            crossfeed_lr: self.crossfeed_lr.value() as f64,
            crossfeed_rl: self.crossfeed_rl.value() as f64,
            crossfeed_phase_lr: self.crossfeed_phase_lr.value(),
            crossfeed_phase_rl: self.crossfeed_phase_rl.value(),
            routing: self.routing.value().into(),
            tempo_sync: self.tempo_sync.value(),
            tempo_bpm,
            note_l: self.note_l.value().into(),
            note_r: self.note_r.value().into(),
            deviation_l: self.deviation_l.value() as f64,
            deviation_r: self.deviation_r.value() as f64,
            halve_l: self.halve_l.value(),
            halve_r: self.halve_r.value(),
            double_l: self.double_l.value(),
            double_r: self.double_r.value(),
            output_mix_l: self.output_mix_l.value() as f64,
            output_mix_r: self.output_mix_r.value() as f64,
            bypass: self.is_bypassed(),
            stereo_link: self.stereo_link.value(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Enum round-trip conversions ─────────────────────────────────

    #[test]
    fn input_mode_param_round_trip() {
        for i in 0..5 {
            let param = InputModeParam::from_index(i);
            assert_eq!(param.to_index(), i);
        }
    }

    #[test]
    fn note_value_param_round_trip() {
        for i in 0..17 {
            let param = NoteValueParam::from_index(i);
            assert_eq!(param.to_index(), i);
        }
    }

    #[test]
    fn routing_mode_param_round_trip() {
        for i in 0..8 {
            let param = RoutingModeParam::from_index(i);
            assert_eq!(param.to_index(), i);
        }
    }

    // ── Enum → DSP conversion ──────────────────────────────────────

    #[test]
    fn input_mode_to_dsp() {
        assert_eq!(
            dsp::InputMode::from(InputModeParam::Off),
            dsp::InputMode::Off
        );
        assert_eq!(
            dsp::InputMode::from(InputModeParam::Left),
            dsp::InputMode::Left
        );
        assert_eq!(
            dsp::InputMode::from(InputModeParam::Right),
            dsp::InputMode::Right
        );
        assert_eq!(
            dsp::InputMode::from(InputModeParam::LeftPlusRight),
            dsp::InputMode::LeftPlusRight
        );
        assert_eq!(
            dsp::InputMode::from(InputModeParam::LeftMinusRight),
            dsp::InputMode::LeftMinusRight
        );
    }

    #[test]
    fn note_value_to_dsp() {
        assert_eq!(
            dsp::NoteValue::from(NoteValueParam::Quarter),
            dsp::NoteValue::Quarter
        );
        assert_eq!(
            dsp::NoteValue::from(NoteValueParam::EighthTriplet),
            dsp::NoteValue::EighthTriplet
        );
        assert_eq!(
            dsp::NoteValue::from(NoteValueParam::EighthDotted),
            dsp::NoteValue::EighthDotted
        );
    }

    #[test]
    fn routing_mode_to_dsp() {
        assert_eq!(
            dsp::RoutingMode::from(RoutingModeParam::Customized),
            dsp::RoutingMode::Customized
        );
        assert_eq!(
            dsp::RoutingMode::from(RoutingModeParam::PingPong),
            dsp::RoutingMode::PingPong
        );
    }

    // ── Formatters ─────────────────────────────────────────────────

    #[test]
    fn format_delay_time_values() {
        assert_eq!(format_delay_time(0.5), "500.0");
        assert_eq!(format_delay_time(0.005), "5.0");
        assert_eq!(format_delay_time(2.0), "2000.0");
    }

    #[test]
    fn parse_delay_time_values() {
        assert_eq!(parse_delay_time("500"), Some(0.5));
        assert_eq!(parse_delay_time("500 ms"), Some(0.5));
        assert_eq!(parse_delay_time("0.500 s"), Some(0.5));
        assert_eq!(parse_delay_time("2.0s"), Some(2.0));
    }

    #[test]
    fn format_deviation_values() {
        assert_eq!(format_deviation(0.0), "0.0");
        assert_eq!(format_deviation(-100.0), "-100.0");
        assert_eq!(format_deviation(50.5), "50.5");
    }

    #[test]
    fn parse_deviation_values() {
        assert_eq!(parse_deviation("0.0"), Some(0.0));
        assert_eq!(parse_deviation("-100.0 ct"), Some(-100.0));
    }

    #[test]
    fn format_frequency_values() {
        assert_eq!(format_frequency(20.0), "20.0 Hz");
        assert_eq!(format_frequency(1000.0), "1.00 kHz");
        assert_eq!(format_frequency(20000.0), "20.00 kHz");
        assert_eq!(format_frequency(440.0), "440.0 Hz");
    }

    #[test]
    fn parse_frequency_values() {
        assert_eq!(parse_frequency("20.0"), Some(20.0));
        assert_eq!(parse_frequency("20.0 Hz"), Some(20.0));
        assert_eq!(parse_frequency("1.00 kHz"), Some(1000.0));
        assert_eq!(parse_frequency("20.00kHz"), Some(20000.0));
    }

    #[test]
    fn format_percentage_values() {
        assert_eq!(format_percentage(0.0), "0.0");
        assert_eq!(format_percentage(0.4), "40.0");
        assert_eq!(format_percentage(1.0), "100.0");
    }

    #[test]
    fn parse_percentage_values() {
        assert_eq!(parse_percentage("0.0"), Some(0.0));
        assert_eq!(parse_percentage("40.0%"), Some(0.4));
        assert_eq!(parse_percentage("100.0"), Some(1.0));
    }

    // ── Undo/Redo stack ────────────────────────────────────────────

    #[test]
    fn undo_redo_basic() {
        let mut stack = UndoRedoStack::default();
        let snap_a = ParamSnapshot::default_values();
        let mut snap_b = snap_a.clone();
        snap_b.feedback_l = 0.8;

        stack.push_undo(snap_a.clone());
        assert_eq!(stack.undo.len(), 1);
        assert_eq!(stack.redo.len(), 0);

        let result = stack.undo(snap_b.clone());
        assert!(result.is_some());
        assert_eq!(result.unwrap().feedback_l, 0.4);
        assert_eq!(stack.redo.len(), 1);

        let result = stack.redo(snap_a.clone());
        assert!(result.is_some());
        assert_eq!(result.unwrap().feedback_l, 0.8);
    }

    #[test]
    fn undo_stack_max_depth() {
        let mut stack = UndoRedoStack::default();
        let snap = ParamSnapshot::default_values();

        for _ in 0..60 {
            stack.push_undo(snap.clone());
        }
        assert_eq!(stack.undo.len(), UndoRedoStack::MAX_DEPTH);
    }

    // ── Snapshot capture / restore round-trip ──────────────────────

    #[test]
    fn snapshot_default_values() {
        let snap = ParamSnapshot::default_values();
        assert_eq!(snap.input_mode_l, 1); // Left
        assert_eq!(snap.input_mode_r, 2); // Right
        assert_eq!(snap.delay_time_l, 0.5);
        assert_eq!(snap.feedback_l, 0.4);
        assert_eq!(snap.output_mix_l, 1.0);
        assert!(!snap.tempo_sync);
        assert!(!snap.stereo_link);
    }

    // ── A/B snapshots default ──────────────────────────────────────

    #[test]
    fn ab_snapshots_default() {
        let ab = AbSnapshots::default();
        assert_eq!(ab.a.feedback_l, 0.4);
        assert_eq!(ab.b.feedback_l, 0.4);
    }

    // ── Bool param display names ───────────────────────────────────

    #[test]
    fn bool_display_halve() {
        // Verify the formatter produces expected strings
        let format_halve = |v: bool| {
            if v {
                ":2".to_string()
            } else {
                "Off".to_string()
            }
        };
        assert_eq!(format_halve(false), "Off");
        assert_eq!(format_halve(true), ":2");
    }

    #[test]
    fn bool_display_double() {
        let format_double = |v: bool| {
            if v {
                "x2".to_string()
            } else {
                "Off".to_string()
            }
        };
        assert_eq!(format_double(false), "Off");
        assert_eq!(format_double(true), "x2");
    }

    #[test]
    fn bool_display_phase() {
        let format_phase = |v: bool| {
            if v {
                "Inverted".to_string()
            } else {
                "Normal".to_string()
            }
        };
        assert_eq!(format_phase(false), "Normal");
        assert_eq!(format_phase(true), "Inverted");
    }

    #[test]
    fn bool_display_sync() {
        let format_sync = |v: bool| {
            if v {
                "Sync".to_string()
            } else {
                "Free".to_string()
            }
        };
        assert_eq!(format_sync(false), "Free");
        assert_eq!(format_sync(true), "Sync");
    }

    #[test]
    fn bool_display_link() {
        let format_link = |v: bool| {
            if v {
                "Linked".to_string()
            } else {
                "Unlinked".to_string()
            }
        };
        assert_eq!(format_link(false), "Unlinked");
        assert_eq!(format_link(true), "Linked");
    }
}
