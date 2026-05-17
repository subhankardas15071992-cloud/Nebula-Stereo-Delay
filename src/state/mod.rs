//! State management module for **Nebula Stereo Delay** by Nebula Audio.
//!
//! This module provides the high-level state management layer that sits above
//! the [`crate::parameters`] module. Whereas `parameters` defines the
//! automatable parameter surface, A/B snapshot storage, and undo/redo stacks,
//! **this module** handles runtime state that does *not* belong in the host's
//! automation graph:
//!
//! - **Peak meter values** — lock-free, per-sample updates from the audio
//!   thread with zero-cost reads on the GUI thread.
//! - **Spectrum analyser data** — periodic FFT analysis with a `try_lock`
//!   pattern so the audio thread never blocks.
//!
//! # Lock-Free Design & Memory Ordering
//!
//! ## Meter values (`AtomicU32`)
//!
//! Meter values are stored as `AtomicU32`, where each `u32` holds the raw
//! bit-pattern of an `f32` (via [`f32::to_bits`] / [`f32::from_bits`]).
//! All reads and writes use **`Ordering::Relaxed`**:
//!
//! - The audio thread writes meters once per sample (or once per buffer,
//!   depending on the caller's choice). The GUI thread reads them at its
//!   repaint rate (typically 30–60 Hz).
//! - We only need *atomicity* — the guarantee that a read observes either
//!   the old value or the new value, never a torn intermediate state.
//! - We do **not** need synchronisation ordering with respect to any other
//!   memory location. Meter values are self-contained; no other data depends
//!   on the ordering of a meter write relative to other writes.
//! - `Relaxed` is the weakest (and cheapest) ordering that provides this
//!   atomicity guarantee. On x86-64 and AArch64 it compiles to a plain
//!   load or store with no fence, making it essentially free.
//!
//! **Why not `Ordering::SeqCst`?** Sequential consistency would add
//! unnecessary memory fences on architectures with weaker memory models
//! (e.g., ARMv7). Since meter values carry no happens-before
//! relationship with other data, the stronger ordering would be pure
//! overhead with no correctness benefit.
//!
//! **Why not `Ordering::Release`/`Acquire`?** Release-Acquire establishes a
//! synchronises-with relationship: if thread A stores with `Release` and
//! thread B loads with `Acquire`, then all writes that happened before A's
//! store are visible to B after its load. We have no secondary data to
//! protect with this pattern, so `Relaxed` suffices.
//!
//! ## Spectrum data (`Mutex` + `try_lock`)
//!
//! The spectrum analyser uses a [`std::sync::Mutex`] to protect a
//! [`SpectrumFrame`]. The audio thread calls [`SpectrumData::try_write`],
//! which uses `try_lock()`. If the GUI thread currently holds the lock, the
//! audio thread simply **skips** the write — it never blocks. This is the
//! standard "try-lock" pattern for real-time audio:
//!
//! - A skipped frame is invisible to the user (the GUI will just display the
//!   previous frame's data for one extra repaint cycle).
//! - The `ready` [`AtomicBool`] (also `Relaxed`) signals to the GUI that
//!   fresh data is available, allowing it to avoid unnecessary cloning when
//!   the spectrum has not changed.
//!
//! # A/B Comparison and Undo/Redo
//!
//! A/B comparison and undo/redo are **not** implemented in this module. They
//! are handled by [`crate::parameters::AbSnapshots`] and
//! [`crate::parameters::UndoRedoStack`] in the `parameters` module, because
//! those mechanisms must directly manipulate the plugin's automatable
//! parameter surface. This module provides only the *non-parameter* runtime
//! state (meters, spectrum) that the GUI and audio thread share.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use crate::parameters::NebulaStereoDelayParams;

// ═══════════════════════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════════════════════

/// FFT size for the spectrum analyser.
///
/// Must be a power of two (required by the radix-2 algorithm). A 512-point
/// FFT gives 256 output bins, covering 0 Hz to Nyquist at a resolution of
/// `sample_rate / 512` per bin. At 44 100 Hz that is ~86 Hz per bin; at
/// 96 000 Hz it is ~188 Hz per bin — adequate for a visual analyser.
const FFT_SIZE: usize = 512;

/// Number of frequency bins output by the spectrum analyser.
///
/// For a real-valued N-point FFT, the output is conjugate-symmetric, so only
/// the first N/2 bins carry unique information.
const NUM_BINS: usize = FFT_SIZE / 2;

