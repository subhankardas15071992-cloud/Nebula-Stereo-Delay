//! Main plugin entry point for **Nebula Stereo Delay** by Nebula Audio.
//!
//! This crate implements a professional stereo delay audio plugin built on
//! [nih-plug](https://github.com/robbert-vdh/nih-plug). It exports CLAP and
//! VST3 plugin formats on macOS/Linux, and VST3 only on Windows.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────┐
//! │                      NebulaStereoDelay                          │
//! │                                                                  │
//! │  params: Arc<NebulaStereoDelayParams>  ◄── shared with GUI      │
//! │  engine: DelayEngine                   ◄── f64 DSP core         │
//! │  state_manager: StateManager           ◄── meters + spectrum    │
//! │  midi_runtime: MidiRuntime             ◄── lock-free CC routing │
//! │  preset_manager: PresetManager         ◄── factory + user banks │
//! │  sample_rate: f64                      ◄── current host SR      │
//! │                                                                  │
//! │  process() flow per sample:                                      │
//! │    host f32 → f64 → DelayEngine → f64 → denormal flush → f32   │
//! └──────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Signal Flow (per sample)
//!
//! 1. Read smoothed parameter values from [`NebulaStereoDelayParams`].
//! 2. Build a [`dsp::DelayParams`] struct (f32 → f64 conversion).
//! 3. Pass input samples through [`DelayEngine::process()`] in f64.
//! 4. Flush denormal numbers to zero (prevents CPU spikes).
//! 5. Convert output back to f32 for the host buffer.
//! 6. Update lock-free peak meters via [`StateManager`].
//!
//! # Plugin Formats
//!
//! | Format   | Export Macro                     | Platform             |
//! |----------|----------------------------------|----------------------|
//! | CLAP     | `nih_export_clap!`               | Linux, macOS         |
//! | VST3     | `nih_export_vst3!`               | Linux, macOS, Windows|

/// DSP engine — always available (no plugin dependency).
pub mod dsp;

#[cfg(all(feature = "plugin", feature = "gui", not(target_os = "windows")))]
pub mod gui;
#[cfg(feature = "plugin")]
pub mod midi;
#[cfg(feature = "plugin")]
pub mod parameters;
#[cfg(feature = "plugin")]
pub mod preset;
#[cfg(feature = "plugin")]
pub mod state;
#[cfg(feature = "plugin")]
mod storage;
#[cfg(all(feature = "plugin", feature = "gui", target_os = "windows"))]
mod windows_editor;

// ──────────────────────────────────────────────────────────────────────────
// Everything below requires the "plugin" feature (nih_plug, GUI, etc.)
// ──────────────────────────────────────────────────────────────────────────

#[cfg(feature = "plugin")]
use std::sync::atomic::Ordering;
#[cfg(feature = "plugin")]
use std::sync::Arc;

#[cfg(feature = "plugin")]
use nih_plug::prelude::*;

#[cfg(feature = "plugin")]
use crate::dsp::DelayEngine;
#[cfg(feature = "plugin")]
use crate::midi::{sync_runtime_from_learn_state, MidiRuntime, MidiTarget};
#[cfg(feature = "plugin")]
use crate::parameters::NebulaStereoDelayParams;
#[cfg(feature = "plugin")]
use crate::preset::PresetManager;
#[cfg(feature = "plugin")]
use crate::state::{MeterValues, StateManager};

#[cfg(feature = "plugin")]
const DENORMAL_THRESHOLD_F64: f64 = 1e-30;
#[cfg(feature = "plugin")]
const DENORMAL_THRESHOLD_F32: f32 = 1e-30;
#[cfg(feature = "plugin")]
const DEFAULT_TEMPO_BPM: f64 = 120.0;
#[cfg(feature = "plugin")]
const MAX_OVERSAMPLING_FACTOR: usize = 8;

#[cfg(feature = "plugin")]
#[inline(always)]
fn flush_denormal_f64(x: f64) -> f64 {
    if x.abs() < DENORMAL_THRESHOLD_F64 {
        0.0
    } else {
        x
    }
}

#[cfg(feature = "plugin")]
#[inline(always)]
fn flush_denormal_f32(x: f32) -> f32 {
    if x.abs() < DENORMAL_THRESHOLD_F32 {
        0.0
    } else {
        x
    }
}

#[cfg(feature = "plugin")]
pub struct NebulaStereoDelay {
    params: Arc<NebulaStereoDelayParams>,
    engine: DelayEngine,
    state_manager: StateManager,
    meters: Arc<MeterValues>,
    midi_runtime: Arc<MidiRuntime>,
    _preset_manager: PresetManager,
    sample_rate: f64,
    oversampling_factor: usize,
    prev_input_l: f64,
    prev_input_r: f64,
}

