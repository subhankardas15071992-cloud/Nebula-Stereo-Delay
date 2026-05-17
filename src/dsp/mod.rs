//! DSP Engine for **Nebula Stereo Delay** by Nebula Audio
//!
//! A professional stereo delay engine with double-precision (`f64`) processing
//! throughout the entire signal chain. Only the final output stage should
//! convert to `f32` when interfacing with the audio host.
//!
//! # Signal Flow
//!
//! ```text
//! Input ──► Hard Bypass Gate ──► Input Mode Selection ─────────────────► Dry/Wet Mix ──► Output
//!                                      │                                  ▲
//!                                      ▼                                  │
//!                                 Delay Line (cubic interp) ──► Filtered Signal (wet)
//!                                      ▲                          │
//!                                      │                          ▼
//!                                      └──────── Feedback ◄─── Complementary Filters
//!                                                ▲
//!                                                │
//!                                           Routing Matrix
//!                                           (crossfeed + phase)
//! ```
//!
//! # Design Principles
//!
//! - **All internal math in `f64`**: No precision loss until the final output.
//! - **Safe math**: All divisions guarded, feedback clamped, cutoffs bounded.
//! - **No panics in DSP paths**: All edge cases handled gracefully.
//! - **Anti-zipper noise**: Every continuous parameter smoothed via 64-sample linear ramps.
//! - **Hard bypass**: Immediate passthrough when bypass is engaged.

use std::f64::consts::TAU;

// ────────────────────────────────────────────────────────────────────────────
// Constants
// ────────────────────────────────────────────────────────────────────────────

/// Minimum delay time in seconds (5 ms).
const MIN_DELAY_SECS: f64 = 0.005;

/// Maximum delay time in seconds (2 seconds).
const MAX_DELAY_SECS: f64 = 2.0;

/// Number of samples for parameter-smoothing ramps (anti-zipper noise).
const SMOOTH_SAMPLES: u64 = 64;

/// Minimum delay in samples — prevents the read head from colliding with
/// the write head and avoids division-by-zero in interpolation.
const MIN_DELAY_SAMPLES: f64 = 1.0;

/// Extra buffer slots beyond the maximum delay, giving cubic interpolation
/// safe room to read past the nominal read position.
const BUFFER_MARGIN: usize = 8;

/// One first-order complementary filter stage is 6 dB/oct.
const FILTER_STAGE_SLOPE_DB: f64 = 6.0;

/// Enough one-pole stages to cover the 1–100 dB/oct slope range.
const MAX_FILTER_STAGES: usize = 17;

/// Final output ceiling used by the always-on safety limiter.
const SAFETY_OUTPUT_CEILING: f64 = 1.0;

/// Emergency rail for internal delay/filter state. This is intentionally far
/// above any sane audio level, and only exists to prevent runaway feedback or
/// non-finite values from poisoning the engine.
const INTERNAL_SAMPLE_LIMIT: f64 = 1.0e6;

#[inline]
fn db_to_gain(db: f64) -> f64 {
    10.0f64.powf(db / 20.0)
}

#[inline]
fn protect_output_sample(sample: f64) -> f64 {
    if !sample.is_finite() {
        return 0.0;
    }

    sample.clamp(-SAFETY_OUTPUT_CEILING, SAFETY_OUTPUT_CEILING)
}

#[inline]
fn sanitize_internal_sample(sample: f64) -> f64 {
    if sample.is_finite() {
        sample.clamp(-INTERNAL_SAMPLE_LIMIT, INTERNAL_SAMPLE_LIMIT)
    } else {
        0.0
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Public Enums
// ────────────────────────────────────────────────────────────────────────────

/// Routing modes that determine how the L and R delay channels interact
/// in the feedback network and (for some modes) at the output stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoutingMode {
    /// Independent L/R channels with full crossfeed control via the
    /// `crossfeed_lr` / `crossfeed_rl` parameters.
    Customized,
    /// Straight: L→L, R→R with no crossfeed whatsoever.
    #[default]
    Straight,
    /// Full symmetric crossfeed: each channel's delayed signal crosses
    /// to the opposite channel (no self-feedback).
    Crossfeed,
    /// Same-channel feedback with rounded 10 % crossfeed.
    NinetyTen,
    /// Crossfeed with rounded 10 % same-channel feedback.
    TenNinety,
    /// Ping-pong from the left input side.
    PingPong,
    /// Ping-pong from the right input side.
    PingPongR,
    /// Pan from left delay toward right.
    Pan,
    /// Pan from right delay toward left.
    PanRl,
    /// Rotate toward the left side using phase-inverted feedback/crossfeed.
    Rotate,
    /// Rotate toward the right side using phase-inverted feedback/crossfeed.
    RotateR,
}

/// Per-channel input source selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    /// No input (muted).
    Off,
    /// Use the left input channel only.
    #[default]
    Left,
    /// Use the right input channel only.
    Right,
    /// Sum of left and right inputs (mono sum), scaled by 0.5.
    LeftPlusRight,
    /// Difference of left and right inputs (side signal), scaled by 0.5.
    LeftMinusRight,
}

/// Musical note values for tempo-sync quantization.
///
/// `T` variants are *triplet* divisions (2/3 of the straight value).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoteValue {
    /// Whole note (1/1) — 4 beats.
    Whole,
    /// Half note (1/2) — 2 beats.
    Half,
    /// Half-note triplet (1/2T) — 4/3 beats.
    HalfTriplet,
    /// Quarter note (1/4) — 1 beat.
    #[default]
    Quarter,
    /// Quarter-note triplet (1/4T) — 2/3 beats.
    QuarterTriplet,
    /// Eighth note (1/8) — 0.5 beats.
    Eighth,
    /// Eighth-note triplet (1/8T) — 1/3 beats.
    EighthTriplet,
    /// Sixteenth note (1/16) — 0.25 beats.
    Sixteenth,
    /// Sixteenth-note triplet (1/16T) — 1/6 beats.
    SixteenthTriplet,
    /// Thirty-second note (1/32) — 0.125 beats.
    ThirtySecond,
    /// Thirty-second-note triplet (1/32T) — 1/12 beats.
    ThirtySecondTriplet,
    /// Sixty-fourth note (1/64) — 0.0625 beats.
    SixtyFourth,
    /// Dotted half note (1/2.) — 3 beats.
    HalfDotted,
    /// Dotted quarter note (1/4.) — 1.5 beats.
    QuarterDotted,
    /// Dotted eighth note (1/8.) — 0.75 beats.
    EighthDotted,
    /// Dotted sixteenth note (1/16.) — 0.375 beats.
    SixteenthDotted,
    /// Dotted thirty-second note (1/32.) — 0.1875 beats.
    ThirtySecondDotted,
}