// ═══════════════════════════════════════════════════════════════════════════
// Meter Values
// ═══════════════════════════════════════════════════════════════════════════

/// Peak meter values for GUI display, updated lock-free from the audio
/// thread.
///
/// Each field stores an [`AtomicU32`] that holds the raw bit-pattern of an
/// `f32` value (0.0–1.0 range). Using `AtomicU32` instead of `AtomicF32`
/// (from the `atomic_float` crate) avoids an extra dependency while providing
/// identical performance — on all supported architectures, a 32-bit atomic
/// load/store compiles to a single instruction.
///
/// # Memory ordering
///
/// All operations use [`Ordering::Relaxed`] (see module-level documentation
/// for the full rationale). In summary: we only need atomicity, not
/// synchronisation, because meter values are independent of all other state.
pub struct MeterValues {
    /// Left input channel peak level after input trim (f32 bits, linear).
    pub input_l: AtomicU32,
    /// Right input channel peak level after input trim (f32 bits, linear).
    pub input_r: AtomicU32,
    /// Left output channel peak level (f32 bits, 0.0–1.0).
    pub output_l: AtomicU32,
    /// Right output channel peak level (f32 bits, 0.0–1.0).
    pub output_r: AtomicU32,
    /// Left feedback path peak level (f32 bits, 0.0–1.0).
    pub feedback_l: AtomicU32,
    /// Right feedback path peak level (f32 bits, 0.0–1.0).
    pub feedback_r: AtomicU32,
}

impl MeterValues {
    /// Create a new `MeterValues` with all meters initialised to silence
    /// (0.0).
    pub fn new() -> Self {
        let zero_bits = 0.0f32.to_bits();
        Self {
            input_l: AtomicU32::new(zero_bits),
            input_r: AtomicU32::new(zero_bits),
            output_l: AtomicU32::new(zero_bits),
            output_r: AtomicU32::new(zero_bits),
            feedback_l: AtomicU32::new(zero_bits),
            feedback_r: AtomicU32::new(zero_bits),
        }
    }

    /// Store the left output meter value.
    ///
    /// Called from the audio thread (per-sample or per-buffer). The value is
    /// written as a raw `u32` bit-pattern to avoid the overhead of any
    /// floating-point comparison or branching.
    #[inline]
    pub fn set_output_l(&self, value: f32) {
        self.output_l.store(value.to_bits(), Ordering::Relaxed);
    }

    /// Store the left input meter value.
    #[inline]
    pub fn set_input_l(&self, value: f32) {
        self.input_l.store(value.to_bits(), Ordering::Relaxed);
    }

    /// Store the right input meter value.
    #[inline]
    pub fn set_input_r(&self, value: f32) {
        self.input_r.store(value.to_bits(), Ordering::Relaxed);
    }

    /// Store the right output meter value.
    #[inline]
    pub fn set_output_r(&self, value: f32) {
        self.output_r.store(value.to_bits(), Ordering::Relaxed);
    }

    /// Store the left feedback meter value.
    #[inline]
    pub fn set_feedback_l(&self, value: f32) {
        self.feedback_l.store(value.to_bits(), Ordering::Relaxed);
    }

    /// Store the right feedback meter value.
    #[inline]
    pub fn set_feedback_r(&self, value: f32) {
        self.feedback_r.store(value.to_bits(), Ordering::Relaxed);
    }

    /// Read the left output meter value.
    ///
    /// Called from the GUI thread. Returns the most recent value written by
    /// the audio thread, or 0.0 if no write has occurred yet.
    #[inline]
    pub fn get_output_l(&self) -> f32 {
        f32::from_bits(self.output_l.load(Ordering::Relaxed))
    }

    /// Read the left input meter value.
    #[inline]
    pub fn get_input_l(&self) -> f32 {
        f32::from_bits(self.input_l.load(Ordering::Relaxed))
    }

    /// Read the right input meter value.
    #[inline]
    pub fn get_input_r(&self) -> f32 {
        f32::from_bits(self.input_r.load(Ordering::Relaxed))
    }

    /// Read the right output meter value.
    #[inline]
    pub fn get_output_r(&self) -> f32 {
        f32::from_bits(self.output_r.load(Ordering::Relaxed))
    }

