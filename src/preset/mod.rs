//! Preset management module for **Nebula Stereo Delay** by Nebula Audio.
//!
//! Provides factory presets, user preset save/load, and import/export
//! functionality. All preset data is serialised as JSON for transparency
//! and forward-compatibility.
//!
//! # Factory Presets
//!
//! Ten built-in presets covering common delay use-cases, from a clean
//! initialisation state to emulations of classic hardware:
//!
//! | Preset             | Character                                            |
//! |--------------------|------------------------------------------------------|
//! | Init               | Default values — neutral starting point               |
//! | Simple Slap        | Short single-repeat slapback                         |
//! | Ambient Wash       | Long, highly-regenerative wash with low-cut           |
//! | Ping Pong          | Stereo ping-pong routing                             |
//! | Rotary             | Rotary-speaker-style modulated delay                 |
//! | Tight Doubler      | Micro-delay for double-tracking effect                |
//! | Space Echo         | Tape-echo emulation with filtered feedback            |
//! | Stereo Widener     | Crossfeed-based stereo image enhancement              |
//! | Rhythmic Delay     | Tempo-synced with triplet subdivision                 |
//! | Vintage Tape       | Band-limited feedback for vintage tape character      |
//!
//! # User Presets
//!
//! Users can save, load, and delete their own presets in a platform-
//! specific application-data directory:
//!
//! - **Linux**: `$XDG_DATA_HOME/NebulaAudio/NebulaStereoDelay/Presets`
//!   (falls back to `~/.local/share/…`)
//! - **macOS**:   `~/Library/Application Support/NebulaAudio/NebulaStereoDelay/Presets`
//! - **Windows**: `%APPDATA%\NebulaAudio\NebulaStereoDelay\Presets`
//!
//! # File Format
//!
//! Each preset file is a `.json` file containing a [`PresetData`] object.
//! The file name is derived from the preset name with unsafe filesystem
//! characters replaced by underscores.

use std::fs;
use std::path::{Path, PathBuf};

use nih_plug::params::enums::Enum;
use serde::{Deserialize, Serialize};

use crate::parameters::{
    InputModeParam, NebulaStereoDelayParams, NoteValueParam, RoutingModeParam,
};

// ═══════════════════════════════════════════════════════════════════════════
// Data Structures
// ═══════════════════════════════════════════════════════════════════════════

/// Complete preset data including metadata and all parameter values.
///
/// This is the top-level object that gets serialised to JSON when a
/// preset is saved or exported. The `version` field allows future
/// migrations if the parameter set changes.
#[derive(Serialize, Deserialize, Clone)]
pub struct PresetData {
    /// Human-readable preset name (e.g. "Ambient Wash").
    pub name: String,
    /// Author of the preset. Factory presets use "Nebula Audio".
    pub author: String,
    /// ISO 8601 creation timestamp (e.g. "2025-01-15T12:00:00Z").
    pub created: String,
    /// Preset format version, currently "1.0.0".
    pub version: String,
    /// All parameter plain values.
    pub values: PresetValues,
}

/// All parameter plain values for a single preset.
///
/// Every field stores the **plain** (un-normalised) value exactly as it
/// appears to the user — delay times are in seconds, feedback is 0.0–1.0,
/// frequencies are in Hz, and enums are stored as their variant index.
///
/// # Enum Index Mapping
///
/// | Field         | Index | Value                |
/// |---------------|-------|----------------------|
/// | `input_mode`  | 0     | Off                  |
/// | `input_mode`  | 1     | Left                 |
/// | `input_mode`  | 2     | Right                |
/// | `input_mode`  | 3     | L+R                  |
/// | `input_mode`  | 4     | L-R                  |
/// | `note`        | 0     | 1/1 (Whole)          |
/// | `note`        | 1     | 1/2 (Half)           |
/// | `note`        | 2     | 1/2T (Half Triplet)  |
/// | `note`        | 3     | 1/4 (Quarter)        |
/// | `note`        | 4     | 1/4T (Quarter T.)    |
/// | `note`        | 5     | 1/8 (Eighth)         |
/// | `note`        | 6     | 1/8T (Eighth T.)     |
/// | `note`        | 7     | 1/16 (Sixteenth)     |
/// | `note`        | 8     | 1/16T (Sixteenth T.) |
/// | `note`        | 9     | 1/32                 |
/// | `note`        | 10    | 1/32T                |
/// | `note`        | 11    | 1/64                 |
/// | `routing`     | 0     | Customized           |
/// | `routing`     | 1     | Straight             |
/// | `routing`     | 2     | Crossfeed            |
/// | `routing`     | 3     | 90/10                |
/// | `routing`     | 4     | 10/90                |
/// | `routing`     | 5     | Ping Pong            |
/// | `routing`     | 6     | Pan L/R              |
/// | `routing`     | 7     | Rotate L/R           |
#[derive(Serialize, Deserialize, Clone)]
pub struct PresetValues {
    // ── Per-Channel: Input Mode ────────────────────────────────────────
    pub input_mode_l: u8,
    pub input_mode_r: u8,

    // ── Per-Channel: Delay Time (seconds) ──────────────────────────────
    pub delay_time_l: f32,
    pub delay_time_r: f32,

    // ── Per-Channel: Tempo-Sync Note ───────────────────────────────────
    pub note_l: u8,
    pub note_r: u8,

    // ── Per-Channel: Deviation (cents) ─────────────────────────────────
    pub deviation_l: f32,
    pub deviation_r: f32,

    // ── Per-Channel: Halve / Double ────────────────────────────────────
    pub halve_l: bool,
    pub halve_r: bool,
    pub double_l: bool,
    pub double_r: bool,

