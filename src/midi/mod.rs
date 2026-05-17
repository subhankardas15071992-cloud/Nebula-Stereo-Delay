//! MIDI Learn module for **Nebula Stereo Delay** by Nebula Audio.
//!
//! This module implements MIDI CC (Continuous Controller) learning and mapping
//! for plugin parameters. The design is strictly lock-free on the audio thread
//! path, using atomic operations exclusively — no mutexes, no allocations, and
//! no syscalls on the real-time audio path.
//!
//! # Architecture
//!
//! The module is split into two core types that serve fundamentally different
//! threading domains:
//!
//! - [`MidiRuntime`]: A **lock-free** runtime map of CC → target associations
//!   and target values. The audio callback writes incoming CCs into this map
//!   with atomics only, while the GUI drains dirty target values into real
//!   plugin parameters so the host editor and custom controls stay in sync.
//!
//! - [`MidiCcValues`]: A lower-level lock-free CC value store used by the legacy
//!   learning tests and kept as a small, reusable atomic primitive.
//!
//! - [`MidiLearnState`]: The learning/mapping configuration state, accessed
//!   **only** from the GUI/main thread. Manages which parameters are mapped to
//!   which CC numbers, and handles the learn/rollback workflow. Because this
//!   struct is never accessed from the audio thread, it can freely use
//!   non-atomic types like `Vec` and `HashSet`.
//!
//! # Lock-Free Design Rationale
//!
//! In a professional audio plugin, the audio processing callback runs on a
//! real-time thread at high priority. Traditional mutexes or read-write locks
//! can cause **priority inversion**: if the audio thread blocks waiting for a
//! lock held by a lower-priority thread (e.g., the GUI), the scheduler may
//! not promote the lock-holder quickly enough, causing a buffer underrun
//! (audio glitch). Even lock-free queues can introduce unbounded latency if
//! the producer thread is preempted at the wrong moment.
//!
//! This module avoids all such issues by:
//!
//! 1. **Separating concerns**: The audio thread only reads from [`MidiCcValues`],
//!    which uses simple atomics — no CAS loops, no spin-waiting, no retry
//!    logic. A single `swap` + `load` is all that's needed.
//!
//! 2. **Single-writer, single-reader per slot**: Each CC slot has exactly one
//!    writer (the MIDI input handler) and one reader (the audio thread). The
//!    dirty-flag pattern eliminates the need for sequence locks or epoch-based
//!    reclamation.
//!
//! 3. **Latest-value semantics**: MIDI CCs are continuously sent (e.g., a
//!    physical knob generates many CC messages per second). Missing a single
//!    value is acceptable because the next one will arrive shortly. This
//!    allows us to use a simple store/load instead of a queue.
//!
//! # Memory Ordering
//!
//! The writer (MIDI input handler) performs:
//! 1. `store(value_bits, Release)` on the value atomic
//! 2. `store(true, Release)` on the dirty flag
//!
//! The reader (audio thread) performs:
//! 1. `swap(false, AcqRel)` on the dirty flag
//! 2. `load(Acquire)` on the value atomic (only if dirty was true)
//!
//! The Release-Acquire pair on the dirty flag guarantees: if the reader
//! observes `dirty == true`, it will also observe the value that was written
//! *before* the dirty flag was set. The `AcqRel` ordering on the swap ensures
//! both acquire (to see the writer's stores) and release (so the writer can
//! see our flag-clear on the next round).
//!
//! # Usage
//!
//! ```text
//! // Initialization (main thread):
//! let cc_values = Arc::new(MidiCcValues::new());
//! let learn_state = MidiLearnState::new();
//!
//! // MIDI input handler:
//! cc_values.set(channel, cc, normalized_value);
//! // Or, for learn mode:
//! learn_state.process_cc(channel, cc, midi_value, &cc_values);
//!
//! // Audio thread:
//! if let Some(value) = cc_values.get_and_clear(channel, cc) {
//!     param.set_normalized_value(value);
//! }
//!
//! // GUI right-click context menu:
//! learn_state.start_learn("delay_time");  // Begin learning
//! learn_state.toggle_midi("delay_time");  // Enable/disable MIDI
//! learn_state.clean_up("delay_time");     // Remove mapping
//! learn_state.save_for_rollback();        // Snapshot for rollback
//! learn_state.roll_back();                // Restore snapshot
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU8, Ordering};

// ═══════════════════════════════════════════════════════════════════════════
// MidiMapping
// ═══════════════════════════════════════════════════════════════════════════

/// A single MIDI CC → Parameter mapping.
///
/// Each mapping associates a MIDI CC number on a specific channel with a
/// plugin parameter identified by its `param_id` string (matching nih_plug's
/// parameter ID convention).
///
/// # Invariants
///
/// - `cc` is in the range 0–127 (7-bit MIDI CC number).
/// - `channel` is in the range 0–15 (4-bit MIDI channel).
/// - Within a [`MidiLearnState`], no two mappings share the same
///   `(channel, cc)` pair, and no two mappings share the same `param_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidiMapping {
    /// MIDI CC number (0–127).
    pub cc: u8,
    /// MIDI channel (0–15).
    pub channel: u8,
    /// nih_plug parameter ID this CC controls.
    pub param_id: String,
}

// ═══════════════════════════════════════════════════════════════════════════
// MidiCcValues — Lock-Free CC Value Store
// ═══════════════════════════════════════════════════════════════════════════

/// Total number of CC slots: 16 MIDI channels × 128 CC numbers.
const CC_SLOT_COUNT: usize = 16 * 128; // 2048

/// Number of MIDI-controllable plugin targets.
pub const MIDI_TARGET_COUNT: usize = 35;

const NO_TARGET: u16 = u16::MAX;

/// Stable MIDI-controllable target IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum MidiTarget {
    InputModeL,
    InputModeR,
    DelayTimeL,
    DelayTimeR,
    NoteL,
    NoteR,
    DeviationL,
    DeviationR,
    HalveL,
    HalveR,
    DoubleL,
    DoubleR,
    LowCutL,
    LowCutR,
    LowCutSlopeL,
    LowCutSlopeR,
    HighCutL,
    HighCutR,
    HighCutSlopeL,
    HighCutSlopeR,
    FeedbackL,
    FeedbackR,
    FeedbackPhaseL,
    FeedbackPhaseR,
    CrossfeedLr,
    CrossfeedRl,
    CrossfeedPhaseLr,
    CrossfeedPhaseRl,
    Routing,
    TempoSync,
    StereoLink,
    OutputMixL,
    OutputMixR,
    Oversampling,
    Bypass,
}

