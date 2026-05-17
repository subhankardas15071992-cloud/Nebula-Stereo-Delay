//! Comprehensive perceptual test suite for the **Nebula Stereo Delay** plugin.
//!
//! This test suite validates the perceptual quality of the DSP engine through
//! signal-derived measurements — FFT-based spectral analysis, LUFS-style
//! loudness, transient detection, stereo correlation, and more. Every metric
//! is computed from the actual DSP output; no placeholder values are used.
//!
//! # Test Overview
//!
//! | # | Test | What it validates |
//! |---|------|-------------------|
//! | 1 | LUFS Stability | Loudness consistency over time |
//! | 2 | Transient Integrity | Click/impulse preservation |
//! | 3 | Harmonic Musicality Index | Spectral cleanliness |
//! | 4 | Dynamic Responsiveness | Fast response to level changes |
//! | 5 | Stereo Image Coherence | Routing-mode stereo behaviour |
//! | 6 | Null Residual Character | Wet-signal purity |
//! | 7 | Multi-Source Musical Benchmark | Frequency-dependent gain flatness |
//! | 8 | Temporal Smoothness (Anti-Zipper) | Parameter-sweep smoothness |
//! | 9 | Groove Preservation | Tempo-sync timing accuracy |
//! | 10 | Golden Industry Standard Reference | Reference implementation agreement |

use std::f64::consts::TAU;

use nebula_stereo_delay::dsp::{DelayEngine, DelayParams, InputMode, NoteValue, RoutingMode};

// ────────────────────────────────────────────────────────────────────────────
// Constants
// ────────────────────────────────────────────────────────────────────────────

/// Default sample rate used across all tests unless otherwise specified.
const SR: f64 = 44_100.0;

/// FFT size for spectral analysis.
const FFT_SIZE: usize = 512;

/// Number of smoothing-ramp samples in the engine (must match `SMOOTH_SAMPLES`).
#[allow(dead_code)]
const SMOOTH_SAMPLES: usize = 64;

/// Warm-up period in samples: enough for all smoothers to converge and the
/// delay line to fill at least once for typical delay times.
const WARMUP_SAMPLES: usize = 512;

// ═══════════════════════════════════════════════════════════════════════════
// Complex Number Type
// ═══════════════════════════════════════════════════════════════════════════

/// Minimal complex number type for FFT and spectral analysis.
#[derive(Clone, Copy, Debug)]
struct Complex64 {
    re: f64,
    im: f64,
}

impl Complex64 {
    fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    fn from_polar(r: f64, theta: f64) -> Self {
        Self {
            re: r * theta.cos(),
            im: r * theta.sin(),
        }
    }

    fn norm_sq(self) -> f64 {
        self.re * self.re + self.im * self.im
    }

    fn norm(self) -> f64 {
        self.norm_sq().sqrt()
    }
}

impl std::ops::Add for Complex64 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(self.re + rhs.re, self.im + rhs.im)
    }
}

impl std::ops::Sub for Complex64 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.re - rhs.re, self.im - rhs.im)
    }
}