    // ── Per-Channel: Filters (Hz) ──────────────────────────────────────
    pub low_cut_l: f32,
    pub low_cut_r: f32,
    #[serde(default = "default_filter_slope")]
    pub low_cut_slope_l: f32,
    #[serde(default = "default_filter_slope")]
    pub low_cut_slope_r: f32,
    pub high_cut_l: f32,
    pub high_cut_r: f32,
    #[serde(default = "default_filter_slope")]
    pub high_cut_slope_l: f32,
    #[serde(default = "default_filter_slope")]
    pub high_cut_slope_r: f32,

    // ── Per-Channel: Feedback (0.0–1.0) ────────────────────────────────
    pub feedback_l: f32,
    pub feedback_r: f32,
    pub feedback_phase_l: bool,
    pub feedback_phase_r: bool,

    // ── Crossfeed (0.0–1.0) ────────────────────────────────────────────
    pub crossfeed_lr: f32,
    pub crossfeed_rl: f32,
    pub crossfeed_phase: bool,

    // ── Global ─────────────────────────────────────────────────────────
    pub routing: u8,
    pub tempo_sync: bool,
    pub stereo_link: bool,
    pub output_mix_l: f32,
    pub output_mix_r: f32,
}

impl Default for PresetValues {
    fn default() -> Self {
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
            crossfeed_phase: false,
            routing: 0, // Customized
            tempo_sync: false,
            stereo_link: false,
            output_mix_l: 1.0,
            output_mix_r: 1.0,
        }
    }
}

fn default_filter_slope() -> f32 {
    12.0
}

// ═══════════════════════════════════════════════════════════════════════════
// Preset Manager
// ═══════════════════════════════════════════════════════════════════════════

/// Manages factory and user presets for the plugin.
///
/// On construction, the manager builds the built-in factory preset list
/// and resolves the platform-specific user-preset directory. The manager
/// is intentionally cheap to construct and holds no runtime state beyond
/// the preset data and directory path.
pub struct PresetManager {
    factory_presets: Vec<PresetData>,
    user_preset_dir: PathBuf,
}

impl PresetManager {
    /// Create a new preset manager with factory presets and a resolved
    /// user-preset directory.
    ///
    /// The user directory is created on first save; this constructor does
    /// **not** create it, so instantiation is always infallible.
    pub fn new() -> Self {
        Self {
            factory_presets: build_factory_presets(),
            user_preset_dir: resolve_user_preset_dir(),
        }
    }

    /// Return a slice of all factory presets.
    pub fn factory_presets(&self) -> &[PresetData] {
        &self.factory_presets
    }

    /// Load and return all user presets found in the user-preset directory.
    ///
    /// Invalid or unreadable files are silently skipped so that a single
    /// corrupt file does not prevent the rest from loading.
    pub fn user_presets(&self) -> Result<Vec<PresetData>, String> {
        let mut presets = Vec::new();

        if !self.user_preset_dir.exists() {
            return Ok(presets);
        }

        let entries = fs::read_dir(&self.user_preset_dir)
            .map_err(|e| format!("Failed to read user preset directory: {e}"))?;

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();

            // Only process .json files
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }

            match Self::load_preset_file(&path) {
                Ok(preset) => presets.push(preset),
                Err(_) => continue, // Skip corrupt files silently
            }
        }

        // Sort alphabetically by name for consistent UI ordering.
        presets.sort_by_key(|preset| preset.name.to_lowercase());

        Ok(presets)
    }

    /// Save the current parameter state as a user preset.
    ///
    /// The preset is written as formatted JSON to the user-preset
    /// directory. If the directory does not exist it is created. The
    /// file name is derived from the preset name with unsafe filesystem
    /// characters replaced by underscores.
    pub fn save_user_preset(
        &self,
        name: &str,
        author: &str,
        values: &PresetValues,
    ) -> Result<(), String> {
        // Ensure the user preset directory exists.
        fs::create_dir_all(&self.user_preset_dir)
            .map_err(|e| format!("Failed to create user preset directory: {e}"))?;

        let preset = PresetData {
            name: name.to_string(),
            author: author.to_string(),
            created: now_iso8601(),
            version: PRESET_VERSION.to_string(),
            values: values.clone(),
        };

        let file_path = self.user_preset_dir.join(preset_filename(name));
        let json = serde_json::to_string_pretty(&preset)
            .map_err(|e| format!("Failed to serialise preset: {e}"))?;

        fs::write(&file_path, json).map_err(|e| format!("Failed to write preset file: {e}"))?;

        Ok(())
    }

    /// Apply a preset's values to the plugin parameters.
    ///
    /// Each parameter is set via the provided [`nih_plug::prelude::ParamSetter`]
    /// so the host is notified of every change (required for automation
    /// and undo support).
    pub fn load_preset(
        &self,
        preset: &PresetData,
        params: &NebulaStereoDelayParams,
        setter: &nih_plug::prelude::ParamSetter,
    ) {
        let v = &preset.values;

        // ── Input Mode ──────────────────────────────────────────────
        setter.set_parameter(
            &params.input_mode_l,
            InputModeParam::from_index(v.input_mode_l as usize),
        );
        setter.set_parameter(
            &params.input_mode_r,
            InputModeParam::from_index(v.input_mode_r as usize),
        );

        // ── Delay Time ──────────────────────────────────────────────
        setter.set_parameter(&params.delay_time_l, v.delay_time_l);
        setter.set_parameter(&params.delay_time_r, v.delay_time_r);

        // ── Note Value ──────────────────────────────────────────────
        setter.set_parameter(
            &params.note_l,
            NoteValueParam::from_index(v.note_l as usize),
        );
        setter.set_parameter(
            &params.note_r,
            NoteValueParam::from_index(v.note_r as usize),
        );

        // ── Deviation ───────────────────────────────────────────────
        setter.set_parameter(&params.deviation_l, v.deviation_l);
        setter.set_parameter(&params.deviation_r, v.deviation_r);

        // ── Halve / Double ──────────────────────────────────────────
        setter.set_parameter(&params.halve_l, v.halve_l);
        setter.set_parameter(&params.halve_r, v.halve_r);
        setter.set_parameter(&params.double_l, v.double_l);
        setter.set_parameter(&params.double_r, v.double_r);

        // ── Filters ─────────────────────────────────────────────────
        setter.set_parameter(&params.low_cut_l, v.low_cut_l);
        setter.set_parameter(&params.low_cut_r, v.low_cut_r);
        setter.set_parameter(&params.low_cut_slope_l, v.low_cut_slope_l);
        setter.set_parameter(&params.low_cut_slope_r, v.low_cut_slope_r);
        setter.set_parameter(&params.high_cut_l, v.high_cut_l);
        setter.set_parameter(&params.high_cut_r, v.high_cut_r);
        setter.set_parameter(&params.high_cut_slope_l, v.high_cut_slope_l);
        setter.set_parameter(&params.high_cut_slope_r, v.high_cut_slope_r);

        // ── Feedback ────────────────────────────────────────────────
        setter.set_parameter(&params.feedback_l, v.feedback_l);
        setter.set_parameter(&params.feedback_r, v.feedback_r);
        setter.set_parameter(&params.feedback_phase_l, v.feedback_phase_l);
        setter.set_parameter(&params.feedback_phase_r, v.feedback_phase_r);

        // ── Crossfeed ───────────────────────────────────────────────
        setter.set_parameter(&params.crossfeed_lr, v.crossfeed_lr);
        setter.set_parameter(&params.crossfeed_rl, v.crossfeed_rl);
        setter.set_parameter(&params.crossfeed_phase, v.crossfeed_phase);

        // ── Global ──────────────────────────────────────────────────
        setter.set_parameter(
            &params.routing,
            RoutingModeParam::from_index(v.routing as usize),
        );
        setter.set_parameter(&params.tempo_sync, v.tempo_sync);
        setter.set_parameter(&params.stereo_link, v.stereo_link);
        setter.set_parameter(&params.output_mix_l, v.output_mix_l);
        setter.set_parameter(&params.output_mix_r, v.output_mix_r);
    }

    /// Delete a user preset by name.
    ///
    /// The corresponding `.json` file is removed from the user-preset
    /// directory. Returns an error if the file cannot be deleted or does
    /// not exist.
    pub fn delete_user_preset(&self, name: &str) -> Result<(), String> {
        let file_path = self.user_preset_dir.join(preset_filename(name));

        if !file_path.exists() {
            return Err(format!("Preset file not found: {}", file_path.display()));
        }

        fs::remove_file(&file_path).map_err(|e| format!("Failed to delete preset file: {e}"))?;

        Ok(())
    }

    /// Export a preset to an arbitrary file path.
    ///
    /// This is typically used for "Save As…" dialogs where the user
    /// picks a destination outside the default user-preset directory.
    pub fn export_preset(&self, preset: &PresetData, path: &Path) -> Result<(), String> {
        // Ensure the parent directory exists.
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create export directory: {e}"))?;
        }

        let json = serde_json::to_string_pretty(preset)
            .map_err(|e| format!("Failed to serialise preset: {e}"))?;

        fs::write(path, json).map_err(|e| format!("Failed to write export file: {e}"))?;

        Ok(())
    }

    /// Import a preset from an arbitrary file path.
    ///
    /// Returns the parsed [`PresetData`] without copying it into the
    /// user-preset directory. The caller can choose to save it as a
    /// user preset if desired.
    pub fn import_preset(&self, path: &Path) -> Result<PresetData, String> {
        Self::load_preset_file(path)
    }

    // ──────────────────────────────────────────────────────────────────
    // Internal helpers
    // ──────────────────────────────────────────────────────────────────

    /// Read and deserialise a preset from a JSON file.
    fn load_preset_file(path: &Path) -> Result<PresetData, String> {
        let contents = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read preset file '{}': {e}", path.display()))?;

        serde_json::from_str(&contents)
            .map_err(|e| format!("Failed to parse preset file '{}': {e}", path.display()))
    }
}