impl MidiTarget {
    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }

    #[inline]
    pub fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::InputModeL),
            1 => Some(Self::InputModeR),
            2 => Some(Self::DelayTimeL),
            3 => Some(Self::DelayTimeR),
            4 => Some(Self::NoteL),
            5 => Some(Self::NoteR),
            6 => Some(Self::DeviationL),
            7 => Some(Self::DeviationR),
            8 => Some(Self::HalveL),
            9 => Some(Self::HalveR),
            10 => Some(Self::DoubleL),
            11 => Some(Self::DoubleR),
            12 => Some(Self::LowCutL),
            13 => Some(Self::LowCutR),
            14 => Some(Self::LowCutSlopeL),
            15 => Some(Self::LowCutSlopeR),
            16 => Some(Self::HighCutL),
            17 => Some(Self::HighCutR),
            18 => Some(Self::HighCutSlopeL),
            19 => Some(Self::HighCutSlopeR),
            20 => Some(Self::FeedbackL),
            21 => Some(Self::FeedbackR),
            22 => Some(Self::FeedbackPhaseL),
            23 => Some(Self::FeedbackPhaseR),
            24 => Some(Self::CrossfeedLr),
            25 => Some(Self::CrossfeedRl),
            26 => Some(Self::CrossfeedPhaseLr),
            27 => Some(Self::CrossfeedPhaseRl),
            28 => Some(Self::Routing),
            29 => Some(Self::TempoSync),
            30 => Some(Self::StereoLink),
            31 => Some(Self::OutputMixL),
            32 => Some(Self::OutputMixR),
            33 => Some(Self::Oversampling),
            34 => Some(Self::Bypass),
            _ => None,
        }
    }

    pub fn from_param_id(param_id: &str) -> Option<Self> {
        match param_id {
            "input_mode_l" => Some(Self::InputModeL),
            "input_mode_r" => Some(Self::InputModeR),
            "delay_time_l" => Some(Self::DelayTimeL),
            "delay_time_r" => Some(Self::DelayTimeR),
            "note_l" => Some(Self::NoteL),
            "note_r" => Some(Self::NoteR),
            "deviation_l" => Some(Self::DeviationL),
            "deviation_r" => Some(Self::DeviationR),
            "halve_l" => Some(Self::HalveL),
            "halve_r" => Some(Self::HalveR),
            "double_l" => Some(Self::DoubleL),
            "double_r" => Some(Self::DoubleR),
            "low_cut_l" => Some(Self::LowCutL),
            "low_cut_r" => Some(Self::LowCutR),
            "low_cut_slope_l" => Some(Self::LowCutSlopeL),
            "low_cut_slope_r" => Some(Self::LowCutSlopeR),
            "high_cut_l" => Some(Self::HighCutL),
            "high_cut_r" => Some(Self::HighCutR),
            "high_cut_slope_l" => Some(Self::HighCutSlopeL),
            "high_cut_slope_r" => Some(Self::HighCutSlopeR),
            "feedback_l" => Some(Self::FeedbackL),
            "feedback_r" => Some(Self::FeedbackR),
            "feedback_phase_l" => Some(Self::FeedbackPhaseL),
            "feedback_phase_r" => Some(Self::FeedbackPhaseR),
            "crossfeed_l_r" => Some(Self::CrossfeedLr),
            "crossfeed_r_l" => Some(Self::CrossfeedRl),
            "crossfeed_phase" | "crossfeed_phase_l_r" => Some(Self::CrossfeedPhaseLr),
            "crossfeed_phase_r_l" => Some(Self::CrossfeedPhaseRl),
            "routing" => Some(Self::Routing),
            "tempo_sync" => Some(Self::TempoSync),
            "stereo_link" => Some(Self::StereoLink),
            "output_mix_l" => Some(Self::OutputMixL),
            "output_mix_r" => Some(Self::OutputMixR),
            "oversampling" => Some(Self::Oversampling),
            "bypass" | "fx_bypass" => Some(Self::Bypass),
            _ => None,
        }
    }

    pub fn param_id(self) -> &'static str {
        match self {
            Self::InputModeL => "input_mode_l",
            Self::InputModeR => "input_mode_r",
            Self::DelayTimeL => "delay_time_l",
            Self::DelayTimeR => "delay_time_r",
            Self::NoteL => "note_l",
            Self::NoteR => "note_r",
            Self::DeviationL => "deviation_l",
            Self::DeviationR => "deviation_r",
            Self::HalveL => "halve_l",
            Self::HalveR => "halve_r",
            Self::DoubleL => "double_l",
            Self::DoubleR => "double_r",
            Self::LowCutL => "low_cut_l",
            Self::LowCutR => "low_cut_r",
            Self::LowCutSlopeL => "low_cut_slope_l",
            Self::LowCutSlopeR => "low_cut_slope_r",
            Self::HighCutL => "high_cut_l",
            Self::HighCutR => "high_cut_r",
            Self::HighCutSlopeL => "high_cut_slope_l",
            Self::HighCutSlopeR => "high_cut_slope_r",
            Self::FeedbackL => "feedback_l",
            Self::FeedbackR => "feedback_r",
            Self::FeedbackPhaseL => "feedback_phase_l",
            Self::FeedbackPhaseR => "feedback_phase_r",
            Self::CrossfeedLr => "crossfeed_l_r",
            Self::CrossfeedRl => "crossfeed_r_l",
            Self::CrossfeedPhaseLr => "crossfeed_phase_l_r",
            Self::CrossfeedPhaseRl => "crossfeed_phase_r_l",
            Self::Routing => "routing",
            Self::TempoSync => "tempo_sync",
            Self::StereoLink => "stereo_link",
            Self::OutputMixL => "output_mix_l",
            Self::OutputMixR => "output_mix_r",
            Self::Oversampling => "oversampling",
            Self::Bypass => "bypass",
        }
    }
}

/// Lock-free MIDI mapping and value runtime shared by audio and GUI.
pub struct MidiRuntime {
    cc_targets: [AtomicU16; CC_SLOT_COUNT],
    target_values: [AtomicU32; MIDI_TARGET_COUNT],
    target_dirty: [AtomicBool; MIDI_TARGET_COUNT],
    target_active: [AtomicBool; MIDI_TARGET_COUNT],
    target_enabled: [AtomicBool; MIDI_TARGET_COUNT],
    global_enabled: AtomicBool,
    learning_target: AtomicU16,
    learned_target: AtomicU16,
    learned_channel: AtomicU8,
    learned_cc: AtomicU8,
    learned_value: AtomicU32,
    learned_dirty: AtomicBool,
}

impl MidiRuntime {
    pub fn new() -> Self {
        Self {
            cc_targets: std::array::from_fn(|_| AtomicU16::new(NO_TARGET)),
            target_values: std::array::from_fn(|_| AtomicU32::new(0.0f32.to_bits())),
            target_dirty: std::array::from_fn(|_| AtomicBool::new(false)),
            target_active: std::array::from_fn(|_| AtomicBool::new(false)),
            target_enabled: std::array::from_fn(|_| AtomicBool::new(false)),
            global_enabled: AtomicBool::new(true),
            learning_target: AtomicU16::new(NO_TARGET),
            learned_target: AtomicU16::new(NO_TARGET),
            learned_channel: AtomicU8::new(0),
            learned_cc: AtomicU8::new(0),
            learned_value: AtomicU32::new(0.0f32.to_bits()),
            learned_dirty: AtomicBool::new(false),
        }
    }

    #[inline]
    fn index(channel: u8, cc: u8) -> usize {
        MidiCcValues::index(channel, cc)
    }

    pub fn start_learn(&self, target: MidiTarget) {
        self.learning_target.store(target as u16, Ordering::Release);
    }

    pub fn stop_learn(&self) {
        self.learning_target.store(NO_TARGET, Ordering::Release);
    }

    pub fn is_learning(&self) -> bool {
        self.learning_target.load(Ordering::Acquire) != NO_TARGET
    }

    pub fn set_global_enabled(&self, enabled: bool) {
        self.global_enabled.store(enabled, Ordering::Release);
        if !enabled {
            self.clear_active_values();
        }
    }

