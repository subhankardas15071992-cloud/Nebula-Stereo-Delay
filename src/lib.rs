//! Main plugin entry point for **Nebula Stereo Delay** by Nebula Audio.
//!
//! This crate implements a professional stereo delay audio plugin built on
//! [nih-plug](https://github.com/robbert-vdh/nih-plug). It exports CLAP and
//! VST3 plugin formats.
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
//! │  midi_cc: MidiCcValues                 ◄── lock-free CC store   │
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
//! | CLAP     | `nih_export_clap!`               | Linux, macOS, Windows|
//! | VST3     | `nih_export_vst3!`               | Linux, macOS, Windows|

/// DSP engine — always available (no plugin dependency).
pub mod dsp;

#[cfg(feature = "plugin")]
pub mod gui;
#[cfg(feature = "plugin")]
pub mod midi;
#[cfg(feature = "plugin")]
pub mod parameters;
#[cfg(feature = "plugin")]
pub mod preset;
#[cfg(feature = "plugin")]
pub mod state;

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
use crate::midi::MidiCcValues;
#[cfg(feature = "plugin")]
use crate::parameters::NebulaStereoDelayParams;
#[cfg(feature = "plugin")]
use crate::preset::PresetManager;
#[cfg(feature = "plugin")]
use crate::state::StateManager;

#[cfg(feature = "plugin")]
const DENORMAL_THRESHOLD_F64: f64 = 1e-30;
#[cfg(feature = "plugin")]
const DENORMAL_THRESHOLD_F32: f32 = 1e-30;
#[cfg(feature = "plugin")]
const DEFAULT_TEMPO_BPM: f64 = 120.0;

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
    midi_cc: MidiCcValues,
    _preset_manager: PresetManager,
    sample_rate: f64,
}

#[cfg(feature = "plugin")]
impl Default for NebulaStereoDelay {
    fn default() -> Self {
        let params = Arc::new(NebulaStereoDelayParams::default());
        let sample_rate = 44_100.0;

        Self {
            state_manager: StateManager::new(params.clone()),
            engine: DelayEngine::new(sample_rate),
            params,
            midi_cc: MidiCcValues::new(),
            _preset_manager: PresetManager::new(),
            sample_rate,
        }
    }
}

#[cfg(feature = "plugin")]
impl Plugin for NebulaStereoDelay {
    const NAME: &'static str = "Nebula Stereo Delay";
    const VENDOR: &'static str = "Nebula Audio";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = "1.0.0";

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
        #[cfg(target_os = "windows")]
        {
            None
        }

        #[cfg(not(target_os = "windows"))]
        gui::create_egui_editor(self.params.clone())
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate as f64;
        self.engine = DelayEngine::new(self.sample_rate);
        self.engine.reset();
        self.state_manager = StateManager::new(self.params.clone());
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

        let soft_bypass = self.params.bypass.load(Ordering::Relaxed);
        self.engine.set_bypass(soft_bypass);

        while let Some(event) = context.next_event() {
            if let NoteEvent::MidiCC {
                channel, cc, value, ..
            } = event
            {
                self.midi_cc.set(channel, cc, value);
            }
        }

        let input_mode_l = self.params.input_mode_l.value().into();
        let input_mode_r = self.params.input_mode_r.value().into();
        let feedback_phase_l = self.params.feedback_phase_l.value();
        let feedback_phase_r = self.params.feedback_phase_r.value();
        let crossfeed_phase = self.params.crossfeed_phase.value();
        let routing = self.params.routing.value().into();
        let tempo_sync = self.params.tempo_sync.value();
        let note_l = self.params.note_l.value().into();
        let note_r = self.params.note_r.value().into();
        let halve_l = self.params.halve_l.value();
        let halve_r = self.params.halve_r.value();
        let double_l = self.params.double_l.value();
        let double_r = self.params.double_r.value();
        let stereo_link = self.params.stereo_link.value();

        for mut sample in buffer.iter_samples() {
            let mut channels = sample.iter_mut();
            let Some(left) = channels.next() else {
                continue;
            };
            let Some(right) = channels.next() else {
                continue;
            };

            let delay_params = dsp::DelayParams {
                input_mode_l,
                input_mode_r,
                delay_time_l: self.params.delay_time_l.smoothed.next() as f64,
                delay_time_r: self.params.delay_time_r.smoothed.next() as f64,
                low_cut_l: self.params.low_cut_l.smoothed.next() as f64,
                low_cut_r: self.params.low_cut_r.smoothed.next() as f64,
                high_cut_l: self.params.high_cut_l.smoothed.next() as f64,
                high_cut_r: self.params.high_cut_r.smoothed.next() as f64,
                feedback_l: self.params.feedback_l.smoothed.next() as f64,
                feedback_r: self.params.feedback_r.smoothed.next() as f64,
                feedback_phase_l,
                feedback_phase_r,
                crossfeed_lr: self.params.crossfeed_lr.smoothed.next() as f64,
                crossfeed_rl: self.params.crossfeed_rl.smoothed.next() as f64,
                crossfeed_phase,
                routing,
                tempo_sync,
                tempo_bpm,
                note_l,
                note_r,
                deviation_l: self.params.deviation_l.smoothed.next() as f64,
                deviation_r: self.params.deviation_r.smoothed.next() as f64,
                halve_l,
                halve_r,
                double_l,
                double_r,
                output_mix_l: self.params.output_mix_l.smoothed.next() as f64,
                output_mix_r: self.params.output_mix_r.smoothed.next() as f64,
                bypass: soft_bypass,
                stereo_link,
            };

            let in_l = *left as f64;
            let in_r = *right as f64;

            let (out_l, out_r) = self.engine.process(in_l, in_r, &delay_params);

            let out_l = flush_denormal_f64(out_l);
            let out_r = flush_denormal_f64(out_r);

            let out_l_f32 = flush_denormal_f32(out_l as f32);
            let out_r_f32 = flush_denormal_f32(out_r as f32);

            *left = out_l_f32;
            *right = out_r_f32;

            self.state_manager
                .update_meters(out_l_f32, out_r_f32, 0.0, 0.0);
        }

        ProcessStatus::Normal
    }
}

#[cfg(feature = "plugin")]
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

#[cfg(feature = "plugin")]
nih_plug::nih_export_clap!(NebulaStereoDelay);

#[cfg(feature = "plugin")]
nih_plug::nih_export_vst3!(NebulaStereoDelay);