#[cfg(feature = "plugin")]
impl Default for NebulaStereoDelay {
    fn default() -> Self {
        let params = Arc::new(NebulaStereoDelayParams::default());
        let meters = Arc::new(MeterValues::new());
        let sample_rate = 44_100.0;
        let mut engine = DelayEngine::new(sample_rate * MAX_OVERSAMPLING_FACTOR as f64);
        engine.set_sample_rate(sample_rate);

        Self {
            state_manager: StateManager::with_meters(params.clone(), meters.clone()),
            engine,
            params,
            meters,
            midi_runtime: Arc::new(MidiRuntime::new()),
            _preset_manager: PresetManager::new(),
            sample_rate,
            oversampling_factor: 1,
            prev_input_l: 0.0,
            prev_input_r: 0.0,
        }
    }
}

#[cfg(feature = "plugin")]
impl Plugin for NebulaStereoDelay {
    const NAME: &'static str = "Nebula Stereo Delay";
    const VENDOR: &'static str = "Nebula Audio";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = "1.2.0";

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: Some(new_nonzero_u32(2)),
        main_output_channels: Some(new_nonzero_u32(2)),
        ..AudioIOLayout::const_default()
    }];
    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        #[cfg(all(feature = "gui", not(target_os = "windows")))]
        {
            gui::create_egui_editor(
                self.params.clone(),
                self.midi_runtime.clone(),
                self.meters.clone(),
            )
        }

        #[cfg(all(feature = "gui", target_os = "windows"))]
        {
            windows_editor::create_editor(
                self.params.clone(),
                self.midi_runtime.clone(),
                self.meters.clone(),
            )
        }

        #[cfg(not(feature = "gui"))]
        {
            None
        }
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate as f64;
        self.oversampling_factor = self.params.oversampling.value().factor();
        self.engine = DelayEngine::new(self.sample_rate * MAX_OVERSAMPLING_FACTOR as f64);
        self.engine
            .set_sample_rate(self.sample_rate * self.oversampling_factor as f64);
        self.engine.reset();
        self.prev_input_l = 0.0;
        self.prev_input_r = 0.0;
        self.state_manager = StateManager::with_meters(self.params.clone(), self.meters.clone());
        if let Ok(learn) = self.params.midi_learn.read() {
            sync_runtime_from_learn_state(&self.midi_runtime, &learn);
        }
        true
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let transport = context.transport();
        let tempo_bpm = transport.tempo.unwrap_or(DEFAULT_TEMPO_BPM);

        while let Some(event) = context.next_event() {
            if let NoteEvent::MidiCC {
                channel, cc, value, ..
            } = event
            {
                self.midi_runtime.process_cc(channel, cc, value);
            }
        }

        let midi_bypass = self
            .midi_runtime
            .target_value(MidiTarget::Bypass)
            .map(|v| v >= 0.5);
        let hard_bypass = midi_bypass.unwrap_or_else(|| self.params.bypass.load(Ordering::Relaxed));
        self.engine.set_bypass(hard_bypass);

        let input_mode_l = self
            .midi_runtime
            .target_value(MidiTarget::InputModeL)
            .map(|v| self.params.input_mode_l.preview_plain(v).into())
            .unwrap_or_else(|| self.params.input_mode_l.value().into());
        let input_mode_r = self
            .midi_runtime
            .target_value(MidiTarget::InputModeR)
            .map(|v| self.params.input_mode_r.preview_plain(v).into())
            .unwrap_or_else(|| self.params.input_mode_r.value().into());
        let feedback_phase_l = self
            .midi_runtime
            .target_value(MidiTarget::FeedbackPhaseL)
            .map(|v| self.params.feedback_phase_l.preview_plain(v))
            .unwrap_or_else(|| self.params.feedback_phase_l.value());
        let feedback_phase_r = self
            .midi_runtime
            .target_value(MidiTarget::FeedbackPhaseR)
            .map(|v| self.params.feedback_phase_r.preview_plain(v))
            .unwrap_or_else(|| self.params.feedback_phase_r.value());
        let crossfeed_phase_lr = self
            .midi_runtime
            .target_value(MidiTarget::CrossfeedPhaseLr)
            .map(|v| self.params.crossfeed_phase_lr.preview_plain(v))
            .unwrap_or_else(|| self.params.crossfeed_phase_lr.value());
        let crossfeed_phase_rl = self
            .midi_runtime
            .target_value(MidiTarget::CrossfeedPhaseRl)
            .map(|v| self.params.crossfeed_phase_rl.preview_plain(v))
            .unwrap_or_else(|| self.params.crossfeed_phase_rl.value());
        let routing = self
            .midi_runtime
            .target_value(MidiTarget::Routing)
            .map(|v| self.params.routing.preview_plain(v).into())
            .unwrap_or_else(|| self.params.routing.value().into());
        let tempo_sync = self
            .midi_runtime
            .target_value(MidiTarget::TempoSync)
            .map(|v| self.params.tempo_sync.preview_plain(v))
            .unwrap_or_else(|| self.params.tempo_sync.value());
        let note_l = self
            .midi_runtime
            .target_value(MidiTarget::NoteL)
            .map(|v| self.params.note_l.preview_plain(v).into())
            .unwrap_or_else(|| self.params.note_l.value().into());
        let note_r = self
            .midi_runtime
            .target_value(MidiTarget::NoteR)
            .map(|v| self.params.note_r.preview_plain(v).into())
            .unwrap_or_else(|| self.params.note_r.value().into());
        let halve_l = self
            .midi_runtime
            .target_value(MidiTarget::HalveL)
            .map(|v| self.params.halve_l.preview_plain(v))
            .unwrap_or_else(|| self.params.halve_l.value());
        let halve_r = self
            .midi_runtime
            .target_value(MidiTarget::HalveR)
            .map(|v| self.params.halve_r.preview_plain(v))
            .unwrap_or_else(|| self.params.halve_r.value());
        let double_l = self
            .midi_runtime
            .target_value(MidiTarget::DoubleL)
            .map(|v| self.params.double_l.preview_plain(v))
            .unwrap_or_else(|| self.params.double_l.value());
        let double_r = self
            .midi_runtime
            .target_value(MidiTarget::DoubleR)
            .map(|v| self.params.double_r.preview_plain(v))
            .unwrap_or_else(|| self.params.double_r.value());
        let stereo_link = self
            .midi_runtime
            .target_value(MidiTarget::StereoLink)
            .map(|v| self.params.stereo_link.preview_plain(v))
            .unwrap_or_else(|| self.params.stereo_link.value());
        let oversampling_factor = self
            .midi_runtime
            .target_value(MidiTarget::Oversampling)
            .map(|v| self.params.oversampling.preview_plain(v).factor())
            .unwrap_or_else(|| self.params.oversampling.value().factor());
        if oversampling_factor != self.oversampling_factor {
            self.oversampling_factor = oversampling_factor;
            self.engine
                .set_sample_rate(self.sample_rate * self.oversampling_factor as f64);
            self.engine.reset();
            self.prev_input_l = 0.0;
            self.prev_input_r = 0.0;
        }

        for mut sample in buffer.iter_samples() {
            let mut channels = sample.iter_mut();
            let Some(left) = channels.next() else {
                continue;
            };
            let Some(right) = channels.next() else {
                continue;
            };

            macro_rules! midi_float {
                ($target:expr, $param:ident) => {
                    self.midi_runtime
                        .target_value($target)
                        .map(|v| self.params.$param.preview_plain(v) as f64)
                        .unwrap_or_else(|| self.params.$param.smoothed.next() as f64)
                };
            }

            let delay_params = dsp::DelayParams {
                input_level_db: midi_float!(MidiTarget::InputLevel, input_level),
                output_level_db: midi_float!(MidiTarget::OutputLevel, output_level),
                input_mode_l,
                input_mode_r,
                delay_time_l: midi_float!(MidiTarget::DelayTimeL, delay_time_l),
                delay_time_r: midi_float!(MidiTarget::DelayTimeR, delay_time_r),
                low_cut_l: midi_float!(MidiTarget::LowCutL, low_cut_l),
                low_cut_r: midi_float!(MidiTarget::LowCutR, low_cut_r),
                low_cut_slope_l: midi_float!(MidiTarget::LowCutSlopeL, low_cut_slope_l),
                low_cut_slope_r: midi_float!(MidiTarget::LowCutSlopeR, low_cut_slope_r),
                high_cut_l: midi_float!(MidiTarget::HighCutL, high_cut_l),
                high_cut_r: midi_float!(MidiTarget::HighCutR, high_cut_r),
                high_cut_slope_l: midi_float!(MidiTarget::HighCutSlopeL, high_cut_slope_l),
                high_cut_slope_r: midi_float!(MidiTarget::HighCutSlopeR, high_cut_slope_r),
                feedback_l: midi_float!(MidiTarget::FeedbackL, feedback_l),
                feedback_r: midi_float!(MidiTarget::FeedbackR, feedback_r),
                feedback_phase_l,
                feedback_phase_r,
                crossfeed_lr: midi_float!(MidiTarget::CrossfeedLr, crossfeed_lr),
                crossfeed_rl: midi_float!(MidiTarget::CrossfeedRl, crossfeed_rl),
                crossfeed_phase_lr,
                crossfeed_phase_rl,
                routing,
                tempo_sync,
                tempo_bpm,
                note_l,
                note_r,
                deviation_l: midi_float!(MidiTarget::DeviationL, deviation_l),
                deviation_r: midi_float!(MidiTarget::DeviationR, deviation_r),
                halve_l,
                halve_r,
                double_l,
                double_r,
                wet_level_l: midi_float!(MidiTarget::WetLevelL, wet_level_l),
                wet_level_r: midi_float!(MidiTarget::WetLevelR, wet_level_r),
                dry_level_l: midi_float!(MidiTarget::DryLevelL, dry_level_l),
                dry_level_r: midi_float!(MidiTarget::DryLevelR, dry_level_r),
                wet_pan_l: midi_float!(MidiTarget::WetPanL, wet_pan_l),
                wet_pan_r: midi_float!(MidiTarget::WetPanR, wet_pan_r),
                dry_pan_l: midi_float!(MidiTarget::DryPanL, dry_pan_l),
                dry_pan_r: midi_float!(MidiTarget::DryPanR, dry_pan_r),
                output_mix_l: midi_float!(MidiTarget::OutputMixL, output_mix_l),
                output_mix_r: midi_float!(MidiTarget::OutputMixR, output_mix_r),
                bypass: hard_bypass,
                stereo_link,
            };

            let in_l = *left as f64;
            let in_r = *right as f64;
            let mut input_meter_l = 0.0f32;
            let mut input_meter_r = 0.0f32;
            let mut output_meter_l = 0.0f32;
            let mut output_meter_r = 0.0f32;

            let (out_l, out_r) = if self.oversampling_factor == 1 {
                let frame = self.engine.process_frame(in_l, in_r, &delay_params);
                input_meter_l = input_meter_l.max(frame.input_meter_l.abs() as f32);
                input_meter_r = input_meter_r.max(frame.input_meter_r.abs() as f32);
                output_meter_l = output_meter_l.max(frame.output_meter_l.abs() as f32);
                output_meter_r = output_meter_r.max(frame.output_meter_r.abs() as f32);
                (frame.output_l, frame.output_r)
            } else {
                let factor = self.oversampling_factor;
                let factor_f = factor as f64;
                let mut acc_l = 0.0;
                let mut acc_r = 0.0;
                for sub in 0..factor {
                    let t = (sub as f64 + 1.0) / factor_f;
                    let os_in_l = self.prev_input_l + (in_l - self.prev_input_l) * t;
                    let os_in_r = self.prev_input_r + (in_r - self.prev_input_r) * t;
                    let frame = self.engine.process_frame(os_in_l, os_in_r, &delay_params);
                    input_meter_l = input_meter_l.max(frame.input_meter_l.abs() as f32);
                    input_meter_r = input_meter_r.max(frame.input_meter_r.abs() as f32);
                    output_meter_l = output_meter_l.max(frame.output_meter_l.abs() as f32);
                    output_meter_r = output_meter_r.max(frame.output_meter_r.abs() as f32);
                    acc_l += frame.output_l;
                    acc_r += frame.output_r;
                }
                (acc_l / factor_f, acc_r / factor_f)
            };
            self.prev_input_l = in_l;
            self.prev_input_r = in_r;

            let out_l = flush_denormal_f64(out_l);
            let out_r = flush_denormal_f64(out_r);

            let out_l_f32 = flush_denormal_f32(out_l as f32);
            let out_r_f32 = flush_denormal_f32(out_r as f32);

            *left = out_l_f32;
            *right = out_r_f32;

            self.state_manager.update_meters(
                input_meter_l,
                input_meter_r,
                output_meter_l.max(out_l_f32.abs()),
                output_meter_r.max(out_r_f32.abs()),
                0.0,
                0.0,
            );
        }

        ProcessStatus::Normal
    }
}

#[cfg(all(feature = "plugin", not(target_os = "windows")))]
impl ClapPlugin for NebulaStereoDelay {
    const CLAP_ID: &'static str = "audio.nebula.stereo-delay";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Professional stereo delay audio effect");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Delay,
        ClapFeature::Stereo,
    ];
}

#[cfg(feature = "plugin")]
impl Vst3Plugin for NebulaStereoDelay {
    const VST3_CLASS_ID: [u8; 16] = *b"NebulaStereoDly!";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[
        Vst3SubCategory::Fx,
        Vst3SubCategory::Delay,
        Vst3SubCategory::Stereo,
    ];
}

#[cfg(all(feature = "plugin", not(target_os = "windows")))]
nih_plug::nih_export_clap!(NebulaStereoDelay);

#[cfg(feature = "plugin")]
nih_plug::nih_export_vst3!(NebulaStereoDelay);