    pub fn clear_active_values(&self) {
        for idx in 0..MIDI_TARGET_COUNT {
            self.target_active[idx].store(false, Ordering::Release);
            self.target_dirty[idx].store(false, Ordering::Release);
        }
    }

    pub fn clear_mappings(&self) {
        for slot in &self.cc_targets {
            slot.store(NO_TARGET, Ordering::Release);
        }
        for idx in 0..MIDI_TARGET_COUNT {
            self.target_enabled[idx].store(false, Ordering::Release);
        }
        self.clear_active_values();
    }

    pub fn set_mapping(&self, channel: u8, cc: u8, target: MidiTarget, enabled: bool) {
        self.cc_targets[Self::index(channel, cc)].store(target as u16, Ordering::Release);
        self.target_enabled[target.index()].store(enabled, Ordering::Release);
    }

    pub fn set_target_enabled(&self, target: MidiTarget, enabled: bool) {
        self.target_enabled[target.index()].store(enabled, Ordering::Release);
        if !enabled {
            self.target_active[target.index()].store(false, Ordering::Release);
            self.target_dirty[target.index()].store(false, Ordering::Release);
        }
    }

    pub fn process_cc(&self, channel: u8, cc: u8, normalized: f32) -> Option<MidiTarget> {
        let normalized = normalized.clamp(0.0, 1.0);
        let learned = self.learning_target.swap(NO_TARGET, Ordering::AcqRel);
        if learned != NO_TARGET {
            if let Some(target) = MidiTarget::from_index(learned as usize) {
                self.set_mapping(channel, cc, target, true);
                self.set_target_value(target, normalized);
                self.learned_channel.store(channel, Ordering::Release);
                self.learned_cc.store(cc, Ordering::Release);
                self.learned_value
                    .store(normalized.to_bits(), Ordering::Release);
                self.learned_target.store(learned, Ordering::Release);
                self.learned_dirty.store(true, Ordering::Release);
                return Some(target);
            }
        }

        if !self.global_enabled.load(Ordering::Acquire) {
            return None;
        }

        let target_idx = self.cc_targets[Self::index(channel, cc)].load(Ordering::Acquire);
        let target = MidiTarget::from_index(target_idx as usize)?;
        if !self.target_enabled[target.index()].load(Ordering::Acquire) {
            return None;
        }
        self.set_target_value(target, normalized);
        Some(target)
    }

    pub fn set_target_value(&self, target: MidiTarget, normalized: f32) {
        let idx = target.index();
        self.target_values[idx].store(normalized.clamp(0.0, 1.0).to_bits(), Ordering::Release);
        self.target_active[idx].store(true, Ordering::Release);
        self.target_dirty[idx].store(true, Ordering::Release);
    }

    pub fn target_value(&self, target: MidiTarget) -> Option<f32> {
        let idx = target.index();
        if self.global_enabled.load(Ordering::Acquire)
            && self.target_active[idx].load(Ordering::Acquire)
        {
            Some(f32::from_bits(
                self.target_values[idx].load(Ordering::Acquire),
            ))
        } else {
            None
        }
    }

    pub fn consume_target_value(&self, target: MidiTarget) -> Option<f32> {
        let idx = target.index();
        if self.target_dirty[idx].swap(false, Ordering::AcqRel) {
            self.target_value(target)
        } else {
            None
        }
    }

    pub fn drain_learned_mapping(&self) -> Option<(MidiTarget, u8, u8, f32)> {
        if self.learned_dirty.swap(false, Ordering::AcqRel) {
            let target =
                MidiTarget::from_index(self.learned_target.load(Ordering::Acquire) as usize)?;
            let channel = self.learned_channel.load(Ordering::Acquire);
            let cc = self.learned_cc.load(Ordering::Acquire);
            let value = f32::from_bits(self.learned_value.load(Ordering::Acquire));
            Some((target, channel, cc, value))
        } else {
            None
        }
    }
}

impl Default for MidiRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// Rebuild the lock-free runtime mapping from the persisted GUI learn state.
pub fn sync_runtime_from_learn_state(runtime: &MidiRuntime, learn_state: &MidiLearnState) {
    let mut preserved_values = [None; MIDI_TARGET_COUNT];
    if learn_state.is_global_enabled() {
        for mapping in learn_state.mappings() {
            if learn_state.is_midi_enabled(&mapping.param_id) {
                if let Some(target) = MidiTarget::from_param_id(&mapping.param_id) {
                    preserved_values[target.index()] = runtime.target_value(target);
                }
            }
        }
    }

    runtime.clear_mappings();
    runtime.set_global_enabled(learn_state.is_global_enabled());

    for mapping in learn_state.mappings() {
        if let Some(target) = MidiTarget::from_param_id(&mapping.param_id) {
            runtime.set_mapping(
                mapping.channel,
                mapping.cc,
                target,
                learn_state.is_midi_enabled(&mapping.param_id),
            );
            if learn_state.is_midi_enabled(&mapping.param_id) {
                if let Some(value) = preserved_values[target.index()] {
                    runtime.set_target_value(target, value);
                }
            }
        }
    }
}

/// Lock-free CC value store for audio thread access.
///
/// This struct is the **sole** data structure accessed from the real-time audio
/// thread. It contains no locks, no allocations, and no syscalls on the read
/// path. CC values are stored as [`AtomicU32`] (the f32 bit pattern), with a
/// companion [`AtomicBool`] dirty flag per slot.
///
/// # Why AtomicU32 instead of AtomicF32?
///
/// Rust's standard library does not provide `AtomicF32`. The canonical
/// workaround is to transmute the `f32` to `u32` via [`f32::to_bits()`] and
/// [`f32::from_bits()`], which is a zero-cost, well-defined operation (it
/// preserves the bit pattern, including NaN payloads, and is not affected by
/// the platform's floating-point endianness because both the store and load
/// happen on the same machine).
///
/// # Thread Safety
///
/// - **Writer** (MIDI input handler, typically on the main or a dedicated
///   MIDI thread): calls [`set()`](Self::set).
/// - **Reader** (audio thread): calls [`get_and_clear()`](Self::get_and_clear).
///
/// The Release-Acquire pair on the dirty flag ensures that a value written
/// before the dirty flag is set will be visible to the reader after it
/// observes the dirty flag.
///
/// # Const Layout
///
/// The internal arrays are fixed-size (`[AtomicU32; 2048]` and
/// `[AtomicBool; 2048]`), so the struct has a predictable memory layout
/// suitable for embedding in shared memory or `Arc`.
pub struct MidiCcValues {
    /// CC values indexed by `(channel * 128 + cc)`.
    ///
    /// Each [`AtomicU32`] stores the bit pattern of an `f32` value in the
    /// range [0.0, 1.0], encoded via [`f32::to_bits()`] and decoded via
    /// [`f32::from_bits()`]. This is safe because `AtomicU32` has the same
    /// size and alignment as `f32` on all supported platforms, and the
    /// bit-transmute is a pure data conversion with no undefined behaviour.
    values: [AtomicU32; CC_SLOT_COUNT],