    /// Read the left feedback meter value.
    #[inline]
    pub fn get_feedback_l(&self) -> f32 {
        f32::from_bits(self.feedback_l.load(Ordering::Relaxed))
    }

    /// Read the right feedback meter value.
    #[inline]
    pub fn get_feedback_r(&self) -> f32 {
        f32::from_bits(self.feedback_r.load(Ordering::Relaxed))
    }
}

impl Default for MeterValues {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Spectrum Analyser
// ═══════════════════════════════════════════════════════════════════════════

/// A single frame of spectrum analyser data.
///
/// Produced by the audio thread (via [`SpectrumData::try_write`]) and
/// consumed by the GUI thread (via [`SpectrumData::read`]).
#[derive(Debug, Clone)]
pub struct SpectrumFrame {
    /// Magnitude values for 256 frequency bins, normalised to the 0.0–1.0
    /// range.
    ///
    /// Bin `i` corresponds to the frequency band from
    /// `i * sample_rate / FFT_SIZE` to `(i + 1) * sample_rate / FFT_SIZE`.
    pub bins: Vec<f32>,

    /// The sample rate that was used to compute this frame, needed by the
    /// GUI to label the frequency axis.
    pub sample_rate: f32,
}

/// Spectrum analyser data shared between the audio thread and the GUI
/// thread.
///
/// # Thread safety
///
/// - The audio thread calls [`try_write`], which uses `try_lock()` on the
///   inner `Mutex`. If the lock is already held by the GUI, the write is
///   silently skipped — the audio thread **never blocks**.
/// - The GUI thread calls [`read`], which acquires the lock normally.
///   The GUI may block briefly if the audio thread holds the lock, but in
///   practice the audio thread's critical section is so short (a single
///   `Vec` clone) that contention is negligible.
/// - The `ready` flag avoids unnecessary cloning: the GUI checks
///   [`SpectrumData::is_ready`] before calling `read`, and the audio
///   thread sets the flag after a successful write.
///
/// [`try_write`]: SpectrumData::try_write
/// [`read`]: SpectrumData::read
pub struct SpectrumData {
    /// The most recent spectrum frame, protected by a `Mutex`.
    ///
    /// We use `std::sync::Mutex` (not `parking_lot::Mutex`) because:
    /// 1. `try_lock()` is available on the standard `Mutex` since Rust 1.63.
    /// 2. The standard `Mutex` is fair (it uses OS-level futex/pthread
    ///    primitives), which prevents the GUI thread from being starved by
    ///    a high-priority audio thread — although in our design the audio
    ///    thread never holds the lock for long.
    pub data: Mutex<SpectrumFrame>,

    /// `true` when new spectrum data has been written by the audio thread
    /// and not yet read by the GUI thread. Uses `Relaxed` ordering because
    /// this flag is advisory — a missed signal just means the GUI paints the
    /// same frame twice, which is harmless.
    pub ready: AtomicBool,
}

impl SpectrumData {
    /// Create a new `SpectrumData` with an empty spectrum frame.
    pub fn new() -> Self {
        Self {
            data: Mutex::new(SpectrumFrame {
                bins: vec![0.0; NUM_BINS],
                sample_rate: 44100.0,
            }),
            ready: AtomicBool::new(false),
        }
    }

    /// Called from the **audio thread**: attempt to write a new spectrum
    /// frame.
    ///
    /// Uses `try_lock()` on the inner `Mutex`. If the lock is currently
    /// held by the GUI thread, this method returns immediately without
    /// writing — the audio thread **never blocks**. This is the cornerstone
    /// of the real-time safety guarantee: a `try_lock()` that fails is a
    /// simple `cmpxchg` instruction, not a system call.
    ///
    /// On success, the `ready` flag is set to `true` so the GUI knows that
    /// fresh data is available.
    pub fn try_write(&self, bins: Vec<f32>, sample_rate: f32) {
        if let Ok(mut guard) = self.data.try_lock() {
            guard.bins = bins;
            guard.sample_rate = sample_rate;
            self.ready.store(true, Ordering::Relaxed);
        }
        // If try_lock fails, the GUI thread holds the lock. We simply
        // skip this frame — the next call will succeed.
    }