impl Default for PresetManager {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════════════════════

/// Current preset format version.
const PRESET_VERSION: &str = "1.0.0";

/// Factory preset author string.
const FACTORY_AUTHOR: &str = "Nebula Audio";

/// Factory preset creation timestamp (epoch start for built-in presets).
const FACTORY_CREATED: &str = "2025-01-01T00:00:00Z";

// ═══════════════════════════════════════════════════════════════════════════
// Factory Presets
// ═══════════════════════════════════════════════════════════════════════════

/// Build the full list of factory presets with musically meaningful values.
fn build_factory_presets() -> Vec<PresetData> {
    vec![
        // ── 1. Init ────────────────────────────────────────────────────
        PresetData {
            name: "Init".to_string(),
            author: FACTORY_AUTHOR.to_string(),
            created: FACTORY_CREATED.to_string(),
            version: PRESET_VERSION.to_string(),
            values: PresetValues {
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
                crossfeed_phase: false,
                routing: 0, // Customized
                tempo_sync: false,
                stereo_link: false,
                output_mix_l: 1.0,
                output_mix_r: 1.0,
            },
        },
        // ── 2. Simple Slap ─────────────────────────────────────────────
        // Classic slapback echo: short delay with zero feedback for a
        // single distinct repeat. Slightly different L/R times add a
        // touch of width without sounding "doubled".
        PresetData {
            name: "Simple Slap".to_string(),
            author: FACTORY_AUTHOR.to_string(),
            created: FACTORY_CREATED.to_string(),
            version: PRESET_VERSION.to_string(),
            values: PresetValues {
                input_mode_l: 1,     // Left
                input_mode_r: 2,     // Right
                delay_time_l: 0.060, // 60 ms
                delay_time_r: 0.080, // 80 ms — offset for width
                note_l: 3,
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
                feedback_l: 0.0, // No feedback — single repeat only
                feedback_r: 0.0,
                feedback_phase_l: false,
                feedback_phase_r: false,
                crossfeed_lr: 0.0,
                crossfeed_rl: 0.0,
                crossfeed_phase: false,
                routing: 1, // Straight
                tempo_sync: false,
                stereo_link: false,
                output_mix_l: 0.7, // Wet level below unity for subtle effect
                output_mix_r: 0.7,
            },
        },
        // ── 3. Ambient Wash ────────────────────────────────────────────
        // Long, lush delay with high feedback for slowly decaying
        // repeats that blend into a wash of sound. Low-cut removes
        // rumble build-up from regenerative feedback. High-cut tames
        // harshness. Crossfeed adds stereo depth.
        PresetData {
            name: "Ambient Wash".to_string(),
            author: FACTORY_AUTHOR.to_string(),
            created: FACTORY_CREATED.to_string(),
            version: PRESET_VERSION.to_string(),
            values: PresetValues {
                input_mode_l: 3, // L+R (mono sum for even wash)
                input_mode_r: 3, // L+R
                delay_time_l: 1.2,
                delay_time_r: 1.6, // Offset creates cascading repeats
                note_l: 1,         // Half note
                note_r: 1,
                deviation_l: 8.0, // Slight detune for richness
                deviation_r: -5.0,
                halve_l: false,
                halve_r: false,
                double_l: false,
                double_r: false,
                low_cut_l: 250.0, // Aggressive low-cut to prevent mud
                low_cut_r: 250.0,
                low_cut_slope_l: 12.0,
                low_cut_slope_r: 12.0,
                high_cut_l: 6000.0, // Soft high-cut for warmth
                high_cut_r: 6000.0,
                high_cut_slope_l: 12.0,
                high_cut_slope_r: 12.0,
                feedback_l: 0.72, // High feedback — long tail
                feedback_r: 0.72,
                feedback_phase_l: false,
                feedback_phase_r: false,
                crossfeed_lr: 0.3, // Crossfeed for spatial depth
                crossfeed_rl: 0.3,
                crossfeed_phase: false,
                routing: 0, // Customized (crossfeed via manual amounts)
                tempo_sync: true,
                stereo_link: false,
                output_mix_l: 0.85,
                output_mix_r: 0.85,
            },
        },
        // ── 4. Ping Pong ───────────────────────────────────────────────
        // Classic ping-pong delay where each repeat bounces L↔R.
        // The Ping Pong routing mode routes all feedback to the
        // opposite channel, creating an alternating stereo pattern.
        // Slightly different L/R delay times add rhythmic interest.
        PresetData {
            name: "Ping Pong".to_string(),
            author: FACTORY_AUTHOR.to_string(),
            created: FACTORY_CREATED.to_string(),
            version: PRESET_VERSION.to_string(),
            values: PresetValues {
                input_mode_l: 3,     // L+R (both channels feed both delay lines)
                input_mode_r: 3,     // L+R
                delay_time_l: 0.375, // Dotted eighth feel (3/8 note)
                delay_time_r: 0.375,
                note_l: 5, // Eighth
                note_r: 5,
                deviation_l: 0.0,
                deviation_r: 0.0,
                halve_l: false,
                halve_r: false,
                double_l: false,
                double_r: false,
                low_cut_l: 100.0, // Gentle low-cut
                low_cut_r: 100.0,
                low_cut_slope_l: 12.0,
                low_cut_slope_r: 12.0,
                high_cut_l: 10000.0,
                high_cut_r: 10000.0,
                high_cut_slope_l: 12.0,
                high_cut_slope_r: 12.0,
                feedback_l: 0.55,
                feedback_r: 0.55,
                feedback_phase_l: false,
                feedback_phase_r: false,
                crossfeed_lr: 0.0, // Not used in Ping Pong routing
                crossfeed_rl: 0.0,
                crossfeed_phase: false,
                routing: 5, // Ping Pong
                tempo_sync: true,
                stereo_link: true,
                output_mix_l: 0.8,
                output_mix_r: 0.8,
            },
        },
        // ── 5. Rotary ──────────────────────────────────────────────────
        // Rotary-speaker-inspired delay using the Rotate routing mode.
        // The LFO in the Rotate mode modulates the stereo panning of
        // the wet signal. Longer delay times with moderate feedback
        // let the rotation effect develop over time.
        PresetData {
            name: "Rotary".to_string(),
            author: FACTORY_AUTHOR.to_string(),
            created: FACTORY_CREATED.to_string(),
            version: PRESET_VERSION.to_string(),
            values: PresetValues {
                input_mode_l: 1, // Left
                input_mode_r: 2, // Right
                delay_time_l: 0.5,
                delay_time_r: 0.5,
                note_l: 3, // Quarter
                note_r: 3,
                deviation_l: 5.0, // Subtle detune for rotary character
                deviation_r: -5.0,
                halve_l: false,
                halve_r: false,
                double_l: false,
                double_r: false,
                low_cut_l: 80.0,
                low_cut_r: 80.0,
                low_cut_slope_l: 12.0,
                low_cut_slope_r: 12.0,
                high_cut_l: 8000.0, // Band-limited for speaker simulation
                high_cut_r: 8000.0,
                high_cut_slope_l: 12.0,
                high_cut_slope_r: 12.0,
                feedback_l: 0.5,
                feedback_r: 0.5,
                feedback_phase_l: false,
                feedback_phase_r: false,
                crossfeed_lr: 0.0, // Not used in Rotate routing
                crossfeed_rl: 0.0,
                crossfeed_phase: false,
                routing: 7, // Rotate
                tempo_sync: false,
                stereo_link: true,
                output_mix_l: 0.75,
                output_mix_r: 0.75,
            },
        },
        // ── 6. Tight Doubler ───────────────────────────────────────────
        // Extremely short delay times that simulate double-tracking.
        // The human ear cannot resolve the delay as a distinct echo
        // below ~30 ms; instead, it perceives a thickening / widening
        // of the sound. Minimal feedback keeps the effect clean.
        PresetData {
            name: "Tight Doubler".to_string(),
            author: FACTORY_AUTHOR.to_string(),
            created: FACTORY_CREATED.to_string(),
            version: PRESET_VERSION.to_string(),
            values: PresetValues {
                input_mode_l: 1,     // Left
                input_mode_r: 2,     // Right
                delay_time_l: 0.012, // 12 ms — below Haas zone threshold
                delay_time_r: 0.024, // 24 ms — still imperceptible as echo
                note_l: 11,          // 1/64 (shortest available)
                note_r: 11,
                deviation_l: 15.0, // Slight pitch drift for realism
                deviation_r: -12.0,
                halve_l: false,
                halve_r: false,
                double_l: false,
                double_r: false,
                low_cut_l: 80.0, // Remove sub build-up
                low_cut_r: 80.0,
                low_cut_slope_l: 12.0,
                low_cut_slope_r: 12.0,
                high_cut_l: 12000.0,
                high_cut_r: 12000.0,
                high_cut_slope_l: 12.0,
                high_cut_slope_r: 12.0,
                feedback_l: 0.0, // No feedback — keep it tight
                feedback_r: 0.0,
                feedback_phase_l: false,
                feedback_phase_r: false,
                crossfeed_lr: 0.15, // Subtle crossfeed for cohesion
                crossfeed_rl: 0.15,
                crossfeed_phase: false,
                routing: 0, // Customized
                tempo_sync: false,
                stereo_link: false,
                output_mix_l: 0.6, // Mix below unity — augment, don't replace
                output_mix_r: 0.6,
            },
        },
        // ── 7. Space Echo ──────────────────────────────────────────────
        // Emulation of the classic Roland Space Echo tape delay.
        // Tape echoes are characterised by:
        //   - Band-limited repeats (tape rolls off highs and lows)
        //   - Moderate feedback with gradual decay
        //   - Slight pitch instability from tape transport
        //   - Warm, saturated character
        PresetData {
            name: "Space Echo".to_string(),
            author: FACTORY_AUTHOR.to_string(),
            created: FACTORY_CREATED.to_string(),
            version: PRESET_VERSION.to_string(),
            values: PresetValues {
                input_mode_l: 1,     // Left
                input_mode_r: 2,     // Right
                delay_time_l: 0.350, // 350 ms — typical tape echo range
                delay_time_r: 0.420, // Offset for stereo tape head spacing
                note_l: 5,           // Eighth
                note_r: 5,
                deviation_l: 7.0, // Tape wow/flutter simulation
                deviation_r: -4.0,
                halve_l: false,
                halve_r: false,
                double_l: false,
                double_r: false,
                low_cut_l: 120.0, // Tape rolls off low end
                low_cut_r: 120.0,
                low_cut_slope_l: 12.0,
                low_cut_slope_r: 12.0,
                high_cut_l: 4500.0, // Tape significantly rolls off highs
                high_cut_r: 4500.0,
                high_cut_slope_l: 12.0,
                high_cut_slope_r: 12.0,
                feedback_l: 0.55, // Moderate feedback for trailing repeats
                feedback_r: 0.55,
                feedback_phase_l: false,
                feedback_phase_r: false,
                crossfeed_lr: 0.1, // Subtle crossfeed between tape heads
                crossfeed_rl: 0.1,
                crossfeed_phase: false,
                routing: 0, // Customized
                tempo_sync: true,
                stereo_link: false,
                output_mix_l: 0.75,
                output_mix_r: 0.75,
            },
        },
        // ── 8. Stereo Widener ──────────────────────────────────────────
        // Uses crossfeed to blend a small amount of each channel's
        // delayed signal into the opposite channel, creating a
        // perceived widening of the stereo image. Very short delay
        // times keep the effect subtle. Inverted crossfeed phase
        // enhances the out-of-phase widening effect.
        PresetData {
            name: "Stereo Widener".to_string(),
            author: FACTORY_AUTHOR.to_string(),
            created: FACTORY_CREATED.to_string(),
            version: PRESET_VERSION.to_string(),
            values: PresetValues {
                input_mode_l: 1,     // Left
                input_mode_r: 2,     // Right
                delay_time_l: 0.020, // 20 ms — within Haas fusion zone
                delay_time_r: 0.025, // 25 ms — asymmetric for natural width
                note_l: 11,          // 1/64
                note_r: 11,
                deviation_l: 0.0,
                deviation_r: 0.0,
                halve_l: false,
                halve_r: false,
                double_l: false,
                double_r: false,
                low_cut_l: 120.0, // Remove low-frequency mono content
                low_cut_r: 120.0,
                low_cut_slope_l: 12.0,
                low_cut_slope_r: 12.0,
                high_cut_l: 9000.0, // Gentle high-cut for smooth blend
                high_cut_r: 9000.0,
                high_cut_slope_l: 12.0,
                high_cut_slope_r: 12.0,
                feedback_l: 0.15, // Low feedback — just a hint of repeat
                feedback_r: 0.15,
                feedback_phase_l: false,
                feedback_phase_r: false,
                crossfeed_lr: 0.4, // Significant crossfeed for width
                crossfeed_rl: 0.4,
                crossfeed_phase: true, // Inverted phase enhances stereo width
                routing: 0,            // Customized
                tempo_sync: false,
                stereo_link: false,
                output_mix_l: 0.65,
                output_mix_r: 0.65,
            },
        },
        // ── 9. Rhythmic Delay ──────────────────────────────────────────
        // Tempo-synced delay with a triplet feel for creating
        // rhythmic patterns. The left channel uses a dotted-eighth
        // feel while the right uses quarter-note triplets, producing
        // polyrhythmic interplay. The 1/4T note gives a classic
        // dotted-eighth delay sound when used with timing offsets.
        PresetData {
            name: "Rhythmic Delay".to_string(),
            author: FACTORY_AUTHOR.to_string(),
            created: FACTORY_CREATED.to_string(),
            version: PRESET_VERSION.to_string(),
            values: PresetValues {
                input_mode_l: 3,   // L+R (mono input for even rhythmic pattern)
                input_mode_r: 3,   // L+R
                delay_time_l: 0.5, // Overridden by tempo sync + note value
                delay_time_r: 0.5,
                note_l: 6, // 1/8T — triplet eighth (dotted feel)
                note_r: 4, // 1/4T — triplet quarter
                deviation_l: 0.0,
                deviation_r: 0.0,
                halve_l: false,
                halve_r: false,
                double_l: false,
                double_r: false,
                low_cut_l: 150.0, // Cut lows to keep rhythm tight
                low_cut_r: 150.0,
                low_cut_slope_l: 12.0,
                low_cut_slope_r: 12.0,
                high_cut_l: 7000.0,
                high_cut_r: 7000.0,
                high_cut_slope_l: 12.0,
                high_cut_slope_r: 12.0,
                feedback_l: 0.5, // Enough repeats to establish the rhythm
                feedback_r: 0.45,
                feedback_phase_l: false,
                feedback_phase_r: false,
                crossfeed_lr: 0.15, // Slight crossfeed for cohesion
                crossfeed_rl: 0.15,
                crossfeed_phase: false,
                routing: 3,       // 90/10 — mostly self-feedback with hint of cross
                tempo_sync: true, // Essential for rhythmic use
                stereo_link: false,
                output_mix_l: 0.7,
                output_mix_r: 0.7,
            },
        },
        // ── 10. Vintage Tape ───────────────────────────────────────────
        // Heavily band-limited feedback simulating the frequency
        // response of aging tape. Lower high-cut than Space Echo for
        // a darker, more lo-fi character. Higher low-cut removes
        // tape rumble. Moderate feedback with the 90/10 routing
        // gives a warm, characterful repeat pattern.
        PresetData {
            name: "Vintage Tape".to_string(),
            author: FACTORY_AUTHOR.to_string(),
            created: FACTORY_CREATED.to_string(),
            version: PRESET_VERSION.to_string(),
            values: PresetValues {
                input_mode_l: 1,     // Left
                input_mode_r: 2,     // Right
                delay_time_l: 0.280, // 280 ms
                delay_time_r: 0.310, // Slight offset for analog feel
                note_l: 5,           // Eighth
                note_r: 5,
                deviation_l: 12.0, // More wow than Space Echo
                deviation_r: -9.0,
                halve_l: false,
                halve_r: false,
                double_l: false,
                double_r: false,
                low_cut_l: 200.0, // Aggressive low-cut — old tape has no subs
                low_cut_r: 200.0,
                low_cut_slope_l: 12.0,
                low_cut_slope_r: 12.0,
                high_cut_l: 3000.0, // Dark — aged tape rolls off highs severely
                high_cut_r: 3000.0,
                high_cut_slope_l: 12.0,
                high_cut_slope_r: 12.0,
                feedback_l: 0.6, // Moderate-high feedback for decaying tape repeats
                feedback_r: 0.6,
                feedback_phase_l: false,
                feedback_phase_r: false,
                crossfeed_lr: 0.0,
                crossfeed_rl: 0.0,
                crossfeed_phase: false,
                routing: 3, // 90/10 — mostly straight with subtle cross
                tempo_sync: true,
                stereo_link: false,
                output_mix_l: 0.75,
                output_mix_r: 0.75,
            },
        },
    ]
}

// ═══════════════════════════════════════════════════════════════════════════
// Path Helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Resolve the platform-specific user-preset directory.
///
/// | Platform | Path                                                              |
/// |----------|-------------------------------------------------------------------|
/// | Linux    | `$XDG_DATA_HOME/NebulaAudio/NebulaStereoDelay/Presets`            |
/// |          | or `~/.local/share/NebulaAudio/NebulaStereoDelay/Presets`         |
/// | macOS    | `~/Library/Application Support/NebulaAudio/NebulaStereoDelay/Presets` |
/// | Windows  | `%APPDATA%\NebulaAudio\NebulaStereoDelay\Presets`                 |
fn resolve_user_preset_dir() -> PathBuf {
    let base = if cfg!(target_os = "macos") {
        // macOS: ~/Library/Application Support/
        dirs_data_home_macos()
    } else if cfg!(target_os = "windows") {
        // Windows: %APPDATA% or %LOCALAPPDATA%
        dirs_data_home_windows()
    } else {
        // Linux / BSD / other: XDG_DATA_HOME or ~/.local/share
        dirs_data_home_xdg()
    };

    base.join("NebulaAudio")
        .join("NebulaStereoDelay")
        .join("Presets")
}

/// Return the macOS Application Support directory for the current user.
fn dirs_data_home_macos() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
    } else {
        PathBuf::from("/tmp/NebulaAudio") // Fallback for unusual environments
    }
}