    /// Dirty flags indexed by `(channel * 128 + cc)`.
    ///
    /// `true` means a new CC value has been written since the last time
    /// the audio thread read it. The audio thread atomically swaps the
    /// flag to `false` when it consumes the value, using `AcqRel` ordering
    /// to form a proper synchronisation pair with the writer's `Release`
    /// store.
    dirty: [AtomicBool; CC_SLOT_COUNT],
}

impl MidiCcValues {
    /// Create a new `MidiCcValues` with all slots initialised to 0.0 and
    /// all dirty flags cleared.
    ///
    /// This is typically called once at plugin initialisation and shared
    /// (e.g., via `Arc`) between the MIDI input handler and the audio
    /// processing callback.
    ///
    /// # Implementation Note
    ///
    /// [`AtomicU32`] and [`AtomicBool`] do not implement `Copy`, so we
    /// cannot use `[AtomicU32::new(0); 2048]`. Instead, we use
    /// [`std::array::from_fn`] which constructs each element via a closure.
    pub fn new() -> Self {
        Self {
            values: std::array::from_fn(|_| AtomicU32::new(0)),
            dirty: std::array::from_fn(|_| AtomicBool::new(false)),
        }
    }

    /// Compute the flat index for a given (channel, cc) pair.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if `channel >= 16` or `cc >= 128`.
    /// In release builds, the index wraps silently (the caller is
    /// responsible for valid input).
    #[inline]
    fn index(channel: u8, cc: u8) -> usize {
        debug_assert!(channel < 16, "MIDI channel must be 0–15, got {channel}");
        debug_assert!(cc < 128, "MIDI CC must be 0–127, got {cc}");
        (channel as usize) * 128 + (cc as usize)
    }

    /// Called from the audio thread: retrieve the latest CC value for the
    /// given `(channel, cc)` pair, if one has been written since the last
    /// call.
    ///
    /// If a new value is available, this returns `Some(value)` and atomically
    /// clears the dirty flag. If no new value has arrived, returns `None`.
    ///
    /// # Lock-Free Guarantee
    ///
    /// This method performs exactly two atomic operations on the fast path
    /// (dirty flag swap + value load) and never blocks, allocates, or
    /// performs a syscall. It is safe to call from any real-time audio
    /// callback.
    ///
    /// # Memory Ordering
    ///
    /// Uses `AcqRel` on the dirty flag swap and `Acquire` on the value load.
    /// The `AcqRel` swap ensures:
    /// - **Acquire**: we see all writes that happened before the corresponding
    ///   `Release` store to the dirty flag in [`set()`](Self::set).
    /// - **Release**: our clearing of the flag is visible to future writers,
    ///   so they know the slot has been consumed.
    ///
    /// # Edge Case: Concurrent Write
    ///
    /// If a writer calls [`set()`](Self::set) between our `swap` and
    /// `load`, we will read the *newer* value (which is correct — we always
    /// want the latest). The dirty flag will be `true` again, so the next
    /// call to `get_and_clear` will also return `Some`, but the value will
    /// be the same (or a newer one). This is benign: the audio thread
    /// simply processes the most recent CC value, which is the desired
    /// behaviour for continuous controllers.
    #[inline]
    pub fn get_and_clear(&self, channel: u8, cc: u8) -> Option<f32> {
        let idx = Self::index(channel, cc);

        // Atomically swap the dirty flag to false. If it was true, a new
        // value is available. The AcqRel ordering ensures:
        // - Acquire: we see all writes that happened before the writer's
        //   Release store to the dirty flag.
        // - Release: the writer's next set() will see our flag-clear.
        let was_dirty = self.dirty[idx].swap(false, Ordering::AcqRel);

        if was_dirty {
            // Load the value with Acquire ordering to ensure we see the
            // value written before the dirty flag was set.
            let bits = self.values[idx].load(Ordering::Acquire);
            Some(f32::from_bits(bits))
        } else {
            None
        }
    }

    /// Called from the MIDI input handler: store a new CC value and mark it
    /// as dirty so the audio thread will pick it up.
    ///
    /// The `value` parameter should be a normalised `f32` in the range
    /// [0.0, 1.0], typically computed as `midi_value as f32 / 127.0`.
    ///
    /// # Memory Ordering
    ///
    /// Stores the value with `Release` ordering first, then sets the dirty
    /// flag with `Release` ordering. This two-step Release ensures that the
    /// audio thread, upon observing `dirty == true` via its `AcqRel` swap,
    /// will also observe the value written here. The ordering of these two
    /// stores is critical: if the dirty flag were set before the value,
    /// the reader could see a stale value.
    #[inline]
    pub fn set(&self, channel: u8, cc: u8, value: f32) {
        let idx = Self::index(channel, cc);

        // Write the value first with Release ordering. This ensures the
        // store is visible before the dirty flag store below.
        self.values[idx].store(value.to_bits(), Ordering::Release);

        // Then set the dirty flag with Release ordering. The audio thread's
        // AcqRel swap on the dirty flag forms a Release-Acquire pair that
        // guarantees the value store above is visible when dirty == true
        // is observed.
        self.dirty[idx].store(true, Ordering::Release);
    }
}

impl Default for MidiCcValues {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// MidiLearnState — GUI-Thread Learning/Mapping State
// ═══════════════════════════════════════════════════════════════════════════

/// State for MIDI learn functionality.
///
/// This struct is intended to be accessed **only** from the GUI/main thread.
/// It is *not* safe to share across threads without external synchronisation.
/// The audio thread should only interact with [`MidiCcValues`], never with
/// this struct directly.
///
/// # Context Menu Integration
///
/// The GUI right-click context menu for a parameter calls methods on this
/// struct:
///
/// | Menu Item      | Method Called                                            |
/// |----------------|----------------------------------------------------------|
/// | "MIDI On/Off"  | [`toggle_midi()`](Self::toggle_midi)                     |
/// | "Clean Up"     | [`clean_up()`](Self::clean_up)                           |
/// | "Roll Back"    | [`roll_back()`](Self::roll_back)                         |
/// | "Save"         | [`save_for_rollback()`](Self::save_for_rollback)         |
///
/// # MIDI Enable/Disable
///
/// A parameter can have a mapping but have its MIDI control temporarily
/// disabled. This is tracked in the `disabled_params` set. When MIDI is
/// disabled for a parameter, incoming CC values for its mapped CC are
/// still stored in [`MidiCcValues`] (so the dirty flag is not lost), but
/// the parameter's value is **not** updated from the CC. This allows the
/// user to temporarily suspend MIDI control without losing the mapping.
///
/// Wait — actually, when MIDI is disabled, we should *not* write to
/// `MidiCcValues`, because the audio thread would then pick up a stale
/// value when MIDI is re-enabled. Instead, we simply skip the `set()`
/// call, so the audio thread never sees a value for a disabled parameter.
/// When re-enabled, the next incoming CC message will be applied normally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidiLearnState {
    /// Active CC → Parameter mappings.
    ///
    /// Invariant: no two entries share the same `(channel, cc)` pair,
    /// and no two entries share the same `param_id`.
    #[serde(default)]
    mappings: Vec<MidiMapping>,

    /// Global MIDI parameter-control enable. This does not delete mappings.
    #[serde(default = "default_midi_enabled")]
    global_enabled: bool,

    /// Whether we are currently in MIDI learn mode.
    #[serde(skip)]
    learning: bool,

    /// The parameter we are learning a CC for (if in learn mode).
    #[serde(skip)]
    learning_param: Option<String>,