    /// Called from the **GUI thread**: read the most recent spectrum frame.
    ///
    /// Returns `Some(SpectrumFrame)` if data is available, or `None` if no
    /// new data has been written since the last read. The `ready` flag is
    /// cleared after a successful read.
    ///
    /// This method acquires the `Mutex`, which may briefly block if the
    /// audio thread is in the middle of a `try_write`. In practice the
    /// audio thread's critical section is extremely short (two assignments
    /// and an atomic store), so blocking is unlikely and brief.
    pub fn read(&self) -> Option<SpectrumFrame> {
        if !self.ready.load(Ordering::Relaxed) {
            return None;
        }

        let guard = self.data.lock().ok()?;
        self.ready.store(false, Ordering::Relaxed);
        Some(guard.clone())
    }
}

impl Default for SpectrumData {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// State Manager
// ═══════════════════════════════════════════════════════════════════════════

/// High-level state manager for the **Nebula Stereo Delay** plugin.
///
/// Owns the lock-free meter values and spectrum analyser data that are shared
/// between the audio thread and the GUI thread. The `parameters` field
/// provides a reference to the plugin's automatable parameter struct (which
/// lives in the `parameters` module and handles A/B snapshots, undo/redo,
/// and MIDI learn).
///
/// # Usage
///
/// ```ignore
/// let state = Arc::new(StateManager::new());
///
/// // Audio thread (inside `process`):
/// state.update_meters(out_l, out_r, fb_l, fb_r);
/// state.compute_spectrum(&buffer_l, &buffer_r, sample_rate);
///
/// // GUI thread (inside `update`):
/// let out_l = state.meters.get_output_l();
/// if let Some(frame) = state.spectrum.read() {
///     // draw spectrum using frame.bins and frame.sample_rate
/// }
/// ```
pub struct StateManager {
    /// Lock-free peak meter values for the GUI.
    pub meters: Arc<MeterValues>,

    /// Spectrum analyser data (try_lock pattern).
    pub spectrum: SpectrumData,

    /// Reference to the plugin's automatable parameters.
    ///
    /// This is stored here so that future high-level operations (e.g., "swap
    /// A/B and update meters") can access the parameter surface through the
    /// state manager, keeping the API surface small and cohesive. The A/B
    /// and undo/redo logic itself lives in the `parameters` module.
    pub parameters: std::sync::Arc<NebulaStereoDelayParams>,
}

impl StateManager {
    /// Create a new `StateManager` bound to the given parameter struct.
    pub fn new(params: std::sync::Arc<NebulaStereoDelayParams>) -> Self {
        Self::with_meters(params, Arc::new(MeterValues::new()))
    }

    /// Create a new `StateManager` with a caller-owned meter block.
    pub fn with_meters(
        params: std::sync::Arc<NebulaStereoDelayParams>,
        meters: Arc<MeterValues>,
    ) -> Self {
        Self {
            meters,
            spectrum: SpectrumData::new(),
            parameters: params,
        }
    }

    /// Update all peak meters from the audio thread.
    ///
    /// This is designed to be called **per-sample** inside the `process`
    /// callback. The implementation stores the absolute value of each
    /// sample, so the GUI always sees a positive peak. For a smoother
    /// visual, the caller may prefer to track a running peak with decay
    /// externally and pass the decayed value here instead.
    ///
    /// # Parameters
    ///
    /// - `in_l` / `in_r` — left and right input sample values after input trim.
    /// - `out_l` / `out_r` — left and right output sample values after output trim.
    /// - `fb_l` / `fb_r` — left and right feedback-path sample values.
    #[inline]
    pub fn update_meters(
        &self,
        in_l: f32,
        in_r: f32,
        out_l: f32,
        out_r: f32,
        fb_l: f32,
        fb_r: f32,
    ) {
        self.meters.set_input_l(in_l.abs());
        self.meters.set_input_r(in_r.abs());
        self.meters.set_output_l(out_l.abs());
        self.meters.set_output_r(out_r.abs());
        self.meters.set_feedback_l(fb_l.abs());
        self.meters.set_feedback_r(fb_r.abs());
    }