impl NoteValue {
    /// Returns the delay duration in seconds for this note value at the
    /// given tempo in BPM.
    ///
    /// The formula is: `duration = (beats_per_note / bpm) * 60`.
    /// BPM is clamped to a minimum of 1.0 to prevent division by zero.
    #[inline]
    pub fn duration_seconds(&self, bpm: f64) -> f64 {
        let beats = match self {
            NoteValue::Whole => 4.0,
            NoteValue::Half => 2.0,
            NoteValue::HalfTriplet => 4.0 / 3.0,
            NoteValue::Quarter => 1.0,
            NoteValue::QuarterTriplet => 2.0 / 3.0,
            NoteValue::Eighth => 0.5,
            NoteValue::EighthTriplet => 1.0 / 3.0,
            NoteValue::Sixteenth => 0.25,
            NoteValue::SixteenthTriplet => 1.0 / 6.0,
            NoteValue::ThirtySecond => 0.125,
            NoteValue::ThirtySecondTriplet => 1.0 / 12.0,
            NoteValue::SixtyFourth => 0.0625,
            NoteValue::HalfDotted => 3.0,
            NoteValue::QuarterDotted => 1.5,
            NoteValue::EighthDotted => 0.75,
            NoteValue::SixteenthDotted => 0.375,
            NoteValue::ThirtySecondDotted => 0.1875,
        };
        let safe_bpm = bpm.max(1.0);
        (beats / safe_bpm) * 60.0
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Public Parameter & Config Structs
// ────────────────────────────────────────────────────────────────────────────

/// All parameters required for a single `process()` call.
///
/// The host or UI layer fills this struct per-sample (or per-block) and
/// passes it by reference.  Every continuous parameter is smoothed
/// internally, so it is safe to update them abruptly.
#[derive(Debug, Clone)]
pub struct DelayParams {
    // ── Input/output level trims ─────────────────────────────────────
    /// Input trim before the delay network, in dB.
    pub input_level_db: f64,
    /// Output trim after the delay network, in dB.
    pub output_level_db: f64,

    // ── Input selection ───────────────────────────────────────────────
    /// Input source for the left delay channel.
    pub input_mode_l: InputMode,
    /// Input source for the right delay channel.
    pub input_mode_r: InputMode,

    // ── Delay time (seconds) — used when `tempo_sync` is off ──────────
    /// Base delay time for the left channel in seconds.
    pub delay_time_l: f64,
    /// Base delay time for the right channel in seconds.
    pub delay_time_r: f64,

    // ── Filter cutoffs (Hz) ───────────────────────────────────────────
    /// Low-cut (high-pass) frequency for the left channel. Range: 20–20 000 Hz.
    pub low_cut_l: f64,
    /// Low-cut (high-pass) frequency for the right channel.
    pub low_cut_r: f64,
    /// Low-cut slope for the left channel. Range: 1–100 dB/oct.
    pub low_cut_slope_l: f64,
    /// Low-cut slope for the right channel. Range: 1–100 dB/oct.
    pub low_cut_slope_r: f64,
    /// High-cut (low-pass) frequency for the left channel. Range: 20–20 000 Hz.
    pub high_cut_l: f64,
    /// High-cut (low-pass) frequency for the right channel.
    pub high_cut_r: f64,
    /// High-cut slope for the left channel. Range: 1–100 dB/oct.
    pub high_cut_slope_l: f64,
    /// High-cut slope for the right channel. Range: 1–100 dB/oct.
    pub high_cut_slope_r: f64,

    // ── Feedback ──────────────────────────────────────────────────────
    /// Feedback amount for the left channel (0.0 – 1.0).
    pub feedback_l: f64,
    /// Feedback amount for the right channel (0.0 – 1.0).
    pub feedback_r: f64,
    /// Invert left-channel feedback phase (180° flip).
    pub feedback_phase_l: bool,
    /// Invert right-channel feedback phase (180° flip).
    pub feedback_phase_r: bool,

    // ── Crossfeed ─────────────────────────────────────────────────────
    /// L→R crossfeed amount (0.0 – 1.0).
    pub crossfeed_lr: f64,
    /// R→L crossfeed amount (0.0 – 1.0).
    pub crossfeed_rl: f64,
    /// Invert the L→R crossfeed path.
    pub crossfeed_phase_lr: bool,
    /// Invert the R→L crossfeed path.
    pub crossfeed_phase_rl: bool,

    // ── Routing ───────────────────────────────────────────────────────
    /// Active routing mode.
    pub routing: RoutingMode,

    // ── Tempo sync ────────────────────────────────────────────────────
    /// When `true`, delay time is derived from `tempo_bpm` and `note_l`/`note_r`.
    pub tempo_sync: bool,
    /// Host tempo in BPM.
    pub tempo_bpm: f64,
    /// Note value for the left channel (used when `tempo_sync` is on).
    pub note_l: NoteValue,
    /// Note value for the right channel (used when `tempo_sync` is on).
    pub note_r: NoteValue,
    /// Deviation from the quantized delay time for the left channel in
    /// cents (±100). Applied as `2^(deviation/1200)`.
    pub deviation_l: f64,
    /// Deviation from the quantized delay time for the right channel in cents.
    pub deviation_r: f64,

    // ── /:2 and ×2 ───────────────────────────────────────────────────
    /// Halve the left-channel delay time.
    pub halve_l: bool,
    /// Halve the right-channel delay time.
    pub halve_r: bool,
    /// Double the left-channel delay time.
    pub double_l: bool,
    /// Double the right-channel delay time.
    pub double_r: bool,

    // ── Output ────────────────────────────────────────────────────────
    /// Dry/wet mix for the left channel (0.0 = fully dry, 1.0 = fully wet).
    pub output_mix_l: f64,
    /// Dry/wet mix for the right channel (0.0 = fully dry, 1.0 = fully wet).
    pub output_mix_r: f64,

    // ── Bypass ────────────────────────────────────────────────────────
    /// Hard-bypass flag. When enabled, the engine immediately passes input
    /// to output without processing the delay network or level trims.
    pub bypass: bool,

    // ── Stereo link ───────────────────────────────────────────────────
    /// UI gesture state. The DSP intentionally does not collapse L/R
    /// parameters when this is enabled; linked controls preserve channel
    /// ratios where possible at the parameter layer.
    pub stereo_link: bool,
}

impl Default for DelayParams {
    fn default() -> Self {
        Self {
            input_level_db: 0.0,
            output_level_db: 0.0,
            input_mode_l: InputMode::Left,
            input_mode_r: InputMode::Right,
            delay_time_l: 0.5,
            delay_time_r: 0.5,
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
            routing: RoutingMode::default(),
            tempo_sync: false,
            tempo_bpm: 120.0,
            note_l: NoteValue::default(),
            note_r: NoteValue::default(),
            deviation_l: 0.0,
            deviation_r: 0.0,
            halve_l: false,
            halve_r: false,
            double_l: false,
            double_r: false,
            output_mix_l: 1.0,
            output_mix_r: 1.0,
            bypass: false,
            stereo_link: false,
        }
    }
}

/// Creation-time configuration for the delay engine.
///
/// These values are set once when the engine is instantiated and typically
/// do not change during the lifetime of the plugin.
#[derive(Debug, Clone)]
pub struct DelayEngineConfig {
    /// Maximum sample rate the engine should allocate buffers for.
    /// Defaults to 192 000 Hz.  If `set_sample_rate` is later called with
    /// a higher rate, the buffers will be reallocated automatically.
    pub max_sample_rate: f64,
}

impl Default for DelayEngineConfig {
    fn default() -> Self {
        Self {
            max_sample_rate: 192_000.0,
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Internal: Smoothed Parameter
// ────────────────────────────────────────────────────────────────────────────

/// A linear-ramp smoother that eliminates zipper noise on parameter changes.
///
/// When `set_target` is called with a new value, the smoother computes a
/// per-sample increment that will glide from the current value to the
/// target over `smooth_samples` samples.  Once the ramp completes the
/// value snaps exactly to the target.
#[derive(Debug, Clone)]
struct SmoothedValue {
    /// Current output value (advanced every sample).
    current: f64,
    /// Value we are ramping towards.
    target: f64,
    /// Per-sample increment = `(target - start) / smooth_samples`.
    increment: f64,
    /// How many samples remain in the current ramp.
    samples_remaining: u64,
    /// Ramp length in samples.
    smooth_samples: u64,
}

impl SmoothedValue {
    /// Create a new smoother initialised to `initial` with the given ramp
    /// length in samples.
    fn new(initial: f64, smooth_samples: u64) -> Self {
        Self {
            current: initial,
            target: initial,
            increment: 0.0,
            samples_remaining: 0,
            smooth_samples: smooth_samples.max(1), // at least 1 sample
        }
    }

    /// Set a new target value.  If the target equals the current target
    /// (within `f64::EPSILON`), nothing happens — this avoids
    /// re-triggering a ramp for identical values.
    fn set_target(&mut self, target: f64) {
        // Avoid floating-point noise triggering constant re-ramps.
        if (target - self.target).abs() < f64::EPSILON {
            return;
        }
        self.target = target;
        let diff = target - self.current;
        self.increment = diff / self.smooth_samples as f64;
        self.samples_remaining = self.smooth_samples;
    }

    /// Advance the smoother by one sample and return the current value.
    #[inline]
    fn next(&mut self) -> f64 {
        if self.samples_remaining > 0 {
            self.current += self.increment;
            self.samples_remaining -= 1;
            if self.samples_remaining == 0 {
                // Snap exactly to avoid accumulated floating-point drift.
                self.current = self.target;
            }
        }
        self.current
    }

    /// Immediately jump to a value (no ramp).  Useful during `reset()`.
    fn reset(&mut self, value: f64) {
        self.current = value;
        self.target = value;
        self.increment = 0.0;
        self.samples_remaining = 0;
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Internal: Delay Line (circular buffer, cubic Hermite interpolation)
// ────────────────────────────────────────────────────────────────────────────

/// A circular-buffer delay line with **cubic Hermite interpolation** and
/// full `f64` precision.
///
/// The buffer length is always a power of two so that modular arithmetic
/// can be performed with a bitmask instead of the `%` operator.
#[derive(Debug, Clone)]
struct DelayLine {
    /// Sample storage.
    buffer: Vec<f64>,
    /// Index of the next slot to write.
    write_pos: usize,
    /// `buffer.len() - 1` — used for fast index wrapping via `& mask`.
    mask: usize,
    /// Current sample rate (used to convert seconds → samples).
    sample_rate: f64,
}

impl DelayLine {
    /// Create a delay line sized for `MAX_DELAY_SECS` at `alloc_rate` Hz,
    /// but operating at `working_rate` Hz for delay-time calculations.
    ///
    /// This allows pre-allocating a buffer large enough for a high sample
    /// rate while correctly computing delay positions at a lower rate.
    fn new(alloc_rate: f64, working_rate: f64) -> Self {
        let ar = alloc_rate.max(1.0);
        let wr = working_rate.max(1.0);
        let min_slots = (MAX_DELAY_SECS * ar).ceil() as usize + BUFFER_MARGIN;
        let buf_size = min_slots.next_power_of_two();
        Self {
            buffer: vec![0.0; buf_size],
            write_pos: 0,
            mask: buf_size - 1,
            sample_rate: wr,
        }
    }

    /// Reinitialise the delay line for a new sample rate.
    ///
    /// The buffer is only reallocated when it would be too small for the
    /// new rate; otherwise only the working sample-rate field is updated
    /// (zero-cost).
    fn set_sample_rate(&mut self, sample_rate: f64) {
        let sr = sample_rate.max(1.0);
        if (sr - self.sample_rate).abs() < f64::EPSILON {
            return;
        }
        self.sample_rate = sr;
        // Reallocate only if the existing buffer is too small.
        let min_slots = (MAX_DELAY_SECS * sr).ceil() as usize + BUFFER_MARGIN;
        let needed = min_slots.next_power_of_two();
        if needed > self.buffer.len() {
            self.buffer = vec![0.0; needed];
            self.write_pos = 0;
            self.mask = needed - 1;
        }
    }

    /// Read a sample from the delay line at the given delay time in
    /// seconds, using **4-point cubic Hermite interpolation**.
    ///
    /// The Hermite interpolant preserves continuity of the first
    /// derivative, yielding smooth pitch transitions when the delay time
    /// is modulated and minimising aliasing artifacts.
    ///
    /// # Safety
    ///
    /// The delay time is clamped to `[MIN_DELAY_SAMPLES / sr,
    /// buffer_len - margin]` so the read head can never collide with the
    /// write head and the four interpolation taps are always valid.
    #[inline]
    fn read(&self, delay_time_secs: f64) -> f64 {
        let delay_samples = delay_time_secs * self.sample_rate;
        // Clamp to safe range.
        let delay_samples = delay_samples.max(MIN_DELAY_SAMPLES);
        let max_delay = (self.buffer.len() - BUFFER_MARGIN) as f64;
        let delay_samples = delay_samples.min(max_delay);

        // Fractional read position = write_pos - delay_samples.
        let read_pos = self.write_pos as f64 - delay_samples;
        let int_pos = read_pos.floor() as isize;
        let frac = read_pos - int_pos as f64;

        // Fetch the four neighbouring samples needed for cubic
        // interpolation.  `sample_at` handles circular wrapping.
        let y0 = self.sample_at(int_pos - 1);
        let y1 = self.sample_at(int_pos);
        let y2 = self.sample_at(int_pos + 1);
        let y3 = self.sample_at(int_pos + 2);

        // Cubic Hermite interpolation (4-point, 3rd-order).
        //
        // Coefficients chosen so that:
        //   - At frac = 0 the output equals y1 (exact sample).
        //   - The first derivative matches the central difference at
        //     y1, giving smooth modulation.
        let c0 = y1;
        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);

        sanitize_internal_sample(((c3 * frac + c2) * frac + c1) * frac + c0)
    }

    /// Write a sample at the current write position and advance the
    /// write pointer by one slot.
    #[inline]
    fn write_and_advance(&mut self, sample: f64) {
        self.buffer[self.write_pos] = sanitize_internal_sample(sample);
        self.write_pos = (self.write_pos + 1) & self.mask;
    }

    /// Retrieve a sample at an arbitrary (possibly negative) index,
    /// wrapping around the circular buffer.
    ///
    /// Because `buffer.len()` is always a power of two, casting a
    /// negative `isize` to `usize` and masking produces the correct
    /// wrapped index in two's-complement arithmetic.
    #[inline]
    fn sample_at(&self, index: isize) -> f64 {
        self.buffer[(index as usize) & self.mask]
    }

    /// Clear the buffer and reset the write pointer.
    fn reset(&mut self) {
        for s in self.buffer.iter_mut() {
            *s = 0.0;
        }
        self.write_pos = 0;
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Internal: Complementary Variable-Slope Filter
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComplementaryMode {
    LowPass,
    HighPass,
}

#[derive(Debug, Clone, Copy)]
struct OnePoleLowPass {
    a: f64,
    z: f64,
}

impl OnePoleLowPass {
    fn new() -> Self {
        Self { a: 1.0, z: 0.0 }
    }

    fn set_cutoff(&mut self, cutoff_hz: f64, sample_rate: f64) {
        let safe_sr = sample_rate.max(1.0);
        let nyquist = safe_sr * 0.499;
        let fc = cutoff_hz.max(1.0).min(nyquist);
        self.a = 1.0 - (-TAU * fc / safe_sr).exp();
    }

    #[inline]
    fn process(&mut self, input: f64) -> f64 {
        let input = sanitize_internal_sample(input);
        if !self.z.is_finite() {
            self.z = 0.0;
        }
        self.z += self.a * (input - self.z);
        self.z = sanitize_internal_sample(self.z);
        self.z
    }

    fn reset(&mut self) {
        self.z = 0.0;
    }
}

/// Cascaded one-pole complementary filter with continuously variable slope.
///
/// Every stage computes `lo = LP(x)` and `hi = x - lo`. The high-pass and
/// low-pass outputs therefore recombine to the exact input at every sample.
/// The continuous 1–100 dB/oct slope control is implemented by cascading full
/// 6 dB/oct stages and blending the next fractional stage.
#[derive(Debug, Clone)]
struct ComplementaryFilter {
    stages: [OnePoleLowPass; MAX_FILTER_STAGES],
    order: f64,
    cutoff_hz: f64,
    sample_rate: f64,
}

impl ComplementaryFilter {
    fn new() -> Self {
        Self {
            stages: [OnePoleLowPass::new(); MAX_FILTER_STAGES],
            order: 2.0,
            cutoff_hz: 20.0,
            sample_rate: 0.0,
        }
    }

    fn update(&mut self, cutoff_hz: f64, slope_db_oct: f64, sample_rate: f64) {
        let cutoff_hz = cutoff_hz.clamp(20.0, 20000.0);
        let sample_rate = sample_rate.max(1.0);
        if (cutoff_hz - self.cutoff_hz).abs() > f64::EPSILON
            || (sample_rate - self.sample_rate).abs() > f64::EPSILON
        {
            for stage in &mut self.stages {
                stage.set_cutoff(cutoff_hz, sample_rate);
            }
            self.cutoff_hz = cutoff_hz;
            self.sample_rate = sample_rate;
        }
        self.order = (slope_db_oct.clamp(1.0, 100.0) / FILTER_STAGE_SLOPE_DB)
            .clamp(1.0 / FILTER_STAGE_SLOPE_DB, MAX_FILTER_STAGES as f64);
    }

    #[inline]
    fn process(&mut self, input: f64, mode: ComplementaryMode) -> f64 {
        if mode == ComplementaryMode::LowPass && self.cutoff_hz >= 19_999.999 {
            return input;
        }

        let full_stages = self.order.floor() as usize;
        let fractional = self.order - full_stages as f64;

        match mode {
            ComplementaryMode::LowPass => {
                let mut low = input;
                for stage in self.stages.iter_mut().take(full_stages) {
                    low = stage.process(low);
                }

                if fractional > 0.000_001 && full_stages < MAX_FILTER_STAGES {
                    let next_low = self.stages[full_stages].process(low);
                    low += (next_low - low) * fractional;
                }

                low
            }
            ComplementaryMode::HighPass => {
                let mut high = input;
                for stage in self.stages.iter_mut().take(full_stages) {
                    let low = stage.process(high);
                    high -= low;
                }

                if fractional > 0.000_001 && full_stages < MAX_FILTER_STAGES {
                    let low = self.stages[full_stages].process(high);
                    let next_high = high - low;
                    high += (next_high - high) * fractional;
                }

                high
            }
        }
    }

    fn reset(&mut self) {
        for stage in &mut self.stages {
            stage.reset();
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Main Engine
// ────────────────────────────────────────────────────────────────────────────

/// The main DSP engine for the **Nebula Stereo Delay** plugin.
///
/// # Precision
///
/// Every calculation is performed in `f64`.  The only place `f32`
/// should appear is when the caller converts the `(f64, f64)` output of
/// [`process`](Self::process) to `f32` for the audio host.
///
/// # Thread Safety
///
/// `DelayEngine` is **not** `Sync` — it is designed to be used from a
/// single real-time audio thread.  Parameter updates from the UI should
/// be communicated via atomics or a lock-free queue to the audio thread,
/// which then writes them into a `DelayParams` before each `process`
/// call.
///
/// # Example (simplified)
///
/// ```ignore
/// let mut engine = DelayEngine::new(44_100.0);
/// let params = DelayParams::default();
///
/// for frame in audio_buffer {
///     let (out_l, out_r) = engine.process(frame.l as f64, frame.r as f64, &params);
///     frame.l = out_l as f32;
///     frame.r = out_r as f32;
/// }
/// ```
pub struct DelayEngine {
    // ── Sample rate ───────────────────────────────────────────────────
    sample_rate: f64,

    // ── Delay lines ───────────────────────────────────────────────────
    delay_l: DelayLine,
    delay_r: DelayLine,

    // ── Complementary filters: low-cut (HP) and high-cut (LP) per channel ───
    low_cut_l: ComplementaryFilter,
    low_cut_r: ComplementaryFilter,
    high_cut_l: ComplementaryFilter,
    high_cut_r: ComplementaryFilter,

    // ── Smoothed parameters (anti-zipper noise) ───────────────────────
    smooth_input_gain: SmoothedValue,
    smooth_output_gain: SmoothedValue,
    smooth_delay_l: SmoothedValue,
    smooth_delay_r: SmoothedValue,
    smooth_low_cut_l: SmoothedValue,
    smooth_low_cut_r: SmoothedValue,
    smooth_low_cut_slope_l: SmoothedValue,
    smooth_low_cut_slope_r: SmoothedValue,
    smooth_high_cut_l: SmoothedValue,
    smooth_high_cut_r: SmoothedValue,
    smooth_high_cut_slope_l: SmoothedValue,
    smooth_high_cut_slope_r: SmoothedValue,
    smooth_feedback_l: SmoothedValue,
    smooth_feedback_r: SmoothedValue,
    smooth_crossfeed_lr: SmoothedValue,
    smooth_crossfeed_rl: SmoothedValue,
    smooth_mix_l: SmoothedValue,
    smooth_mix_r: SmoothedValue,

    // ── Hard bypass ───────────────────────────────────────────────────
    /// Internal bypass latch set by `set_bypass()`.
    bypass_latch: bool,

    // ── Config ────────────────────────────────────────────────────────
    config: DelayEngineConfig,
}

/// One processed stereo frame plus the exact post-trim levels used for meters.
#[derive(Debug, Clone, Copy)]
pub struct ProcessedFrame {
    pub output_l: f64,
    pub output_r: f64,
    pub input_meter_l: f64,
    pub input_meter_r: f64,
    pub output_meter_l: f64,
    pub output_meter_r: f64,
}

struct FeedbackInputs {
    routing: RoutingMode,
    fb_l: f64,
    fb_r: f64,
    cf_lr: f64,
    cf_rl: f64,
    sign_l: f64,
    sign_r: f64,
    sign_cf_lr: f64,
    sign_cf_rl: f64,
    filt_l: f64,
    filt_r: f64,
}

impl DelayEngine {
    // ──────────────────────────────────────────────────────────────────
    // Public API
    // ──────────────────────────────────────────────────────────────────

    /// Return a reference to the engine's configuration.
    #[allow(dead_code)]
    pub fn config(&self) -> &DelayEngineConfig {
        &self.config
    }

    /// Create a new delay engine initialised for the given sample rate.
    ///
    /// The delay-line buffers are allocated to hold `MAX_DELAY_SECS`
    /// seconds at the provided rate (or the configured
    /// `max_sample_rate`, whichever is larger) so that no reallocation
    /// is needed for sample-rate changes within that range.
    pub fn new(sample_rate: f64) -> Self {
        let config = DelayEngineConfig::default();
        let sr = sample_rate.max(1.0);
        // Allocate delay-line buffers for the larger of the requested rate
        // and the configured max_sample_rate so that a subsequent call to
        // set_sample_rate() within that range never requires reallocation.
        let alloc_sr = sr.max(config.max_sample_rate);

        Self {
            sample_rate: sr,
            delay_l: DelayLine::new(alloc_sr, sr),
            delay_r: DelayLine::new(alloc_sr, sr),
            low_cut_l: ComplementaryFilter::new(),
            low_cut_r: ComplementaryFilter::new(),
            high_cut_l: ComplementaryFilter::new(),
            high_cut_r: ComplementaryFilter::new(),
            smooth_input_gain: SmoothedValue::new(1.0, SMOOTH_SAMPLES),
            smooth_output_gain: SmoothedValue::new(1.0, SMOOTH_SAMPLES),
            smooth_delay_l: SmoothedValue::new(0.5, SMOOTH_SAMPLES),
            smooth_delay_r: SmoothedValue::new(0.5, SMOOTH_SAMPLES),
            smooth_low_cut_l: SmoothedValue::new(20.0, SMOOTH_SAMPLES),
            smooth_low_cut_r: SmoothedValue::new(20.0, SMOOTH_SAMPLES),
            smooth_low_cut_slope_l: SmoothedValue::new(12.0, SMOOTH_SAMPLES),
            smooth_low_cut_slope_r: SmoothedValue::new(12.0, SMOOTH_SAMPLES),
            smooth_high_cut_l: SmoothedValue::new(20000.0, SMOOTH_SAMPLES),
            smooth_high_cut_r: SmoothedValue::new(20000.0, SMOOTH_SAMPLES),
            smooth_high_cut_slope_l: SmoothedValue::new(12.0, SMOOTH_SAMPLES),
            smooth_high_cut_slope_r: SmoothedValue::new(12.0, SMOOTH_SAMPLES),
            smooth_feedback_l: SmoothedValue::new(0.0, SMOOTH_SAMPLES),
            smooth_feedback_r: SmoothedValue::new(0.0, SMOOTH_SAMPLES),
            smooth_crossfeed_lr: SmoothedValue::new(0.0, SMOOTH_SAMPLES),
            smooth_crossfeed_rl: SmoothedValue::new(0.0, SMOOTH_SAMPLES),
            smooth_mix_l: SmoothedValue::new(0.5, SMOOTH_SAMPLES),
            smooth_mix_r: SmoothedValue::new(0.5, SMOOTH_SAMPLES),
            bypass_latch: false,
            config,
        }
    }

    /// Change the operating sample rate.
    ///
    /// If the new rate exceeds the buffer capacity the delay lines are
    /// reallocated (which causes a brief audio glitch, so only call this
    /// when the host genuinely changes the project sample rate).
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        let sr = sample_rate.max(1.0);
        self.sample_rate = sr;
        self.delay_l.set_sample_rate(sr);
        self.delay_r.set_sample_rate(sr);
    }

    /// Reset all internal state to silence.
    ///
    /// Clears the delay buffers, filter states, and snaps all smoothed
    /// parameters to their current targets.
    pub fn reset(&mut self) {
        self.delay_l.reset();
        self.delay_r.reset();
        self.low_cut_l.reset();
        self.low_cut_r.reset();
        self.high_cut_l.reset();
        self.high_cut_r.reset();
        // Snap smoothers to their current targets (no ramp).
        self.smooth_input_gain.reset(self.smooth_input_gain.target);
        self.smooth_output_gain
            .reset(self.smooth_output_gain.target);
        self.smooth_delay_l.reset(self.smooth_delay_l.target);
        self.smooth_delay_r.reset(self.smooth_delay_r.target);
        self.smooth_low_cut_l.reset(self.smooth_low_cut_l.target);
        self.smooth_low_cut_r.reset(self.smooth_low_cut_r.target);
        self.smooth_low_cut_slope_l
            .reset(self.smooth_low_cut_slope_l.target);
        self.smooth_low_cut_slope_r
            .reset(self.smooth_low_cut_slope_r.target);
        self.smooth_high_cut_l.reset(self.smooth_high_cut_l.target);
        self.smooth_high_cut_r.reset(self.smooth_high_cut_r.target);
        self.smooth_high_cut_slope_l
            .reset(self.smooth_high_cut_slope_l.target);
        self.smooth_high_cut_slope_r
            .reset(self.smooth_high_cut_slope_r.target);
        self.smooth_feedback_l.reset(self.smooth_feedback_l.target);
        self.smooth_feedback_r.reset(self.smooth_feedback_r.target);
        self.smooth_crossfeed_lr
            .reset(self.smooth_crossfeed_lr.target);
        self.smooth_crossfeed_rl
            .reset(self.smooth_crossfeed_rl.target);
        self.smooth_mix_l.reset(self.smooth_mix_l.target);
        self.smooth_mix_r.reset(self.smooth_mix_r.target);
    }

    /// Engage or release hard bypass.
    pub fn set_bypass(&mut self, bypass: bool) {
        self.bypass_latch = bypass;
    }

    /// Process a single sample pair and return the processed output.
    ///
    /// # Arguments
    ///
    /// * `input_l` — Left input sample (any `f64` value).
    /// * `input_r` — Right input sample (any `f64` value).
    /// * `params`  — All delay parameters for this sample.
    ///
    /// # Returns
    ///
    /// `(left_output, right_output)` in `f64`.  Convert to `f32` at the
    /// very end of the plugin's output stage.
    pub fn process(&mut self, input_l: f64, input_r: f64, params: &DelayParams) -> (f64, f64) {
        let frame = self.process_frame(input_l, input_r, params);
        (frame.output_l, frame.output_r)
    }

    /// Process one stereo sample and return both audio and exact meter taps.
    pub fn process_frame(
        &mut self,
        input_l: f64,
        input_r: f64,
        params: &DelayParams,
    ) -> ProcessedFrame {
        // ── 1. Parameter snapshot ─────────────────────────────────────
        // Stereo Link is handled by the editor/automation layer. The audio
        // engine keeps the left and right values independent so enabling the
        // link never forces both channels to identical settings.
        let p = params.clone();

        // Hard bypass is intentionally immediate. It skips trims, delay,
        // filters, feedback, and smoothing; only the final safety catch
        // remains so the plugin never emits non-finite samples.
        if p.bypass || self.bypass_latch {
            let out_l = protect_output_sample(input_l);
            let out_r = protect_output_sample(input_r);
            return ProcessedFrame {
                output_l: out_l,
                output_r: out_r,
                input_meter_l: input_l,
                input_meter_r: input_r,
                output_meter_l: out_l,
                output_meter_r: out_r,
            };
        }

        let input_gain_target = db_to_gain(p.input_level_db.clamp(-50.0, 50.0));
        let output_gain_target = db_to_gain(p.output_level_db.clamp(-50.0, 50.0));
        self.smooth_input_gain.set_target(input_gain_target);
        self.smooth_output_gain.set_target(output_gain_target);
        let s_input_gain = self.smooth_input_gain.next();
        let s_output_gain = self.smooth_output_gain.next();
        let trimmed_input_l = input_l * s_input_gain;
        let trimmed_input_r = input_r * s_input_gain;

        // ── 2. Input mode selection ───────────────────────────────────
        let in_l = Self::apply_input_mode(p.input_mode_l, trimmed_input_l, trimmed_input_r);
        let in_r = Self::apply_input_mode(p.input_mode_r, trimmed_input_l, trimmed_input_r);

        // ── 3. Effective delay times ──────────────────────────────────
        let eff_delay_l = Self::effective_delay_time(
            p.delay_time_l,
            p.tempo_sync,
            p.tempo_bpm,
            p.note_l,
            p.deviation_l,
            p.halve_l,
            p.double_l,
        );
        let eff_delay_r = Self::effective_delay_time(
            p.delay_time_r,
            p.tempo_sync,
            p.tempo_bpm,
            p.note_r,
            p.deviation_r,
            p.halve_r,
            p.double_r,
        );

        // ── 4. Push new targets into smoothers ────────────────────────
        self.smooth_delay_l.set_target(eff_delay_l);
        self.smooth_delay_r.set_target(eff_delay_r);
        self.smooth_low_cut_l
            .set_target(p.low_cut_l.clamp(20.0, 20000.0));
        self.smooth_low_cut_r
            .set_target(p.low_cut_r.clamp(20.0, 20000.0));
        self.smooth_low_cut_slope_l
            .set_target(p.low_cut_slope_l.clamp(1.0, 100.0));
        self.smooth_low_cut_slope_r
            .set_target(p.low_cut_slope_r.clamp(1.0, 100.0));
        self.smooth_high_cut_l
            .set_target(p.high_cut_l.clamp(20.0, 20000.0));
        self.smooth_high_cut_r
            .set_target(p.high_cut_r.clamp(20.0, 20000.0));
        self.smooth_high_cut_slope_l
            .set_target(p.high_cut_slope_l.clamp(1.0, 100.0));
        self.smooth_high_cut_slope_r
            .set_target(p.high_cut_slope_r.clamp(1.0, 100.0));
        self.smooth_feedback_l
            .set_target(p.feedback_l.clamp(0.0, 1.0));
        self.smooth_feedback_r
            .set_target(p.feedback_r.clamp(0.0, 1.0));
        self.smooth_crossfeed_lr
            .set_target(p.crossfeed_lr.clamp(0.0, 1.0));
        self.smooth_crossfeed_rl
            .set_target(p.crossfeed_rl.clamp(0.0, 1.0));
        self.smooth_mix_l.set_target(p.output_mix_l.clamp(0.0, 1.0));
        self.smooth_mix_r.set_target(p.output_mix_r.clamp(0.0, 1.0));

        // ── 5. Advance smoothers and capture values ───────────────────
        let s_delay_l = self.smooth_delay_l.next();
        let s_delay_r = self.smooth_delay_r.next();
        let s_lc_l = self.smooth_low_cut_l.next();
        let s_lc_r = self.smooth_low_cut_r.next();
        let s_lcs_l = self.smooth_low_cut_slope_l.next();
        let s_lcs_r = self.smooth_low_cut_slope_r.next();
        let s_hc_l = self.smooth_high_cut_l.next();
        let s_hc_r = self.smooth_high_cut_r.next();
        let s_hcs_l = self.smooth_high_cut_slope_l.next();
        let s_hcs_r = self.smooth_high_cut_slope_r.next();
        let s_fb_l = self.smooth_feedback_l.next();
        let s_fb_r = self.smooth_feedback_r.next();
        let s_cf_lr = self.smooth_crossfeed_lr.next();
        let s_cf_rl = self.smooth_crossfeed_rl.next();
        let s_mix_l = self.smooth_mix_l.next();
        let s_mix_r = self.smooth_mix_r.next();

        // ── 6. Update complementary filters from smoothed cutoffs/slopes ─────
        self.low_cut_l.update(s_lc_l, s_lcs_l, self.sample_rate);
        self.low_cut_r.update(s_lc_r, s_lcs_r, self.sample_rate);
        self.high_cut_l.update(s_hc_l, s_hcs_l, self.sample_rate);
        self.high_cut_r.update(s_hc_r, s_hcs_r, self.sample_rate);

        // ── 7. Read from delay lines (cubic interpolation) ────────────
        let raw_l = self.delay_l.read(s_delay_l);
        let raw_r = self.delay_r.read(s_delay_r);

        // ── 8. Apply complementary filters (HPF then LPF) ────────────
        let hp_l = self.low_cut_l.process(raw_l, ComplementaryMode::HighPass);
        let hp_r = self.low_cut_r.process(raw_r, ComplementaryMode::HighPass);
        let filt_l = self.high_cut_l.process(hp_l, ComplementaryMode::LowPass);
        let filt_r = self.high_cut_r.process(hp_r, ComplementaryMode::LowPass);

        // ── 9. Compute phase-inversion sign bits ──────────────────────
        let sign_l: f64 = if p.feedback_phase_l { -1.0 } else { 1.0 };
        let sign_r: f64 = if p.feedback_phase_r { -1.0 } else { 1.0 };
        let sign_cf_lr: f64 = if p.crossfeed_phase_lr { -1.0 } else { 1.0 };
        let sign_cf_rl: f64 = if p.crossfeed_phase_rl { -1.0 } else { 1.0 };

        // ── 10. Routing: compute feedback signals for each delay line ──
        let (fb_to_l, fb_to_r) = Self::compute_feedback(FeedbackInputs {
            routing: p.routing,
            fb_l: s_fb_l,
            fb_r: s_fb_r,
            cf_lr: s_cf_lr,
            cf_rl: s_cf_rl,
            sign_l,
            sign_r,
            sign_cf_lr,
            sign_cf_rl,
            filt_l,
            filt_r,
        });

        // ── 11. Write input + feedback into delay lines ───────────────
        self.delay_l.write_and_advance(in_l + fb_to_l);
        self.delay_r.write_and_advance(in_r + fb_to_r);

        // ── 12. Output routing ────────────────────────────────────────
        let (wet_l, wet_r) = Self::apply_output_routing(p.routing, filt_l, filt_r);

        // ── 13. Dry/wet mix ───────────────────────────────────────────
        //   out = dry * (1 - mix) + wet * mix
        let out_l = in_l * (1.0 - s_mix_l) + wet_l * s_mix_l;
        let out_r = in_r * (1.0 - s_mix_r) + wet_r * s_mix_r;

        let out_l = protect_output_sample(out_l * s_output_gain);
        let out_r = protect_output_sample(out_r * s_output_gain);

        ProcessedFrame {
            output_l: out_l,
            output_r: out_r,
            input_meter_l: trimmed_input_l,
            input_meter_r: trimmed_input_r,
            output_meter_l: out_l,
            output_meter_r: out_r,
        }
    }

    // ──────────────────────────────────────────────────────────────────
    // Private helpers
    // ──────────────────────────────────────────────────────────────────

    /// Apply input-mode selection to derive a single channel's input from
    /// the stereo pair.
    #[inline]
    fn apply_input_mode(mode: InputMode, in_l: f64, in_r: f64) -> f64 {
        match mode {
            InputMode::Off => 0.0,
            InputMode::Left => in_l,
            InputMode::Right => in_r,
            InputMode::LeftPlusRight => (in_l + in_r) * 0.5,
            InputMode::LeftMinusRight => (in_l - in_r) * 0.5,
        }
    }

    /// Compute the effective delay time in seconds, accounting for tempo
    /// sync, deviation, and the /:2 and ×2 modifiers.
    ///
    /// Returns a value clamped to `[MIN_DELAY_SECS, MAX_DELAY_SECS]`.
    fn effective_delay_time(
        base_time: f64,
        tempo_sync: bool,
        bpm: f64,
        note: NoteValue,
        deviation_cents: f64,
        halve: bool,
        double: bool,
    ) -> f64 {
        // Start with either the tempo-synced value or the free-running time.
        let mut t = if tempo_sync {
            note.duration_seconds(bpm)
        } else {
            base_time
        };

        // Deviation: ±100 cents → ratio = 2^(cents / 1200).
        let cents = deviation_cents.clamp(-100.0, 100.0);
        t *= (2.0_f64).powf(cents / 1200.0);

        // /:2 and ×2 modifiers (both active → net 1×).
        if halve {
            t *= 0.5;
        }
        if double {
            t *= 2.0;
        }

        t.clamp(MIN_DELAY_SECS, MAX_DELAY_SECS)
    }

    /// Compute the feedback signal that will be added to each delay
    /// line's input, based on the active routing mode.
    ///
    /// Returns `(feedback_to_L_delay, feedback_to_R_delay)`.
    ///
    /// # Routing matrix per mode
    ///
    /// | Mode                 | Self-L | Cross L→R | Cross R→L | Self-R |
    /// |----------------------|--------|-----------|-----------|--------|
    /// | Customized           | fb_l   | cf_lr     | cf_rl     | fb_r   |
    /// | Straight             | fb_l   | 0         | 0         | fb_r   |
    /// | Crossfeed/Ping Pong  | 0      | cf_lr     | cf_rl     | 0      |
    /// | 90/10, 10/90, Pan    | fb_l   | cf_lr     | cf_rl     | fb_r   |
    /// | Rotate               | fb_l   | cf_lr     | cf_rl     | fb_r   |
    #[inline]
    fn compute_feedback(inputs: FeedbackInputs) -> (f64, f64) {
        let FeedbackInputs {
            routing,
            fb_l,
            fb_r,
            cf_lr,
            cf_rl,
            sign_l,
            sign_r,
            sign_cf_lr,
            sign_cf_rl,
            filt_l,
            filt_r,
        } = inputs;

        // Pre-compute the basic signal contributions.
        let self_l = filt_l * fb_l * sign_l;
        let self_r = filt_r * fb_r * sign_r;
        let cross_lr = filt_l * cf_lr * sign_cf_lr; // L→R
        let cross_rl = filt_r * cf_rl * sign_cf_rl; // R→L

        match routing {
            RoutingMode::Customized => {
                // Self + cross from params.
                (self_l + cross_rl, self_r + cross_lr)
            }

            RoutingMode::Straight => {
                // No crossfeed.
                (self_l, self_r)
            }

            RoutingMode::Crossfeed => {
                // Cross only. If an older preset only has feedback set,
                // fall back to that for compatibility.
                let cf_l = if cf_lr > 0.0 { cf_lr } else { fb_l };
                let cf_r = if cf_rl > 0.0 { cf_rl } else { fb_r };
                (
                    filt_r * cf_r * sign_r * sign_cf_rl,
                    filt_l * cf_l * sign_l * sign_cf_lr,
                )
            }

            RoutingMode::NinetyTen => {
                // The UI writes the rounded 10 % crossfeed amount into the
                // crossfeed controls. Older host-side changes may only set
                // feedback, so synthesize the old 90/10 split as a fallback.
                let cf_l = if cf_lr > 0.0 { cf_lr } else { fb_l * 0.1 };
                let cf_r = if cf_rl > 0.0 { cf_rl } else { fb_r * 0.1 };
                (
                    self_l + filt_r * cf_r * sign_r * sign_cf_rl,
                    self_r + filt_l * cf_l * sign_l * sign_cf_lr,
                )
            }

            RoutingMode::TenNinety => {
                // The UI writes both the dominant crossfeed and rounded
                // feedback amount. Fall back to the previous 10/90 matrix
                // when older automation only changes feedback.
                let cf_l = if cf_lr > 0.0 { cf_lr } else { fb_l * 0.9 };
                let cf_r = if cf_rl > 0.0 { cf_rl } else { fb_r * 0.9 };
                (
                    self_l + filt_r * cf_r * sign_r * sign_cf_rl,
                    self_r + filt_l * cf_l * sign_l * sign_cf_lr,
                )
            }

            RoutingMode::PingPong | RoutingMode::PingPongR => {
                // Cross only, no self-feedback. Fall back to feedback for
                // compatibility with older ping-pong presets.
                let cf_l = if cf_lr > 0.0 { cf_lr } else { fb_l };
                let cf_r = if cf_rl > 0.0 { cf_rl } else { fb_r };
                (
                    filt_r * cf_r * sign_r * sign_cf_rl,
                    filt_l * cf_l * sign_l * sign_cf_lr,
                )
            }

            RoutingMode::Pan | RoutingMode::PanRl => {
                // Parameter preset shape handles direction; process the
                // resulting feedback/crossfeed values directly.
                (self_l + cross_rl, self_r + cross_lr)
            }

            RoutingMode::Rotate | RoutingMode::RotateR => {
                // Rotation is represented by input, amount and phase
                // parameters rather than an invisible LFO matrix.
                (self_l + cross_rl, self_r + cross_lr)
            }
        }
    }

    /// Apply routing-specific transformations to the wet (filtered
    /// delayed) signal before the dry/wet mix.
    ///
    /// Routing presets now write their topology into the visible
    /// parameters, so this stage passes the wet signals through unchanged.
    #[inline]
    fn apply_output_routing(routing: RoutingMode, wet_l: f64, wet_r: f64) -> (f64, f64) {
        let _ = routing;
        (wet_l, wet_r)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── NoteValue durations ───────────────────────────────────────────

    #[test]
    fn note_value_quarter_at_120_bpm() {
        // Quarter note at 120 BPM = 0.5 seconds.
        let dur = NoteValue::Quarter.duration_seconds(120.0);
        assert!((dur - 0.5).abs() < 1e-9, "expected 0.5, got {dur}");
    }

    #[test]
    fn note_value_whole_at_60_bpm() {
        // Whole note at 60 BPM = 4 beats * (60/60) = 4 seconds.
        let dur = NoteValue::Whole.duration_seconds(60.0);
        assert!((dur - 4.0).abs() < 1e-9, "expected 4.0, got {dur}");
    }

    #[test]
    fn note_value_eighth_triplet_at_120_bpm() {
        // 1/3 beat at 120 BPM = (1/3) * 0.5 = 1/6 ≈ 0.16667 s.
        let dur = NoteValue::EighthTriplet.duration_seconds(120.0);
        let expected = 1.0 / 6.0;
        assert!(
            (dur - expected).abs() < 1e-9,
            "expected {expected}, got {dur}"
        );
    }

    #[test]
    fn note_value_sixty_fourth_at_120_bpm() {
        // 0.0625 beats at 120 BPM = 0.0625 * 0.5 = 0.03125 s.
        let dur = NoteValue::SixtyFourth.duration_seconds(120.0);
        assert!((dur - 0.03125).abs() < 1e-9, "expected 0.03125, got {dur}");
    }

    // ── SmoothedValue ─────────────────────────────────────────────────

    #[test]
    fn smoother_reaches_target() {
        let mut s = SmoothedValue::new(0.0, 64);
        s.set_target(1.0);
        let mut val = 0.0;
        for _ in 0..64 {
            val = s.next();
        }
        assert!((val - 1.0).abs() < 1e-12, "expected 1.0, got {val}");
    }

    #[test]
    fn smoother_stays_at_target() {
        let mut s = SmoothedValue::new(0.5, 64);
        for _ in 0..200 {
            let v = s.next();
            assert!((v - 0.5).abs() < 1e-12);
        }
    }

    // ── DelayLine basics ──────────────────────────────────────────────

    #[test]
    fn delay_line_read_write() {
        let mut dl = DelayLine::new(44100.0, 44100.0);
        // Write 5 samples then read with a 3-sample delay.
        dl.write_and_advance(10.0);
        dl.write_and_advance(20.0);
        dl.write_and_advance(30.0);
        dl.write_and_advance(40.0);
        dl.write_and_advance(50.0);
        // At this point write_pos = 5.  A 3-sample delay should read
        // from position 2, which holds 30.0.
        let val = dl.read(3.0 / 44100.0);
        assert!((val - 30.0).abs() < 1e-6, "expected ~30.0, got {val}");
    }

    // ── ComplementaryFilter ───────────────────────────────────────────

    #[test]
    fn complementary_lowpass_passes_dc() {
        let mut filter = ComplementaryFilter::new();
        filter.update(20000.0, 12.0, 44100.0);
        // A low-pass well above the signal frequency should pass DC.
        let mut out = 0.0;
        for _ in 0..1000 {
            out = filter.process(1.0, ComplementaryMode::LowPass);
        }
        assert!(
            (out - 1.0).abs() < 0.01,
            "low-pass should pass DC, got {out}"
        );
    }

    #[test]
    fn complementary_highpass_rejects_dc() {
        let mut filter = ComplementaryFilter::new();
        filter.update(100.0, 12.0, 44100.0);
        // An active high-pass should reject DC (0 Hz).
        let mut out = 1.0;
        for _ in 0..100_000 {
            out = filter.process(1.0, ComplementaryMode::HighPass);
        }
        assert!(out.abs() < 0.01, "high-pass should reject DC, got {out}");
    }

    #[test]
    fn complementary_outputs_recombine_exactly() {
        let mut lp = ComplementaryFilter::new();
        let mut hp = ComplementaryFilter::new();
        lp.update(1000.0, 6.0, 44100.0);
        hp.update(1000.0, 6.0, 44100.0);

        for n in 0..2000 {
            let x = ((n as f64) * 0.017).sin() * 0.7;
            let lo = lp.process(x, ComplementaryMode::LowPass);
            let hi = hp.process(x, ComplementaryMode::HighPass);
            assert!(
                ((lo + hi) - x).abs() < 1e-12,
                "low+high should exactly recombine"
            );
        }
    }

    // ── DelayEngine integration ───────────────────────────────────────

    #[test]
    fn engine_process_does_not_crash() {
        let mut engine = DelayEngine::new(44100.0);
        let params = DelayParams::default();
        for _ in 0..1000 {
            let (l, r) = engine.process(0.1, -0.1, &params);
            assert!(l.is_finite(), "left output is not finite: {l}");
            assert!(r.is_finite(), "right output is not finite: {r}");
        }
    }

    #[test]
    fn engine_bypass_outputs_dry() {
        let mut engine = DelayEngine::new(44100.0);
        let params = DelayParams {
            bypass: true,
            input_level_db: 50.0,
            output_level_db: 50.0,
            output_mix_l: 1.0,
            output_mix_r: 1.0,
            feedback_l: 1.0,
            feedback_r: 1.0,
            ..DelayParams::default()
        };

        let (l, r) = engine.process(0.5, -0.3, &params);
        assert_eq!(l, 0.5, "hard-bypassed left should be dry immediately");
        assert_eq!(r, -0.3, "hard-bypassed right should be dry immediately");

        engine.set_bypass(true);
        let active_params = DelayParams {
            bypass: false,
            input_level_db: 50.0,
            output_level_db: 50.0,
            output_mix_l: 1.0,
            output_mix_r: 1.0,
            ..DelayParams::default()
        };
        let (l, r) = engine.process(0.25, -0.25, &active_params);
        assert_eq!(l, 0.25, "bypass latch should hard-pass left input");
        assert!(
            (r + 0.25).abs() < f64::EPSILON,
            "bypass latch should hard-pass right input"
        );
    }

    #[test]
    fn level_trims_are_transparent_multipliers() {
        let mut engine = DelayEngine::new(44100.0);
        let params = DelayParams {
            input_level_db: 6.0,
            output_level_db: -12.0,
            output_mix_l: 0.0,
            output_mix_r: 0.0,
            ..DelayParams::default()
        };

        let mut l = 0.0;
        let mut r = 0.0;
        for _ in 0..1000 {
            (l, r) = engine.process(0.25, -0.5, &params);
        }

        let expected_gain = db_to_gain(params.input_level_db + params.output_level_db);
        assert!((l - 0.25 * expected_gain).abs() < 1e-6);
        assert!((r - -0.5 * expected_gain).abs() < 1e-6);
    }

    #[test]
    fn process_frame_reports_post_trim_meter_taps() {
        let mut engine = DelayEngine::new(44100.0);
        let params = DelayParams {
            input_level_db: 6.0,
            output_level_db: 3.0,
            output_mix_l: 0.0,
            output_mix_r: 0.0,
            ..DelayParams::default()
        };

        let mut frame = ProcessedFrame {
            output_l: 0.0,
            output_r: 0.0,
            input_meter_l: 0.0,
            input_meter_r: 0.0,
            output_meter_l: 0.0,
            output_meter_r: 0.0,
        };
        for _ in 0..1000 {
            frame = engine.process_frame(0.25, -0.25, &params);
        }

        let input_gain = db_to_gain(params.input_level_db);
        let output_gain = db_to_gain(params.output_level_db);
        assert!((frame.input_meter_l - 0.25 * input_gain).abs() < 1e-6);
        assert!((frame.input_meter_r - -0.25 * input_gain).abs() < 1e-6);
        assert!((frame.output_meter_l - frame.input_meter_l * output_gain).abs() < 1e-6);
        assert!((frame.output_meter_r - frame.input_meter_r * output_gain).abs() < 1e-6);
    }

    #[test]
    fn output_protection_is_transparent_at_and_below_full_scale() {
        assert_eq!(protect_output_sample(0.25), 0.25);
        assert_eq!(protect_output_sample(-0.75), -0.75);
        assert_eq!(protect_output_sample(1.0), 1.0);
        assert_eq!(protect_output_sample(-1.0), -1.0);
    }

    #[test]
    fn output_protection_catches_extreme_boosts() {
        let mut engine = DelayEngine::new(44100.0);
        let params = DelayParams {
            input_level_db: 50.0,
            output_level_db: 50.0,
            output_mix_l: 0.0,
            output_mix_r: 0.0,
            ..DelayParams::default()
        };

        let mut out_l = 0.0;
        let mut out_r = 0.0;
        for _ in 0..1000 {
            (out_l, out_r) = engine.process(1.0, -1.0, &params);
        }

        assert!(out_l.is_finite());
        assert!(out_r.is_finite());
        assert!(out_l.abs() <= SAFETY_OUTPUT_CEILING);
        assert!(out_r.abs() <= SAFETY_OUTPUT_CEILING);
    }

    #[test]
    fn output_protection_sanitizes_non_finite_samples() {
        assert_eq!(protect_output_sample(f64::NAN), 0.0);
        assert_eq!(protect_output_sample(f64::INFINITY), 0.0);
        assert_eq!(protect_output_sample(f64::NEG_INFINITY), 0.0);
    }

    #[test]
    fn runaway_feedback_cannot_escape_output_protection() {
        let mut engine = DelayEngine::new(44100.0);
        let params = DelayParams {
            input_level_db: 50.0,
            output_level_db: 50.0,
            delay_time_l: 0.005,
            delay_time_r: 0.005,
            feedback_l: 1.0,
            feedback_r: 1.0,
            crossfeed_lr: 1.0,
            crossfeed_rl: 1.0,
            routing: RoutingMode::Customized,
            output_mix_l: 1.0,
            output_mix_r: 1.0,
            ..DelayParams::default()
        };

        let mut max_output = 0.0_f64;
        for _ in 0..20_000 {
            let (out_l, out_r) = engine.process(1.0, -1.0, &params);
            assert!(out_l.is_finite());
            assert!(out_r.is_finite());
            max_output = max_output.max(out_l.abs()).max(out_r.abs());
        }

        assert!(max_output <= SAFETY_OUTPUT_CEILING);
    }

    #[test]
    fn engine_reset_clears_state() {
        let mut engine = DelayEngine::new(44100.0);
        let params = DelayParams {
            feedback_l: 0.9,
            feedback_r: 0.9,
            ..DelayParams::default()
        };
        // Feed signal to build up feedback.
        for _ in 0..5000 {
            engine.process(1.0, 1.0, &params);
        }
        engine.reset();
        // After reset, processing silence should output near-zero.
        let silence_params = DelayParams {
            feedback_l: 0.5,
            feedback_r: 0.5,
            ..DelayParams::default()
        };
        let mut max_val = 0.0_f64;
        for _ in 0..100 {
            let (l, r) = engine.process(0.0, 0.0, &silence_params);
            max_val = max_val.max(l.abs()).max(r.abs());
        }
        assert!(
            max_val < 1e-10,
            "after reset, output should be near-zero, got max {max_val}"
        );
    }

    #[test]
    fn engine_all_routing_modes_run() {
        let modes = [
            RoutingMode::Customized,
            RoutingMode::Straight,
            RoutingMode::Crossfeed,
            RoutingMode::NinetyTen,
            RoutingMode::TenNinety,
            RoutingMode::PingPong,
            RoutingMode::Pan,
            RoutingMode::Rotate,
            RoutingMode::PingPongR,
            RoutingMode::PanRl,
            RoutingMode::RotateR,
        ];
        for mode in modes {
            let mut engine = DelayEngine::new(44100.0);
            let params = DelayParams {
                routing: mode,
                feedback_l: 0.5,
                feedback_r: 0.5,
                crossfeed_lr: 0.3,
                crossfeed_rl: 0.3,
                ..DelayParams::default()
            };
            for _ in 0..500 {
                let (l, r) = engine.process(0.1, 0.1, &params);
                assert!(l.is_finite(), "mode {mode:?}: L not finite");
                assert!(r.is_finite(), "mode {mode:?}: R not finite");
            }
        }
    }

    #[test]
    fn engine_all_input_modes_run() {
        let modes = [
            InputMode::Off,
            InputMode::Left,
            InputMode::Right,
            InputMode::LeftPlusRight,
            InputMode::LeftMinusRight,
        ];
        for ml in modes {
            for mr in modes {
                let mut engine = DelayEngine::new(44100.0);
                let params = DelayParams {
                    input_mode_l: ml,
                    input_mode_r: mr,
                    ..DelayParams::default()
                };
                for _ in 0..200 {
                    let (l, r) = engine.process(0.1, 0.1, &params);
                    assert!(l.is_finite(), "input {ml:?}/{mr:?}: L not finite");
                    assert!(r.is_finite(), "input {ml:?}/{mr:?}: R not finite");
                }
            }
        }
    }

    #[test]
    fn engine_tempo_sync_produces_correct_delay() {
        let mut engine = DelayEngine::new(44100.0);
        let params = DelayParams {
            tempo_sync: true,
            tempo_bpm: 120.0,
            note_l: NoteValue::Quarter, // 0.5 s
            note_r: NoteValue::Quarter,
            feedback_l: 0.0, // No feedback for cleaner measurement.
            feedback_r: 0.0,
            output_mix_l: 1.0, // Fully wet.
            output_mix_r: 1.0,
            ..DelayParams::default()
        };

        // Feed an impulse at sample 0, then silence.
        let mut results = Vec::new();
        for i in 0..30000 {
            let in_l = if i == 0 { 1.0 } else { 0.0 };
            let in_r = if i == 0 { 1.0 } else { 0.0 };
            let (l, _r) = engine.process(in_l, in_r, &params);
            results.push(l);
        }

        // The impulse should appear at approximately sample 22050
        // (0.5 s × 44100 Hz).  Allow ±10 samples tolerance for the
        // smoothing ramp and interpolation.
        let expected_sample = 22050_usize;
        let peak_sample = results
            .iter()
            .enumerate()
            .skip(100) // skip initial transient
            .max_by(|(_, a), (_, b)| a.abs().partial_cmp(&b.abs()).unwrap())
            .map(|(i, _)| i)
            .unwrap_or(0);

        assert!(
            (peak_sample as isize - expected_sample as isize).abs() < 20,
            "tempo-synced delay peak at sample {peak_sample}, expected ≈{expected_sample}"
        );
    }

    #[test]
    fn effective_delay_time_halve_and_double() {
        // 0.5 s, halved → 0.25 s.
        let t = DelayEngine::effective_delay_time(
            0.5,
            false,
            120.0,
            NoteValue::Quarter,
            0.0,
            true,
            false,
        );
        assert!((t - 0.25).abs() < 1e-12, "expected 0.25, got {t}");

        // 0.5 s, doubled → 1.0 s.
        let t = DelayEngine::effective_delay_time(
            0.5,
            false,
            120.0,
            NoteValue::Quarter,
            0.0,
            false,
            true,
        );
        assert!((t - 1.0).abs() < 1e-12, "expected 1.0, got {t}");

        // Both halve and double → net 1×.
        let t = DelayEngine::effective_delay_time(
            0.5,
            false,
            120.0,
            NoteValue::Quarter,
            0.0,
            true,
            true,
        );
        assert!((t - 0.5).abs() < 1e-12, "expected 0.5, got {t}");
    }

    #[test]
    fn effective_delay_time_deviation() {
        // +100 cents = 2^(100/1200) ≈ 1.05946 multiplier.
        let t = DelayEngine::effective_delay_time(
            1.0,
            false,
            120.0,
            NoteValue::Quarter,
            100.0,
            false,
            false,
        );
        let expected = 2.0_f64.powf(100.0 / 1200.0);
        assert!((t - expected).abs() < 1e-12, "expected {expected}, got {t}");
    }
}