    /// Previous mapping state, saved for the "Roll Back" feature.
    /// `None` means no snapshot has been saved; `Some(vec)` holds the
    /// snapshot (even if the vec is empty, indicating the user saved
    /// when no mappings were active).
    #[serde(default)]
    prev_mappings: Option<Vec<MidiMapping>>,

    /// Parameters that have their MIDI control temporarily disabled.
    /// The mapping still exists, but CC values are not applied to the
    /// parameter until re-enabled.
    #[serde(skip)]
    disabled_params: HashSet<String>,
}

impl MidiLearnState {
    /// Create a new `MidiLearnState` with no mappings and learn mode disabled.
    pub fn new() -> Self {
        Self {
            mappings: Vec::new(),
            global_enabled: true,
            learning: false,
            learning_param: None,
            prev_mappings: None,
            disabled_params: HashSet::new(),
        }
    }

    /// Start MIDI learn mode for the given parameter.
    ///
    /// While in learn mode, the next incoming MIDI CC message will be
    /// mapped to this parameter. Learn mode is automatically exited when
    /// a mapping is created (see [`process_cc()`](Self::process_cc)).
    ///
    /// If learn mode is already active for a different parameter, it is
    /// silently replaced with the new one.
    pub fn start_learn(&mut self, param_id: &str) {
        self.learning = true;
        self.learning_param = Some(param_id.to_string());
    }

    /// Stop MIDI learn mode without creating a mapping.
    ///
    /// This is typically called when the user cancels the learn operation
    /// (e.g., by clicking away from the learn UI or pressing Escape).
    pub fn stop_learn(&mut self) {
        self.learning = false;
        self.learning_param = None;
    }

    /// Returns `true` if we are currently in MIDI learn mode.
    pub fn is_learning(&self) -> bool {
        self.learning
    }

    /// Process an incoming MIDI CC message.
    ///
    /// This method handles two cases:
    ///
    /// 1. **Learn mode active**: Creates a new mapping from the incoming
    ///    `(channel, cc)` to the learning parameter, then exits learn mode.
    ///    If an existing mapping for the same `(channel, cc)` or the same
    ///    `param_id` exists, it is replaced (a CC can only control one
    ///    parameter, and a parameter can only be controlled by one CC).
    ///    The incoming value is stored in `cc_values`. Returns
    ///    `Some(param_id)` to signal that a new mapping was created.
    ///
    /// 2. **Learn mode inactive**: If the `(channel, cc)` pair matches an
    ///    existing mapping and MIDI is enabled for that parameter, stores
    ///    the normalised value in `cc_values`. Returns `None`.
    ///
    /// # MIDI Value Normalisation
    ///
    /// The raw 7-bit MIDI value (0–127) is normalised to the [0.0, 1.0]
    /// range via `value as f32 / 127.0`. This produces a value suitable
    /// for `nih_plug`'s `Param::set_normalized_value()`.
    pub fn process_cc(
        &mut self,
        channel: u8,
        cc: u8,
        value: u8,
        cc_values: &MidiCcValues,
    ) -> Option<String> {
        self.process_cc_event(channel, cc, value as f32 / 127.0, cc_values)
            .map(|(param_id, _)| param_id)
    }

    /// Process an incoming MIDI CC message with a normalized value.
    ///
    /// Returns the affected parameter ID and normalized value when the CC
    /// should update a parameter target.
    pub fn process_cc_event(
        &mut self,
        channel: u8,
        cc: u8,
        normalized: f32,
        cc_values: &MidiCcValues,
    ) -> Option<(String, f32)> {
        let normalized = normalized.clamp(0.0, 1.0);

        if self.learning {
            if let Some(ref param_id) = self.learning_param {
                let new_mapping = MidiMapping {
                    cc,
                    channel,
                    param_id: param_id.clone(),
                };

                // Remove any existing mapping for the same (channel, cc) pair.
                // A single CC can only control one parameter.
                self.mappings
                    .retain(|m| !(m.channel == channel && m.cc == cc));

                // Remove any existing mapping for the same param_id.
                // A parameter can only be controlled by one CC.
                self.mappings.retain(|m| m.param_id != *param_id);

                self.mappings.push(new_mapping);

                // Store the incoming value immediately so the parameter
                // reflects the knob position right away.
                cc_values.set(channel, cc, normalized);

                // Ensure MIDI is enabled for the newly-mapped parameter.
                self.disabled_params.remove(param_id);

                // Exit learn mode.
                let result = param_id.clone();
                self.learning = false;
                self.learning_param = None;

                return Some((result, normalized));
            }
        } else if self.global_enabled {
            // Look for an existing mapping for this (channel, cc) pair.
            for mapping in &self.mappings {
                if mapping.channel == channel && mapping.cc == cc {
                    // Only update the value if MIDI is enabled for this param.
                    if !self.disabled_params.contains(&mapping.param_id) {
                        cc_values.set(channel, cc, normalized);
                        return Some((mapping.param_id.clone(), normalized));
                    }
                    return None;
                }
            }
        }

        None
    }

    /// Directly assign a CC mapping to a parameter.
    ///
    /// This is used by the GUI after the lock-free runtime captures the next
    /// CC during learn mode. Keeping this on the GUI side avoids touching this
    /// `Vec`-backed state from the audio callback.
    pub fn assign_mapping(&mut self, param_id: &str, channel: u8, cc: u8) {
        self.mappings
            .retain(|m| !(m.channel == channel && m.cc == cc));
        self.mappings.retain(|m| m.param_id != param_id);
        self.mappings.push(MidiMapping {
            cc,
            channel,
            param_id: param_id.to_string(),
        });
        self.disabled_params.remove(param_id);
        self.stop_learn();
    }

    /// Remove the mapping for the given parameter.
    ///
    /// This is the "Clean Up" context menu action. It removes the
    /// association between the parameter and its MIDI CC, but does not
    /// affect the parameter's current value. The parameter is also removed
    /// from the disabled set (since it no longer has a mapping to disable).
    pub fn clean_up(&mut self, param_id: &str) {
        self.mappings.retain(|m| m.param_id != param_id);
        self.disabled_params.remove(param_id);
    }

    /// Remove every MIDI mapping.
    pub fn clear_all(&mut self) {
        self.mappings.clear();
        self.disabled_params.clear();
        self.stop_learn();
    }

    /// Roll back to the previously saved mapping state.
    ///
    /// This is the "Roll Back" context menu action. It replaces the current
    /// mappings with the state saved by [`save_for_rollback()`]. If no
    /// rollback state has been saved, this is a no-op (the user hasn't
    /// pressed "Save" yet).
    ///
    /// After rolling back, the disabled-params set is pruned to only
    /// contain entries that still have a mapping (orphaned disabled entries
    /// are removed).
    pub fn roll_back(&mut self) {
        if let Some(prev) = self.prev_mappings.clone() {
            self.mappings = prev;

            // Rebuild disabled_params: keep only entries that still have
            // a mapping after the rollback.
            let mapped_ids: HashSet<String> =
                self.mappings.iter().map(|m| m.param_id.clone()).collect();
            self.disabled_params.retain(|id| mapped_ids.contains(id));
        }
    }