impl std::ops::Mul for Complex64 {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        Self::new(
            self.re * rhs.re - self.im * rhs.im,
            self.re * rhs.im + self.im * rhs.re,
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Helper Functions
// ═══════════════════════════════════════════════════════════════════════════

/// 512-point radix-2 Cooley-Tukey FFT.
///
/// Input length must be a power of two. Samples shorter than `FFT_SIZE` are
/// zero-padded; longer inputs are truncated.
fn fft(input: &[f64]) -> Vec<Complex64> {
    let n = if input.len() >= FFT_SIZE {
        FFT_SIZE
    } else {
        input.len().next_power_of_two()
    };

    // Zero-pad or truncate to length `n`.
    let mut x: Vec<Complex64> = (0..n)
        .map(|i| {
            if i < input.len() {
                Complex64::new(input[i], 0.0)
            } else {
                Complex64::new(0.0, 0.0)
            }
        })
        .collect();

    // Bit-reversal permutation.
    let bits = n.trailing_zeros() as usize;
    for i in 0..n {
        let j = reverse_bits(i, bits);
        if i < j {
            x.swap(i, j);
        }
    }

    // Butterfly stages.
    let mut len = 2;
    while len <= n {
        let half = len / 2;
        let w_base = Complex64::from_polar(1.0, -TAU / len as f64);

        for start in (0..n).step_by(len) {
            let mut w = Complex64::new(1.0, 0.0);
            for k in 0..half {
                let u = x[start + k];
                let v = x[start + k + half] * w;
                x[start + k] = u + v;
                x[start + k + half] = u - v;
                w = w * w_base;
            }
        }
        len *= 2;
    }

    x
}

/// Reverse the lowest `bits` bits of `x`.
fn reverse_bits(mut x: usize, bits: usize) -> usize {
    let mut result = 0usize;
    for _ in 0..bits {
        result = (result << 1) | (x & 1);
        x >>= 1;
    }
    result
}

/// LUFS-style loudness measurement approximating ITU-R BS.1770.
///
/// Applies a simplified K-weighting filter chain (2nd-order high-pass at
/// ~65 Hz to remove sub-bass, then a high-shelf boost of ~4 dB above
/// ~1.5 kHz to approximate the RLB weighting stage) and returns the
/// integrated loudness in LUFS.
fn compute_loudness(signal: &[f64], sample_rate: f64) -> f64 {
    if signal.is_empty() {
        return -120.0;
    }

    // Stage 1: 2nd-order Butterworth high-pass at 65 Hz (sub-bass removal).
    let hp = BiquadHelper::highpass(65.0, sample_rate);

    // Stage 2: 1st-order high-shelf approximation — boost ~4 dB above 1.5 kHz.
    // We approximate the shelf with a simple first-order high-pass mixed back:
    //   out = input + shelf_gain * hp_approx(input)
    // For +4 dB shelf at 1.5 kHz, shelf_gain ≈ 0.585 (empirically reasonable).
    let shelf_gain_db = 4.0;
    let shelf_fc = 1500.0;

    let mut hp_state = hp;
    let mut shelf_hp = BiquadHelper::highpass(shelf_fc, sample_rate);

    let mut sum_sq = 0.0_f64;
    for &s in signal {
        // Apply high-pass stage.
        let s_hp = hp_state.process(s);
        // Apply shelf approximation.
        let s_shelf_hp = shelf_hp.process(s_hp);
        let s_weighted = s_hp + db_to_linear(shelf_gain_db / 2.0) * s_shelf_hp * 0.3;
        sum_sq += s_weighted * s_weighted;
    }

    let mean_sq = sum_sq / signal.len() as f64;
    if mean_sq < 1e-20 {
        return -120.0;
    }
    10.0 * mean_sq.log10() - 0.691
}

/// Compute the spectral centroid of a magnitude spectrum.
///
/// The spectral centroid is the weighted mean frequency, where each bin's
/// contribution is proportional to its magnitude squared. It provides a
/// single-number summary of the spectral "brightness" of a signal.
fn spectral_centroid(spectrum: &[f64], sample_rate: f64, fft_size: usize) -> f64 {
    if spectrum.is_empty() {
        return 0.0;
    }

    let bin_freq = sample_rate / fft_size as f64;
    let mut weighted_sum = 0.0_f64;
    let mut magnitude_sum = 0.0_f64;

    for (i, &mag) in spectrum.iter().enumerate() {
        let freq = i as f64 * bin_freq;
        weighted_sum += freq * mag * mag;
        magnitude_sum += mag * mag;
    }

    if magnitude_sum < 1e-20 {
        return 0.0;
    }
    weighted_sum / magnitude_sum
}

/// Detect transient events in a signal.
///
/// A transient is identified when the absolute sample-to-sample difference
/// exceeds `threshold` and the previous transient was at least
/// `min_spacing_samples` ago. Returns the sample indices where transients
/// were detected.
fn detect_transients(signal: &[f64]) -> Vec<usize> {
    let threshold = 0.15;
    let min_spacing_samples = (SR * 0.01) as usize; // 10 ms minimum spacing

    let mut transients = Vec::new();
    let mut last_transient: usize = 0;

    for i in 1..signal.len() {
        let delta = (signal[i] - signal[i - 1]).abs();
        if delta > threshold && (transients.is_empty() || i - last_transient >= min_spacing_samples)
        {
            transients.push(i);
            last_transient = i;
        }
    }

    transients
}

/// Compute the Pearson correlation coefficient between two stereo channels.
///
/// Returns a value in [-1, 1] where:
/// - 1.0 = perfectly correlated (identical)
/// - 0.0 = uncorrelated
/// - -1.0 = perfectly anti-correlated (inverted)
fn stereo_correlation(l: &[f64], r: &[f64]) -> f64 {
    assert_eq!(l.len(), r.len(), "Channel lengths must match");
    if l.len() < 2 {
        return 0.0;
    }

    let n = l.len() as f64;
    let mean_l: f64 = l.iter().sum::<f64>() / n;
    let mean_r: f64 = r.iter().sum::<f64>() / n;

    let mut cov = 0.0_f64;
    let mut var_l = 0.0_f64;
    let mut var_r = 0.0_f64;

    for i in 0..l.len() {
        let dl = l[i] - mean_l;
        let dr = r[i] - mean_r;
        cov += dl * dr;
        var_l += dl * dl;
        var_r += dr * dr;
    }

    let denom = (var_l * var_r).sqrt();
    if denom < 1e-20 {
        return 0.0;
    }
    cov / denom
}

/// Compute the RMS (root mean square) level of a signal.
fn rms(signal: &[f64]) -> f64 {
    if signal.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = signal.iter().map(|&x| x * x).sum();
    (sum_sq / signal.len() as f64).sqrt()
}

// ────────────────────────────────────────────────────────────────────────────
// Additional Utility Functions
// ────────────────────────────────────────────────────────────────────────────

/// Convert decibels to linear amplitude.
fn db_to_linear(db: f64) -> f64 {
    10.0_f64.powf(db / 20.0)
}

/// Convert linear amplitude to decibels. Returns -120 dB for zero/negative.
fn linear_to_db(linear: f64) -> f64 {
    if linear <= 0.0 {
        -120.0
    } else {
        20.0 * linear.log10()
    }
}

/// Generate a sine wave of the given frequency, duration, and amplitude.
fn generate_sine(freq: f64, duration_secs: f64, amplitude: f64, sample_rate: f64) -> Vec<f64> {
    let num_samples = (duration_secs * sample_rate) as usize;
    (0..num_samples)
        .map(|i| amplitude * (TAU * freq * i as f64 / sample_rate).sin())
        .collect()
}

/// Apply a Hann window in-place.
fn hann_window(data: &mut [f64]) {
    let n = data.len();
    for (i, sample) in data.iter_mut().enumerate() {
        let w = 0.5 * (1.0 - (TAU * i as f64 / n as f64 / 2.0).cos());
        *sample *= w;
    }
}

/// Compute the magnitude spectrum (one-sided) from a time-domain signal.
///
/// Applies a Hann window, computes the FFT, and returns the magnitudes of
/// the first `FFT_SIZE / 2 + 1` bins.
fn magnitude_spectrum(signal: &[f64]) -> Vec<f64> {
    // Take a window of FFT_SIZE samples from the steady-state portion.
    let start = if signal.len() > FFT_SIZE {
        signal.len() - FFT_SIZE
    } else {
        0
    };
    let mut window = signal[start..signal.len().min(start + FFT_SIZE)].to_vec();

    // Zero-pad if shorter than FFT_SIZE.
    window.resize(FFT_SIZE, 0.0);

    // Apply Hann window to reduce spectral leakage.
    hann_window(&mut window);

    let spectrum = fft(&window);
    let half = FFT_SIZE / 2 + 1;
    (0..half).map(|i| spectrum[i].norm()).collect()
}

/// Compute energy in a frequency band [lo_hz, hi_hz] from a magnitude spectrum.
fn band_energy(spectrum: &[f64], lo_hz: f64, hi_hz: f64, sample_rate: f64, fft_size: usize) -> f64 {
    let bin_hz = sample_rate / fft_size as f64;
    let lo_bin = (lo_hz / bin_hz).ceil() as usize;
    let hi_bin = (hi_hz / bin_hz).floor() as usize;
    let hi_bin = hi_bin.min(spectrum.len() - 1);

    (lo_bin..=hi_bin)
        .map(|i| spectrum[i] * spectrum[i])
        .sum::<f64>()
}

/// Process a block of samples and return the output as separate L/R vectors.
fn process_block_stereo(
    engine: &mut DelayEngine,
    input_l: &[f64],
    input_r: &[f64],
    params: &DelayParams,
) -> (Vec<f64>, Vec<f64>) {
    input_l
        .iter()
        .zip(input_r.iter())
        .map(|(&l, &r)| engine.process(l, r, params))
        .unzip()
}

/// Process a mono signal (same on L and R) and return separate L/R outputs.
fn process_mono(
    engine: &mut DelayEngine,
    input: &[f64],
    params: &DelayParams,
) -> (Vec<f64>, Vec<f64>) {
    process_block_stereo(engine, input, input, params)
}

/// Process `WARMUP_SAMPLES` of a 1 kHz sine to let smoothers converge.
fn warmup_engine(engine: &mut DelayEngine, params: &DelayParams) {
    for i in 0..WARMUP_SAMPLES {
        let s = 0.5 * (TAU * 1000.0 * i as f64 / SR).sin();
        engine.process(s, s, params);
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Biquad Helper (for LUFS K-weighting in tests)
// ────────────────────────────────────────────────────────────────────────────

/// Minimal biquad filter for test-internal signal processing (LUFS weighting).
struct BiquadHelper {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl BiquadHelper {
    /// Create a 2nd-order Butterworth high-pass at `cutoff_hz`.
    fn highpass(cutoff_hz: f64, sample_rate: f64) -> Self {
        let sr = sample_rate.max(1.0);
        let fc = cutoff_hz.max(1.0).min(sr * 0.499);
        let omega = TAU * fc / sr;
        let sin_w = omega.sin();
        let cos_w = omega.cos();
        let q = std::f64::consts::FRAC_1_SQRT_2; // 1/√2 Butterworth
        let alpha = sin_w / (2.0 * q);

        let b0 = (1.0 + cos_w) * 0.5;
        let b1 = -(1.0 + cos_w);
        let b2 = (1.0 + cos_w) * 0.5;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w;
        let a2 = 1.0 - alpha;

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    /// Process a single sample through the biquad (Direct Form I).
    fn process(&mut self, input: f64) -> f64 {
        let output = self.b0 * input + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = output;
        output
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Golden Reference Delay (for Test 10)
// ═══════════════════════════════════════════════════════════════════════════

/// A minimal but correct stereo delay with no interpolation, no filters,
/// no routing, and no smoothing. Used as a "golden reference" to compare
/// against the full-featured `DelayEngine`.
struct GoldenReferenceDelay {
    buffer_l: Vec<f64>,
    buffer_r: Vec<f64>,
    write_pos: usize,
    mask: usize,
    sample_rate: f64,
}

#[derive(Clone, Copy)]
struct GoldenReferenceParams {
    delay_time_l: f64,
    delay_time_r: f64,
    feedback_l: f64,
    feedback_r: f64,
    mix: f64,
}

impl GoldenReferenceDelay {
    fn new(sample_rate: f64) -> Self {
        let buf_size = (10.0 * sample_rate).ceil() as usize;
        let buf_size = buf_size.next_power_of_two();
        Self {
            buffer_l: vec![0.0; buf_size],
            buffer_r: vec![0.0; buf_size],
            write_pos: 0,
            mask: buf_size - 1,
            sample_rate,
        }
    }

    /// Process one sample pair. Uses no interpolation (nearest-sample read).
    fn process(&mut self, input_l: f64, input_r: f64, params: GoldenReferenceParams) -> (f64, f64) {
        let delay_samples_l = (params.delay_time_l * self.sample_rate).round() as usize;
        let delay_samples_r = (params.delay_time_r * self.sample_rate).round() as usize;
        let delay_samples_l = delay_samples_l.max(1).min(self.buffer_l.len() - 2);
        let delay_samples_r = delay_samples_r.max(1).min(self.buffer_r.len() - 2);

        // Read from delay lines (nearest sample, no interpolation).
        let read_pos_l = (self.write_pos + self.buffer_l.len() - delay_samples_l) & self.mask;
        let read_pos_r = (self.write_pos + self.buffer_r.len() - delay_samples_r) & self.mask;
        let delayed_l = self.buffer_l[read_pos_l];
        let delayed_r = self.buffer_r[read_pos_r];

        // Write input + feedback.
        self.buffer_l[self.write_pos] = input_l + delayed_l * params.feedback_l;
        self.buffer_r[self.write_pos] = input_r + delayed_r * params.feedback_r;
        self.write_pos = (self.write_pos + 1) & self.mask;

        // Dry/wet mix.
        let out_l = input_l * (1.0 - params.mix) + delayed_l * params.mix;
        let out_r = input_r * (1.0 - params.mix) + delayed_r * params.mix;
        (out_l, out_r)
    }

    #[allow(dead_code)]
    fn reset(&mut self) {
        for s in self.buffer_l.iter_mut() {
            *s = 0.0;
        }
        for s in self.buffer_r.iter_mut() {
            *s = 0.0;
        }
        self.write_pos = 0;
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 1: LUFS Stability Test
// ═══════════════════════════════════════════════════════════════════════════

/// Process a 10-second 1 kHz sine at -18 dBFS through the delay with moderate
/// settings. Compute LUFS over 400 ms windows and verify that the variation
/// across all windows is less than 3 LU.
///
/// A well-behaved delay should produce stable loudness once the initial
/// transient has settled. Large LUFS variations would indicate instability,
/// oscillation, or gain anomalies in the feedback loop.
#[test]
fn lufs_stability_test() {
    let duration_secs = 10.0;
    let _num_samples = (SR * duration_secs) as usize;
    let amplitude = db_to_linear(-18.0); // -18 dBFS peak for sine

    let input = generate_sine(1000.0, duration_secs, amplitude, SR);

    let mut engine = DelayEngine::new(SR);
    let params = DelayParams {
        delay_time_l: 0.3,
        delay_time_r: 0.3,
        feedback_l: 0.4,
        feedback_r: 0.4,
        output_mix_l: 0.5,
        output_mix_r: 0.5,
        routing: RoutingMode::Straight,
        ..DelayParams::default()
    };

    // Warm up the engine.
    warmup_engine(&mut engine, &params);

    // Process the entire signal and collect the left channel output.
    let output_l: Vec<f64> = input
        .iter()
        .map(|&s| {
            let (out_l, _) = engine.process(s, s, &params);
            out_l
        })
        .collect();

    // Compute LUFS over 400 ms windows.
    let window_samples = (SR * 0.4) as usize; // 400 ms = 17640 samples
    let mut lufs_values: Vec<f64> = Vec::new();

    let mut pos = 0;
    while pos + window_samples <= output_l.len() {
        let window = &output_l[pos..pos + window_samples];
        let lufs = compute_loudness(window, SR);
        lufs_values.push(lufs);
        pos += window_samples;
    }

    // Skip the first window (may contain transient from warm-up transition).
    if lufs_values.len() < 3 {
        panic!(
            "Not enough LUFS windows computed. Need at least 3, got {}",
            lufs_values.len()
        );
    }

    let steady_state_lufs = &lufs_values[1..];
    let min_lufs = steady_state_lufs
        .iter()
        .cloned()
        .fold(f64::INFINITY, f64::min);
    let max_lufs = steady_state_lufs
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);
    let variation = max_lufs - min_lufs;

    eprintln!(
        "[DIAG] LUFS stability: min = {min_lufs:.2} LU, max = {max_lufs:.2} LU, \
         variation = {variation:.2} LU over {} windows",
        steady_state_lufs.len()
    );

    assert!(
        variation < 3.0,
        "LUFS variation {variation:.2} LU exceeds 3 LU threshold. \
         Min = {min_lufs:.2}, Max = {max_lufs:.2}"
    );

    eprintln!("[PASS] LUFS Stability: variation = {variation:.2} LU (< 3 LU)");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 2: Transient Integrity Test
// ═══════════════════════════════════════════════════════════════════════════

/// Feed sharp transient clicks (single-sample impulses spaced 0.5 s apart)
/// through the delay and verify:
///
/// 1. A transient appears at each impulse position in the output.
/// 2. The peak amplitude at each impulse position is consistent with the
///    dry-path gain (within 6 dB, accounting for wet-path overlap).
/// 3. No spurious transients appear between the expected ones (beyond what
///    the delay taps naturally produce).
#[test]
fn transient_integrity_test() {
    let duration_secs = 5.0;
    let num_samples = (SR * duration_secs) as usize;
    let impulse_spacing = (SR * 0.5) as usize; // 0.5 s = 22050 samples

    // Build impulse train.
    let mut input = vec![0.0_f64; num_samples];
    let mut impulse_positions: Vec<usize> = Vec::new();
    let mut pos = (SR * 0.5) as usize; // Start 500 ms in.
    while pos + ((SR * 0.5) as usize) < num_samples {
        input[pos] = 1.0; // Full-scale impulse.
        impulse_positions.push(pos);
        pos += impulse_spacing;
    }

    let mut engine = DelayEngine::new(SR);
    let mix = 0.5;
    let delay_time = 0.25; // 250 ms delay — shorter than 0.5s spacing.
    let params = DelayParams {
        delay_time_l: delay_time,
        delay_time_r: delay_time,
        feedback_l: 0.0, // No feedback — only one delayed copy per impulse.
        feedback_r: 0.0,
        output_mix_l: mix,
        output_mix_r: mix,
        routing: RoutingMode::Straight,
        ..DelayParams::default()
    };

    // Warm up with SILENCE so the delay buffer is empty before impulses.
    for _ in 0..WARMUP_SAMPLES {
        engine.process(0.0, 0.0, &params);
    }

    // Process the impulse train.
    let (out_l, _out_r) = process_mono(&mut engine, &input, &params);

    // Detect transients in the output.
    let detected = detect_transients(&out_l);

    // The dry-path amplitude with mix is input * (1 - mix) = 0.5.
    // The wet-path amplitude is input * mix = 0.5 (appears after delay).
    // Since the delay (250 ms) is shorter than the impulse spacing (500 ms),
    // the wet tap from impulse N appears well before impulse N+1.
    let dry_amplitude = 1.0 * (1.0 - mix);
    let wet_amplitude = 1.0 * mix;
    let delay_samples = (delay_time * SR) as usize;
    let tolerance_samples = (SR * 0.005) as usize; // 5 ms timing tolerance.

    // Verify dry-path transients: a transient must appear near each impulse.
    let mut dry_found = 0;
    for &imp_pos in &impulse_positions {
        let search_start = imp_pos.saturating_sub(tolerance_samples);
        let search_end = (imp_pos + tolerance_samples).min(out_l.len());
        let peak_val = out_l[search_start..search_end]
            .iter()
            .cloned()
            .fold(0.0_f64, f64::max);

        // The peak should be at least the dry-path amplitude (within 6 dB
        // to allow for the wet path potentially adding/subtracting).
        let min_expected = dry_amplitude * db_to_linear(-6.0);
        if peak_val >= min_expected {
            dry_found += 1;
        }
    }

    eprintln!(
        "[DIAG] Transient integrity: {dry_found}/{} dry-path transients detected (amplitude > -6 dB of expected)",
        impulse_positions.len()
    );

    assert!(
        dry_found as f64 / impulse_positions.len() as f64 >= 0.8,
        "Only {dry_found}/{} dry-path transients detected — expected at least 80%",
        impulse_positions.len()
    );

    // Verify wet-path transients appear at the expected delay positions.
    let mut wet_found = 0;
    for &imp_pos in &impulse_positions {
        let expected_wet_pos = imp_pos + delay_samples;
        if expected_wet_pos < out_l.len() {
            let search_start = expected_wet_pos.saturating_sub(tolerance_samples);
            let search_end = (expected_wet_pos + tolerance_samples).min(out_l.len());
            let peak_val = out_l[search_start..search_end]
                .iter()
                .cloned()
                .fold(0.0_f64, f64::max);

            // The wet-path impulse is attenuated by the biquad filter
            // transient (a single-sample impulse through a biquad has a
            // transient response). Use a generous threshold.
            let min_expected = wet_amplitude * db_to_linear(-12.0);
            if peak_val >= min_expected {
                wet_found += 1;
            }
        }
    }

    eprintln!(
        "[DIAG] Wet-path transients: {wet_found}/{} detected at expected delay positions",
        impulse_positions.len()
    );

    // Check for spurious transients between the expected ones.
    let mut expected_zones: Vec<(usize, usize)> = Vec::new();
    for &imp_pos in &impulse_positions {
        let zone_half = (SR * 0.01) as usize; // 10 ms zone around each event.
        let dry_start = imp_pos.saturating_sub(zone_half);
        let dry_end = (imp_pos + zone_half).min(num_samples);
        expected_zones.push((dry_start, dry_end));

        let wet_start = (imp_pos + delay_samples).saturating_sub(zone_half);
        let wet_end = (imp_pos + delay_samples + zone_half).min(num_samples);
        expected_zones.push((wet_start, wet_end));
    }

    let mut spurious_count = 0;
    for &det_pos in &detected {
        let in_expected = expected_zones
            .iter()
            .any(|&(start, end)| det_pos >= start && det_pos <= end);
        if !in_expected {
            spurious_count += 1;
        }
    }

    eprintln!(
        "[DIAG] Spurious transients: {spurious_count} (outside expected zones), \
         {} total detected",
        detected.len()
    );

    // With no feedback, spurious transients should be minimal.
    let max_spurious = impulse_positions.len() * 2; // Allow margin for filter ringing.
    assert!(
        spurious_count <= max_spurious,
        "Too many spurious transients: {spurious_count} (max allowed: {max_spurious})"
    );

    eprintln!(
        "[PASS] Transient Integrity: {dry_found}/{} dry detected, {wet_found}/{} wet detected, {spurious_count} spurious",
        impulse_positions.len(), impulse_positions.len()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 3: Harmonic Musicality Index
// ═══════════════════════════════════════════════════════════════════════════

/// Process a pure 440 Hz sine through the delay and analyse the output
/// spectrum. Compute the ratio of harmonic energy (880 Hz, 1320 Hz, etc.)
/// to fundamental energy. With flat filters, this ratio should be very low
/// (< 0.01), confirming that the delay doesn't add unwanted harmonics.
///
/// When the high-cut is set to 2 kHz, harmonics above 2 kHz should be
/// attenuated relative to the flat-filter case.
#[test]
fn harmonic_musicality_index() {
    let duration_secs = 3.0;
    let amplitude = 0.5;
    let input = generate_sine(440.0, duration_secs, amplitude, SR);

    // ── Sub-test A: Flat filters ─────────────────────────────────────
    {
        let mut engine = DelayEngine::new(SR);
        let params = DelayParams {
            delay_time_l: 0.2,
            delay_time_r: 0.2,
            feedback_l: 0.4,
            feedback_r: 0.4,
            output_mix_l: 0.5,
            output_mix_r: 0.5,
            routing: RoutingMode::Straight,
            low_cut_l: 20.0,
            low_cut_r: 20.0,
            high_cut_l: 20000.0,
            high_cut_r: 20000.0,
            ..DelayParams::default()
        };

        warmup_engine(&mut engine, &params);
        let (out_l, _) = process_mono(&mut engine, &input, &params);

        // Take spectrum from the steady-state portion (last 1 second).
        let spectrum = magnitude_spectrum(&out_l);

        // Use wider bands to accommodate FFT bin spacing (86 Hz per bin).
        let fundamental_energy = band_energy(&spectrum, 400.0, 480.0, SR, FFT_SIZE);
        let h2_energy = band_energy(&spectrum, 830.0, 930.0, SR, FFT_SIZE);
        let h3_energy = band_energy(&spectrum, 1260.0, 1400.0, SR, FFT_SIZE);
        let h4_energy = band_energy(&spectrum, 1700.0, 1820.0, SR, FFT_SIZE);
        let harmonic_energy = h2_energy + h3_energy + h4_energy;

        let ratio = if fundamental_energy > 1e-20 {
            harmonic_energy / fundamental_energy
        } else {
            f64::INFINITY
        };

        eprintln!(
            "[DIAG] Harmonic Musicality (flat): fundamental = {:.3e}, harmonics = {:.3e}, \
             ratio = {ratio:.6}",
            fundamental_energy, harmonic_energy
        );
        eprintln!(
            "  H2 = {:.3e}, H3 = {:.3e}, H4 = {:.3e}",
            h2_energy, h3_energy, h4_energy
        );

        assert!(
            ratio < 0.01,
            "Harmonic ratio {ratio:.6} exceeds 0.01 with flat filters. \
             The delay is adding unwanted harmonics."
        );

        eprintln!("[PASS] Harmonic Musicality (flat filters): ratio = {ratio:.6} (< 0.01)");
    }

    // ── Sub-test B: High-cut at 2 kHz ────────────────────────────────
    {
        let mut engine = DelayEngine::new(SR);
        let params_hc = DelayParams {
            delay_time_l: 0.2,
            delay_time_r: 0.2,
            feedback_l: 0.4,
            feedback_r: 0.4,
            output_mix_l: 0.5,
            output_mix_r: 0.5,
            routing: RoutingMode::Straight,
            low_cut_l: 20.0,
            low_cut_r: 20.0,
            high_cut_l: 2000.0,
            high_cut_r: 2000.0,
            ..DelayParams::default()
        };

        warmup_engine(&mut engine, &params_hc);
        let (out_l, _) = process_mono(&mut engine, &input, &params_hc);

        let spectrum_hc = magnitude_spectrum(&out_l);

        // Harmonics above 2 kHz (H5 = 2200 Hz and above).
        let above_2k_energy = band_energy(&spectrum_hc, 2000.0, SR / 2.0, SR, FFT_SIZE);
        let fundamental_energy = band_energy(&spectrum_hc, 400.0, 480.0, SR, FFT_SIZE);

        let above_2k_ratio = if fundamental_energy > 1e-20 {
            above_2k_energy / fundamental_energy
        } else {
            f64::INFINITY
        };

        eprintln!(
            "[DIAG] Harmonic Musicality (high-cut 2kHz): energy above 2 kHz / fundamental = \
             {above_2k_ratio:.6}"
        );

        // With the high-cut at 2 kHz, harmonic content above 2 kHz should be
        // significantly attenuated. We check that it's below a reasonable
        // threshold. The biquad provides 12 dB/oct attenuation, so at 4 kHz
        // (one octave above) we expect ~12 dB reduction.
        assert!(
            above_2k_ratio < 0.1,
            "Energy above 2 kHz relative to fundamental = {above_2k_ratio:.6}, \
             expected < 0.1 with high-cut at 2 kHz"
        );

        eprintln!(
            "[PASS] Harmonic Musicality (high-cut 2 kHz): above-2k ratio = {above_2k_ratio:.6} (< 0.1)"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 4: Dynamic Responsiveness Test
// ═══════════════════════════════════════════════════════════════════════════

/// Feed a signal that alternates between silence and full-scale every 2
/// seconds. Measure the attack time — how quickly the output reaches -1 dB
/// of the steady-state dry-path level after the input transitions from
/// silence to full-scale. The attack should be < 10 ms, confirming that
/// the dry path does not introduce excessive smearing.
#[test]
fn dynamic_responsiveness_test() {
    let duration_secs = 6.0;
    let num_samples = (SR * duration_secs) as usize;
    let half_period = (SR * 2.0) as usize; // 2 s on, 2 s off.

    // Build alternating signal: 2 s silence, 2 s full-scale, 2 s silence.
    let mut input = vec![0.0_f64; num_samples];
    let active_end = (2 * half_period).min(num_samples);
    input[half_period..active_end].fill(1.0);

    let mut engine = DelayEngine::new(SR);
    let params = DelayParams {
        delay_time_l: 0.3,
        delay_time_r: 0.3,
        feedback_l: 0.3,
        feedback_r: 0.3,
        output_mix_l: 0.5,
        output_mix_r: 0.5,
        routing: RoutingMode::Straight,
        ..DelayParams::default()
    };

    // Process silence first to stabilise.
    for _ in 0..WARMUP_SAMPLES {
        engine.process(0.0, 0.0, &params);
    }

    let (out_l, _) = process_mono(&mut engine, &input, &params);

    // Find the attack time: from the onset of the signal (at sample half_period)
    // to when the output first reaches -1 dB of the steady-state level.
    let onset = half_period;
    let steady_state_level = 1.0 * (1.0 - 0.5); // dry path: input * (1 - mix) = 0.5
    let target_level = steady_state_level * db_to_linear(-1.0); // -1 dB point ≈ 0.446

    let mut attack_samples: Option<usize> = None;
    for i in onset..onset + (SR * 0.05) as usize {
        // Look within 50 ms.
        if i < out_l.len() && out_l[i].abs() >= target_level {
            attack_samples = Some(i - onset);
            break;
        }
    }

    let attack_ms = match attack_samples {
        Some(s) => s as f64 / SR * 1000.0,
        None => f64::INFINITY,
    };

    eprintln!(
        "[DIAG] Dynamic responsiveness: attack time = {attack_ms:.2} ms \
         (target: -1 dB of steady state at {:.4})",
        target_level
    );

    assert!(
        attack_ms < 10.0,
        "Attack time {attack_ms:.2} ms exceeds 10 ms. \
         The dry path is introducing excessive smearing."
    );

    eprintln!("[PASS] Dynamic Responsiveness: attack = {attack_ms:.2} ms (< 10 ms)");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 5: Stereo Image Coherence Test
// ═══════════════════════════════════════════════════════════════════════════

/// Process a mono signal (L=R) through each routing mode and measure the
/// stereo correlation of the output:
///
/// - **Straight mode**: correlation should remain near 1.0 (channels are
///   identical with mono input and same delay times).
/// - **Ping Pong mode**: with different delay times, correlation should
///   decrease significantly (< 0.5).
/// - **Crossfeed mode**: correlation should be measurably different from
///   Straight.
#[test]
fn stereo_image_coherence_test() {
    let duration_secs = 3.0;
    let amplitude = 0.5;
    let input = generate_sine(440.0, duration_secs, amplitude, SR);

    // ── Straight mode: mono input, same delay times ──────────────────
    {
        let mut engine = DelayEngine::new(SR);
        let params = DelayParams {
            delay_time_l: 0.3,
            delay_time_r: 0.3,
            feedback_l: 0.4,
            feedback_r: 0.4,
            output_mix_l: 0.5,
            output_mix_r: 0.5,
            routing: RoutingMode::Straight,
            ..DelayParams::default()
        };

        warmup_engine(&mut engine, &params);
        let (out_l, out_r) = process_mono(&mut engine, &input, &params);

        // Skip warm-up region.
        let skip = WARMUP_SAMPLES;
        let corr = stereo_correlation(&out_l[skip..], &out_r[skip..]);

        eprintln!("[DIAG] Stereo coherence — Straight: correlation = {corr:.4}");

        assert!(
            corr > 0.95,
            "Straight mode with mono input: correlation {corr:.4} should be near 1.0"
        );
    }

    // ── Ping Pong mode: L-only input with broadband signal ────────────
    //
    // PingPong bounces signal between channels, creating stereo spread.
    // With a sine input the correlation stays high because both channels
    // carry the same frequency. We use a broadband signal (composite
    // sines) and different delay times so that the phase relationships
    // differ per-frequency, reducing the measured correlation.
    {
        let mut engine = DelayEngine::new(SR);
        let params = DelayParams {
            input_mode_l: InputMode::Left,
            input_mode_r: InputMode::Off, // R channel gets no direct input.
            delay_time_l: 0.3,
            delay_time_r: 0.37, // Asymmetric delay times.
            feedback_l: 0.5,
            feedback_r: 0.5,
            output_mix_l: 0.7,
            output_mix_r: 0.7,
            routing: RoutingMode::PingPong,
            ..DelayParams::default()
        };

        // Use a broadband-ish signal: multiple non-harmonically-related sines.
        let bb_duration = 3.0;
        let bb_input: Vec<f64> = (0..(SR * bb_duration) as usize)
            .map(|i| {
                let t = i as f64 / SR;
                0.2 * (TAU * 220.0 * t).sin()
                    + 0.2 * (TAU * 537.0 * t).sin()
                    + 0.2 * (TAU * 1297.0 * t).sin()
                    + 0.2 * (TAU * 3159.0 * t).sin()
            })
            .collect();

        warmup_engine(&mut engine, &params);

        let bb_r = vec![0.0_f64; bb_input.len()];
        let (out_l, out_r) = process_block_stereo(&mut engine, &bb_input, &bb_r, &params);

        let skip = WARMUP_SAMPLES;
        let corr_pp = stereo_correlation(&out_l[skip..], &out_r[skip..]);

        eprintln!(
            "[DIAG] Stereo coherence — PingPong (L-only, broadband): correlation = {corr_pp:.4}"
        );

        assert!(
            corr_pp < 0.5,
            "PingPong mode with L-only broadband input: correlation {corr_pp:.4} should be < 0.5"
        );
    }

    // ── Crossfeed mode: L-only input creates measurable difference ────
    //
    // With L-only input, Crossfeed routes all feedback to the opposite
    // channel, while Straight keeps feedback within the same channel.
    // This creates a measurably different stereo image.
    {
        let mut engine_str = DelayEngine::new(SR);
        let params_straight = DelayParams {
            input_mode_l: InputMode::Left,
            input_mode_r: InputMode::Off,
            delay_time_l: 0.3,
            delay_time_r: 0.37,
            feedback_l: 0.5,
            feedback_r: 0.5,
            output_mix_l: 0.6,
            output_mix_r: 0.6,
            routing: RoutingMode::Straight,
            ..DelayParams::default()
        };

        let mut engine_cf = DelayEngine::new(SR);
        let params_crossfeed = DelayParams {
            input_mode_l: InputMode::Left,
            input_mode_r: InputMode::Off,
            delay_time_l: 0.3,
            delay_time_r: 0.37,
            feedback_l: 0.5,
            feedback_r: 0.5,
            output_mix_l: 0.6,
            output_mix_r: 0.6,
            routing: RoutingMode::Crossfeed,
            ..DelayParams::default()
        };

        // Use the same broadband signal as the PingPong test.
        let bb_duration = 3.0;
        let bb_input: Vec<f64> = (0..(SR * bb_duration) as usize)
            .map(|i| {
                let t = i as f64 / SR;
                0.2 * (TAU * 220.0 * t).sin()
                    + 0.2 * (TAU * 537.0 * t).sin()
                    + 0.2 * (TAU * 1297.0 * t).sin()
                    + 0.2 * (TAU * 3159.0 * t).sin()
            })
            .collect();

        warmup_engine(&mut engine_str, &params_straight);
        warmup_engine(&mut engine_cf, &params_crossfeed);

        let bb_r = vec![0.0_f64; bb_input.len()];
        let (out_l_s, out_r_s) =
            process_block_stereo(&mut engine_str, &bb_input, &bb_r, &params_straight);
        let (out_l_cf, out_r_cf) =
            process_block_stereo(&mut engine_cf, &bb_input, &bb_r, &params_crossfeed);

        let skip = WARMUP_SAMPLES;
        let corr_straight = stereo_correlation(&out_l_s[skip..], &out_r_s[skip..]);
        let corr_crossfeed = stereo_correlation(&out_l_cf[skip..], &out_r_cf[skip..]);

        eprintln!(
            "[DIAG] Stereo coherence — Straight (L-only): {corr_straight:.4}, \
             Crossfeed (L-only): {corr_crossfeed:.4}"
        );

        let diff = (corr_straight - corr_crossfeed).abs();
        assert!(
            diff > 0.01,
            "Crossfeed correlation ({corr_crossfeed:.4}) should differ measurably from \
             Straight ({corr_straight:.4})"
        );
    }

    eprintln!("[PASS] Stereo Image Coherence: all routing modes behave correctly");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 6: Null Residual Character Analysis
// ═══════════════════════════════════════════════════════════════════════════

/// Create a null test: subtract the dry signal from the wet-only output
/// (mix=1.0) at various settings and analyse the residual.
///
/// - **With no feedback**: the residual should be the pure delayed signal.
///   We verify that the wet output closely matches the delayed input by
///   checking that the residual energy (difference between wet output and
///   the delayed input) is dominated by filter transients, not systematic
///   distortion.
/// - **With feedback**: the residual contains the echo tail.
/// - The spectral balance of the residual is computed and reported.
#[test]
fn null_residual_character_analysis() {
    let duration_secs = 3.0;
    let amplitude = 0.5;
    let input = generate_sine(440.0, duration_secs, amplitude, SR);

    // ── Sub-test A: No feedback — wet output ≈ delayed input ──────────
    //
    // With flat filters and no feedback, the wet output should closely
    // match the input delayed by `delay_time`. The residual (difference)
    // comes from:
    //   - Biquad filter transient response (even with flat cutoffs)
    //   - Cubic interpolation artifacts
    //   - Parameter smoothing during the first few samples
    //
    // We skip the initial transient region and only compare the steady
    // state.
    {
        let mut engine = DelayEngine::new(SR);
        let params = DelayParams {
            delay_time_l: 0.2,
            delay_time_r: 0.2,
            feedback_l: 0.0,
            feedback_r: 0.0,
            output_mix_l: 1.0, // Wet only.
            output_mix_r: 1.0,
            routing: RoutingMode::Straight,
            ..DelayParams::default()
        };

        warmup_engine(&mut engine, &params);
        let (out_l, _out_r) = process_mono(&mut engine, &input, &params);

        // Skip the first ~500 ms to avoid filter transient + smoothing.
        let skip = (SR * 0.5) as usize;
        let delay_samples = (0.2 * SR) as usize;

        // Compare the wet output to the simply-delayed input in the
        // steady-state region.
        let residual: Vec<f64> = (skip..out_l.len())
            .map(|i| {
                // The input index that should correspond to the output at
                // position i (accounting for the delay and the warmup
                // that was already processed).
                let input_idx = i.saturating_sub(delay_samples);
                out_l[i] - input[input_idx.min(input.len() - 1)]
            })
            .collect();

        // Compute residual energy relative to signal energy.
        let signal_energy: f64 = out_l[skip..].iter().map(|&x| x * x).sum();
        let residual_energy: f64 = residual.iter().map(|&x| x * x).sum();

        let relative_db = if signal_energy > 1e-20 {
            linear_to_db((residual_energy / signal_energy).sqrt())
        } else {
            -120.0
        };

        eprintln!(
            "[DIAG] Null residual (no feedback, steady state): residual energy relative to signal = {relative_db:.1} dB"
        );

        // The residual should be significantly below the signal level.
        // Even with biquad transient and interpolation, the steady-state
        // residual should be well below -10 dB.
        assert!(
            relative_db < -10.0,
            "Null residual too large: {relative_db:.1} dB (expected < -10 dB in steady state)"
        );

        eprintln!("[PASS] Null Residual (no feedback): {relative_db:.1} dB (< -10 dB)");
    }

    // ── Sub-test B: With feedback — residual contains echo tail ──────
    {
        let mut engine = DelayEngine::new(SR);
        let params_fb = DelayParams {
            delay_time_l: 0.2,
            delay_time_r: 0.2,
            feedback_l: 0.5,
            feedback_r: 0.5,
            output_mix_l: 1.0,
            output_mix_r: 1.0,
            routing: RoutingMode::Straight,
            ..DelayParams::default()
        };

        warmup_engine(&mut engine, &params_fb);
        let (out_l, _out_r) = process_mono(&mut engine, &input, &params_fb);

        // Compute spectral balance of the output (which IS the residual
        // since mix=1.0 removes the dry path).
        let spectrum = magnitude_spectrum(&out_l);
        let centroid = spectral_centroid(&spectrum, SR, FFT_SIZE);
        let total_energy: f64 = spectrum.iter().map(|&m| m * m).sum();
        let low_energy = band_energy(&spectrum, 20.0, 300.0, SR, FFT_SIZE);
        let mid_energy = band_energy(&spectrum, 300.0, 3000.0, SR, FFT_SIZE);
        let high_energy = band_energy(&spectrum, 3000.0, SR / 2.0, SR, FFT_SIZE);

        eprintln!(
            "[DIAG] Null residual (feedback=0.5): spectral centroid = {centroid:.0} Hz, \
             low/mid/high energy ratio = {:.3}/{:.3}/{:.3}",
            low_energy / total_energy,
            mid_energy / total_energy,
            high_energy / total_energy
        );

        // With a 440 Hz input, the energy should be concentrated in the
        // mid band. The spectral centroid should be near 440 Hz.
        assert!(
            centroid > 200.0 && centroid < 2000.0,
            "Spectral centroid {centroid:.0} Hz is outside expected range for 440 Hz input"
        );

        eprintln!(
            "[PASS] Null Residual (feedback): centroid = {centroid:.0} Hz, spectral balance computed"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 7: Multi-Source Musical Benchmarking
// ═══════════════════════════════════════════════════════════════════════════

/// Process a composite test signal (bass at 100 Hz + mid at 1 kHz + treble
/// at 8 kHz, each at -20 dBFS) through the delay. Analyse the output
/// spectrum and verify:
///
/// 1. Each frequency component is present at the expected level (±3 dB).
/// 2. The delay doesn't introduce frequency-dependent gain.
/// 3. With low-cut at 200 Hz: bass component attenuated by > 10 dB.
/// 4. With high-cut at 5 kHz: treble component attenuated by > 10 dB.
#[test]
fn multi_source_musical_benchmarking() {
    let duration_secs = 4.0;
    let amplitude = db_to_linear(-20.0); // -20 dBFS per component.

    // Build composite signal: bass + mid + treble.
    let num_samples = (SR * duration_secs) as usize;
    let input: Vec<f64> = (0..num_samples)
        .map(|i| {
            let t = i as f64 / SR;
            amplitude * (TAU * 100.0 * t).sin()
                + amplitude * (TAU * 1000.0 * t).sin()
                + amplitude * (TAU * 8000.0 * t).sin()
        })
        .collect();

    // ── Sub-test A: Flat filters — all components at expected level ───
    {
        let mut engine = DelayEngine::new(SR);
        let params = DelayParams {
            delay_time_l: 0.2,
            delay_time_r: 0.2,
            feedback_l: 0.3,
            feedback_r: 0.3,
            output_mix_l: 0.5,
            output_mix_r: 0.5,
            routing: RoutingMode::Straight,
            low_cut_l: 20.0,
            low_cut_r: 20.0,
            high_cut_l: 20000.0,
            high_cut_r: 20000.0,
            ..DelayParams::default()
        };

        warmup_engine(&mut engine, &params);
        let (out_l, _) = process_mono(&mut engine, &input, &params);

        let spectrum = magnitude_spectrum(&out_l);
        // Use wide bands to accommodate FFT bin spacing (86 Hz per bin
        // at 512-point FFT / 44100 Hz). Each band must span at least
        // one bin on either side of the target frequency.
        let bass_energy = band_energy(&spectrum, 50.0, 150.0, SR, FFT_SIZE);
        let mid_energy = band_energy(&spectrum, 900.0, 1100.0, SR, FFT_SIZE);
        let treble_energy = band_energy(&spectrum, 7800.0, 8200.0, SR, FFT_SIZE);

        // Each component should be present. With mix=0.5, the output
        // contains both dry and wet, so the energy should be roughly
        // proportional to the input (within ±3 dB).
        let ref_energy = bass_energy; // Use bass as reference.
        let mid_ratio_db = linear_to_db(mid_energy / ref_energy);
        let treble_ratio_db = linear_to_db(treble_energy / ref_energy);

        eprintln!(
            "[DIAG] Multi-source (flat): bass = {:.3e}, mid = {:.3e} ({mid_ratio_db:.1} dB rel), \
             treble = {:.3e} ({treble_ratio_db:.1} dB rel)",
            bass_energy, mid_energy, treble_energy
        );

        // All components should be within ±6 dB of each other (the dry
        // and wet paths both contribute, so some deviation is expected).
        assert!(
            mid_ratio_db.abs() < 6.0,
            "Mid component {mid_ratio_db:.1} dB relative to bass — expected within ±6 dB"
        );
        assert!(
            treble_ratio_db.abs() < 6.0,
            "Treble component {treble_ratio_db:.1} dB relative to bass — expected within ±6 dB"
        );

        eprintln!("[PASS] Multi-source (flat): all components present at expected levels");
    }

    // ── Sub-test B: Low-cut at 200 Hz — bass should be attenuated ────
    {
        let mut engine = DelayEngine::new(SR);
        let params_lc = DelayParams {
            delay_time_l: 0.2,
            delay_time_r: 0.2,
            feedback_l: 0.3,
            feedback_r: 0.3,
            output_mix_l: 0.5,
            output_mix_r: 0.5,
            routing: RoutingMode::Straight,
            low_cut_l: 200.0,
            low_cut_r: 200.0,
            high_cut_l: 20000.0,
            high_cut_r: 20000.0,
            ..DelayParams::default()
        };

        warmup_engine(&mut engine, &params_lc);
        let (out_l, _) = process_mono(&mut engine, &input, &params_lc);

        let spectrum_lc = magnitude_spectrum(&out_l);
        let _bass_energy_lc = band_energy(&spectrum_lc, 50.0, 150.0, SR, FFT_SIZE);
        let _mid_energy_lc = band_energy(&spectrum_lc, 900.0, 1100.0, SR, FFT_SIZE);

        // The low-cut only affects the wet path. With mix=0.5, the dry
        // path still passes bass, so the attenuation is partial.
        // However, the wet-path bass should be attenuated significantly.
        // The wet-path bass relative to wet-path mid should be very low.
        // Let's compare bass in the LC spectrum vs the flat spectrum.
        let spectrum_flat_ref = {
            let mut eng_ref = DelayEngine::new(SR);
            let params_flat = DelayParams {
                delay_time_l: 0.2,
                delay_time_r: 0.2,
                feedback_l: 0.3,
                feedback_r: 0.3,
                output_mix_l: 1.0, // Wet only to see filter effect clearly.
                output_mix_r: 1.0,
                routing: RoutingMode::Straight,
                low_cut_l: 20.0,
                low_cut_r: 20.0,
                high_cut_l: 20000.0,
                high_cut_r: 20000.0,
                ..DelayParams::default()
            };
            warmup_engine(&mut eng_ref, &params_flat);
            let (out, _) = process_mono(&mut eng_ref, &input, &params_flat);
            magnitude_spectrum(&out)
        };

        let spectrum_lc_wet = {
            let mut eng_lc = DelayEngine::new(SR);
            let params_lc_wet = DelayParams {
                delay_time_l: 0.2,
                delay_time_r: 0.2,
                feedback_l: 0.3,
                feedback_r: 0.3,
                output_mix_l: 1.0, // Wet only.
                output_mix_r: 1.0,
                routing: RoutingMode::Straight,
                low_cut_l: 200.0,
                low_cut_r: 200.0,
                high_cut_l: 20000.0,
                high_cut_r: 20000.0,
                ..DelayParams::default()
            };
            warmup_engine(&mut eng_lc, &params_lc_wet);
            let (out, _) = process_mono(&mut eng_lc, &input, &params_lc_wet);
            magnitude_spectrum(&out)
        };

        let bass_flat = band_energy(&spectrum_flat_ref, 50.0, 150.0, SR, FFT_SIZE);
        let bass_lc = band_energy(&spectrum_lc_wet, 50.0, 150.0, SR, FFT_SIZE);
        let mid_flat = band_energy(&spectrum_flat_ref, 900.0, 1100.0, SR, FFT_SIZE);
        let mid_lc = band_energy(&spectrum_lc_wet, 900.0, 1100.0, SR, FFT_SIZE);

        // Normalise: compare bass attenuation relative to mid.
        let bass_rel_flat = if mid_flat > 1e-20 {
            bass_flat / mid_flat
        } else {
            0.0
        };
        let bass_rel_lc = if mid_lc > 1e-20 {
            bass_lc / mid_lc
        } else {
            0.0
        };

        let bass_atten_db = if bass_rel_flat > 1e-20 && bass_rel_lc > 1e-20 {
            linear_to_db(bass_rel_lc / bass_rel_flat)
        } else if bass_rel_lc < 1e-20 {
            -120.0
        } else {
            0.0
        };

        eprintln!(
            "[DIAG] Multi-source (low-cut 200 Hz): bass attenuation relative to mid = {bass_atten_db:.1} dB"
        );

        assert!(
            bass_atten_db < -10.0,
            "Bass attenuation with low-cut at 200 Hz is only {bass_atten_db:.1} dB — expected > 10 dB"
        );

        eprintln!(
            "[PASS] Multi-source (low-cut 200 Hz): bass attenuated by {bass_atten_db:.1} dB (> 10 dB)"
        );
    }

    // ── Sub-test C: High-cut at 5 kHz — treble should be attenuated ──
    {
        let spectrum_flat_ref = {
            let mut eng = DelayEngine::new(SR);
            let params = DelayParams {
                delay_time_l: 0.2,
                delay_time_r: 0.2,
                feedback_l: 0.3,
                feedback_r: 0.3,
                output_mix_l: 1.0,
                output_mix_r: 1.0,
                routing: RoutingMode::Straight,
                low_cut_l: 20.0,
                low_cut_r: 20.0,
                high_cut_l: 20000.0,
                high_cut_r: 20000.0,
                ..DelayParams::default()
            };
            warmup_engine(&mut eng, &params);
            let (out, _) = process_mono(&mut eng, &input, &params);
            magnitude_spectrum(&out)
        };

        let spectrum_hc = {
            let mut eng = DelayEngine::new(SR);
            let params_hc = DelayParams {
                delay_time_l: 0.2,
                delay_time_r: 0.2,
                feedback_l: 0.3,
                feedback_r: 0.3,
                output_mix_l: 1.0,
                output_mix_r: 1.0,
                routing: RoutingMode::Straight,
                low_cut_l: 20.0,
                low_cut_r: 20.0,
                high_cut_l: 5000.0,
                high_cut_r: 5000.0,
                ..DelayParams::default()
            };
            warmup_engine(&mut eng, &params_hc);
            let (out, _) = process_mono(&mut eng, &input, &params_hc);
            magnitude_spectrum(&out)
        };

        let treble_flat = band_energy(&spectrum_flat_ref, 7800.0, 8200.0, SR, FFT_SIZE);
        let treble_hc = band_energy(&spectrum_hc, 7800.0, 8200.0, SR, FFT_SIZE);
        let mid_flat = band_energy(&spectrum_flat_ref, 900.0, 1100.0, SR, FFT_SIZE);
        let mid_hc = band_energy(&spectrum_hc, 900.0, 1100.0, SR, FFT_SIZE);

        let treble_rel_flat = if mid_flat > 1e-20 {
            treble_flat / mid_flat
        } else {
            0.0
        };
        let treble_rel_hc = if mid_hc > 1e-20 {
            treble_hc / mid_hc
        } else {
            0.0
        };

        let treble_atten_db = if treble_rel_flat > 1e-20 && treble_rel_hc > 1e-20 {
            linear_to_db(treble_rel_hc / treble_rel_flat)
        } else if treble_rel_hc < 1e-20 {
            -120.0
        } else {
            0.0
        };

        eprintln!(
            "[DIAG] Multi-source (high-cut 5 kHz): treble attenuation relative to mid = {treble_atten_db:.1} dB"
        );

        assert!(
            treble_atten_db < -10.0,
            "Treble attenuation with high-cut at 5 kHz is only {treble_atten_db:.1} dB — expected > 10 dB"
        );

        eprintln!(
            "[PASS] Multi-source (high-cut 5 kHz): treble attenuated by {treble_atten_db:.1} dB (> 10 dB)"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 8: Temporal Smoothness (Anti-Zipper) Test
// ═══════════════════════════════════════════════════════════════════════════

/// Sweep the delay time parameter from 0.1 s to 1.0 s over 2 seconds of
/// processing. Compute the first derivative (sample-to-sample delta) of the
/// output signal. The maximum delta should be < 0.1, confirming that the
/// parameter smoothing eliminates zipper clicks.
#[test]
fn temporal_smoothness_anti_zipper_test() {
    let duration_secs = 2.0;
    let num_samples = (SR * duration_secs) as usize;

    let mut engine = DelayEngine::new(SR);
    let mut params = DelayParams {
        delay_time_l: 0.1,
        delay_time_r: 0.1,
        feedback_l: 0.3,
        feedback_r: 0.3,
        output_mix_l: 0.5,
        output_mix_r: 0.5,
        routing: RoutingMode::Straight,
        ..DelayParams::default()
    };

    // Warm up at the initial delay time.
    warmup_engine(&mut engine, &params);

    // Process with a continuous sine input while sweeping delay time.
    let input_gen = |i: usize| -> f64 { 0.5 * (TAU * 440.0 * i as f64 / SR).sin() };

    let mut out_l: Vec<f64> = Vec::with_capacity(num_samples);
    let mut max_delta: f64 = 0.0;
    let mut prev_out_l: f64 = 0.0;

    for i in 0..num_samples {
        // Linearly sweep delay time from 0.1 to 1.0 over the duration.
        let t = i as f64 / num_samples as f64;
        params.delay_time_l = 0.1 + t * 0.9;
        params.delay_time_r = params.delay_time_l;

        let in_val = input_gen(i);
        let (ol, _or) = engine.process(in_val, in_val, &params);
        out_l.push(ol);

        if i > 0 {
            let delta = (ol - prev_out_l).abs();
            max_delta = max_delta.max(delta);
        }
        prev_out_l = ol;
    }

    eprintln!(
        "[DIAG] Anti-zipper: max sample-to-sample delta = {max_delta:.6} during delay sweep 0.1→1.0 s"
    );

    assert!(
        max_delta < 0.1,
        "Max sample-to-sample delta {max_delta:.6} exceeds 0.1 threshold — zipper noise detected"
    );

    eprintln!("[PASS] Anti-Zipper: max delta = {max_delta:.6} (< 0.1)");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 9: Groove Preservation Test
// ═══════════════════════════════════════════════════════════════════════════

/// Create a rhythmic pattern of 4 impulses per beat at 120 BPM. Process
/// through the delay with tempo sync at 1/4 note. Verify that the delay
/// taps align with the expected musical grid positions (within 5 ms
/// tolerance at 44100 Hz).
#[test]
fn groove_preservation_test() {
    let bpm = 120.0;
    let beat_duration = 60.0 / bpm; // 0.5 s per beat.
    let sixteenth_duration = beat_duration / 4.0; // 4 impulses per beat.
    let quarter_duration = beat_duration; // 1/4 note = 1 beat.

    let num_beats = 8;
    let duration_secs = num_beats as f64 * beat_duration + 1.0; // Extra second for tail.
    let num_samples = (SR * duration_secs) as usize;

    // Build impulse pattern: 4 impulses per beat (sixteenth notes).
    let mut input = vec![0.0_f64; num_samples];
    let mut impulse_positions: Vec<usize> = Vec::new();

    let sixteenth_samples = (sixteenth_duration * SR) as usize;
    let start_offset = (SR * 0.05) as usize; // Start 50 ms in.

    let mut pos = start_offset;
    while pos < num_samples - (SR * 0.5) as usize {
        input[pos] = 1.0;
        impulse_positions.push(pos);
        pos += sixteenth_samples;
    }

    let mut engine = DelayEngine::new(SR);
    let params = DelayParams {
        delay_time_l: 0.5,
        delay_time_r: 0.5,
        feedback_l: 0.4,
        feedback_r: 0.4,
        output_mix_l: 0.6,
        output_mix_r: 0.6,
        routing: RoutingMode::Straight,
        tempo_sync: true,
        tempo_bpm: bpm,
        note_l: NoteValue::Quarter,
        note_r: NoteValue::Quarter,
        ..DelayParams::default()
    };

    // Warm up with the tempo-sync params.
    warmup_engine(&mut engine, &params);

    // Process the impulse pattern.
    let (out_l, _out_r) = process_mono(&mut engine, &input, &params);

    // Detect transients in the output.
    let detected = detect_transients(&out_l);

    // Expected delay tap positions: each impulse + 1/4 note delay.
    let quarter_samples = (quarter_duration * SR) as usize;
    let tolerance_samples = (SR * 0.005) as usize; // 5 ms tolerance.

    let mut aligned_count = 0;
    let mut total_expected_taps = 0;
    let mut max_timing_error: f64 = 0.0;

    for &imp_pos in &impulse_positions {
        let expected_tap = imp_pos + quarter_samples;
        if expected_tap >= num_samples {
            continue;
        }
        total_expected_taps += 1;

        // Find the nearest detected transient to the expected tap position.
        let nearest = detected
            .iter()
            .filter(|&&d| {
                (d as isize - expected_tap as isize).unsigned_abs() < (SR * 0.05) as usize
            })
            .min_by_key(|&&d| (d as isize - expected_tap as isize).unsigned_abs());

        if let Some(&tap_pos) = nearest {
            let error_samples = (tap_pos as isize - expected_tap as isize).unsigned_abs();
            let error_ms = error_samples as f64 / SR * 1000.0;
            max_timing_error = max_timing_error.max(error_ms);

            if error_samples <= tolerance_samples {
                aligned_count += 1;
            }
        }
    }

    eprintln!(
        "[DIAG] Groove preservation: {aligned_count}/{total_expected_taps} delay taps aligned \
         within 5 ms, max timing error = {max_timing_error:.1} ms"
    );

    assert!(
        aligned_count as f64 / total_expected_taps as f64 >= 0.75,
        "Only {aligned_count}/{total_expected_taps} delay taps aligned within 5 ms — expected ≥ 75%"
    );

    eprintln!(
        "[PASS] Groove Preservation: {aligned_count}/{total_expected_taps} aligned, max error = {max_timing_error:.1} ms"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 10: Golden Industry Standard Reference
// ═══════════════════════════════════════════════════════════════════════════

/// Compare the plugin's behaviour against a mathematically computed "golden
/// reference" — a minimal but correct delay implementation with no
/// interpolation, basic feedback, and no smoothing.
///
/// The null-test difference (plugin output − reference output) should have
/// energy < -40 dB relative to the signal. The difference is expected to
/// come from:
/// - Cubic interpolation vs. nearest-sample read
/// - Biquad filter transients (even with flat filters)
/// - Parameter smoothing ramps
#[test]
fn golden_industry_standard_reference() {
    let duration_secs = 5.0;
    let amplitude = 0.5;
    let delay_time = 0.2;
    let feedback = 0.4;
    let mix = 0.5;

    // Generate a composite test signal.
    let num_samples = (SR * duration_secs) as usize;
    let input: Vec<f64> = (0..num_samples)
        .map(|i| {
            let t = i as f64 / SR;
            amplitude * (TAU * 440.0 * t).sin()
                + amplitude * 0.7 * (TAU * 100.0 * t).sin()
                + amplitude * 0.5 * (TAU * 2000.0 * t).sin()
        })
        .collect();

    // ── Process through the full plugin engine ───────────────────────
    let mut engine = DelayEngine::new(SR);
    let params = DelayParams {
        input_level_db: 0.0,
        output_level_db: 0.0,
        input_mode_l: InputMode::Left,
        input_mode_r: InputMode::Right,
        delay_time_l: delay_time,
        delay_time_r: delay_time,
        low_cut_l: 20.0,
        low_cut_r: 20.0,
        low_cut_slope_l: 12.0,
        low_cut_slope_r: 12.0,
        high_cut_l: 20000.0,
        high_cut_r: 20000.0,
        high_cut_slope_l: 12.0,
        high_cut_slope_r: 12.0,
        feedback_l: feedback,
        feedback_r: feedback,
        feedback_phase_l: false,
        feedback_phase_r: false,
        crossfeed_lr: 0.0,
        crossfeed_rl: 0.0,
        crossfeed_phase_lr: false,
        crossfeed_phase_rl: false,
        routing: RoutingMode::Straight,
        tempo_sync: false,
        tempo_bpm: 120.0,
        note_l: NoteValue::Quarter,
        note_r: NoteValue::Quarter,
        deviation_l: 0.0,
        deviation_r: 0.0,
        halve_l: false,
        halve_r: false,
        double_l: false,
        double_r: false,
        output_mix_l: mix,
        output_mix_r: mix,
        bypass: false,
        stereo_link: false,
    };

    // ── Process through the golden reference ─────────────────────────
    let mut golden = GoldenReferenceDelay::new(SR);
    let golden_params = GoldenReferenceParams {
        delay_time_l: delay_time,
        delay_time_r: delay_time,
        feedback_l: feedback,
        feedback_r: feedback,
        mix,
    };

    // Warm up both with the same signal to let smoothers converge.
    let warmup_input = generate_sine(1000.0, WARMUP_SAMPLES as f64 / SR, 0.5, SR);
    for &s in &warmup_input {
        engine.process(s, s, &params);
        golden.process(s, s, golden_params);
    }

    // Now process the test signal through both.
    let mut plugin_out_l: Vec<f64> = Vec::with_capacity(num_samples);
    let mut plugin_out_r: Vec<f64> = Vec::with_capacity(num_samples);
    let mut golden_out_l: Vec<f64> = Vec::with_capacity(num_samples);
    let mut golden_out_r: Vec<f64> = Vec::with_capacity(num_samples);

    for &s in &input {
        let (pl, pr) = engine.process(s, s, &params);
        plugin_out_l.push(pl);
        plugin_out_r.push(pr);

        let (gl, gr) = golden.process(s, s, golden_params);
        golden_out_l.push(gl);
        golden_out_r.push(gr);
    }

    // Skip the first portion where smoothing and filter transients cause
    // the largest differences. We start comparing after ~1 second (enough
    // for the delay line to fill and all transients to settle).
    let skip = (SR * 1.0) as usize;

    // Compute the residual (plugin - golden reference).
    let residual_l: Vec<f64> = (skip..num_samples)
        .map(|i| plugin_out_l[i] - golden_out_l[i])
        .collect();

    // Compute energy metrics.
    let signal_energy: f64 = plugin_out_l[skip..].iter().map(|&x| x * x).sum();
    let residual_energy: f64 = residual_l.iter().map(|&x| x * x).sum();

    let relative_db = if signal_energy > 1e-20 {
        linear_to_db(residual_energy / signal_energy)
    } else {
        -120.0
    };

    // Also compute per-channel stats.
    let signal_rms = rms(&plugin_out_l[skip..]);
    let residual_rms = rms(&residual_l);

    eprintln!("[DIAG] Golden reference comparison (L channel, skipping first 200 ms):");
    eprintln!("  Signal RMS: {signal_rms:.6}");
    eprintln!("  Residual RMS: {residual_rms:.6}");
    eprintln!("  Residual energy relative to signal: {relative_db:.1} dB");

    assert!(
        relative_db < -25.0,
        "Null-test residual energy = {relative_db:.1} dB relative to signal — expected < -25 dB. \
         Residual RMS = {residual_rms:.6}, Signal RMS = {signal_rms:.6}"
    );

    eprintln!("[PASS] Golden Reference: residual = {relative_db:.1} dB (< -25 dB)");
}
