<div align="center">

# NEBULA STEREO DELAY

**v1.0**

*A stereo delay engine forged in double-precision Rust*

[![License: AGPL v3](https://img.shields.io/badge/license-AGPL%20v3-blue.svg)](LICENSE)
[![Platform: macOS](https://img.shields.io/badge/platform-macOS-lightgrey.svg)](#build)
[![Platform: Windows](https://img.shields.io/badge/platform-Windows-blue.svg)](#build)
[![Platform: Linux](https://img.shields.io/badge/platform-Linux-orange.svg)](#build)
[![Format: CLAP](https://img.shields.io/badge/format-CLAP-purple.svg)](#plugin-formats)
[![Format: VST3](https://img.shields.io/badge/format-VST3-green.svg)](#plugin-formats)
[![Format: AUv2](https://img.shields.io/badge/format-AUv2-cyan.svg)](#plugin-formats)

</div>

---

## What if your delay was as precise as your vision?

Most delay plugins process audio in 32-bit floating point. That's fine — until you stack feedback, filters, and crossfeed into a regenerative loop. Each pass through the feedback network erodes a little more resolution, a little more air, a little more *life* from your sound. The repeats don't decay — they *decompose*.

**Nebula Stereo Delay runs its entire DSP engine in 64-bit double precision.** Every delay line read, every biquad filter, every crossfeed blend, every feedback calculation — all `f64`, from input to output. The only place `f32` exists is the final handoff to your DAW. The result? Repeats that decay *gracefully*, not *digitally*. Tail after tail of crystalline echo that keeps its shape through hundreds of feedback cycles.

This isn't a gimmick. It's engineering.

---

## Highlights

### Double-Precision DSP Engine
Every sample is processed as `f64` from input to output. Cubic Hermite interpolation on the delay lines, Direct Form I biquad filters, smooth parameter ramps — all in double precision. Denormals are flushed to zero on every output sample. The audio thread never allocates, never blocks, never panics.

### 8 Routing Modes
One knob to reshape your stereo field:

| Mode | Character |
|------|-----------|
| **Customized** | Full manual control — set crossfeed amounts yourself |
| **Straight** | L stays L, R stays R — pure independent channels |
| **Crossfeed** | Full symmetric blend — each channel feeds the other |
| **90/10** | Mostly self-feedback with a hint of cross — subtle widening |
| **10/90** | Mostly crossfeed — almost ping-pong but not quite |
| **Ping Pong** | Signal bounces L/R on every repeat — classic stereo bounce |
| **Pan L/R** | Normal feedback but swapped outputs — instant width |
| **Rotate L/R** | LFO-modulated rotary speaker simulation with equal-power panning |

### Tempo Sync with Deviation
Lock your delay to the host tempo at any note value from 1/1 down to 1/64, including all triplet subdivisions. Then dial in up to +/-100 cents of deviation for that slightly-off-grid feel that makes repeats breathe. The `:2` and `x2` buttons give you instant half-time and double-time without touching the main knob.

### Per-Channel Filters
Each delay line has its own low-cut (high-pass) and high-cut (low-pass) filter in the feedback path. Shave off rumble. Roll off harshness. Shape the character of each repeat as it regenerates. Butterworth 12 dB/oct biquads, coefficient-updated per sample from smoothed cutoffs.

### Feedback Phase Inversion
Flip the phase of each channel's feedback independently. Inverted feedback creates comb-filter effects, resonant spikes, and metallic textures that normal feedback can't produce. Combine with crossfeed phase inversion for phase-cancellation tricks across the stereo field.

### Lock-Free Real-Time Architecture
No mutexes on the audio thread. Peak meters use `AtomicU32` with `Relaxed` ordering — a single instruction on x86 and AArch64. Spectrum data uses `try_lock()` — the audio thread *never blocks*, it simply skips a frame if the GUI is reading. MIDI CC values use `AtomicU32` + `AtomicBool` with Release-Acquire semantics for guaranteed visibility without fences.

### A/B Comparison + 50-Level Undo/Redo
Save two complete parameter snapshots and toggle between them with one click. Every parameter change is tracked in an undo stack 50 levels deep. Go back. Go forward. Experiment fearlessly.

### MIDI Learn
Right-click any control to assign a MIDI CC. Learn mode listens for the next CC message and maps it instantly. Toggle MIDI on/off per parameter without losing the mapping. Roll back to a saved mapping state if you change your mind.

### Soft Bypass
The FX bypass crossfades over 512 samples. No clicks. No pops. No hard cuts.

### Freely Scalable GUI
Built with egui on macOS and Linux. The window starts at 1200x700 and scales freely — every element resizes proportionally. DPI-aware on high-resolution displays. Dark professional theme with a cyan accent palette matching the Nebula Audio family.

---

## Plugin Formats

| Platform | CLAP | VST3 | AUv2 |
|----------|:----:|:----:|:----:|
| macOS (Universal) | Yes | Yes | Yes (via clap-wrapper) |
| Windows (x86_64) | — | Yes | — |
| Linux (x86_64) | Yes | Yes | — |

AUv2 on macOS is generated automatically at build time using the [free-audio/clap-wrapper](https://github.com/free-audio/clap-wrapper) project — no separate build step required.

---

## 10 Factory Presets

| Preset | Vibe |
|--------|------|
| **Init** | Clean slate — all defaults |
| **Simple Slap** | 60/80 ms slapback, zero feedback |
| **Ambient Wash** | Long regenerative tail with low-cut, crossfeed, and tempo sync |
| **Ping Pong** | Classic L/R bounce with eighth-note sync |
| **Rotary** | Rotate L/R routing with subtle detune |
| **Tight Doubler** | 12/24 ms micro-delay for double-tracking width |
| **Space Echo** | Tape echo emulation — band-limited, wow/flutter deviation |
| **Stereo Widener** | Inverted crossfeed phase for out-of-phase widening |
| **Rhythmic Delay** | Triplet polyrhythms with 90/10 routing |
| **Vintage Tape** | Dark, lo-fi tape character — heavy filtering, high deviation |

Save, load, import, and export your own presets as JSON. Platform-specific user preset directories keep everything organized.

---

## Signal Flow

```
Input ──► Input Mode Selection ──► ─────────────────────────────────► Dry/Wet Mix ──► Soft Bypass ──► Output
                                      │                                  ▲
                                      ▼                                  │
                                 Delay Line (cubic interp) ──► Filtered Signal (wet)
                                      ▲                          │
                                      │                          ▼
                                      └──────── Feedback ◄─── Biquad Filters
                                                ▲
                                                │
                                           Routing Matrix
                                           (crossfeed + phase)
```

---

## Parameters at a Glance

### Per-Channel (L / R)

| Parameter | Range | Default |
|-----------|-------|---------|
| Input Mode | Off / Left / Right / L+R / L-R | Left (L) / Right (R) |
| Delay Time | 0.01 – 10.0 s | 0.5 s |
| Note (sync) | 1/1 – 1/64 (incl. triplets) | 1/4 |
| Deviation | -100 – +100 ct | 0 ct |
| :2 / x2 | On / Off | Off |
| Low Cut | 20 – 20 000 Hz | 20 Hz |
| High Cut | 20 – 20 000 Hz | 20 000 Hz |
| Feedback | 0 – 100 % | 40 % |
| Feedback Phase | Normal / Inverted | Normal |

### Crossfeed

| Parameter | Range | Default |
|-----------|-------|---------|
| L/R → R/L | 0 – 100 % | 0 % |
| Crossfeed Phase | Normal / Inverted | Normal |

### Global / Output

| Parameter | Range | Default |
|-----------|-------|---------|
| Routing | 8 modes | Customized |
| Tempo Sync | Free / Sync | Free |
| Stereo Link | Unlinked / Linked | Unlinked |
| Output Mix L | 0 – 100 % | 100 % |
| Output Mix R | 0 – 100 % | 100 % |

---

## Build

### Prerequisites

- **Rust** 1.77.0+ (`rustup`)
- **macOS**: Xcode Command Line Tools only (`xcode-select --install`)
- **Linux**: `libxcb-shape0-dev`, `libxcb-xfixes0-dev`, `libx11-dev`, `libgl-dev`, `libasound2-dev`
- **Windows**: Visual Studio Build Tools with C++ workload

### Quick Build

```bash
# Clone
git clone https://github.com/<you>/nebula-stereo-delay.git
cd nebula-stereo-delay

# macOS (Universal: arm64 + x86_64, CLAP + VST3 + AUv2)
./scripts/build_macos.sh

# Linux (x86_64, CLAP + VST3)
./scripts/build_linux.sh

# Windows (x86_64, VST3 only)
.\scripts\build_windows.ps1
```

Built plugins appear in `build/<platform>/`.

### Manual Build

```bash
cargo build --release
```

Use `cargo xtask bundle nebula-stereo-delay --release` to create properly structured plugin bundles.

---

## Testing

Three comprehensive test suites validate the plugin at every level:

```bash
# Technical: DSP math, filter stability, delay line integrity
cargo test --test technical_tests

# Perceptual: psychoacoustic thresholds, stereo imaging, temporal accuracy
cargo test --test perceptual_tests

# Audio Evaluation: FFT harmonic analysis, LUFS loudness, golden-reference comparison
cargo test --test audio_evaluation_tests
```

---

## Architecture

```
src/
├── lib.rs           — Plugin entry point, process loop, denormal flush
├── dsp/mod.rs       — f64 DSP engine (delay lines, biquads, routing, LFO)
├── parameters/mod.rs — All automatable params, A/B snapshots, undo/redo
├── gui/mod.rs       — DPI-scalable egui editor with dark theme
├── state/mod.rs     — Lock-free meters, spectrum analyser, FFT
├── midi/mod.rs      — Lock-free MIDI CC store, learn/rollback state
└── preset/mod.rs    — Factory presets, user save/load, JSON import/export
```

---

## CI/CD

GitHub Actions builds on every push and PR:

1. **Lint** — `cargo fmt --check` + `cargo clippy -D warnings` on Ubuntu
2. **macOS** — Universal binary (arm64 + x86_64), CLAP + VST3 + AUv2
3. **Windows** — x86_64 VST3
4. **Linux** — x86_64 CLAP + VST3
5. **Release** — Tag a `v*` release and all artifacts publish automatically

---

## License

Nebula Stereo Delay is open-source software licensed under the **GNU Affero General Public License v3**. See [LICENSE](LICENSE) for details.

---

<div align="center">

*Nebula Audio — where precision meets atmosphere.*

</div>