    /// Save the current mapping state for a future rollback.
    ///
    /// This is the "Save" context menu action. It snapshots the current
    /// mappings so they can be restored via [`roll_back()`].
    ///
    /// Calling this again overwrites the previous rollback state.
    /// The disabled-params set is **not** saved as part of the rollback
    /// state; it is transient UI state.
    pub fn save_for_rollback(&mut self) {
        self.prev_mappings = Some(self.mappings.clone());
    }

    /// Get the mapping for a specific parameter, if one exists.
    pub fn get_mapping(&self, param_id: &str) -> Option<&MidiMapping> {
        self.mappings.iter().find(|m| m.param_id == param_id)
    }

    /// Get a slice of all active mappings.
    pub fn mappings(&self) -> &[MidiMapping] {
        &self.mappings
    }

    /// Globally enable or disable MIDI parameter control.
    pub fn set_global_enabled(&mut self, enabled: bool) {
        self.global_enabled = enabled;
    }

    /// Toggle global MIDI parameter control.
    pub fn toggle_global_enabled(&mut self) {
        self.global_enabled = !self.global_enabled;
    }

    /// Returns whether global MIDI parameter control is enabled.
    pub fn is_global_enabled(&self) -> bool {
        self.global_enabled
    }

    /// Toggle MIDI control for a parameter.
    ///
    /// This is the "MIDI On" / "MIDI Off" context menu action. When MIDI
    /// is toggled **off** for a parameter, the mapping is preserved but
    /// incoming CC values are no longer applied to the parameter (the
    /// `set()` call on [`MidiCcValues`] is skipped). Toggling it back
    /// **on** re-enables CC processing for that parameter.
    ///
    /// If the parameter has no mapping, this is a no-op (there is nothing
    /// to enable or disable).
    pub fn toggle_midi(&mut self, param_id: &str) {
        let has_mapping = self.mappings.iter().any(|m| m.param_id == param_id);
        if !has_mapping {
            return;
        }

        if self.disabled_params.contains(param_id) {
            // Currently disabled → enable.
            self.disabled_params.remove(param_id);
        } else {
            // Currently enabled → disable.
            self.disabled_params.insert(param_id.to_string());
        }
    }

    /// Returns `true` if MIDI control is currently enabled for the given
    /// parameter.
    ///
    /// A parameter has MIDI enabled if and only if:
    /// 1. It has a mapping (a `(channel, cc)` pair is associated with it), AND
    /// 2. It is not in the disabled set.
    pub fn is_midi_enabled(&self, param_id: &str) -> bool {
        self.mappings.iter().any(|m| m.param_id == param_id)
            && !self.disabled_params.contains(param_id)
    }

    /// Serialize all mappings to a JSON string for persistence.
    ///
    /// The serialized format includes only the mappings themselves — not the
    /// learn state, disabled parameters, or rollback data. Those are
    /// transient UI state that should not persist across sessions.
    ///
    /// # Errors
    ///
    /// In the extremely unlikely event that serialisation fails, this
    /// returns the fallback string `"[]"` (an empty JSON array). In
    /// practice, `Vec<MidiMapping>` always serialises successfully.
    pub fn serialize_mappings(&self) -> String {
        serde_json::to_string(&self.mappings).unwrap_or_else(|_| "[]".to_string())
    }

    /// Deserialize mappings from a JSON string, replacing the current
    /// mappings.
    ///
    /// This is typically called during plugin initialisation to restore
    /// previously saved mappings. The disabled-params set is cleared
    /// because disabled status is transient UI state that should not
    /// persist across sessions.
    ///
    /// # Errors
    ///
    /// Returns a [`serde_json::Error`] if the input is not valid JSON or
    /// does not match the expected `Vec<MidiMapping>` structure.
    pub fn deserialize_mappings(&mut self, data: &str) -> Result<(), serde_json::Error> {
        let restored: Vec<MidiMapping> = serde_json::from_str(data)?;
        self.mappings = restored;
        self.disabled_params.clear();
        Ok(())
    }
}

impl Default for MidiLearnState {
    fn default() -> Self {
        Self::new()
    }
}