/// Return the Windows %APPDATA% directory.
fn dirs_data_home_windows() -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        PathBuf::from(appdata)
    } else if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        PathBuf::from(local)
    } else {
        PathBuf::from("/tmp/NebulaAudio") // Fallback for unusual environments
    }
}

/// Return the XDG data home directory (Linux / BSD).
///
/// Uses `$XDG_DATA_HOME` if set, otherwise falls back to
/// `~/.local/share`.
fn dirs_data_home_xdg() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(xdg)
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".local").join("share")
    } else {
        PathBuf::from("/tmp/NebulaAudio") // Fallback for unusual environments
    }
}

/// Convert a preset name into a safe filename (`.json` extension added).
///
/// Characters that are problematic on common filesystems (slashes,
/// backslashes, colons, etc.) are replaced with underscores. Whitespace
/// is preserved for readability.
fn preset_filename(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c == '/'
                || c == '\\'
                || c == ':'
                || c == '*'
                || c == '?'
                || c == '"'
                || c == '<'
                || c == '>'
                || c == '|'
                || c == '\0'
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    format!("{safe}.json")
}

/// Return the current UTC time as an ISO 8601 string.
///
/// Uses a simple manual format to avoid depending on the `chrono` or
/// `time` crates. The format is `YYYY-MM-DDThh:mm:ssZ`.
fn now_iso8601() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::ZERO);

    let total_secs = duration.as_secs();

    // Days from epoch
    let mut remaining = total_secs;
    let mut year: u32 = 1970;
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        let secs_in_year = days_in_year as u64 * 86_400;
        if remaining < secs_in_year {
            break;
        }
        remaining -= secs_in_year;
        year += 1;
    }

    let days_before_month = if is_leap_year(year) {
        [0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335]
    } else {
        [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334]
    };

    let day_of_year = (remaining / 86_400) as u32;
    remaining %= 86_400;

    let (month, day) = day_of_year_to_month_day(day_of_year, &days_before_month);

    let hour = (remaining / 3600) as u32;
    remaining %= 3600;
    let minute = (remaining / 60) as u32;
    let second = (remaining % 60) as u32;

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Check if a year is a leap year (Gregorian calendar).
fn is_leap_year(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Convert a zero-based day-of-year into month (1–12) and day (1–31).
fn day_of_year_to_month_day(day_of_year: u32, days_before_month: &[u32; 12]) -> (u32, u32) {
    for (i, &start) in days_before_month.iter().enumerate() {
        let next = if i + 1 < 12 {
            days_before_month[i + 1]
        } else {
            366
        };
        if day_of_year < next {
            return ((i as u32) + 1, day_of_year - start + 1);
        }
    }
    // Fallback (should not happen for valid day_of_year)
    (12, 31)
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_preset_count() {
        let manager = PresetManager::new();
        assert_eq!(manager.factory_presets().len(), 10);
    }

    #[test]
    fn factory_preset_names() {
        let manager = PresetManager::new();
        let names: Vec<&str> = manager
            .factory_presets()
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "Init",
                "Simple Slap",
                "Ambient Wash",
                "Ping Pong",
                "Rotary",
                "Tight Doubler",
                "Space Echo",
                "Stereo Widener",
                "Rhythmic Delay",
                "Vintage Tape",
            ]
        );
    }

    #[test]
    fn factory_presets_have_consistent_metadata() {
        let manager = PresetManager::new();
        for preset in manager.factory_presets() {
            assert_eq!(preset.author, FACTORY_AUTHOR);
            assert_eq!(preset.version, PRESET_VERSION);
            assert!(!preset.name.is_empty());
        }
    }

    #[test]
    fn init_preset_matches_defaults() {
        let manager = PresetManager::new();
        let init = &manager.factory_presets()[0];
        let defaults = PresetValues::default();
        let v = &init.values;

        assert_eq!(v.input_mode_l, defaults.input_mode_l);
        assert_eq!(v.input_mode_r, defaults.input_mode_r);
        assert!((v.delay_time_l - defaults.delay_time_l).abs() < f32::EPSILON);
        assert!((v.delay_time_r - defaults.delay_time_r).abs() < f32::EPSILON);
        assert_eq!(v.feedback_l, defaults.feedback_l);
        assert_eq!(v.feedback_r, defaults.feedback_r);
        assert!((v.output_mix_l - defaults.output_mix_l).abs() < f32::EPSILON);
        assert!((v.output_mix_r - defaults.output_mix_r).abs() < f32::EPSILON);
    }

    #[test]
    fn simple_slap_has_no_feedback() {
        let manager = PresetManager::new();
        let slap = manager
            .factory_presets()
            .iter()
            .find(|p| p.name == "Simple Slap")
            .unwrap();
        assert_eq!(slap.values.feedback_l, 0.0);
        assert_eq!(slap.values.feedback_r, 0.0);
    }

    #[test]
    fn ping_pong_uses_correct_routing() {
        let manager = PresetManager::new();
        let pp = manager
            .factory_presets()
            .iter()
            .find(|p| p.name == "Ping Pong")
            .unwrap();
        assert_eq!(pp.values.routing, 5); // PingPong
    }

    #[test]
    fn rotary_uses_correct_routing() {
        let manager = PresetManager::new();
        let rot = manager
            .factory_presets()
            .iter()
            .find(|p| p.name == "Rotary")
            .unwrap();
        assert_eq!(rot.values.routing, 7); // Rotate
    }

    #[test]
    fn rhythmic_delay_is_tempo_synced() {
        let manager = PresetManager::new();
        let rd = manager
            .factory_presets()
            .iter()
            .find(|p| p.name == "Rhythmic Delay")
            .unwrap();
        assert!(rd.values.tempo_sync);
    }

    #[test]
    fn stereo_widener_has_inverted_crossfeed() {
        let manager = PresetManager::new();
        let sw = manager
            .factory_presets()
            .iter()
            .find(|p| p.name == "Stereo Widener")
            .unwrap();
        assert!(sw.values.crossfeed_phase);
        assert!(sw.values.crossfeed_lr > 0.0);
        assert!(sw.values.crossfeed_rl > 0.0);
    }

    #[test]
    fn tight_doubler_short_delay() {
        let manager = PresetManager::new();
        let td = manager
            .factory_presets()
            .iter()
            .find(|p| p.name == "Tight Doubler")
            .unwrap();
        // Both delays should be well under 30 ms (Haas zone)
        assert!(td.values.delay_time_l < 0.030);
        assert!(td.values.delay_time_r < 0.030);
    }

    #[test]
    fn ambient_wash_has_low_cut() {
        let manager = PresetManager::new();
        let aw = manager
            .factory_presets()
            .iter()
            .find(|p| p.name == "Ambient Wash")
            .unwrap();
        assert!(aw.values.low_cut_l > 100.0);
        assert!(aw.values.low_cut_r > 100.0);
        assert!(aw.values.feedback_l > 0.6); // High feedback
    }

    #[test]
    fn vintage_tape_is_band_limited() {
        let manager = PresetManager::new();
        let vt = manager
            .factory_presets()
            .iter()
            .find(|p| p.name == "Vintage Tape")
            .unwrap();
        assert!(vt.values.high_cut_l < 5000.0); // Dark
        assert!(vt.values.low_cut_l > 100.0); // Low-cut
    }

    #[test]
    fn preset_filename_sanitises_characters() {
        assert_eq!(preset_filename("My Preset"), "My Preset.json");
        assert_eq!(preset_filename("A/B:C*D"), "A_B_C_D.json");
        assert_eq!(preset_filename("Test<|>"), "Test___.json");
    }

    #[test]
    fn preset_data_roundtrip_json() {
        let manager = PresetManager::new();
        let original = &manager.factory_presets()[3]; // Ping Pong

        let json = serde_json::to_string_pretty(original).unwrap();
        let deserialized: PresetData = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, original.name);
        assert_eq!(deserialized.author, original.author);
        assert_eq!(deserialized.version, original.version);
        assert_eq!(deserialized.values.routing, original.values.routing);
        assert!((deserialized.values.feedback_l - original.values.feedback_l).abs() < f32::EPSILON);
    }

    #[test]
    fn save_and_load_user_preset() {
        let dir = std::env::temp_dir().join("nebula_preset_test_save_load");
        let _ = fs::remove_dir_all(&dir);

        // Manually construct a PresetManager with a temp directory.
        let manager = PresetManager {
            factory_presets: build_factory_presets(),
            user_preset_dir: dir.clone(),
        };

        let values = PresetValues {
            delay_time_l: 0.123,
            feedback_l: 0.78,
            ..PresetValues::default()
        };

        manager
            .save_user_preset("Test Preset", "Unit Test", &values)
            .unwrap();

        let loaded = manager.user_presets().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "Test Preset");
        assert_eq!(loaded[0].author, "Unit Test");
        assert!((loaded[0].values.delay_time_l - 0.123).abs() < f32::EPSILON);
        assert!((loaded[0].values.feedback_l - 0.78).abs() < f32::EPSILON);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_user_preset() {
        let dir = std::env::temp_dir().join("nebula_preset_test_delete");
        let _ = fs::remove_dir_all(&dir);

        let manager = PresetManager {
            factory_presets: build_factory_presets(),
            user_preset_dir: dir.clone(),
        };

        let values = PresetValues::default();
        manager
            .save_user_preset("To Delete", "Test", &values)
            .unwrap();

        assert_eq!(manager.user_presets().unwrap().len(), 1);

        manager.delete_user_preset("To Delete").unwrap();
        assert_eq!(manager.user_presets().unwrap().len(), 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_and_import_preset() {
        let dir = std::env::temp_dir().join("nebula_preset_test_export");
        let _ = fs::remove_dir_all(&dir);

        let manager = PresetManager {
            factory_presets: build_factory_presets(),
            user_preset_dir: dir.join("user"),
        };

        let preset = manager.factory_presets()[6].clone(); // Space Echo
        let export_path = dir.join("exported").join("space_echo.json");

        manager.export_preset(&preset, &export_path).unwrap();

        let imported = manager.import_preset(&export_path).unwrap();
        assert_eq!(imported.name, "Space Echo");
        assert!((imported.values.high_cut_l - 4500.0).abs() < f32::EPSILON);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn user_presets_sorted_alphabetically() {
        let dir = std::env::temp_dir().join("nebula_preset_test_sort");
        let _ = fs::remove_dir_all(&dir);

        let manager = PresetManager {
            factory_presets: build_factory_presets(),
            user_preset_dir: dir.clone(),
        };

        let values = PresetValues::default();
        manager.save_user_preset("Zebra", "Test", &values).unwrap();
        manager.save_user_preset("Alpha", "Test", &values).unwrap();
        manager.save_user_preset("Middle", "Test", &values).unwrap();

        let loaded = manager.user_presets().unwrap();
        let names: Vec<&str> = loaded.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "Middle", "Zebra"]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_nonexistent_preset_fails() {
        let dir = std::env::temp_dir().join("nebula_preset_test_delete_fail");
        let _ = fs::remove_dir_all(&dir);

        let manager = PresetManager {
            factory_presets: build_factory_presets(),
            user_preset_dir: dir.clone(),
        };

        assert!(manager.delete_user_preset("DoesNotExist").is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn now_iso8601_format() {
        let ts = now_iso8601();
        // Verify the general format: YYYY-MM-DDThh:mm:ssZ
        assert!(ts.len() == 20, "Expected 20-char ISO 8601, got: {ts}");
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[7..8], "-");
        assert_eq!(&ts[10..11], "T");
        assert_eq!(&ts[13..14], ":");
        assert_eq!(&ts[16..17], ":");
        assert_eq!(&ts[19..20], "Z");
    }

    #[test]
    fn leap_year_logic() {
        assert!(is_leap_year(2000));
        assert!(!is_leap_year(1900));
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(2023));
    }
}