    /// Compute a spectrum from the given audio buffers and write it to the
    /// spectrum analyser.
    ///
    /// This method performs a 512-point FFT on a mono mixdown of the two
    /// input channels (averaged), applies a Hann window, and writes 256
    /// magnitude bins (normalised to 0.0–1.0) via [`SpectrumData::try_write`].
    ///
    /// # When to call
    ///
    /// This should be called **periodically** (e.g., once per process buffer
    /// or once every N buffers), not per-sample. A 512-point FFT on `f32`
    /// takes roughly 2–5 μs on a modern CPU, which is well within the
    /// typical per-buffer budget but would be excessive per-sample.
    ///
    /// # Buffer length
    ///
    /// If the input buffers contain **more** than `FFT_SIZE` samples, only
    /// the last `FFT_SIZE` samples are used (the most recent data). If they
    /// contain **fewer**, the input is zero-padded at the beginning.
    pub fn compute_spectrum(&self, buffer_l: &[f32], buffer_r: &[f32], sample_rate: f32) {
        let len = buffer_l.len().min(buffer_r.len());

        // Prepare the real and imaginary arrays for the FFT.
        let mut re = [0.0f32; FFT_SIZE];
        let mut im = [0.0f32; FFT_SIZE];

        // Fill the FFT input with a mono mixdown of the last FFT_SIZE samples,
        // applying a Hann window to reduce spectral leakage.
        let start = len.saturating_sub(FFT_SIZE);
        let offset = FFT_SIZE.saturating_sub(len);

        for i in start..len {
            let window_idx = offset + (i - start);
            let window_val = hann_window(window_idx, FFT_SIZE);
            let mixed = (buffer_l[i] + buffer_r[i]) * 0.5;
            re[window_idx] = mixed * window_val;
            // im[] is already zeroed
        }

        // Perform the in-place radix-2 Cooley-Tukey FFT.
        fft_radix2(&mut re, &mut im);

        // Compute magnitude for the first NUM_BINS bins, normalised to [0, 1].
        // The normalisation factor is N/2 (= 256 for a 512-point FFT). A
        // full-scale sinusoid at amplitude 1.0 produces a bin magnitude of
        // N/2, so dividing by N/2 maps full-scale to 1.0. The Hann window
        // has a coherent gain of 0.5, which we compensate for by multiplying
        // the normalisation factor by 2 (i.e., dividing by N/4 = 128).
        let normalisation = FFT_SIZE as f32 / 4.0; // compensates Hann gain

        let mut bins = Vec::with_capacity(NUM_BINS);
        for k in 0..NUM_BINS {
            let magnitude = (re[k] * re[k] + im[k] * im[k]).sqrt();
            let normalised = (magnitude / normalisation).clamp(0.0, 1.0);
            bins.push(normalised);
        }

        // Write to the shared spectrum data. try_write will silently skip
        // if the GUI thread currently holds the lock.
        self.spectrum.try_write(bins, sample_rate);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// FFT Implementation (512-point Radix-2 Cooley-Tukey)
// ═══════════════════════════════════════════════════════════════════════════

/// Apply a Hann window to sample `n` of a window of length `n_total`.
///
/// The Hann window is defined as:
///
/// ```text
/// w(n) = 0.5 * (1 - cos(2 * pi * n / (N - 1)))
/// ```
///
/// It has a coherent gain of 0.5, which means that a sinusoid windowed with
/// Hann has half the amplitude it would have with a rectangular window. The
/// normalisation in [`StateManager::compute_spectrum`] compensates for this.
#[inline]
fn hann_window(n: usize, n_total: usize) -> f32 {
    let n_f = n as f32;
    let nm1 = (n_total - 1) as f32;
    0.5 * (1.0 - (2.0 * std::f32::consts::PI * n_f / nm1).cos())
}

/// Reverse the bits of `x` within a field of `bits` width.
///
/// For example, with `bits = 3`:
/// - `0b000` (0) → `0b000` (0)
/// - `0b001` (1) → `0b100` (4)
/// - `0b010` (2) → `0b010` (2)
/// - `0b011` (3) → `0b110` (6)
///
/// This is used for the bit-reversal permutation stage of the
/// Cooley-Tukey FFT.
#[inline]
fn bit_reverse(mut x: usize, bits: u32) -> usize {
    let mut result = 0usize;
    for _ in 0..bits {
        result = (result << 1) | (x & 1);
        x >>= 1;
    }
    result
}

/// In-place radix-2 Decimation-In-Time (DIT) Cooley-Tukey FFT.
///
/// Transforms the complex array `(re[], im[])` in place. The array length
/// must be a power of two; this is guaranteed by the `FFT_SIZE` constant.
///
/// # Algorithm
///
/// 1. **Bit-reversal permutation**: rearrange the input array so that
///    element `i` moves to position `bit_reverse(i)`. This places the
///    input in the order required by the DIT butterfly structure.
///
/// 2. **Butterfly stages**: iterate over `log2(N)` stages. In each stage,
///    combine pairs of elements using "twiddle factors" (complex roots of
///    unity). The butterfly operation for a pair `(a, b)` with twiddle
///    factor `W` is:
///
///    ```text
///    A = a + W * b
///    B = a - W * b
///    ```
///
/// The twiddle factor for stage `s`, sub-index `j` is:
///
/// ```text
/// W(s, j) = exp(-2 * pi * i * j / (1 << s))
/// ```
///
/// where `s` is 1-indexed (stage 1 has butterflies of size 2, stage 2 has
/// size 4, etc.).
///
/// # Complexity
///
/// O(N log N) — for N = 512 this is approximately 4608 complex multiply-add
/// operations, which completes in a few microseconds on any modern CPU.
fn fft_radix2(re: &mut [f32; FFT_SIZE], im: &mut [f32; FFT_SIZE]) {
    let n = FFT_SIZE;
    let log2n = n.trailing_zeros(); // log2(512) = 9

    // ── Stage 1: Bit-reversal permutation ─────────────────────────────
    for i in 0..n {
        let j = bit_reverse(i, log2n);
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    // ── Stage 2: Butterfly operations ─────────────────────────────────
    // Iterate over stages: butterfly sizes 2, 4, 8, …, N.
    let mut stage_size = 2usize;
    while stage_size <= n {
        let half_stage = stage_size / 2;

        // Pre-compute the base angle for this stage. Using exp(-2*pi*i/N)
        // gives the forward (analysis) DFT convention.
        let base_angle = -2.0 * std::f32::consts::PI / stage_size as f32;

        // Process each butterfly group.
        for group_start in (0..n).step_by(stage_size) {
            for j in 0..half_stage {
                // Twiddle factor: W = cos(angle * j) + i * sin(angle * j)
                let angle = base_angle * j as f32;
                let wr = angle.cos();
                let wi = angle.sin();

                // Indices of the two elements in the butterfly.
                let even_idx = group_start + j;
                let odd_idx = group_start + j + half_stage;

                // Complex multiply: t = W * X[odd]
                let t_re = wr * re[odd_idx] - wi * im[odd_idx];
                let t_im = wr * im[odd_idx] + wi * re[odd_idx];

                // Butterfly: A = even + t, B = even - t
                let even_re = re[even_idx];
                let even_im = im[even_idx];

                re[even_idx] = even_re + t_re;
                im[even_idx] = even_im + t_im;
                re[odd_idx] = even_re - t_re;
                im[odd_idx] = even_im - t_im;
            }
        }

        stage_size *= 2;
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    // ── MeterValues ───────────────────────────────────────────────────

    #[test]
    fn meter_values_new_starts_at_zero() {
        let mv = MeterValues::new();
        assert_eq!(mv.get_input_l(), 0.0);
        assert_eq!(mv.get_input_r(), 0.0);
        assert_eq!(mv.get_output_l(), 0.0);
        assert_eq!(mv.get_output_r(), 0.0);
        assert_eq!(mv.get_feedback_l(), 0.0);
        assert_eq!(mv.get_feedback_r(), 0.0);
    }

    #[test]
    fn meter_values_set_and_get() {
        let mv = MeterValues::new();
        mv.set_input_l(0.125);
        mv.set_input_r(0.375);
        mv.set_output_l(0.5);
        mv.set_output_r(0.75);
        mv.set_feedback_l(0.25);
        mv.set_feedback_r(1.0);

        assert!((mv.get_input_l() - 0.125).abs() < 1e-6);
        assert!((mv.get_input_r() - 0.375).abs() < 1e-6);
        assert!((mv.get_output_l() - 0.5).abs() < 1e-6);
        assert!((mv.get_output_r() - 0.75).abs() < 1e-6);
        assert!((mv.get_feedback_l() - 0.25).abs() < 1e-6);
        assert!((mv.get_feedback_r() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn meter_values_negative_values() {
        let mv = MeterValues::new();
        mv.set_output_l(-0.5);
        // The raw f32 bits of -0.5 will be stored and retrieved as -0.5.
        // The caller is responsible for taking .abs() before display.
        assert!((mv.get_output_l() - (-0.5)).abs() < 1e-6);
    }

    // ── SpectrumData ──────────────────────────────────────────────────

    #[test]
    fn spectrum_data_new_is_not_ready() {
        let sd = SpectrumData::new();
        assert!(!sd.ready.load(Ordering::Relaxed));
        assert!(sd.read().is_none());
    }

    #[test]
    fn spectrum_data_write_then_read() {
        let sd = SpectrumData::new();
        let bins = vec![0.5; NUM_BINS];
        sd.try_write(bins.clone(), 48000.0);

        assert!(sd.ready.load(Ordering::Relaxed));
        let frame = sd.read().expect("should have data");
        assert!(!sd.ready.load(Ordering::Relaxed)); // cleared after read

        for b in &frame.bins {
            assert!((b - 0.5).abs() < 1e-6);
        }
        assert!((frame.sample_rate - 48000.0).abs() < 1e-3);
    }

    #[test]
    fn spectrum_data_read_without_write_is_none() {
        let sd = SpectrumData::new();
        assert!(sd.read().is_none());
    }

    // ── FFT ───────────────────────────────────────────────────────────

    #[test]
    fn fft_dc_signal() {
        // A constant (DC) signal should have all energy in bin 0.
        let mut re = [1.0f32; FFT_SIZE];
        let mut im = [0.0f32; FFT_SIZE];
        fft_radix2(&mut re, &mut im);

        // Bin 0 magnitude should equal FFT_SIZE (DC sum).
        let dc_mag = (re[0] * re[0] + im[0] * im[0]).sqrt();
        assert!(
            (dc_mag - FFT_SIZE as f32).abs() < 1e-3,
            "DC magnitude: expected {}, got {}",
            FFT_SIZE,
            dc_mag
        );

        // All other bins should be near zero.
        for k in 1..NUM_BINS {
            let mag = (re[k] * re[k] + im[k] * im[k]).sqrt();
            assert!(mag < 1e-3, "Bin {} magnitude: expected ~0, got {}", k, mag);
        }
    }

    #[test]
    fn fft_pure_sine() {
        // A pure sine wave at bin frequency should produce a single peak.
        // Use bin 10 (frequency = 10 * 44100 / 512 ≈ 861 Hz).
        let sample_rate = 44100.0f32;
        let target_bin = 10usize;
        let freq = target_bin as f32 * sample_rate / FFT_SIZE as f32;

        let mut re = [0.0f32; FFT_SIZE];
        let mut im = [0.0f32; FFT_SIZE];
        for (n, sample) in re.iter_mut().enumerate() {
            *sample = (2.0 * PI * freq * n as f32 / sample_rate).sin();
        }
        fft_radix2(&mut re, &mut im);

        // Bin target_bin should have the largest magnitude.
        let target_mag = (re[target_bin] * re[target_bin] + im[target_bin] * im[target_bin]).sqrt();
        for k in 1..NUM_BINS {
            if k == target_bin {
                continue;
            }
            let mag = (re[k] * re[k] + im[k] * im[k]).sqrt();
            assert!(
                target_mag > mag,
                "Bin {} mag {} should be less than target bin {} mag {}",
                k,
                mag,
                target_bin,
                target_mag
            );
        }
    }

    #[test]
    fn bit_reverse_correctness() {
        assert_eq!(bit_reverse(0, 3), 0);
        assert_eq!(bit_reverse(1, 3), 4);
        assert_eq!(bit_reverse(2, 3), 2);
        assert_eq!(bit_reverse(3, 3), 6);
        assert_eq!(bit_reverse(4, 3), 1);
        assert_eq!(bit_reverse(5, 3), 5);
        assert_eq!(bit_reverse(6, 3), 3);
        assert_eq!(bit_reverse(7, 3), 7);

        // For 9 bits (log2(512) = 9), verify that bit_reverse is an
        // involution: applying it twice returns the original value.
        for i in 0..512usize {
            let j = bit_reverse(i, 9);
            let k = bit_reverse(j, 9);
            assert_eq!(i, k, "bit_reverse(bit_reverse({})) = {} != {}", i, k, i);
        }
    }

    #[test]
    fn hann_window_symmetry() {
        // The Hann window should be symmetric: w(n) == w(N-1-n).
        for n in 0..FFT_SIZE / 2 {
            let a = hann_window(n, FFT_SIZE);
            let b = hann_window(FFT_SIZE - 1 - n, FFT_SIZE);
            assert!(
                (a - b).abs() < 1e-6,
                "Hann window not symmetric at n={}: {} != {}",
                n,
                a,
                b
            );
        }
    }

    #[test]
    fn hann_window_boundary_values() {
        // Hann window should be 0.0 at n=0 and n=N-1.
        assert!((hann_window(0, FFT_SIZE) - 0.0).abs() < 1e-6);
        assert!((hann_window(FFT_SIZE - 1, FFT_SIZE) - 0.0).abs() < 1e-6);

        // Hann window peaks near the centre. For an even-length window
        // (N=512), no sample falls exactly at the true peak (n = N/2 - 0.5),
        // so the closest sample is slightly below 1.0 — within 0.02%.
        let centre = FFT_SIZE / 2;
        let peak = hann_window(centre, FFT_SIZE);
        assert!(
            peak > 0.999,
            "Hann centre value should be > 0.999, got {}",
            peak
        );
        assert!(
            peak <= 1.0,
            "Hann centre value should be <= 1.0, got {}",
            peak
        );
    }

    // ── StateManager integration ──────────────────────────────────────

    #[test]
    fn state_manager_update_meters() {
        let params = std::sync::Arc::new(NebulaStereoDelayParams::default());
        let sm = StateManager::new(params);

        sm.update_meters(0.2, -0.4, 0.3, -0.7, 0.1, -0.9);

        assert!((sm.meters.get_input_l() - 0.2).abs() < 1e-6);
        assert!((sm.meters.get_input_r() - 0.4).abs() < 1e-6);
        assert!((sm.meters.get_output_l() - 0.3).abs() < 1e-6);
        assert!((sm.meters.get_output_r() - 0.7).abs() < 1e-6);
        assert!((sm.meters.get_feedback_l() - 0.1).abs() < 1e-6);
        assert!((sm.meters.get_feedback_r() - 0.9).abs() < 1e-6);
    }

    #[test]
    fn state_manager_compute_spectrum_short_buffer() {
        // Buffer shorter than FFT_SIZE should be zero-padded.
        let params = std::sync::Arc::new(NebulaStereoDelayParams::default());
        let sm = StateManager::new(params);

        let buf_l = vec![1.0; 64];
        let buf_r = vec![1.0; 64];
        sm.compute_spectrum(&buf_l, &buf_r, 44100.0);

        let frame = sm.spectrum.read();
        assert!(
            frame.is_some(),
            "spectrum data should be available after compute"
        );
        let frame = frame.unwrap();
        assert_eq!(frame.bins.len(), NUM_BINS);
        assert!((frame.sample_rate - 44100.0).abs() < 1e-3);
    }

    #[test]
    fn state_manager_compute_spectrum_exact_fft_size() {
        let params = std::sync::Arc::new(NebulaStereoDelayParams::default());
        let sm = StateManager::new(params);

        let buf_l = vec![0.5; FFT_SIZE];
        let buf_r = vec![0.5; FFT_SIZE];
        sm.compute_spectrum(&buf_l, &buf_r, 48000.0);

        let frame = sm.spectrum.read();
        assert!(frame.is_some());
        let frame = frame.unwrap();
        // DC signal after windowing and normalisation: bin 0 should be
        // the largest bin.
        let dc = frame.bins[0];
        for k in 1..NUM_BINS {
            assert!(
                dc >= frame.bins[k],
                "Bin {} ({}) should not exceed DC bin ({}) for DC input",
                k,
                frame.bins[k],
                dc
            );
        }
    }

    #[test]
    fn state_manager_compute_spectrum_long_buffer() {
        // Buffer longer than FFT_SIZE: only the last FFT_SIZE samples
        // should be used.
        let params = std::sync::Arc::new(NebulaStereoDelayParams::default());
        let sm = StateManager::new(params);

        let mut buf_l = vec![0.0; 1024];
        let mut buf_r = vec![0.0; 1024];
        // Fill the last 512 samples with a DC signal.
        for i in 512..1024 {
            buf_l[i] = 0.5;
            buf_r[i] = 0.5;
        }
        sm.compute_spectrum(&buf_l, &buf_r, 44100.0);

        let frame = sm.spectrum.read();
        assert!(frame.is_some());
        let frame = frame.unwrap();
        // The DC bin should be non-trivially large (the windowed DC input
        // produces a clear peak at bin 0).
        assert!(
            frame.bins[0] > 0.1,
            "DC bin should be > 0.1, got {}",
            frame.bins[0]
        );
    }
}