fn default_midi_enabled() -> bool {
    true
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── MidiCcValues ───────────────────────────────────────────────────

    #[test]
    fn cc_values_new_all_zero_and_clean() {
        let vals = MidiCcValues::new();
        for ch in 0u8..16 {
            for cc in 0u8..128 {
                assert_eq!(
                    vals.get_and_clear(ch, cc),
                    None,
                    "New MidiCcValues should have no dirty values (ch={ch}, cc={cc})"
                );
            }
        }
    }

    #[test]
    fn cc_values_set_and_get() {
        let vals = MidiCcValues::new();

        vals.set(0, 1, 0.5);
        assert_eq!(vals.get_and_clear(0, 1), Some(0.5));
        // After clearing, should return None.
        assert_eq!(vals.get_and_clear(0, 1), None);
    }

    #[test]
    fn cc_values_multiple_channels() {
        let vals = MidiCcValues::new();

        vals.set(0, 1, 0.25);
        vals.set(1, 1, 0.75);
        vals.set(15, 127, 1.0);

        assert_eq!(vals.get_and_clear(0, 1), Some(0.25));
        assert_eq!(vals.get_and_clear(1, 1), Some(0.75));
        assert_eq!(vals.get_and_clear(15, 127), Some(1.0));
    }

    #[test]
    fn cc_values_overwrite() {
        let vals = MidiCcValues::new();

        vals.set(0, 1, 0.3);
        vals.set(0, 1, 0.7);

        // Should get the latest value.
        assert_eq!(vals.get_and_clear(0, 1), Some(0.7));
    }

    #[test]
    fn cc_values_boundary_values() {
        let vals = MidiCcValues::new();

        // Minimum normalised value (MIDI 0).
        vals.set(0, 0, 0.0 / 127.0);
        assert_eq!(vals.get_and_clear(0, 0), Some(0.0));

        // Maximum normalised value (MIDI 127).
        vals.set(15, 127, 127.0 / 127.0);
        assert_eq!(vals.get_and_clear(15, 127), Some(1.0));
    }

    #[test]
    fn cc_values_independent_slots() {
        let vals = MidiCcValues::new();

        vals.set(0, 1, 0.1);
        vals.set(0, 2, 0.2);
        vals.set(1, 1, 0.3);

        // Reading one slot should not affect others.
        assert_eq!(vals.get_and_clear(0, 1), Some(0.1));
        assert_eq!(vals.get_and_clear(0, 2), Some(0.2));
        assert_eq!(vals.get_and_clear(1, 1), Some(0.3));

        // All should be clean now.
        assert_eq!(vals.get_and_clear(0, 1), None);
        assert_eq!(vals.get_and_clear(0, 2), None);
        assert_eq!(vals.get_and_clear(1, 1), None);
    }

    // ── MidiRuntime ───────────────────────────────────────────────────

    #[test]
    fn midi_target_count_matches_indices() {
        assert_eq!(MidiTarget::Bypass.index() + 1, MIDI_TARGET_COUNT);
    }

    #[test]
    fn runtime_learn_maps_cc_and_reports_dirty_target() {
        let runtime = MidiRuntime::new();

        runtime.start_learn(MidiTarget::FeedbackL);
        assert!(runtime.is_learning());

        assert_eq!(runtime.process_cc(0, 7, 0.5), Some(MidiTarget::FeedbackL));
        assert!(!runtime.is_learning());
        assert_eq!(
            runtime.drain_learned_mapping(),
            Some((MidiTarget::FeedbackL, 0, 7, 0.5))
        );
        assert_eq!(
            runtime.consume_target_value(MidiTarget::FeedbackL),
            Some(0.5)
        );
        assert_eq!(runtime.process_cc(0, 7, 0.75), Some(MidiTarget::FeedbackL));
        assert_eq!(
            runtime.consume_target_value(MidiTarget::FeedbackL),
            Some(0.75)
        );
    }

    #[test]
    fn runtime_global_off_blocks_mapped_ccs() {
        let runtime = MidiRuntime::new();

        runtime.set_mapping(0, 7, MidiTarget::FeedbackL, true);
        runtime.set_global_enabled(false);

        assert_eq!(runtime.process_cc(0, 7, 1.0), None);
        assert_eq!(runtime.consume_target_value(MidiTarget::FeedbackL), None);
    }

    #[test]
    fn learn_state_sync_controls_runtime_enabled_state() {
        let runtime = MidiRuntime::new();
        let mut learn = MidiLearnState::new();

        learn.assign_mapping("feedback_l", 0, 7);
        sync_runtime_from_learn_state(&runtime, &learn);
        assert_eq!(runtime.process_cc(0, 7, 1.0), Some(MidiTarget::FeedbackL));

        learn.toggle_midi("feedback_l");
        sync_runtime_from_learn_state(&runtime, &learn);
        assert_eq!(runtime.process_cc(0, 7, 0.25), None);

        learn.toggle_midi("feedback_l");
        sync_runtime_from_learn_state(&runtime, &learn);
        assert_eq!(runtime.process_cc(0, 7, 0.25), Some(MidiTarget::FeedbackL));
    }

    #[test]
    fn learn_state_sync_preserves_active_mapped_values() {
        let runtime = MidiRuntime::new();
        let mut learn = MidiLearnState::new();

        learn.assign_mapping("feedback_l", 0, 7);
        sync_runtime_from_learn_state(&runtime, &learn);
        runtime.process_cc(0, 7, 0.75);

        sync_runtime_from_learn_state(&runtime, &learn);
        assert_eq!(runtime.target_value(MidiTarget::FeedbackL), Some(0.75));

        learn.clean_up("feedback_l");
        sync_runtime_from_learn_state(&runtime, &learn);
        assert_eq!(runtime.target_value(MidiTarget::FeedbackL), None);
    }

    // ── MidiLearnState — Learn Mode ────────────────────────────────────

    #[test]
    fn learn_start_and_stop() {
        let mut state = MidiLearnState::new();

        assert!(!state.is_learning());

        state.start_learn("delay_time");
        assert!(state.is_learning());

        state.stop_learn();
        assert!(!state.is_learning());
    }

    #[test]
    fn learn_process_cc_creates_mapping() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        state.start_learn("delay_time");
        let result = state.process_cc(0, 7, 64, &cc_values);

        assert_eq!(result, Some("delay_time".to_string()));
        assert!(!state.is_learning());

        let mapping = state.get_mapping("delay_time").unwrap();
        assert_eq!(mapping.cc, 7);
        assert_eq!(mapping.channel, 0);
        assert_eq!(mapping.param_id, "delay_time");
    }

    #[test]
    fn learn_stores_value_on_creation() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        state.start_learn("delay_time");
        state.process_cc(0, 7, 64, &cc_values);

        // The value should have been stored (MIDI 64 / 127 ≈ 0.5039).
        let expected = 64.0_f32 / 127.0;
        assert_eq!(cc_values.get_and_clear(0, 7), Some(expected));
    }

    #[test]
    fn learn_auto_exits_after_mapping() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        state.start_learn("delay_time");
        assert!(state.is_learning());

        state.process_cc(0, 7, 64, &cc_values);
        assert!(!state.is_learning());
    }

    // ── MidiLearnState — CC Processing ─────────────────────────────────

    #[test]
    fn process_cc_updates_value_for_mapped_param() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        // Create a mapping.
        state.start_learn("delay_time");
        state.process_cc(0, 7, 64, &cc_values);
        let _ = cc_values.get_and_clear(0, 7); // Consume initial value.

        // Now send CC values — should update cc_values.
        state.process_cc(0, 7, 127, &cc_values);
        assert_eq!(cc_values.get_and_clear(0, 7), Some(1.0));
    }

    #[test]
    fn process_cc_unmapped_cc_is_ignored() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        // No mapping exists; CC should be ignored.
        let result = state.process_cc(0, 7, 64, &cc_values);
        assert_eq!(result, None);
        assert_eq!(cc_values.get_and_clear(0, 7), None);
    }

    // ── MidiLearnState — Mapping Replacement ───────────────────────────

    #[test]
    fn replace_mapping_same_cc_different_param() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        // Map CC7 ch0 → delay_time.
        state.start_learn("delay_time");
        state.process_cc(0, 7, 64, &cc_values);

        // Map CC7 ch0 → feedback — should replace delay_time mapping.
        state.start_learn("feedback");
        state.process_cc(0, 7, 100, &cc_values);

        assert!(
            state.get_mapping("delay_time").is_none(),
            "Old param should no longer be mapped"
        );
        let mapping = state.get_mapping("feedback").unwrap();
        assert_eq!(mapping.cc, 7);
        assert_eq!(mapping.channel, 0);
        assert_eq!(state.mappings().len(), 1);
    }

    #[test]
    fn replace_mapping_same_param_different_cc() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        // Map CC7 → delay_time.
        state.start_learn("delay_time");
        state.process_cc(0, 7, 64, &cc_values);

        // Re-learn delay_time on CC10 — should replace.
        state.start_learn("delay_time");
        state.process_cc(0, 10, 100, &cc_values);

        let mapping = state.get_mapping("delay_time").unwrap();
        assert_eq!(mapping.cc, 10);
        assert_eq!(mapping.channel, 0);
        assert_eq!(state.mappings().len(), 1);
    }

    // ── MidiLearnState — Clean Up ──────────────────────────────────────

    #[test]
    fn clean_up_removes_mapping() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        state.start_learn("delay_time");
        state.process_cc(0, 7, 64, &cc_values);

        assert!(state.get_mapping("delay_time").is_some());
        state.clean_up("delay_time");
        assert!(state.get_mapping("delay_time").is_none());
    }

    #[test]
    fn clean_up_also_removes_from_disabled_set() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        state.start_learn("delay_time");
        state.process_cc(0, 7, 64, &cc_values);

        state.toggle_midi("delay_time"); // Disable.
        assert!(!state.is_midi_enabled("delay_time"));

        state.clean_up("delay_time");
        // After cleanup, no mapping exists → is_midi_enabled is false.
        assert!(!state.is_midi_enabled("delay_time"));
        assert!(state.get_mapping("delay_time").is_none());
    }

    #[test]
    fn clean_up_nonexistent_is_noop() {
        let mut state = MidiLearnState::new();
        state.clean_up("nonexistent"); // Should not panic.
    }

    // ── MidiLearnState — Toggle MIDI ───────────────────────────────────

    #[test]
    fn toggle_midi_disables_and_enables() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        state.start_learn("delay_time");
        state.process_cc(0, 7, 64, &cc_values);
        let _ = cc_values.get_and_clear(0, 7); // Consume initial value.

        assert!(state.is_midi_enabled("delay_time"));

        // Toggle off.
        state.toggle_midi("delay_time");
        assert!(!state.is_midi_enabled("delay_time"));

        // CC values should not be applied when disabled.
        state.process_cc(0, 7, 127, &cc_values);
        assert_eq!(
            cc_values.get_and_clear(0, 7),
            None,
            "CC should not be stored when MIDI is disabled"
        );

        // Toggle back on.
        state.toggle_midi("delay_time");
        assert!(state.is_midi_enabled("delay_time"));

        // CC values should now be applied.
        state.process_cc(0, 7, 127, &cc_values);
        assert_eq!(cc_values.get_and_clear(0, 7), Some(1.0));
    }

    #[test]
    fn toggle_midi_no_mapping_is_noop() {
        let mut state = MidiLearnState::new();
        state.toggle_midi("nonexistent");
        assert!(!state.is_midi_enabled("nonexistent"));
    }

    // ── MidiLearnState — Roll Back ─────────────────────────────────────

    #[test]
    fn roll_back_restores_previous_state() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        // Create first mapping.
        state.start_learn("delay_time");
        state.process_cc(0, 7, 64, &cc_values);

        // Save state.
        state.save_for_rollback();

        // Create another mapping.
        state.start_learn("feedback");
        state.process_cc(0, 10, 100, &cc_values);

        assert!(state.get_mapping("delay_time").is_some());
        assert!(state.get_mapping("feedback").is_some());
        assert_eq!(state.mappings().len(), 2);

        // Roll back — should restore to the state with only delay_time.
        state.roll_back();

        assert!(state.get_mapping("delay_time").is_some());
        assert!(state.get_mapping("feedback").is_none());
        assert_eq!(state.mappings().len(), 1);
    }

    #[test]
    fn roll_back_without_save_is_noop() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        state.start_learn("delay_time");
        state.process_cc(0, 7, 64, &cc_values);

        // Roll back without saving — should be a no-op.
        state.roll_back();
        assert!(state.get_mapping("delay_time").is_some());
    }

    #[test]
    fn roll_back_to_empty_state() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        // Save when there are no mappings.
        state.save_for_rollback();

        // Add a mapping.
        state.start_learn("delay_time");
        state.process_cc(0, 7, 64, &cc_values);
        assert_eq!(state.mappings().len(), 1);

        // Roll back — should clear all mappings.
        state.roll_back();
        assert_eq!(state.mappings().len(), 0);
    }

    #[test]
    fn roll_back_prunes_disabled_set() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        state.start_learn("delay_time");
        state.process_cc(0, 7, 64, &cc_values);

        state.save_for_rollback();

        // Add another mapping and disable it.
        state.start_learn("feedback");
        state.process_cc(0, 10, 100, &cc_values);
        state.toggle_midi("feedback");

        // Roll back — feedback mapping is gone, so its disabled entry
        // should also be gone.
        state.roll_back();
        assert!(!state.disabled_params.contains("feedback"));
    }

    #[test]
    fn save_for_rollback_overwrites_previous() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        // Create and save mapping A.
        state.start_learn("delay_time");
        state.process_cc(0, 7, 64, &cc_values);
        state.save_for_rollback();

        // Create and save mapping B (overwrites previous save).
        state.start_learn("feedback");
        state.process_cc(0, 10, 100, &cc_values);
        state.save_for_rollback();

        // Create mapping C.
        state.start_learn("output_mix");
        state.process_cc(0, 1, 50, &cc_values);

        // Roll back — should restore to state with both delay_time and
        // feedback (the second save).
        state.roll_back();
        assert!(state.get_mapping("delay_time").is_some());
        assert!(state.get_mapping("feedback").is_some());
        assert!(state.get_mapping("output_mix").is_none());
    }

    // ── MidiLearnState — Serialization ─────────────────────────────────

    #[test]
    fn serialize_deserialize_roundtrip() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        state.start_learn("delay_time");
        state.process_cc(0, 7, 64, &cc_values);

        state.start_learn("feedback");
        state.process_cc(1, 10, 100, &cc_values);

        let json = state.serialize_mappings();

        let mut restored = MidiLearnState::new();
        restored.deserialize_mappings(&json).unwrap();

        assert_eq!(restored.mappings().len(), 2);

        let m1 = restored.get_mapping("delay_time").unwrap();
        assert_eq!(m1.cc, 7);
        assert_eq!(m1.channel, 0);

        let m2 = restored.get_mapping("feedback").unwrap();
        assert_eq!(m2.cc, 10);
        assert_eq!(m2.channel, 1);
    }

    #[test]
    fn deserialize_clears_disabled_set() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        state.start_learn("delay_time");
        state.process_cc(0, 7, 64, &cc_values);
        state.toggle_midi("delay_time");
        assert!(!state.is_midi_enabled("delay_time"));

        let json = state.serialize_mappings();

        let mut restored = MidiLearnState::new();
        restored.deserialize_mappings(&json).unwrap();

        // After deserialization, MIDI should be enabled (disabled set
        // is cleared).
        assert!(restored.is_midi_enabled("delay_time"));
    }

    #[test]
    fn deserialize_invalid_json_returns_error() {
        let mut state = MidiLearnState::new();
        let result = state.deserialize_mappings("not valid json");
        assert!(result.is_err());
    }

    #[test]
    fn serialize_empty_mappings() {
        let state = MidiLearnState::new();
        let json = state.serialize_mappings();
        assert_eq!(json, "[]");
    }

    // ── MidiLearnState — Multiple Parameters ───────────────────────────

    #[test]
    fn multiple_independent_mappings() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        state.start_learn("delay_time");
        state.process_cc(0, 7, 64, &cc_values);

        state.start_learn("feedback");
        state.process_cc(0, 10, 100, &cc_values);

        state.start_learn("output_mix");
        state.process_cc(0, 1, 32, &cc_values);

        assert_eq!(state.mappings().len(), 3);

        // Each should be independently controllable.
        let _ = cc_values.get_and_clear(0, 7);
        let _ = cc_values.get_and_clear(0, 10);
        let _ = cc_values.get_and_clear(0, 1);

        state.process_cc(0, 7, 0, &cc_values);
        assert_eq!(cc_values.get_and_clear(0, 7), Some(0.0));

        state.process_cc(0, 10, 127, &cc_values);
        assert_eq!(cc_values.get_and_clear(0, 10), Some(1.0));
    }

    #[test]
    fn different_channels_same_cc_number() {
        let mut state = MidiLearnState::new();
        let cc_values = MidiCcValues::new();

        // CC7 on channel 0 → delay_time.
        state.start_learn("delay_time");
        state.process_cc(0, 7, 64, &cc_values);

        // CC7 on channel 1 → feedback (same CC number, different channel).
        state.start_learn("feedback");
        state.process_cc(1, 7, 100, &cc_values);

        assert_eq!(state.mappings().len(), 2);

        let _ = cc_values.get_and_clear(0, 7);
        let _ = cc_values.get_and_clear(1, 7);

        // Each should update independently.
        state.process_cc(0, 7, 0, &cc_values);
        state.process_cc(1, 7, 127, &cc_values);

        assert_eq!(cc_values.get_and_clear(0, 7), Some(0.0));
        assert_eq!(cc_values.get_and_clear(1, 7), Some(1.0));
    }
}
