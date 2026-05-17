#![allow(clippy::too_many_arguments)]

use std::any::Any;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Once};

use nih_plug::params::{BoolParam, FloatParam, Param};
use nih_plug::prelude::{Editor, GuiContext, ParamSetter, ParentWindowHandle};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_UNKNOWN, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_DEMI_BOLD, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_TRAILING,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, InvalidateRect, UpdateWindow, HBRUSH, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    GetDpiForSystem, GetDpiForWindow, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_BACK, VK_CONTROL, VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW, KillTimer,
    LoadCursorW, RegisterClassW, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    CREATESTRUCTW, CS_DBLCLKS, CS_HREDRAW, CS_VREDRAW, DLGC_WANTALLKEYS, DLGC_WANTCHARS,
    GWLP_USERDATA, HMENU, IDC_ARROW, SWP_NOACTIVATE, SWP_NOZORDER, SW_SHOW, WINDOW_EX_STYLE,
    WM_CHAR, WM_DPICHANGED, WM_DPICHANGED_AFTERPARENT, WM_DPICHANGED_BEFOREPARENT, WM_ERASEBKGND,
    WM_GETDLGCODE, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_RBUTTONDOWN, WM_SIZE, WM_TIMER, WNDCLASSW, WS_CHILD,
    WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_VISIBLE,
};
use windows_numerics::Vector2;

use crate::dsp::NoteValue;
use crate::midi::{sync_runtime_from_learn_state, MidiRuntime, MidiTarget, MIDI_TARGET_COUNT};
use crate::parameters::{
    InputModeParam, NebulaStereoDelayParams, NoteValueParam, OversamplingParam, ParamSnapshot,
    RoutingModeParam,
};
use crate::preset::{PresetManager, PresetValues};
use crate::state::MeterValues;

const BASE_W: f32 = 1000.0;
const BASE_H: f32 = 640.0;
const DEFAULT_DPI: u32 = 96;
const TIMER_ID: usize = 8801;
const TIMER_MS: u32 = 33;

const ARC_START: f32 = std::f32::consts::PI * 0.75;
const ARC_SWEEP: f32 = std::f32::consts::PI * 1.5;
const ROUTE_EPS: f32 = 0.006;

pub(super) fn create_editor(
    params: Arc<NebulaStereoDelayParams>,
    midi_runtime: Arc<MidiRuntime>,
    meters: Arc<MeterValues>,
) -> Option<Box<dyn Editor>> {
    if let Ok(learn) = params.midi_learn.read() {
        sync_runtime_from_learn_state(&midi_runtime, &learn);
    }

    Some(Box::new(NativeEditor {
        params,
        midi_runtime,
        meters,
        scale_bits: AtomicU32::new(1.0_f32.to_bits()),
        size_scale_bits: Arc::new(AtomicU32::new(1.0_f32.to_bits())),
    }))
}

struct NativeEditor {
    params: Arc<NebulaStereoDelayParams>,
    midi_runtime: Arc<MidiRuntime>,
    meters: Arc<MeterValues>,
    scale_bits: AtomicU32,
    size_scale_bits: Arc<AtomicU32>,
}

impl Editor for NativeEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn Any + Send> {
        let ParentWindowHandle::Win32Hwnd(parent_hwnd) = parent else {
            return Box::new(());
        };
        if parent_hwnd.is_null() || !register_window_class() {
            return Box::new(());
        }

        let parent_hwnd = HWND(parent_hwnd);
        let dpi_scope = DpiAwarenessScope::enter();
        let host_scale = f32::from_bits(self.scale_bits.load(Ordering::Acquire)).clamp(0.5, 3.0);
        let initial_dpi = dpi_for_window(parent_hwnd);
        let render_scale = host_scale.max(dpi_scale(initial_dpi)).clamp(0.5, 3.0);
        let size_scale = (render_scale / host_scale.max(0.5)).clamp(1.0, 3.0);
        self.size_scale_bits
            .store(size_scale.to_bits(), Ordering::Release);

        let desired_w = (BASE_W * render_scale).round() as i32;
        let desired_h = (BASE_H * render_scale).round() as i32;
        let (width, height) = client_size(parent_hwnd)
            .filter(|(w, h)| *w > 100 && *h > 100)
            .map(|(w, h)| ((w as i32).max(desired_w), (h as i32).max(desired_h)))
            .unwrap_or((desired_w, desired_h));

        let request_resize = size_scale > 1.01;
        let resize_context = context.clone();
        let state = Box::new(NativeWindowState::new(
            self.params.clone(),
            self.midi_runtime.clone(),
            self.meters.clone(),
            self.size_scale_bits.clone(),
            context,
            parent_hwnd,
            host_scale,
            initial_dpi,
        ));
        let state_ptr = Box::into_raw(state);

        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name(),
                w!("Nebula Stereo Delay"),
                WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0,
                0,
                width,
                height,
                Some(parent_hwnd),
                Option::<HMENU>::None,
                module_instance(),
                Some(state_ptr.cast::<c_void>()),
            )
        };
        drop(dpi_scope);

        match hwnd {
            Ok(hwnd) => unsafe {
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = UpdateWindow(hwnd);
                if request_resize {
                    let _ = resize_context.request_resize();
                }
                Box::new(NativeWindowHandle {
                    hwnd: hwnd.0 as isize,
                })
            },
            Err(_) => unsafe {
                drop(Box::from_raw(state_ptr));
                Box::new(())
            },
        }
    }

    fn size(&self) -> (u32, u32) {
        let size_scale =
            f32::from_bits(self.size_scale_bits.load(Ordering::Acquire)).clamp(1.0, 3.0);
        (
            (BASE_W * size_scale).round() as u32,
            (BASE_H * size_scale).round() as u32,
        )
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        self.scale_bits
            .store(factor.max(0.5).to_bits(), Ordering::Release);
        self.size_scale_bits
            .store(1.0_f32.to_bits(), Ordering::Release);
        true
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}

struct NativeWindowHandle {
    hwnd: isize,
}

unsafe impl Send for NativeWindowHandle {}

impl Drop for NativeWindowHandle {
    fn drop(&mut self) {
        if self.hwnd != 0 {
            let hwnd = HWND(self.hwnd as *mut c_void);
            let _ = unsafe { DestroyWindow(hwnd) };
            self.hwnd = 0;
        }
    }
}

struct NativeWindowState {
    hwnd: HWND,
    parent_hwnd: HWND,
    params: Arc<NebulaStereoDelayParams>,
    midi_runtime: Arc<MidiRuntime>,
    meters: Arc<MeterValues>,
    size_scale_bits: Arc<AtomicU32>,
    context: Arc<dyn GuiContext>,
    d2d_factory: Option<ID2D1Factory>,
    dwrite_factory: Option<IDWriteFactory>,
    render_target: Option<ID2D1HwndRenderTarget>,
    text_formats: Option<TextFormats>,
    drag: Option<DragState>,
    popup: Option<Popup>,
    numeric_input: Option<NumericInput>,
    text_input: Option<TextInput>,
    preset_manager: PresetManager,
    preset_name: String,
    status: String,
    midi_learn_waiting_for_control: bool,
    midi_learn_target: Option<String>,
    host_scale: f32,
    dpi: u32,
}

impl NativeWindowState {
    fn new(
        params: Arc<NebulaStereoDelayParams>,
        midi_runtime: Arc<MidiRuntime>,
        meters: Arc<MeterValues>,
        size_scale_bits: Arc<AtomicU32>,
        context: Arc<dyn GuiContext>,
        parent_hwnd: HWND,
        host_scale: f32,
        dpi: u32,
    ) -> Self {
        Self {
            hwnd: HWND::default(),
            parent_hwnd,
            params,
            midi_runtime,
            meters,
            size_scale_bits,
            context,
            d2d_factory: None,
            dwrite_factory: None,
            render_target: None,
            text_formats: None,
            drag: None,
            popup: None,
            numeric_input: None,
            text_input: None,
            preset_manager: PresetManager::new(),
            preset_name: "User Preset".to_string(),
            status: "Native Direct2D".to_string(),
            midi_learn_waiting_for_control: false,
            midi_learn_target: None,
            host_scale,
            dpi: dpi.max(DEFAULT_DPI),
        }
    }

    fn paint(&mut self) {
        self.refresh_dpi();
        let Some((w, h)) = self.logical_client_size() else {
            return;
        };
        let layout = Layout::new(w, h);
        let context = self.context.clone();
        let setter = ParamSetter::new(context.as_ref());
        self.drain_midi_runtime_to_gui(&setter);
        self.sync_routing_display_to_parameters(&setter);

        let Some(rt) = self.ensure_render_target() else {
            return;
        };
        let Some(formats) = self.ensure_text_formats(layout.s) else {
            return;
        };
        let Some(brushes) = Brushes::new(&rt) else {
            return;
        };

        unsafe {
            rt.BeginDraw();
            rt.Clear(Some(&Colors::BG));
        }

        self.draw_background(&rt, &brushes, &formats, &layout);
        self.draw_meters(&rt, &brushes, &formats, &layout);
        self.draw_channel(&rt, &brushes, &formats, &layout.left, Channel::Left);
        self.draw_channel(&rt, &brushes, &formats, &layout.right, Channel::Right);
        self.draw_global(&rt, &brushes, &formats, &layout);
        self.draw_popups(&rt, &brushes, &formats, &layout);

        if unsafe { rt.EndDraw(None, None) }.is_err() {
            self.render_target = None;
        }
    }

    fn draw_background(
        &self,
        rt: &ID2D1HwndRenderTarget,
        brushes: &Brushes,
        formats: &TextFormats,
        layout: &Layout,
    ) {
        fill_rect(rt, layout.full, &brushes.bg);
        fill_rect(rt, layout.header, &brushes.top);
        fill_rect(rt, layout.footer, &brushes.footer);
        card(rt, layout.left.panel, 7.0 * layout.s, brushes);
        card(rt, layout.right.panel, 7.0 * layout.s, brushes);
        card(rt, layout.global, 7.0 * layout.s, brushes);
        card(rt, layout.input_meter, 7.0 * layout.s, brushes);
        card(rt, layout.output_meter, 7.0 * layout.s, brushes);

        let bypass = self.params.is_bypassed();
        draw_text(
            rt,
            "Nebula Stereo Delay",
            UiRect::new(
                18.0 * layout.s,
                11.0 * layout.s,
                210.0 * layout.s,
                24.0 * layout.s,
            ),
            &formats.title,
            &brushes.text_light,
            Align::Leading,
        );
        draw_text(
            rt,
            "Stereo Delay  |  Native Direct2D  |  VST3",
            UiRect::new(
                18.0 * layout.s,
                34.0 * layout.s,
                275.0 * layout.s,
                18.0 * layout.s,
            ),
            &formats.small,
            &brushes.text_secondary,
            Align::Leading,
        );
        draw_text(
            rt,
            concat!("v", env!("CARGO_PKG_VERSION")),
            UiRect::new(
                896.0 * layout.s,
                16.0 * layout.s,
                80.0 * layout.s,
                18.0 * layout.s,
            ),
            &formats.small,
            &brushes.text_secondary,
            Align::Trailing,
        );

        for zone in self.top_buttons(layout) {
            let active = match zone.action {
                Action::TopButton(TopButton::Bypass) => !bypass,
                Action::TopButton(TopButton::Midi) => {
                    self.midi_learn_waiting_for_control || self.midi_runtime.is_learning()
                }
                Action::TopButton(TopButton::Ab) => {
                    self.params.ab_state.load(Ordering::Relaxed) == 1
                }
                _ => false,
            };
            let label = match zone.action {
                Action::TopButton(TopButton::Preset) => "Preset",
                Action::TopButton(TopButton::Undo) => "Undo",
                Action::TopButton(TopButton::Redo) => "Redo",
                Action::TopButton(TopButton::Ab) => {
                    if self.params.ab_state.load(Ordering::Relaxed) == 0 {
                        "A/B [A]"
                    } else {
                        "A/B [B]"
                    }
                }
                Action::TopButton(TopButton::Midi) => {
                    if self.midi_learn_waiting_for_control {
                        "Pick Control"
                    } else if self.midi_runtime.is_learning() {
                        "Move CC"
                    } else {
                        "MIDI Learn"
                    }
                }
                Action::TopButton(TopButton::Bypass) => {
                    if bypass {
                        "FX Bypassed"
                    } else {
                        "FX On"
                    }
                }
                _ => "",
            };
            draw_button(rt, zone.rect, label, active, brushes, formats, layout.s);
        }

        draw_text(
            rt,
            &self.status,
            UiRect::new(
                332.0 * layout.s,
                35.0 * layout.s,
                320.0 * layout.s,
                18.0 * layout.s,
            ),
            &formats.small,
            &brushes.text_secondary,
            Align::Center,
        );
        draw_text(
            rt,
            "Nebula Stereo Delay",
            layout.footer,
            &formats.title,
            &brushes.text_light,
            Align::Center,
        );
    }

    fn draw_meters(
        &self,
        rt: &ID2D1HwndRenderTarget,
        brushes: &Brushes,
        formats: &TextFormats,
        layout: &Layout,
    ) {
        let in_level = self
            .meters
            .get_input_l()
            .max(self.meters.get_input_r())
            .max(0.000_001);
        let out_level = self
            .meters
            .get_output_l()
            .max(self.meters.get_output_r())
            .max(0.000_001);
        self.draw_meter(
            rt,
            brushes,
            formats,
            layout.input_meter,
            "Input",
            in_level,
            FloatControl::InputLevel,
            layout.s,
        );
        self.draw_meter(
            rt,
            brushes,
            formats,
            layout.output_meter,
            "Output",
            out_level,
            FloatControl::OutputLevel,
            layout.s,
        );
    }

    fn draw_meter(
        &self,
        rt: &ID2D1HwndRenderTarget,
        brushes: &Brushes,
        formats: &TextFormats,
        rect: UiRect,
        title: &str,
        linear_level: f32,
        trim: FloatControl,
        s: f32,
    ) {
        draw_text(
            rt,
            title,
            UiRect::new(rect.x, rect.y + 10.0 * s, rect.w, 16.0 * s),
            &formats.body,
            &brushes.accent,
            Align::Center,
        );
        let level_db = 20.0 * linear_level.log10();
        let top = UiRect::new(
            rect.x + 8.0 * s,
            rect.y + 33.0 * s,
            rect.w - 16.0 * s,
            22.0 * s,
        );
        draw_value_box(
            rt,
            top,
            &format!("{:.1} dB", level_db.clamp(-100.0, 24.0)),
            brushes,
            formats,
        );

        let rail = UiRect::new(
            rect.center_x() - 10.0 * s,
            rect.y + 74.0 * s,
            20.0 * s,
            rect.h - 150.0 * s,
        );
        fill_round(rt, rail, 5.0 * s, &brushes.control);
        stroke_round(rt, rail, 5.0 * s, &brushes.border, 1.0 * s);
        let level_norm = ((level_db + 80.0) / 100.0).clamp(0.0, 1.0);
        let meter_h = rail.h * level_norm;
        fill_round(
            rt,
            UiRect::new(
                rail.x + 4.0 * s,
                rail.bottom() - meter_h,
                rail.w - 8.0 * s,
                meter_h,
            ),
            3.0 * s,
            &brushes.green,
        );
        for i in 0..=5 {
            let y = rail.y + rail.h * i as f32 / 5.0;
            draw_line(
                rt,
                rail.right() + 5.0 * s,
                y,
                rail.right() + 12.0 * s,
                y,
                &brushes.divider,
                1.0,
            );
        }

        let trim_norm = self.float_param(trim).modulated_normalized_value();
        let handle_y = rail.bottom() - rail.h * trim_norm;
        fill_round(
            rt,
            UiRect::new(
                rect.x + 17.0 * s,
                handle_y - 4.0 * s,
                rect.w - 34.0 * s,
                8.0 * s,
            ),
            2.5 * s,
            &brushes.accent_light,
        );
        let bottom = UiRect::new(
            rect.x + 8.0 * s,
            rect.bottom() - 45.0 * s,
            rect.w - 16.0 * s,
            22.0 * s,
        );
        draw_value_box(
            rt,
            bottom,
            &format_float(self.float_param(trim)),
            brushes,
            formats,
        );
    }

    fn draw_channel(
        &self,
        rt: &ID2D1HwndRenderTarget,
        brushes: &Brushes,
        formats: &TextFormats,
        panel: &ChannelLayout,
        ch: Channel,
    ) {
        let s = panel.s;
        let title = if ch == Channel::Left {
            "Left Delay"
        } else {
            "Right Delay"
        };
        draw_text(
            rt,
            title,
            panel.title,
            &formats.title,
            &brushes.accent,
            Align::Center,
        );
        let input = if ch == Channel::Left {
            EnumControl::InputL
        } else {
            EnumControl::InputR
        };
        let note = if ch == Channel::Left {
            EnumControl::NoteL
        } else {
            EnumControl::NoteR
        };
        draw_text(
            rt,
            "Input",
            panel.input_label,
            &formats.small,
            &brushes.text_secondary,
            Align::Center,
        );
        self.draw_dropdown(
            rt,
            brushes,
            formats,
            panel.input,
            self.enum_value_label(input),
            s,
        );
        draw_text(
            rt,
            "Note",
            panel.note_label,
            &formats.small,
            &brushes.text_secondary,
            Align::Center,
        );
        self.draw_dropdown(
            rt,
            brushes,
            formats,
            panel.note,
            self.enum_value_label(note),
            s,
        );

        draw_text(
            rt,
            "Delay Time",
            panel.delay_label,
            &formats.small,
            &brushes.text_secondary,
            Align::Center,
        );
        let delay_control = if ch == Channel::Left {
            FloatControl::DelayTimeL
        } else {
            FloatControl::DelayTimeR
        };
        let note_param = if ch == Channel::Left {
            self.params.note_l.value()
        } else {
            self.params.note_r.value()
        };
        let dev_param = if ch == Channel::Left {
            self.params.deviation_l.value()
        } else {
            self.params.deviation_r.value()
        };
        let delay_norm = if self.params.tempo_sync.value() {
            sync_knob_normalized(note_param, dev_param)
        } else {
            self.float_display_norm(delay_control)
        };
        if self.params.tempo_sync.value() {
            draw_note_ring(
                rt,
                panel.delay_cx,
                panel.delay_cy,
                panel.delay_r + 12.0 * s,
                brushes,
                s,
            );
        }
        draw_knob(
            rt,
            panel.delay_cx,
            panel.delay_cy,
            panel.delay_r,
            delay_norm,
            &brushes.accent,
            brushes,
            s,
        );
        let delay_value = if self.params.tempo_sync.value() {
            format!("{:.0} ms", synced_delay_ms(note_param, dev_param))
        } else {
            format_float(self.float_param(delay_control))
        };
        draw_text(
            rt,
            &delay_value,
            UiRect::new(
                panel.delay_cx - 34.0 * s,
                panel.delay_cy - 8.0 * s,
                68.0 * s,
                16.0 * s,
            ),
            &formats.small,
            &brushes.text_light,
            Align::Center,
        );
        draw_button(
            rt,
            panel.halve,
            ":2",
            self.bool_for_channel(ch, BoolControl::Halve),
            brushes,
            formats,
            s,
        );
        draw_button(
            rt,
            panel.double,
            "x2",
            self.bool_for_channel(ch, BoolControl::Double),
            brushes,
            formats,
            s,
        );

        draw_text(
            rt,
            "Deviation",
            panel.deviation_label,
            &formats.small,
            &brushes.text_secondary,
            Align::Center,
        );
        let dev = if ch == Channel::Left {
            FloatControl::DeviationL
        } else {
            FloatControl::DeviationR
        };
        draw_value_box(
            rt,
            panel.deviation,
            &format_float(self.float_param(dev)),
            brushes,
            formats,
        );

        let knobs = [
            (
                "HPF",
                if ch == Channel::Left {
                    FloatControl::LowCutL
                } else {
                    FloatControl::LowCutR
                },
                &brushes.accent,
            ),
            (
                "HPFS",
                if ch == Channel::Left {
                    FloatControl::LowCutSlopeL
                } else {
                    FloatControl::LowCutSlopeR
                },
                &brushes.purple,
            ),
            (
                "LPF",
                if ch == Channel::Left {
                    FloatControl::HighCutL
                } else {
                    FloatControl::HighCutR
                },
                &brushes.orange,
            ),
            (
                "LPFS",
                if ch == Channel::Left {
                    FloatControl::HighCutSlopeL
                } else {
                    FloatControl::HighCutSlopeR
                },
                &brushes.magenta,
            ),
        ];
        for (idx, (label, control, accent)) in knobs.iter().enumerate() {
            let cx = panel.filter_x[idx];
            draw_text(
                rt,
                label,
                UiRect::new(cx - 30.0 * s, panel.filter_y - 38.0 * s, 60.0 * s, 14.0 * s),
                &formats.small,
                &brushes.text_secondary,
                Align::Center,
            );
            draw_knob(
                rt,
                cx,
                panel.filter_y,
                23.0 * s,
                self.float_display_norm(*control),
                accent,
                brushes,
                s,
            );
            draw_value_box(
                rt,
                UiRect::new(cx - 37.0 * s, panel.filter_y + 29.0 * s, 74.0 * s, 18.0 * s),
                &format_float(self.float_param(*control)),
                brushes,
                formats,
            );
        }

        let feedback = if ch == Channel::Left {
            FloatControl::FeedbackL
        } else {
            FloatControl::FeedbackR
        };
        let cross = if ch == Channel::Left {
            FloatControl::CrossfeedLr
        } else {
            FloatControl::CrossfeedRl
        };
        draw_text(
            rt,
            "Feedback",
            panel.feedback_label,
            &formats.small,
            &brushes.text_secondary,
            Align::Center,
        );
        draw_knob(
            rt,
            panel.feedback_cx,
            panel.feedback_cy,
            32.0 * s,
            self.float_display_norm(feedback),
            &brushes.accent,
            brushes,
            s,
        );
        draw_value_box(
            rt,
            UiRect::new(
                panel.feedback_cx - 44.0 * s,
                panel.feedback_cy + 39.0 * s,
                88.0 * s,
                18.0 * s,
            ),
            &format_float(self.float_param(feedback)),
            brushes,
            formats,
        );
        draw_button(
            rt,
            panel.feedback_phase,
            "Phase",
            self.bool_for_channel(ch, BoolControl::FeedbackPhase),
            brushes,
            formats,
            s,
        );

        let cross_label = if ch == Channel::Left {
            "Crossfeed L-R"
        } else {
            "Crossfeed R-L"
        };
        draw_text(
            rt,
            cross_label,
            panel.crossfeed_label,
            &formats.small,
            &brushes.text_secondary,
            Align::Center,
        );
        draw_knob(
            rt,
            panel.crossfeed_cx,
            panel.crossfeed_cy,
            32.0 * s,
            self.float_display_norm(cross),
            &brushes.magenta,
            brushes,
            s,
        );
        draw_value_box(
            rt,
            UiRect::new(
                panel.crossfeed_cx - 44.0 * s,
                panel.crossfeed_cy + 39.0 * s,
                88.0 * s,
                18.0 * s,
            ),
            &format_float(self.float_param(cross)),
            brushes,
            formats,
        );
        draw_button(
            rt,
            panel.crossfeed_phase,
            "Phase",
            self.bool_for_channel(ch, BoolControl::CrossfeedPhase),
            brushes,
            formats,
            s,
        );
    }

    fn draw_global(
        &self,
        rt: &ID2D1HwndRenderTarget,
        brushes: &Brushes,
        formats: &TextFormats,
        layout: &Layout,
    ) {
        let s = layout.s;
        let g = &layout.global_controls;
        draw_text(
            rt,
            "Global",
            g.title,
            &formats.title,
            &brushes.accent,
            Align::Center,
        );
        draw_text(
            rt,
            "Routing",
            g.routing_label,
            &formats.small,
            &brushes.text_secondary,
            Align::Center,
        );
        self.draw_dropdown(
            rt,
            brushes,
            formats,
            g.routing,
            self.enum_value_label(EnumControl::Routing),
            s,
        );
        draw_text(
            rt,
            "Oversampling",
            g.os_label,
            &formats.small,
            &brushes.text_secondary,
            Align::Center,
        );
        self.draw_dropdown(
            rt,
            brushes,
            formats,
            g.oversampling,
            self.enum_value_label(EnumControl::Oversampling),
            s,
        );
        draw_button(
            rt,
            g.sync,
            "Tempo Sync",
            self.params.tempo_sync.value(),
            brushes,
            formats,
            s,
        );
        draw_button(
            rt,
            g.link,
            "Stereo Link",
            self.params.stereo_link.value(),
            brushes,
            formats,
            s,
        );

        draw_text(
            rt,
            "Output Mix",
            g.mix_title,
            &formats.title,
            &brushes.accent,
            Align::Center,
        );
        let mix_l = FloatControl::OutputMixL;
        let mix_r = FloatControl::OutputMixR;
        draw_text(
            rt,
            "Left",
            g.mix_l_label,
            &formats.small,
            &brushes.text_secondary,
            Align::Center,
        );
        draw_knob(
            rt,
            g.mix_l_cx,
            g.mix_cy,
            32.0 * s,
            self.float_display_norm(mix_l),
            &brushes.green,
            brushes,
            s,
        );
        draw_value_box(
            rt,
            UiRect::new(
                g.mix_l_cx - 43.0 * s,
                g.mix_cy + 39.0 * s,
                86.0 * s,
                18.0 * s,
            ),
            &format_float(self.float_param(mix_l)),
            brushes,
            formats,
        );
        draw_text(
            rt,
            "Right",
            g.mix_r_label,
            &formats.small,
            &brushes.text_secondary,
            Align::Center,
        );
        draw_knob(
            rt,
            g.mix_r_cx,
            g.mix_cy,
            32.0 * s,
            self.float_display_norm(mix_r),
            &brushes.green,
            brushes,
            s,
        );
        draw_value_box(
            rt,
            UiRect::new(
                g.mix_r_cx - 43.0 * s,
                g.mix_cy + 39.0 * s,
                86.0 * s,
                18.0 * s,
            ),
            &format_float(self.float_param(mix_r)),
            brushes,
            formats,
        );
    }

    fn draw_dropdown(
        &self,
        rt: &ID2D1HwndRenderTarget,
        brushes: &Brushes,
        formats: &TextFormats,
        rect: UiRect,
        label: &str,
        s: f32,
    ) {
        fill_round(rt, rect, 4.0 * s, &brushes.control);
        stroke_round(rt, rect, 4.0 * s, &brushes.border, 1.0 * s);
        draw_text(
            rt,
            label,
            UiRect::new(rect.x + 7.0 * s, rect.y, rect.w - 24.0 * s, rect.h),
            &formats.small,
            &brushes.text_primary,
            Align::Leading,
        );
        draw_text(
            rt,
            "v",
            UiRect::new(rect.right() - 18.0 * s, rect.y, 12.0 * s, rect.h),
            &formats.small,
            &brushes.accent,
            Align::Center,
        );
    }

    fn draw_popups(
        &self,
        rt: &ID2D1HwndRenderTarget,
        brushes: &Brushes,
        formats: &TextFormats,
        layout: &Layout,
    ) {
        if let Some(input) = &self.numeric_input {
            let rect = input.rect;
            fill_round(rt, rect, 5.0 * layout.s, &brushes.card);
            stroke_round(rt, rect, 5.0 * layout.s, &brushes.accent, 1.0 * layout.s);
            draw_text(
                rt,
                &input.text,
                rect.shrink(5.0 * layout.s),
                &formats.body,
                &brushes.text_light,
                Align::Center,
            );
        }

        match self.popup {
            Some(Popup::Dropdown { control, anchor }) => {
                let items = dropdown_items(control);
                let item_h = 23.0 * layout.s;
                let popup = UiRect::new(
                    anchor.x,
                    anchor.bottom() + 3.0 * layout.s,
                    anchor.w.max(118.0 * layout.s),
                    item_h * items.len() as f32,
                );
                fill_round(rt, popup, 5.0 * layout.s, &brushes.card);
                stroke_round(
                    rt,
                    popup,
                    5.0 * layout.s,
                    &brushes.accent_dark,
                    1.0 * layout.s,
                );
                for (idx, (_, label)) in items.iter().enumerate() {
                    let row = UiRect::new(popup.x, popup.y + idx as f32 * item_h, popup.w, item_h);
                    let active = self.enum_value_label(control) == *label;
                    if active {
                        fill_round(
                            rt,
                            row.shrink(2.0 * layout.s),
                            3.0 * layout.s,
                            &brushes.accent_soft,
                        );
                    }
                    draw_text(
                        rt,
                        label,
                        row.shrink(7.0 * layout.s),
                        &formats.small,
                        &brushes.text_primary,
                        Align::Leading,
                    );
                }
            }
            Some(Popup::Preset) => {
                let rect = layout.preset_popup;
                fill_round(rt, rect, 7.0 * layout.s, &brushes.card);
                stroke_round(
                    rt,
                    rect,
                    7.0 * layout.s,
                    &brushes.accent_dark,
                    1.0 * layout.s,
                );
                draw_text(
                    rt,
                    "Preset Manager",
                    UiRect::new(
                        rect.x + 12.0 * layout.s,
                        rect.y + 8.0 * layout.s,
                        rect.w - 24.0 * layout.s,
                        22.0 * layout.s,
                    ),
                    &formats.body,
                    &brushes.accent,
                    Align::Leading,
                );
                let name_rect = preset_name_rect(layout);
                let name_active = matches!(self.text_input, Some(TextInput::PresetName));
                fill_round(
                    rt,
                    name_rect,
                    4.0 * layout.s,
                    if name_active {
                        &brushes.control
                    } else {
                        &brushes.bg
                    },
                );
                stroke_round(
                    rt,
                    name_rect,
                    4.0 * layout.s,
                    if name_active {
                        &brushes.accent
                    } else {
                        &brushes.border
                    },
                    1.0 * layout.s,
                );
                draw_text(
                    rt,
                    &self.preset_name,
                    name_rect.shrink(5.0 * layout.s),
                    &formats.small,
                    &brushes.text_primary,
                    Align::Leading,
                );
                for item in preset_popup_items(layout, &self.preset_manager, &self.params) {
                    match item.kind {
                        PresetItemKind::Header(label) => {
                            draw_text(
                                rt,
                                label,
                                item.rect,
                                &formats.small,
                                &brushes.text_secondary,
                                Align::Leading,
                            );
                        }
                        PresetItemKind::Button(label) => {
                            draw_button(rt, item.rect, label, false, brushes, formats, layout.s);
                        }
                        PresetItemKind::Preset(label, factory) => {
                            let brush = if factory {
                                &brushes.text_primary
                            } else {
                                &brushes.accent
                            };
                            draw_text(
                                rt,
                                &label,
                                item.rect.shrink(5.0 * layout.s),
                                &formats.small,
                                brush,
                                Align::Leading,
                            );
                        }
                    }
                }
            }
            Some(Popup::Midi) => {
                let rect = layout.midi_popup;
                fill_round(rt, rect, 7.0 * layout.s, &brushes.card);
                stroke_round(
                    rt,
                    rect,
                    7.0 * layout.s,
                    &brushes.accent_dark,
                    1.0 * layout.s,
                );
                draw_text(
                    rt,
                    "MIDI Learn",
                    UiRect::new(
                        rect.x + 12.0 * layout.s,
                        rect.y + 8.0 * layout.s,
                        rect.w - 24.0 * layout.s,
                        22.0 * layout.s,
                    ),
                    &formats.body,
                    &brushes.accent,
                    Align::Leading,
                );
                for item in midi_popup_items(layout, &self.params) {
                    match item.kind {
                        MidiItemKind::Header(label) => draw_text(
                            rt,
                            label,
                            item.rect,
                            &formats.small,
                            &brushes.text_secondary,
                            Align::Leading,
                        ),
                        MidiItemKind::Button(label, active) => {
                            draw_button(rt, item.rect, label, active, brushes, formats, layout.s)
                        }
                        MidiItemKind::Mapping(label) => draw_text(
                            rt,
                            &label,
                            item.rect.shrink(5.0 * layout.s),
                            &formats.small,
                            &brushes.text_primary,
                            Align::Leading,
                        ),
                    }
                }
            }
            None => {}
        }
    }

    fn mouse_down(&mut self, x: f32, y: f32) {
        let _ = unsafe { SetFocus(Some(self.hwnd)) };
        let context = self.context.clone();
        let setter = ParamSetter::new(context.as_ref());
        let layout = self.current_layout();

        if self.handle_popup_click(x, y, &setter, &layout) {
            invalidate(self.hwnd);
            return;
        }
        if self.handle_text_click(x, y, &setter, &layout) {
            invalidate(self.hwnd);
            return;
        }

        let action = self.hit_action(x, y, &layout);
        if let Some(action) = action {
            if self.midi_learn_waiting_for_control {
                if let Some(target) = action.midi_target() {
                    self.begin_midi_learn(target);
                    invalidate(self.hwnd);
                    return;
                }
            }

            match action {
                Action::Float(control) | Action::FloatField(control) => {
                    if matches!(action, Action::FloatField(_)) {
                        self.open_numeric_input(control, &layout);
                    } else {
                        self.params.push_undo();
                        let start_norm = self.float_display_norm(control);
                        self.begin_float_gesture(control, &setter);
                        let mut drag = DragState {
                            control,
                            start_y: y,
                            start_norm,
                        };
                        if control.is_meter_trim() {
                            self.update_drag(&mut drag, x, y, &setter, &layout);
                        }
                        self.drag = Some(drag);
                        let _ = unsafe { SetCapture(self.hwnd) };
                    }
                }
                Action::Bool(control) => self.toggle_bool(control, &setter),
                Action::DelayScale(ch, factor) => self.apply_delay_scale(ch, factor, &setter),
                Action::Dropdown(control) => {
                    self.popup = Some(Popup::Dropdown {
                        control,
                        anchor: self.dropdown_anchor(control, &layout),
                    })
                }
                Action::TopButton(button) => self.handle_top_button(button, &setter),
            }
        } else {
            self.popup = None;
            self.numeric_input = None;
            self.text_input = None;
        }
        invalidate(self.hwnd);
    }

    fn mouse_double_click(&mut self, x: f32, y: f32) {
        let layout = self.current_layout();
        match self.hit_action(x, y, &layout) {
            Some(Action::Float(control)) | Some(Action::FloatField(control)) => {
                self.open_numeric_input(control, &layout);
            }
            _ => {}
        }
        invalidate(self.hwnd);
    }

    fn mouse_right_down(&mut self, x: f32, y: f32) {
        let layout = self.current_layout();
        if self.top_buttons(&layout).iter().any(|z| {
            z.rect.contains(x, y) && matches!(z.action, Action::TopButton(TopButton::Midi))
        }) {
            self.popup = Some(Popup::Midi);
        } else {
            self.popup = None;
        }
        invalidate(self.hwnd);
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        if let Some(mut drag) = self.drag.take() {
            let context = self.context.clone();
            let setter = ParamSetter::new(context.as_ref());
            let layout = self.current_layout();
            self.update_drag(&mut drag, x, y, &setter, &layout);
            self.drag = Some(drag);
            invalidate(self.hwnd);
        }
    }

    fn mouse_up(&mut self, _x: f32, _y: f32) {
        if let Some(drag) = self.drag.take() {
            let context = self.context.clone();
            let setter = ParamSetter::new(context.as_ref());
            self.end_float_gesture(drag.control, &setter);
            let _ = unsafe { ReleaseCapture() };
            invalidate(self.hwnd);
        }
    }

    fn char_input(&mut self, ch: char) {
        if self.numeric_input.is_none() && self.text_input.is_none() {
            return;
        }
        match ch {
            '\u{8}' => {
                if let Some(input) = &mut self.numeric_input {
                    input.text.pop();
                } else if matches!(self.text_input, Some(TextInput::PresetName)) {
                    self.preset_name.pop();
                }
            }
            '\r' | '\n' => {
                let context = self.context.clone();
                let setter = ParamSetter::new(context.as_ref());
                self.commit_active_text(&setter);
            }
            ch if !ch.is_control() => {
                if let Some(input) = &mut self.numeric_input {
                    if input.text.len() < 32 {
                        input.text.push(ch);
                    }
                } else if matches!(self.text_input, Some(TextInput::PresetName))
                    && self.preset_name.len() < 64
                {
                    self.preset_name.push(ch);
                }
            }
            _ => {}
        }
        invalidate(self.hwnd);
    }

    fn key_down(&mut self, key: u32) {
        if key == VK_RETURN.0 as u32 {
            let context = self.context.clone();
            let setter = ParamSetter::new(context.as_ref());
            self.commit_active_text(&setter);
        } else if key == VK_ESCAPE.0 as u32 {
            self.numeric_input = None;
            self.text_input = None;
            self.popup = None;
            self.stop_midi_learn();
        } else if key == VK_BACK.0 as u32 {
            self.char_input('\u{8}');
        }
        invalidate(self.hwnd);
    }

    fn handle_top_button(&mut self, button: TopButton, setter: &ParamSetter<'_>) {
        match button {
            TopButton::Preset => {
                self.popup = Some(Popup::Preset);
                self.numeric_input = None;
            }
            TopButton::Undo => {
                if let Some(prev) = self.params.undo() {
                    apply_snapshot(&self.params, setter, &prev);
                    self.status = "Undo".to_string();
                }
            }
            TopButton::Redo => {
                if let Some(next) = self.params.redo() {
                    apply_snapshot(&self.params, setter, &next);
                    self.status = "Redo".to_string();
                }
            }
            TopButton::Ab => {
                self.params.push_undo();
                let snapshot = self.params.ab_toggle();
                apply_snapshot(&self.params, setter, &snapshot);
                let slot = if self.params.ab_state.load(Ordering::Relaxed) == 0 {
                    "A"
                } else {
                    "B"
                };
                self.status = format!("A/B switched to {slot}");
            }
            TopButton::Midi => {
                if self.midi_runtime.is_learning() || self.midi_learn_waiting_for_control {
                    self.stop_midi_learn();
                    self.status = "MIDI learn cancelled".to_string();
                } else {
                    self.midi_learn_waiting_for_control = true;
                    self.status = "Click a control to learn".to_string();
                }
            }
            TopButton::Bypass => {
                self.params.set_bypass(!self.params.is_bypassed());
                self.status = if self.params.is_bypassed() {
                    "Hard bypass engaged".to_string()
                } else {
                    "FX active".to_string()
                };
            }
        }
    }

    fn handle_popup_click(
        &mut self,
        x: f32,
        y: f32,
        setter: &ParamSetter<'_>,
        layout: &Layout,
    ) -> bool {
        match self.popup {
            Some(Popup::Dropdown { control, anchor }) => {
                let items = dropdown_items(control);
                let item_h = 23.0 * layout.s;
                let popup = UiRect::new(
                    anchor.x,
                    anchor.bottom() + 3.0 * layout.s,
                    anchor.w.max(118.0 * layout.s),
                    item_h * items.len() as f32,
                );
                if !popup.contains(x, y) {
                    self.popup = None;
                    return false;
                }
                let idx = ((y - popup.y) / item_h).floor() as usize;
                if let Some((value, _)) = items.get(idx) {
                    self.params.push_undo();
                    self.set_enum_from_index(control, *value, setter);
                    self.popup = None;
                    return true;
                }
                false
            }
            Some(Popup::Preset) => {
                if !layout.preset_popup.contains(x, y) {
                    self.popup = None;
                    return false;
                }
                if preset_name_rect(layout).contains(x, y) {
                    self.text_input = Some(TextInput::PresetName);
                    return true;
                }
                for item in preset_popup_items(layout, &self.preset_manager, &self.params) {
                    if !item.rect.contains(x, y) {
                        continue;
                    }
                    match item.action {
                        PresetAction::SaveCurrent => self.save_current_preset(),
                        PresetAction::SaveA => self.save_ab_preset(0),
                        PresetAction::SaveB => self.save_ab_preset(1),
                        PresetAction::LoadFactory(index) => {
                            if let Some(preset) = self.preset_manager.factory_presets().get(index) {
                                self.params.push_undo();
                                self.preset_manager
                                    .load_preset(preset, &self.params, setter);
                                self.status = format!("Loaded {}", preset.name);
                                self.popup = None;
                            }
                        }
                        PresetAction::LoadUser(index) => {
                            if let Ok(presets) = self.preset_manager.user_presets() {
                                if let Some(preset) = presets.get(index) {
                                    self.params.push_undo();
                                    self.preset_manager
                                        .load_preset(preset, &self.params, setter);
                                    self.status = format!("Loaded {}", preset.name);
                                    self.popup = None;
                                }
                            }
                        }
                        PresetAction::None => {}
                    }
                    return true;
                }
                true
            }
            Some(Popup::Midi) => {
                if !layout.midi_popup.contains(x, y) {
                    self.popup = None;
                    return false;
                }
                for item in midi_popup_items(layout, &self.params) {
                    if !item.rect.contains(x, y) {
                        continue;
                    }
                    match item.action {
                        MidiAction::ToggleGlobal => {
                            if let Ok(mut learn) = self.params.midi_learn.write() {
                                learn.toggle_global_enabled();
                                sync_runtime_from_learn_state(&self.midi_runtime, &learn);
                                self.status = if learn.is_global_enabled() {
                                    "MIDI control enabled".to_string()
                                } else {
                                    "MIDI control disabled".to_string()
                                };
                            }
                        }
                        MidiAction::Clean(param_id) => {
                            if let Ok(mut learn) = self.params.midi_learn.write() {
                                learn.clean_up(&param_id);
                                sync_runtime_from_learn_state(&self.midi_runtime, &learn);
                                self.status = format!("Removed MIDI mapping for {param_id}");
                            }
                        }
                        MidiAction::ClearAll => {
                            if let Ok(mut learn) = self.params.midi_learn.write() {
                                learn.clear_all();
                                sync_runtime_from_learn_state(&self.midi_runtime, &learn);
                                self.status = "Cleared all MIDI mappings".to_string();
                            }
                        }
                        MidiAction::Rollback => {
                            if let Ok(mut learn) = self.params.midi_learn.write() {
                                learn.roll_back();
                                sync_runtime_from_learn_state(&self.midi_runtime, &learn);
                                self.status = "Rolled back MIDI mapping".to_string();
                            }
                        }
                        MidiAction::Save => {
                            if let Ok(mut learn) = self.params.midi_learn.write() {
                                learn.save_for_rollback();
                                self.status = "Saved MIDI mapping".to_string();
                            }
                        }
                        MidiAction::None => {}
                    }
                    return true;
                }
                true
            }
            None => false,
        }
    }

    fn handle_text_click(
        &mut self,
        _x: f32,
        _y: f32,
        setter: &ParamSetter<'_>,
        _layout: &Layout,
    ) -> bool {
        if self.numeric_input.is_some() {
            self.commit_active_text(setter);
            return true;
        }
        false
    }

    fn hit_action(&self, x: f32, y: f32, layout: &Layout) -> Option<Action> {
        self.hit_zones(layout)
            .into_iter()
            .find(|zone| zone.rect.contains(x, y))
            .map(|zone| zone.action)
    }

    fn hit_zones(&self, layout: &Layout) -> Vec<HitZone> {
        let mut zones = Vec::new();
        zones.extend(self.top_buttons(layout));
        zones.push(HitZone::new(
            layout.input_meter,
            Action::Float(FloatControl::InputLevel),
        ));
        zones.push(HitZone::new(
            layout.output_meter,
            Action::Float(FloatControl::OutputLevel),
        ));
        channel_zones(&mut zones, &layout.left, Channel::Left);
        channel_zones(&mut zones, &layout.right, Channel::Right);
        let g = &layout.global_controls;
        zones.push(HitZone::new(
            g.routing,
            Action::Dropdown(EnumControl::Routing),
        ));
        zones.push(HitZone::new(
            g.oversampling,
            Action::Dropdown(EnumControl::Oversampling),
        ));
        zones.push(HitZone::new(g.sync, Action::Bool(BoolControl::TempoSync)));
        zones.push(HitZone::new(g.link, Action::Bool(BoolControl::StereoLink)));
        zones.push(HitZone::new(
            knob_rect(g.mix_l_cx, g.mix_cy, 43.0 * layout.s),
            Action::Float(FloatControl::OutputMixL),
        ));
        zones.push(HitZone::new(
            knob_rect(g.mix_r_cx, g.mix_cy, 43.0 * layout.s),
            Action::Float(FloatControl::OutputMixR),
        ));
        zones.push(HitZone::new(
            UiRect::new(
                g.mix_l_cx - 43.0 * layout.s,
                g.mix_cy + 39.0 * layout.s,
                86.0 * layout.s,
                18.0 * layout.s,
            ),
            Action::FloatField(FloatControl::OutputMixL),
        ));
        zones.push(HitZone::new(
            UiRect::new(
                g.mix_r_cx - 43.0 * layout.s,
                g.mix_cy + 39.0 * layout.s,
                86.0 * layout.s,
                18.0 * layout.s,
            ),
            Action::FloatField(FloatControl::OutputMixR),
        ));
        zones
    }

    fn top_buttons(&self, layout: &Layout) -> Vec<HitZone> {
        let s = layout.s;
        let y = 58.0 * s;
        let h = 24.0 * s;
        let mut x = 18.0 * s;
        let specs = [
            (TopButton::Preset, 82.0),
            (TopButton::Undo, 60.0),
            (TopButton::Redo, 60.0),
            (TopButton::Ab, 72.0),
            (TopButton::Midi, 96.0),
            (TopButton::Bypass, 96.0),
        ];
        let mut zones = Vec::new();
        for (button, w) in specs {
            zones.push(HitZone::new(
                UiRect::new(x, y, w * s, h),
                Action::TopButton(button),
            ));
            x += (w + 8.0) * s;
        }
        zones
    }

    fn dropdown_anchor(&self, control: EnumControl, layout: &Layout) -> UiRect {
        match control {
            EnumControl::InputL => layout.left.input,
            EnumControl::InputR => layout.right.input,
            EnumControl::NoteL => layout.left.note,
            EnumControl::NoteR => layout.right.note,
            EnumControl::Routing => layout.global_controls.routing,
            EnumControl::Oversampling => layout.global_controls.oversampling,
        }
    }

    fn enum_value_label(&self, control: EnumControl) -> &'static str {
        match control {
            EnumControl::InputL => input_mode_label(self.params.input_mode_l.value()),
            EnumControl::InputR => input_mode_label(self.params.input_mode_r.value()),
            EnumControl::NoteL => note_label(self.params.note_l.value()),
            EnumControl::NoteR => note_label(self.params.note_r.value()),
            EnumControl::Routing => routing_label(self.params.routing.value()),
            EnumControl::Oversampling => oversampling_label(self.params.oversampling.value()),
        }
    }

    fn update_drag(
        &mut self,
        drag: &mut DragState,
        x: f32,
        y: f32,
        setter: &ParamSetter<'_>,
        layout: &Layout,
    ) {
        let next = if drag.control.is_meter_trim() {
            let rail = if drag.control == FloatControl::InputLevel {
                meter_rail(layout.input_meter, layout.s)
            } else {
                meter_rail(layout.output_meter, layout.s)
            };
            1.0 - ((y - rail.y) / rail.h).clamp(0.0, 1.0)
        } else {
            let delta = (drag.start_y - y) / (150.0 * layout.s).max(1.0);
            drag.start_norm + delta
        }
        .clamp(0.0, 1.0);

        if self.params.tempo_sync.value() {
            match drag.control {
                FloatControl::DelayTimeL => self.set_sync_norm(Channel::Left, next, setter),
                FloatControl::DelayTimeR => self.set_sync_norm(Channel::Right, next, setter),
                _ => self.set_float_display_norm(drag.control, next, setter),
            }
        } else {
            self.set_float_display_norm(drag.control, next, setter);
        }
        let _ = x;
    }

    fn begin_float_gesture(&self, control: FloatControl, setter: &ParamSetter<'_>) {
        if self.params.tempo_sync.value()
            && matches!(control, FloatControl::DelayTimeL | FloatControl::DelayTimeR)
        {
            let ch = if control == FloatControl::DelayTimeL {
                Channel::Left
            } else {
                Channel::Right
            };
            self.begin_sync_gesture(ch, setter);
            return;
        }
        let param = self.float_param(control);
        setter.begin_set_parameter(param);
        if self.stereo_link_active() {
            if let Some(other) = self.linked_float(control) {
                setter.begin_set_parameter(self.float_param(other));
            }
        }
    }

    fn end_float_gesture(&self, control: FloatControl, setter: &ParamSetter<'_>) {
        if self.params.tempo_sync.value()
            && matches!(control, FloatControl::DelayTimeL | FloatControl::DelayTimeR)
        {
            let ch = if control == FloatControl::DelayTimeL {
                Channel::Left
            } else {
                Channel::Right
            };
            self.end_sync_gesture(ch, setter);
            return;
        }
        let param = self.float_param(control);
        setter.end_set_parameter(param);
        if self.stereo_link_active() {
            if let Some(other) = self.linked_float(control) {
                setter.end_set_parameter(self.float_param(other));
            }
        }
    }

    fn begin_sync_gesture(&self, ch: Channel, setter: &ParamSetter<'_>) {
        let (note, dev) = self.note_dev_params(ch);
        setter.begin_set_parameter(note);
        setter.begin_set_parameter(dev);
        if self.stereo_link_active() {
            let (note, dev) = self.note_dev_params(ch.other());
            setter.begin_set_parameter(note);
            setter.begin_set_parameter(dev);
        }
    }

    fn end_sync_gesture(&self, ch: Channel, setter: &ParamSetter<'_>) {
        let (note, dev) = self.note_dev_params(ch);
        setter.end_set_parameter(note);
        setter.end_set_parameter(dev);
        if self.stereo_link_active() {
            let (note, dev) = self.note_dev_params(ch.other());
            setter.end_set_parameter(note);
            setter.end_set_parameter(dev);
        }
    }

    fn set_float_display_norm(
        &self,
        control: FloatControl,
        display_norm: f32,
        setter: &ParamSetter<'_>,
    ) {
        let actual_norm = if control.is_lpf_cut() {
            1.0 - display_norm.clamp(0.0, 1.0)
        } else {
            display_norm.clamp(0.0, 1.0)
        };
        let param = self.float_param(control);
        let old_norm = param.modulated_normalized_value();
        let old_plain = param.value();
        let new_plain = param.preview_plain(actual_norm);
        setter.set_parameter(param, new_plain);
        if self.stereo_link_active() {
            if let Some(other) = self.linked_float(control) {
                let other_param = self.float_param(other);
                let other_norm = linked_float_target_norm(
                    other_param,
                    old_plain,
                    new_plain,
                    actual_norm - old_norm,
                );
                setter.set_parameter(other_param, other_param.preview_plain(other_norm));
            }
        }
    }

    fn set_sync_norm(&self, ch: Channel, norm: f32, setter: &ParamSetter<'_>) {
        let (note, dev) = self.note_dev_params(ch);
        let old_ms = synced_delay_ms(note.value(), dev.value());
        let (next_note, next_dev) = sync_from_norm(norm);
        let next_ms = synced_delay_ms(next_note, next_dev);
        setter.set_parameter(note, next_note);
        setter.set_parameter(dev, next_dev);
        if self.stereo_link_active() {
            let (other_note, other_dev) = self.note_dev_params(ch.other());
            let ratio = if old_ms > 0.000_001 {
                (next_ms / old_ms).clamp(0.001, 1000.0)
            } else {
                1.0
            };
            let target_ms = synced_delay_ms(other_note.value(), other_dev.value()) * ratio;
            let (on, od) = sync_from_ms(target_ms);
            setter.set_parameter(other_note, on);
            setter.set_parameter(other_dev, od);
        }
    }

    fn open_numeric_input(&mut self, control: FloatControl, layout: &Layout) {
        let rect = self.value_rect(control, layout).unwrap_or_else(|| {
            UiRect::new(
                layout.full.center_x() - 55.0 * layout.s,
                layout.full.center_y() - 13.0 * layout.s,
                110.0 * layout.s,
                26.0 * layout.s,
            )
        });
        self.numeric_input = Some(NumericInput {
            control,
            text: format_float(self.float_param(control)),
            rect: UiRect::new(
                rect.x,
                rect.y,
                rect.w.max(84.0 * layout.s),
                rect.h.max(24.0 * layout.s),
            ),
        });
        self.text_input = None;
        self.popup = None;
    }

    fn value_rect(&self, control: FloatControl, layout: &Layout) -> Option<UiRect> {
        value_rect_for_control(control, &layout.left, Channel::Left)
            .or_else(|| value_rect_for_control(control, &layout.right, Channel::Right))
            .or_else(|| {
                let g = &layout.global_controls;
                match control {
                    FloatControl::InputLevel => Some(UiRect::new(
                        layout.input_meter.x + 8.0 * layout.s,
                        layout.input_meter.bottom() - 45.0 * layout.s,
                        layout.input_meter.w - 16.0 * layout.s,
                        22.0 * layout.s,
                    )),
                    FloatControl::OutputLevel => Some(UiRect::new(
                        layout.output_meter.x + 8.0 * layout.s,
                        layout.output_meter.bottom() - 45.0 * layout.s,
                        layout.output_meter.w - 16.0 * layout.s,
                        22.0 * layout.s,
                    )),
                    FloatControl::OutputMixL => Some(UiRect::new(
                        g.mix_l_cx - 43.0 * layout.s,
                        g.mix_cy + 39.0 * layout.s,
                        86.0 * layout.s,
                        18.0 * layout.s,
                    )),
                    FloatControl::OutputMixR => Some(UiRect::new(
                        g.mix_r_cx - 43.0 * layout.s,
                        g.mix_cy + 39.0 * layout.s,
                        86.0 * layout.s,
                        18.0 * layout.s,
                    )),
                    _ => None,
                }
            })
    }

    fn commit_active_text(&mut self, setter: &ParamSetter<'_>) {
        if let Some(input) = self.numeric_input.take() {
            let param = self.float_param(input.control);
            match param.string_to_normalized_value(&input.text) {
                Some(norm) => {
                    self.params.push_undo();
                    self.begin_float_gesture(input.control, setter);
                    self.set_float_display_norm(
                        input.control,
                        if input.control.is_lpf_cut() {
                            1.0 - norm
                        } else {
                            norm
                        },
                        setter,
                    );
                    self.end_float_gesture(input.control, setter);
                    self.status = format!("Set {}", param.name());
                }
                None => {
                    self.status = format!("Invalid value for {}", param.name());
                }
            }
        }
        self.text_input = None;
    }

    fn toggle_bool(&mut self, control: BoolControl, setter: &ParamSetter<'_>) {
        self.params.push_undo();
        match control {
            BoolControl::TempoSync => set_bool_value(
                setter,
                &self.params.tempo_sync,
                !self.params.tempo_sync.value(),
            ),
            BoolControl::StereoLink => set_bool_value(
                setter,
                &self.params.stereo_link,
                !self.params.stereo_link.value(),
            ),
            BoolControl::FeedbackPhaseL => set_bool_value(
                setter,
                &self.params.feedback_phase_l,
                !self.params.feedback_phase_l.value(),
            ),
            BoolControl::FeedbackPhaseR => set_bool_value(
                setter,
                &self.params.feedback_phase_r,
                !self.params.feedback_phase_r.value(),
            ),
            BoolControl::CrossfeedPhaseLr => set_bool_value(
                setter,
                &self.params.crossfeed_phase_lr,
                !self.params.crossfeed_phase_lr.value(),
            ),
            BoolControl::CrossfeedPhaseRl => set_bool_value(
                setter,
                &self.params.crossfeed_phase_rl,
                !self.params.crossfeed_phase_rl.value(),
            ),
            BoolControl::Halve
            | BoolControl::Double
            | BoolControl::FeedbackPhase
            | BoolControl::CrossfeedPhase => {}
        }
    }

    fn bool_for_channel(&self, ch: Channel, control: BoolControl) -> bool {
        match (ch, control) {
            (Channel::Left, BoolControl::Halve) => self.params.halve_l.value(),
            (Channel::Right, BoolControl::Halve) => self.params.halve_r.value(),
            (Channel::Left, BoolControl::Double) => self.params.double_l.value(),
            (Channel::Right, BoolControl::Double) => self.params.double_r.value(),
            (Channel::Left, BoolControl::FeedbackPhase) => self.params.feedback_phase_l.value(),
            (Channel::Right, BoolControl::FeedbackPhase) => self.params.feedback_phase_r.value(),
            (Channel::Left, BoolControl::CrossfeedPhase) => self.params.crossfeed_phase_lr.value(),
            (Channel::Right, BoolControl::CrossfeedPhase) => self.params.crossfeed_phase_rl.value(),
            _ => false,
        }
    }

    fn apply_delay_scale(&mut self, ch: Channel, factor: f32, setter: &ParamSetter<'_>) {
        self.params.push_undo();
        if self.params.tempo_sync.value() {
            let (note, dev) = self.note_dev_params(ch);
            let current_ms = synced_delay_ms(note.value(), dev.value());
            let (next_note, next_dev) = sync_from_ms(current_ms * factor);
            set_note_dev(setter, note, dev, next_note, next_dev);
            if self.stereo_link_active() {
                let (note, dev) = self.note_dev_params(ch.other());
                let current_ms = synced_delay_ms(note.value(), dev.value());
                let (next_note, next_dev) = sync_from_ms(current_ms * factor);
                set_note_dev(setter, note, dev, next_note, next_dev);
            }
        } else {
            let control = if ch == Channel::Left {
                FloatControl::DelayTimeL
            } else {
                FloatControl::DelayTimeR
            };
            let param = self.float_param(control);
            set_float_plain(setter, param, (param.value() * factor).clamp(0.005, 2.0));
            if self.stereo_link_active() {
                if let Some(other) = self.linked_float(control) {
                    let param = self.float_param(other);
                    set_float_plain(setter, param, (param.value() * factor).clamp(0.005, 2.0));
                }
            }
        }
        self.status = if factor < 1.0 {
            "Delay halved"
        } else {
            "Delay doubled"
        }
        .to_string();
    }

    fn set_enum_from_index(
        &mut self,
        control: EnumControl,
        index: usize,
        setter: &ParamSetter<'_>,
    ) {
        match control {
            EnumControl::InputL => set_input_value(
                setter,
                &self.params.input_mode_l,
                input_mode_from_index(index),
            ),
            EnumControl::InputR => set_input_value(
                setter,
                &self.params.input_mode_r,
                input_mode_from_index(index),
            ),
            EnumControl::NoteL => set_note_dev(
                setter,
                &self.params.note_l,
                &self.params.deviation_l,
                note_from_index(index),
                self.params.deviation_l.value(),
            ),
            EnumControl::NoteR => set_note_dev(
                setter,
                &self.params.note_r,
                &self.params.deviation_r,
                note_from_index(index),
                self.params.deviation_r.value(),
            ),
            EnumControl::Routing => self.apply_routing_preset(routing_from_index(index), setter),
            EnumControl::Oversampling => set_oversampling_value(
                setter,
                &self.params.oversampling,
                oversampling_from_index(index),
            ),
        }
    }

    fn begin_midi_learn(&mut self, target: MidiTarget) {
        let param_id = target.param_id();
        self.midi_runtime.start_learn(target);
        if let Ok(mut learn) = self.params.midi_learn.write() {
            learn.start_learn(param_id);
        }
        self.midi_learn_target = Some(param_id.to_string());
        self.midi_learn_waiting_for_control = false;
        self.status = format!("Move MIDI CC for {param_id}");
    }

    fn stop_midi_learn(&mut self) {
        self.midi_runtime.stop_learn();
        self.midi_learn_waiting_for_control = false;
        self.midi_learn_target = None;
        if let Ok(mut learn) = self.params.midi_learn.write() {
            learn.stop_learn();
        }
    }

    fn drain_midi_runtime_to_gui(&mut self, setter: &ParamSetter<'_>) {
        if let Some((target, channel, cc, normalized)) = self.midi_runtime.drain_learned_mapping() {
            let param_id = self
                .midi_learn_target
                .take()
                .unwrap_or_else(|| target.param_id().to_string());
            self.midi_learn_waiting_for_control = false;
            if let Ok(mut learn) = self.params.midi_learn.write() {
                learn.assign_mapping(&param_id, channel, cc);
                sync_runtime_from_learn_state(&self.midi_runtime, &learn);
            }
            self.midi_runtime.set_target_value(target, normalized);
            self.status = format!("Mapped {param_id} to Ch {} CC {}", channel + 1, cc);
        }

        for idx in 0..MIDI_TARGET_COUNT {
            if let Some(target) = MidiTarget::from_index(idx) {
                if let Some(normalized) = self.midi_runtime.consume_target_value(target) {
                    self.apply_midi_target_normalized(setter, target, normalized);
                }
            }
        }
    }

    fn apply_midi_target_normalized(
        &mut self,
        setter: &ParamSetter<'_>,
        target: MidiTarget,
        normalized: f32,
    ) {
        macro_rules! set_from_normalized {
            ($param:expr) => {{
                let value = $param.preview_plain(normalized);
                setter.begin_set_parameter($param);
                setter.set_parameter($param, value);
                setter.end_set_parameter($param);
            }};
        }
        match target {
            MidiTarget::InputLevel => set_from_normalized!(&self.params.input_level),
            MidiTarget::OutputLevel => set_from_normalized!(&self.params.output_level),
            MidiTarget::InputModeL => set_from_normalized!(&self.params.input_mode_l),
            MidiTarget::InputModeR => set_from_normalized!(&self.params.input_mode_r),
            MidiTarget::DelayTimeL => set_from_normalized!(&self.params.delay_time_l),
            MidiTarget::DelayTimeR => set_from_normalized!(&self.params.delay_time_r),
            MidiTarget::NoteL => set_from_normalized!(&self.params.note_l),
            MidiTarget::NoteR => set_from_normalized!(&self.params.note_r),
            MidiTarget::DeviationL => set_from_normalized!(&self.params.deviation_l),
            MidiTarget::DeviationR => set_from_normalized!(&self.params.deviation_r),
            MidiTarget::HalveL => set_from_normalized!(&self.params.halve_l),
            MidiTarget::HalveR => set_from_normalized!(&self.params.halve_r),
            MidiTarget::DoubleL => set_from_normalized!(&self.params.double_l),
            MidiTarget::DoubleR => set_from_normalized!(&self.params.double_r),
            MidiTarget::LowCutL => set_from_normalized!(&self.params.low_cut_l),
            MidiTarget::LowCutR => set_from_normalized!(&self.params.low_cut_r),
            MidiTarget::LowCutSlopeL => set_from_normalized!(&self.params.low_cut_slope_l),
            MidiTarget::LowCutSlopeR => set_from_normalized!(&self.params.low_cut_slope_r),
            MidiTarget::HighCutL => set_from_normalized!(&self.params.high_cut_l),
            MidiTarget::HighCutR => set_from_normalized!(&self.params.high_cut_r),
            MidiTarget::HighCutSlopeL => set_from_normalized!(&self.params.high_cut_slope_l),
            MidiTarget::HighCutSlopeR => set_from_normalized!(&self.params.high_cut_slope_r),
            MidiTarget::FeedbackL => set_from_normalized!(&self.params.feedback_l),
            MidiTarget::FeedbackR => set_from_normalized!(&self.params.feedback_r),
            MidiTarget::FeedbackPhaseL => set_from_normalized!(&self.params.feedback_phase_l),
            MidiTarget::FeedbackPhaseR => set_from_normalized!(&self.params.feedback_phase_r),
            MidiTarget::CrossfeedLr => set_from_normalized!(&self.params.crossfeed_lr),
            MidiTarget::CrossfeedRl => set_from_normalized!(&self.params.crossfeed_rl),
            MidiTarget::CrossfeedPhaseLr => set_from_normalized!(&self.params.crossfeed_phase_lr),
            MidiTarget::CrossfeedPhaseRl => set_from_normalized!(&self.params.crossfeed_phase_rl),
            MidiTarget::Routing => {
                self.apply_routing_preset(self.params.routing.preview_plain(normalized), setter)
            }
            MidiTarget::TempoSync => set_from_normalized!(&self.params.tempo_sync),
            MidiTarget::StereoLink => set_from_normalized!(&self.params.stereo_link),
            MidiTarget::OutputMixL => set_from_normalized!(&self.params.output_mix_l),
            MidiTarget::OutputMixR => set_from_normalized!(&self.params.output_mix_r),
            MidiTarget::Oversampling => set_from_normalized!(&self.params.oversampling),
            MidiTarget::Bypass => self.params.set_bypass(normalized >= 0.5),
        }
    }

    fn save_current_preset(&mut self) {
        let name = self.preset_name.trim();
        if name.is_empty() {
            self.status = "Preset name cannot be empty".to_string();
            return;
        }
        let values = preset_values_from_snapshot(&self.params.capture_snapshot());
        self.status = match self.preset_manager.save_user_preset(name, "User", &values) {
            Ok(()) => format!("Saved {name}"),
            Err(err) => err,
        };
    }

    fn save_ab_preset(&mut self, slot: u8) {
        let base_name = self.preset_name.trim();
        if base_name.is_empty() {
            self.status = "Preset name cannot be empty".to_string();
            return;
        }
        let Ok(snapshots) = self.params.ab_snapshots.read() else {
            self.status = "Could not read A/B state".to_string();
            return;
        };
        let (suffix, snapshot) = if slot == 0 {
            ("A", &snapshots.a)
        } else {
            ("B", &snapshots.b)
        };
        let name = format!("{base_name} {suffix}");
        let values = preset_values_from_snapshot(snapshot);
        self.status = match self.preset_manager.save_user_preset(&name, "User", &values) {
            Ok(()) => format!("Saved {name}"),
            Err(err) => err,
        };
    }

    fn apply_routing_preset(&mut self, requested: RoutingModeParam, setter: &ParamSetter<'_>) {
        match requested {
            RoutingModeParam::Customized => {
                set_routing_value(setter, &self.params.routing, RoutingModeParam::Customized);
            }
            RoutingModeParam::Straight => {
                let feedback = mean2(
                    self.params.feedback_l.value(),
                    self.params.feedback_r.value(),
                );
                set_standard_inputs(setter, &self.params);
                set_float_plain(setter, &self.params.feedback_l, feedback);
                set_float_plain(setter, &self.params.feedback_r, feedback);
                set_float_plain(setter, &self.params.crossfeed_lr, 0.0);
                set_float_plain(setter, &self.params.crossfeed_rl, 0.0);
                set_normal_phases(setter, &self.params);
                set_routing_value(setter, &self.params.routing, RoutingModeParam::Straight);
            }
            RoutingModeParam::Crossfeed => {
                let crossfeed = audible_crossfeed(mean2(
                    self.params.crossfeed_lr.value(),
                    self.params.crossfeed_rl.value(),
                ));
                set_standard_inputs(setter, &self.params);
                set_float_plain(setter, &self.params.feedback_l, 0.0);
                set_float_plain(setter, &self.params.feedback_r, 0.0);
                set_float_plain(setter, &self.params.crossfeed_lr, crossfeed);
                set_float_plain(setter, &self.params.crossfeed_rl, crossfeed);
                set_normal_phases(setter, &self.params);
                set_routing_value(setter, &self.params.routing, RoutingModeParam::Crossfeed);
            }
            RoutingModeParam::NinetyTen => {
                let feedback = mean2(
                    self.params.feedback_l.value(),
                    self.params.feedback_r.value(),
                );
                let crossfeed = rounded_tenth_amount(feedback);
                set_standard_inputs(setter, &self.params);
                set_float_plain(setter, &self.params.feedback_l, feedback);
                set_float_plain(setter, &self.params.feedback_r, feedback);
                set_float_plain(setter, &self.params.crossfeed_lr, crossfeed);
                set_float_plain(setter, &self.params.crossfeed_rl, crossfeed);
                set_normal_phases(setter, &self.params);
                set_routing_value(
                    setter,
                    &self.params.routing,
                    if crossfeed <= ROUTE_EPS {
                        RoutingModeParam::Straight
                    } else {
                        RoutingModeParam::NinetyTen
                    },
                );
            }
            RoutingModeParam::TenNinety => {
                let source_crossfeed = mean2(
                    self.params.crossfeed_lr.value(),
                    self.params.crossfeed_rl.value(),
                );
                let feedback = rounded_tenth_amount(source_crossfeed);
                let crossfeed = if feedback <= ROUTE_EPS {
                    0.0
                } else {
                    source_crossfeed
                };
                set_standard_inputs(setter, &self.params);
                set_float_plain(setter, &self.params.feedback_l, feedback);
                set_float_plain(setter, &self.params.feedback_r, feedback);
                set_float_plain(setter, &self.params.crossfeed_lr, crossfeed);
                set_float_plain(setter, &self.params.crossfeed_rl, crossfeed);
                set_normal_phases(setter, &self.params);
                set_routing_value(
                    setter,
                    &self.params.routing,
                    if feedback <= ROUTE_EPS {
                        RoutingModeParam::Straight
                    } else {
                        RoutingModeParam::TenNinety
                    },
                );
            }
            RoutingModeParam::PingPong => {
                let crossfeed = audible_crossfeed(mean2(
                    self.params.crossfeed_lr.value(),
                    self.params.crossfeed_rl.value(),
                ));
                set_input_value(
                    setter,
                    &self.params.input_mode_l,
                    InputModeParam::LeftPlusRight,
                );
                set_input_value(setter, &self.params.input_mode_r, InputModeParam::Off);
                set_float_plain(setter, &self.params.feedback_l, 0.0);
                set_float_plain(setter, &self.params.feedback_r, 0.0);
                set_float_plain(setter, &self.params.crossfeed_lr, crossfeed);
                set_float_plain(setter, &self.params.crossfeed_rl, crossfeed);
                set_normal_phases(setter, &self.params);
                set_routing_value(setter, &self.params.routing, RoutingModeParam::PingPong);
            }
            RoutingModeParam::PingPongR => {
                let crossfeed = audible_crossfeed(mean2(
                    self.params.crossfeed_lr.value(),
                    self.params.crossfeed_rl.value(),
                ));
                set_input_value(setter, &self.params.input_mode_l, InputModeParam::Off);
                set_input_value(
                    setter,
                    &self.params.input_mode_r,
                    InputModeParam::LeftPlusRight,
                );
                set_float_plain(setter, &self.params.feedback_l, 0.0);
                set_float_plain(setter, &self.params.feedback_r, 0.0);
                set_float_plain(setter, &self.params.crossfeed_lr, crossfeed);
                set_float_plain(setter, &self.params.crossfeed_rl, crossfeed);
                set_normal_phases(setter, &self.params);
                set_routing_value(setter, &self.params.routing, RoutingModeParam::PingPongR);
            }
            RoutingModeParam::Pan => {
                let feedback = mean2(
                    self.params.feedback_l.value(),
                    self.params.feedback_r.value(),
                );
                let crossfeed = pan_crossfeed(self.params.crossfeed_lr.value(), feedback);
                set_input_value(
                    setter,
                    &self.params.input_mode_l,
                    InputModeParam::LeftPlusRight,
                );
                set_input_value(setter, &self.params.input_mode_r, InputModeParam::Off);
                set_float_plain(setter, &self.params.feedback_l, feedback);
                set_float_plain(setter, &self.params.feedback_r, feedback);
                set_float_plain(setter, &self.params.crossfeed_lr, crossfeed);
                set_float_plain(setter, &self.params.crossfeed_rl, 0.0);
                set_normal_phases(setter, &self.params);
                set_routing_value(setter, &self.params.routing, RoutingModeParam::Pan);
            }
            RoutingModeParam::PanRl => {
                let feedback = mean2(
                    self.params.feedback_l.value(),
                    self.params.feedback_r.value(),
                );
                let crossfeed = pan_crossfeed(self.params.crossfeed_rl.value(), feedback);
                set_input_value(setter, &self.params.input_mode_l, InputModeParam::Off);
                set_input_value(
                    setter,
                    &self.params.input_mode_r,
                    InputModeParam::LeftPlusRight,
                );
                set_float_plain(setter, &self.params.feedback_l, feedback);
                set_float_plain(setter, &self.params.feedback_r, feedback);
                set_float_plain(setter, &self.params.crossfeed_lr, 0.0);
                set_float_plain(setter, &self.params.crossfeed_rl, crossfeed);
                set_normal_phases(setter, &self.params);
                set_routing_value(setter, &self.params.routing, RoutingModeParam::PanRl);
            }
            RoutingModeParam::Rotate => {
                let amount = mean3(
                    self.params.feedback_r.value(),
                    self.params.crossfeed_lr.value(),
                    self.params.crossfeed_rl.value(),
                );
                set_input_value(setter, &self.params.input_mode_l, InputModeParam::Off);
                set_input_value(
                    setter,
                    &self.params.input_mode_r,
                    InputModeParam::LeftPlusRight,
                );
                set_float_plain(setter, &self.params.feedback_l, 0.0);
                set_float_plain(setter, &self.params.feedback_r, amount);
                set_float_plain(setter, &self.params.crossfeed_lr, amount);
                set_float_plain(setter, &self.params.crossfeed_rl, amount);
                set_bool_value(setter, &self.params.feedback_phase_l, false);
                set_bool_value(setter, &self.params.feedback_phase_r, true);
                set_bool_value(setter, &self.params.crossfeed_phase_lr, true);
                set_bool_value(setter, &self.params.crossfeed_phase_rl, false);
                set_routing_value(setter, &self.params.routing, RoutingModeParam::Rotate);
            }
            RoutingModeParam::RotateR => {
                let amount = mean3(
                    self.params.feedback_l.value(),
                    self.params.crossfeed_lr.value(),
                    self.params.crossfeed_rl.value(),
                );
                set_input_value(
                    setter,
                    &self.params.input_mode_l,
                    InputModeParam::LeftPlusRight,
                );
                set_input_value(setter, &self.params.input_mode_r, InputModeParam::Off);
                set_float_plain(setter, &self.params.feedback_l, amount);
                set_float_plain(setter, &self.params.feedback_r, 0.0);
                set_float_plain(setter, &self.params.crossfeed_lr, amount);
                set_float_plain(setter, &self.params.crossfeed_rl, amount);
                set_bool_value(setter, &self.params.feedback_phase_l, true);
                set_bool_value(setter, &self.params.feedback_phase_r, false);
                set_bool_value(setter, &self.params.crossfeed_phase_lr, false);
                set_bool_value(setter, &self.params.crossfeed_phase_rl, true);
                set_routing_value(setter, &self.params.routing, RoutingModeParam::RotateR);
            }
        }
        self.status = format!("Routing: {}", routing_label(self.params.routing.value()));
    }

    fn sync_routing_display_to_parameters(&mut self, setter: &ParamSetter<'_>) {
        let actual = classify_routing_shape(&self.params);
        if self.params.routing.value() != actual {
            set_routing_value(setter, &self.params.routing, actual);
        }
    }

    fn float_param(&self, control: FloatControl) -> &FloatParam {
        match control {
            FloatControl::InputLevel => &self.params.input_level,
            FloatControl::OutputLevel => &self.params.output_level,
            FloatControl::DelayTimeL => &self.params.delay_time_l,
            FloatControl::DelayTimeR => &self.params.delay_time_r,
            FloatControl::DeviationL => &self.params.deviation_l,
            FloatControl::DeviationR => &self.params.deviation_r,
            FloatControl::LowCutL => &self.params.low_cut_l,
            FloatControl::LowCutR => &self.params.low_cut_r,
            FloatControl::LowCutSlopeL => &self.params.low_cut_slope_l,
            FloatControl::LowCutSlopeR => &self.params.low_cut_slope_r,
            FloatControl::HighCutL => &self.params.high_cut_l,
            FloatControl::HighCutR => &self.params.high_cut_r,
            FloatControl::HighCutSlopeL => &self.params.high_cut_slope_l,
            FloatControl::HighCutSlopeR => &self.params.high_cut_slope_r,
            FloatControl::FeedbackL => &self.params.feedback_l,
            FloatControl::FeedbackR => &self.params.feedback_r,
            FloatControl::CrossfeedLr => &self.params.crossfeed_lr,
            FloatControl::CrossfeedRl => &self.params.crossfeed_rl,
            FloatControl::OutputMixL => &self.params.output_mix_l,
            FloatControl::OutputMixR => &self.params.output_mix_r,
        }
    }

    fn float_display_norm(&self, control: FloatControl) -> f32 {
        let norm = self.float_param(control).modulated_normalized_value();
        if control.is_lpf_cut() {
            1.0 - norm
        } else {
            norm
        }
    }

    fn linked_float(&self, control: FloatControl) -> Option<FloatControl> {
        match control {
            FloatControl::DelayTimeL => Some(FloatControl::DelayTimeR),
            FloatControl::DelayTimeR => Some(FloatControl::DelayTimeL),
            FloatControl::DeviationL => Some(FloatControl::DeviationR),
            FloatControl::DeviationR => Some(FloatControl::DeviationL),
            FloatControl::LowCutL => Some(FloatControl::LowCutR),
            FloatControl::LowCutR => Some(FloatControl::LowCutL),
            FloatControl::LowCutSlopeL => Some(FloatControl::LowCutSlopeR),
            FloatControl::LowCutSlopeR => Some(FloatControl::LowCutSlopeL),
            FloatControl::HighCutL => Some(FloatControl::HighCutR),
            FloatControl::HighCutR => Some(FloatControl::HighCutL),
            FloatControl::HighCutSlopeL => Some(FloatControl::HighCutSlopeR),
            FloatControl::HighCutSlopeR => Some(FloatControl::HighCutSlopeL),
            FloatControl::FeedbackL => Some(FloatControl::FeedbackR),
            FloatControl::FeedbackR => Some(FloatControl::FeedbackL),
            FloatControl::CrossfeedLr => Some(FloatControl::CrossfeedRl),
            FloatControl::CrossfeedRl => Some(FloatControl::CrossfeedLr),
            FloatControl::OutputMixL => Some(FloatControl::OutputMixR),
            FloatControl::OutputMixR => Some(FloatControl::OutputMixL),
            FloatControl::InputLevel | FloatControl::OutputLevel => None,
        }
    }

    fn note_dev_params(
        &self,
        ch: Channel,
    ) -> (
        &nih_plug::params::enums::EnumParam<NoteValueParam>,
        &FloatParam,
    ) {
        match ch {
            Channel::Left => (&self.params.note_l, &self.params.deviation_l),
            Channel::Right => (&self.params.note_r, &self.params.deviation_r),
        }
    }

    fn stereo_link_active(&self) -> bool {
        let ctrl_down = unsafe { (GetKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0 };
        self.params.stereo_link.value() ^ ctrl_down
    }

    fn current_layout(&self) -> Layout {
        let (w, h) = self.logical_client_size().unwrap_or((BASE_W, BASE_H));
        Layout::new(w, h)
    }

    fn ensure_render_target(&mut self) -> Option<ID2D1HwndRenderTarget> {
        if self.render_target.is_none() {
            if self.d2d_factory.is_none() {
                self.d2d_factory = unsafe {
                    D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()
                };
            }
            let factory = self.d2d_factory.as_ref()?;
            let (width, height) = client_size(self.hwnd)?;
            let rt_props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_UNKNOWN,
                    alphaMode: D2D1_ALPHA_MODE_UNKNOWN,
                },
                dpiX: self.render_dpi(),
                dpiY: self.render_dpi(),
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd: self.hwnd,
                pixelSize: D2D_SIZE_U {
                    width: width.max(1),
                    height: height.max(1),
                },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            self.render_target =
                unsafe { factory.CreateHwndRenderTarget(&rt_props, &hwnd_props).ok() };
        }
        self.render_target.clone()
    }

    fn ensure_text_formats(&mut self, scale: f32) -> Option<TextFormats> {
        if self.text_formats.is_none() {
            if self.dwrite_factory.is_none() {
                self.dwrite_factory = unsafe {
                    DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED).ok()
                };
            }
            let factory = self.dwrite_factory.as_ref()?;
            self.text_formats = Some(TextFormats::new(factory, scale)?);
        }
        self.text_formats.clone()
    }

    fn resize_to_parent(&mut self) {
        let _dpi_scope = DpiAwarenessScope::enter();
        self.refresh_dpi();
        let Some((parent_width, parent_height)) = client_size(self.parent_hwnd) else {
            return;
        };
        let Some((current_width, current_height)) = client_size(self.hwnd) else {
            return;
        };
        let (desired_width, desired_height) = self.desired_pixel_size();
        let target_width = parent_width.max(desired_width).max(1);
        let target_height = parent_height.max(desired_height).max(1);
        if target_width == current_width && target_height == current_height {
            return;
        }

        let _ = unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                target_width as i32,
                target_height as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
        self.render_target = None;
        self.text_formats = None;
    }

    fn refresh_dpi(&mut self) {
        let dpi = dpi_for_window(self.hwnd);
        if dpi != self.dpi {
            self.dpi = dpi;
            self.update_size_scale();
            self.render_target = None;
            self.text_formats = None;
        }
    }

    fn handle_dpi_changed(&mut self, dpi: u32) {
        self.dpi = dpi.max(DEFAULT_DPI);
        self.update_size_scale();
        self.render_target = None;
        self.text_formats = None;
        let _ = self.context.request_resize();
        self.resize_to_parent();
        invalidate(self.hwnd);
    }

    fn update_size_scale(&self) {
        let size_scale = (self.render_scale() / self.host_scale.max(0.5)).clamp(1.0, 3.0);
        self.size_scale_bits
            .store(size_scale.to_bits(), Ordering::Release);
    }

    fn render_scale(&self) -> f32 {
        self.host_scale.max(dpi_scale(self.dpi)).clamp(0.5, 3.0)
    }

    fn render_dpi(&self) -> f32 {
        DEFAULT_DPI as f32 * self.render_scale()
    }

    fn desired_pixel_size(&self) -> (u32, u32) {
        let scale = self.render_scale();
        (
            (BASE_W * scale).round().max(1.0) as u32,
            (BASE_H * scale).round().max(1.0) as u32,
        )
    }

    fn logical_client_size(&self) -> Option<(f32, f32)> {
        let scale = self.render_scale();
        client_size(self.hwnd).map(|(width, height)| {
            (
                (width as f32 / scale).max(1.0),
                (height as f32 / scale).max(1.0),
            )
        })
    }

    fn logical_point(&self, x: f32, y: f32) -> (f32, f32) {
        let scale = self.render_scale();
        (x / scale, y / scale)
    }

    fn persist_midi_mapping(&mut self) {
        if let Ok(mut learn) = self.params.midi_learn.write() {
            learn.save_for_rollback();
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Channel {
    Left,
    Right,
}

impl Channel {
    fn other(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FloatControl {
    InputLevel,
    OutputLevel,
    DelayTimeL,
    DelayTimeR,
    DeviationL,
    DeviationR,
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
    CrossfeedLr,
    CrossfeedRl,
    OutputMixL,
    OutputMixR,
}

impl FloatControl {
    fn is_lpf_cut(self) -> bool {
        matches!(self, Self::HighCutL | Self::HighCutR)
    }

    fn is_meter_trim(self) -> bool {
        matches!(self, Self::InputLevel | Self::OutputLevel)
    }

    fn midi_target(self) -> MidiTarget {
        match self {
            Self::InputLevel => MidiTarget::InputLevel,
            Self::OutputLevel => MidiTarget::OutputLevel,
            Self::DelayTimeL => MidiTarget::DelayTimeL,
            Self::DelayTimeR => MidiTarget::DelayTimeR,
            Self::DeviationL => MidiTarget::DeviationL,
            Self::DeviationR => MidiTarget::DeviationR,
            Self::LowCutL => MidiTarget::LowCutL,
            Self::LowCutR => MidiTarget::LowCutR,
            Self::LowCutSlopeL => MidiTarget::LowCutSlopeL,
            Self::LowCutSlopeR => MidiTarget::LowCutSlopeR,
            Self::HighCutL => MidiTarget::HighCutL,
            Self::HighCutR => MidiTarget::HighCutR,
            Self::HighCutSlopeL => MidiTarget::HighCutSlopeL,
            Self::HighCutSlopeR => MidiTarget::HighCutSlopeR,
            Self::FeedbackL => MidiTarget::FeedbackL,
            Self::FeedbackR => MidiTarget::FeedbackR,
            Self::CrossfeedLr => MidiTarget::CrossfeedLr,
            Self::CrossfeedRl => MidiTarget::CrossfeedRl,
            Self::OutputMixL => MidiTarget::OutputMixL,
            Self::OutputMixR => MidiTarget::OutputMixR,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BoolControl {
    TempoSync,
    StereoLink,
    FeedbackPhaseL,
    FeedbackPhaseR,
    CrossfeedPhaseLr,
    CrossfeedPhaseRl,
    Halve,
    Double,
    FeedbackPhase,
    CrossfeedPhase,
}

impl BoolControl {
    fn midi_target(self) -> Option<MidiTarget> {
        match self {
            Self::TempoSync => Some(MidiTarget::TempoSync),
            Self::StereoLink => Some(MidiTarget::StereoLink),
            Self::FeedbackPhaseL | Self::FeedbackPhase => Some(MidiTarget::FeedbackPhaseL),
            Self::FeedbackPhaseR => Some(MidiTarget::FeedbackPhaseR),
            Self::CrossfeedPhaseLr | Self::CrossfeedPhase => Some(MidiTarget::CrossfeedPhaseLr),
            Self::CrossfeedPhaseRl => Some(MidiTarget::CrossfeedPhaseRl),
            Self::Halve | Self::Double => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EnumControl {
    InputL,
    InputR,
    NoteL,
    NoteR,
    Routing,
    Oversampling,
}

impl EnumControl {
    fn midi_target(self) -> MidiTarget {
        match self {
            Self::InputL => MidiTarget::InputModeL,
            Self::InputR => MidiTarget::InputModeR,
            Self::NoteL => MidiTarget::NoteL,
            Self::NoteR => MidiTarget::NoteR,
            Self::Routing => MidiTarget::Routing,
            Self::Oversampling => MidiTarget::Oversampling,
        }
    }
}

#[derive(Clone, Copy)]
enum TopButton {
    Preset,
    Undo,
    Redo,
    Ab,
    Midi,
    Bypass,
}

#[derive(Clone, Copy)]
enum Action {
    Float(FloatControl),
    FloatField(FloatControl),
    Bool(BoolControl),
    DelayScale(Channel, f32),
    Dropdown(EnumControl),
    TopButton(TopButton),
}

impl Action {
    fn midi_target(self) -> Option<MidiTarget> {
        match self {
            Self::Float(control) | Self::FloatField(control) => Some(control.midi_target()),
            Self::Bool(control) => control.midi_target(),
            Self::Dropdown(control) => Some(control.midi_target()),
            Self::DelayScale(Channel::Left, factor) if factor < 1.0 => Some(MidiTarget::HalveL),
            Self::DelayScale(Channel::Right, factor) if factor < 1.0 => Some(MidiTarget::HalveR),
            Self::DelayScale(Channel::Left, _) => Some(MidiTarget::DoubleL),
            Self::DelayScale(Channel::Right, _) => Some(MidiTarget::DoubleR),
            Self::TopButton(TopButton::Bypass) => Some(MidiTarget::Bypass),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
struct HitZone {
    rect: UiRect,
    action: Action,
}

impl HitZone {
    fn new(rect: UiRect, action: Action) -> Self {
        Self { rect, action }
    }
}

#[derive(Clone, Copy)]
struct DragState {
    control: FloatControl,
    start_y: f32,
    start_norm: f32,
}

#[derive(Clone, Copy)]
enum Popup {
    Dropdown {
        control: EnumControl,
        anchor: UiRect,
    },
    Preset,
    Midi,
}

struct NumericInput {
    control: FloatControl,
    text: String,
    rect: UiRect,
}

#[derive(Clone, Copy)]
enum TextInput {
    PresetName,
}

#[derive(Clone, Copy)]
struct Layout {
    full: UiRect,
    header: UiRect,
    footer: UiRect,
    input_meter: UiRect,
    output_meter: UiRect,
    left: ChannelLayout,
    right: ChannelLayout,
    global: UiRect,
    global_controls: GlobalLayout,
    preset_popup: UiRect,
    midi_popup: UiRect,
    s: f32,
}

impl Layout {
    fn new(w: f32, h: f32) -> Self {
        let s = (w / BASE_W).min(h / BASE_H).clamp(0.72, 3.0);
        let full = UiRect::new(0.0, 0.0, w, h);
        let header = UiRect::new(0.0, 0.0, w, 90.0 * s);
        let footer = UiRect::new(0.0, h - 34.0 * s, w, 34.0 * s);
        let y = header.bottom() + 8.0 * s;
        let content_h = (footer.y - y - 8.0 * s).max(420.0 * s);
        let input_meter = UiRect::new(8.0 * s, y, 60.0 * s, content_h);
        let output_meter = UiRect::new(w - 68.0 * s, y, 60.0 * s, content_h);
        let x0 = input_meter.right() + 8.0 * s;
        let x3 = output_meter.x - 8.0 * s;
        let gap = 8.0 * s;
        let channel_w = 292.0 * s;
        let global_w = (x3 - x0 - channel_w * 2.0 - gap * 2.0).max(210.0 * s);
        let left_rect = UiRect::new(x0, y, channel_w, content_h);
        let right_rect = UiRect::new(x0 + channel_w + gap, y, channel_w, content_h);
        let global = UiRect::new(right_rect.right() + gap, y, global_w, content_h);
        let left = ChannelLayout::new(left_rect, s);
        let right = ChannelLayout::new(right_rect, s);
        let global_controls = GlobalLayout::new(global, s);
        let preset_popup = UiRect::new(18.0 * s, 86.0 * s, 360.0 * s, 455.0 * s);
        let midi_popup = UiRect::new(360.0 * s, 86.0 * s, 330.0 * s, 390.0 * s);
        Self {
            full,
            header,
            footer,
            input_meter,
            output_meter,
            left,
            right,
            global,
            global_controls,
            preset_popup,
            midi_popup,
            s,
        }
    }
}

#[derive(Clone, Copy)]
struct ChannelLayout {
    panel: UiRect,
    title: UiRect,
    input_label: UiRect,
    input: UiRect,
    note_label: UiRect,
    note: UiRect,
    delay_label: UiRect,
    delay_cx: f32,
    delay_cy: f32,
    delay_r: f32,
    halve: UiRect,
    double: UiRect,
    deviation_label: UiRect,
    deviation: UiRect,
    filter_x: [f32; 4],
    filter_y: f32,
    feedback_label: UiRect,
    feedback_cx: f32,
    feedback_cy: f32,
    feedback_phase: UiRect,
    crossfeed_label: UiRect,
    crossfeed_cx: f32,
    crossfeed_cy: f32,
    crossfeed_phase: UiRect,
    s: f32,
}

impl ChannelLayout {
    fn new(panel: UiRect, s: f32) -> Self {
        let cx = panel.center_x();
        let delay_cx = cx - 18.0 * s;
        let delay_cy = panel.y + 115.0 * s;
        let delay_r = 38.0 * s;
        let button_w = 30.0 * s;
        let button_h = 19.0 * s;
        let br = delay_r + 20.0 * s;
        let halve_x = delay_cx + ARC_START.cos() * br;
        let double_x = delay_cx + (ARC_START + ARC_SWEEP).cos() * br;
        let button_y = delay_cy + ARC_START.sin() * br;
        let filter_y = panel.y + 265.0 * s;
        let f0 = panel.x + 44.0 * s;
        let fw = 67.0 * s;
        let feedback_cx = panel.x + 82.0 * s;
        let crossfeed_cx = panel.right() - 82.0 * s;
        let row_y = panel.y + 390.0 * s;
        Self {
            panel,
            title: UiRect::new(panel.x, panel.y + 12.0 * s, panel.w, 24.0 * s),
            input_label: UiRect::new(panel.x + 16.0 * s, panel.y + 48.0 * s, 80.0 * s, 14.0 * s),
            input: UiRect::new(panel.x + 16.0 * s, panel.y + 64.0 * s, 88.0 * s, 24.0 * s),
            note_label: UiRect::new(
                panel.right() - 105.0 * s,
                panel.y + 48.0 * s,
                90.0 * s,
                14.0 * s,
            ),
            note: UiRect::new(
                panel.right() - 105.0 * s,
                panel.y + 64.0 * s,
                90.0 * s,
                24.0 * s,
            ),
            delay_label: UiRect::new(delay_cx - 56.0 * s, panel.y + 52.0 * s, 112.0 * s, 15.0 * s),
            delay_cx,
            delay_cy,
            delay_r,
            halve: UiRect::new(
                halve_x - button_w * 0.5,
                button_y - button_h * 0.5,
                button_w,
                button_h,
            ),
            double: UiRect::new(
                double_x - button_w * 0.5,
                button_y - button_h * 0.5,
                button_w,
                button_h,
            ),
            deviation_label: UiRect::new(
                panel.right() - 107.0 * s,
                panel.y + 100.0 * s,
                92.0 * s,
                14.0 * s,
            ),
            deviation: UiRect::new(
                panel.right() - 111.0 * s,
                panel.y + 116.0 * s,
                98.0 * s,
                22.0 * s,
            ),
            filter_x: [f0, f0 + fw, f0 + fw * 2.0, f0 + fw * 3.0],
            filter_y,
            feedback_label: UiRect::new(
                feedback_cx - 55.0 * s,
                row_y - 54.0 * s,
                110.0 * s,
                14.0 * s,
            ),
            feedback_cx,
            feedback_cy: row_y,
            feedback_phase: UiRect::new(
                feedback_cx - 38.0 * s,
                row_y + 66.0 * s,
                76.0 * s,
                22.0 * s,
            ),
            crossfeed_label: UiRect::new(
                crossfeed_cx - 66.0 * s,
                row_y - 54.0 * s,
                132.0 * s,
                14.0 * s,
            ),
            crossfeed_cx,
            crossfeed_cy: row_y,
            crossfeed_phase: UiRect::new(
                crossfeed_cx - 38.0 * s,
                row_y + 66.0 * s,
                76.0 * s,
                22.0 * s,
            ),
            s,
        }
    }
}

#[derive(Clone, Copy)]
struct GlobalLayout {
    title: UiRect,
    routing_label: UiRect,
    routing: UiRect,
    os_label: UiRect,
    oversampling: UiRect,
    sync: UiRect,
    link: UiRect,
    mix_title: UiRect,
    mix_l_label: UiRect,
    mix_r_label: UiRect,
    mix_l_cx: f32,
    mix_r_cx: f32,
    mix_cy: f32,
}

impl GlobalLayout {
    fn new(panel: UiRect, s: f32) -> Self {
        let cx = panel.center_x();
        Self {
            title: UiRect::new(panel.x, panel.y + 12.0 * s, panel.w, 24.0 * s),
            routing_label: UiRect::new(
                panel.x + 20.0 * s,
                panel.y + 58.0 * s,
                panel.w - 40.0 * s,
                14.0 * s,
            ),
            routing: UiRect::new(
                panel.x + 28.0 * s,
                panel.y + 76.0 * s,
                panel.w - 56.0 * s,
                26.0 * s,
            ),
            os_label: UiRect::new(
                panel.x + 20.0 * s,
                panel.y + 116.0 * s,
                panel.w - 40.0 * s,
                14.0 * s,
            ),
            oversampling: UiRect::new(
                panel.x + 28.0 * s,
                panel.y + 134.0 * s,
                panel.w - 56.0 * s,
                26.0 * s,
            ),
            sync: UiRect::new(
                panel.x + 28.0 * s,
                panel.y + 178.0 * s,
                panel.w - 56.0 * s,
                25.0 * s,
            ),
            link: UiRect::new(
                panel.x + 28.0 * s,
                panel.y + 212.0 * s,
                panel.w - 56.0 * s,
                25.0 * s,
            ),
            mix_title: UiRect::new(panel.x, panel.y + 280.0 * s, panel.w, 24.0 * s),
            mix_l_label: UiRect::new(cx - 78.0 * s, panel.y + 320.0 * s, 58.0 * s, 14.0 * s),
            mix_r_label: UiRect::new(cx + 20.0 * s, panel.y + 320.0 * s, 58.0 * s, 14.0 * s),
            mix_l_cx: cx - 49.0 * s,
            mix_r_cx: cx + 49.0 * s,
            mix_cy: panel.y + 370.0 * s,
        }
    }
}

#[derive(Clone, Copy)]
struct UiRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl UiRect {
    const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    fn right(self) -> f32 {
        self.x + self.w
    }

    fn bottom(self) -> f32 {
        self.y + self.h
    }

    fn center_x(self) -> f32 {
        self.x + self.w * 0.5
    }

    fn center_y(self) -> f32 {
        self.y + self.h * 0.5
    }

    fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.right() && y >= self.y && y <= self.bottom()
    }

    fn shrink(self, amount: f32) -> Self {
        Self::new(
            self.x + amount,
            self.y + amount,
            (self.w - amount * 2.0).max(1.0),
            (self.h - amount * 2.0).max(1.0),
        )
    }

    fn d2d(self) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.x,
            top: self.y,
            right: self.right(),
            bottom: self.bottom(),
        }
    }
}

#[derive(Clone)]
struct TextFormats {
    small: IDWriteTextFormat,
    body: IDWriteTextFormat,
    title: IDWriteTextFormat,
}

impl TextFormats {
    fn new(factory: &IDWriteFactory, scale: f32) -> Option<Self> {
        let s = scale.clamp(0.7, 3.0);
        Some(Self {
            small: create_text_format(factory, 11.0 * s, false)?,
            body: create_text_format(factory, 13.0 * s, false)?,
            title: create_text_format(factory, 18.0 * s, true)?,
        })
    }
}

fn create_text_format(
    factory: &IDWriteFactory,
    size: f32,
    bold: bool,
) -> Option<IDWriteTextFormat> {
    let format = unsafe {
        factory
            .CreateTextFormat(
                w!("Segoe UI"),
                Option::<&IDWriteFontCollection>::None,
                if bold {
                    DWRITE_FONT_WEIGHT_DEMI_BOLD
                } else {
                    DWRITE_FONT_WEIGHT_NORMAL
                },
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("en-us"),
            )
            .ok()?
    };
    let _ = unsafe { format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING) };
    let _ = unsafe { format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER) };
    Some(format)
}

struct Brushes {
    bg: ID2D1SolidColorBrush,
    top: ID2D1SolidColorBrush,
    footer: ID2D1SolidColorBrush,
    panel: ID2D1SolidColorBrush,
    card: ID2D1SolidColorBrush,
    control: ID2D1SolidColorBrush,
    border: ID2D1SolidColorBrush,
    divider: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    accent_dark: ID2D1SolidColorBrush,
    accent_light: ID2D1SolidColorBrush,
    accent_soft: ID2D1SolidColorBrush,
    orange: ID2D1SolidColorBrush,
    magenta: ID2D1SolidColorBrush,
    purple: ID2D1SolidColorBrush,
    green: ID2D1SolidColorBrush,
    text_light: ID2D1SolidColorBrush,
    text_primary: ID2D1SolidColorBrush,
    text_secondary: ID2D1SolidColorBrush,
}

impl Brushes {
    fn new(rt: &ID2D1HwndRenderTarget) -> Option<Self> {
        Some(Self {
            bg: solid(rt, Colors::BG)?,
            top: solid(rt, Colors::TOP)?,
            footer: solid(rt, Colors::FOOTER)?,
            panel: solid(rt, Colors::PANEL)?,
            card: solid(rt, Colors::CARD)?,
            control: solid(rt, Colors::CONTROL)?,
            border: solid(rt, Colors::BORDER)?,
            divider: solid(rt, Colors::DIVIDER)?,
            accent: solid(rt, Colors::ACCENT)?,
            accent_dark: solid(rt, Colors::ACCENT_DARK)?,
            accent_light: solid(rt, Colors::ACCENT_LIGHT)?,
            accent_soft: solid(rt, Colors::ACCENT_SOFT)?,
            orange: solid(rt, Colors::ORANGE)?,
            magenta: solid(rt, Colors::MAGENTA)?,
            purple: solid(rt, Colors::PURPLE)?,
            green: solid(rt, Colors::GREEN)?,
            text_light: solid(rt, Colors::TEXT_LIGHT)?,
            text_primary: solid(rt, Colors::TEXT_PRIMARY)?,
            text_secondary: solid(rt, Colors::TEXT_SECONDARY)?,
        })
    }
}

struct Colors;

impl Colors {
    const BG: D2D1_COLOR_F = color(4, 2, 14, 255);
    const TOP: D2D1_COLOR_F = color(9, 6, 20, 255);
    const FOOTER: D2D1_COLOR_F = color(20, 18, 27, 255);
    const PANEL: D2D1_COLOR_F = color(11, 7, 30, 255);
    const CARD: D2D1_COLOR_F = color(15, 9, 38, 255);
    const CONTROL: D2D1_COLOR_F = color(19, 12, 46, 255);
    const BORDER: D2D1_COLOR_F = color(55, 32, 102, 255);
    const DIVIDER: D2D1_COLOR_F = color(36, 24, 74, 255);
    const ACCENT: D2D1_COLOR_F = color(0, 218, 255, 255);
    const ACCENT_DARK: D2D1_COLOR_F = color(0, 96, 140, 255);
    const ACCENT_LIGHT: D2D1_COLOR_F = color(100, 240, 255, 255);
    const ACCENT_SOFT: D2D1_COLOR_F = color(0, 218, 255, 42);
    const ORANGE: D2D1_COLOR_F = color(255, 170, 0, 255);
    const MAGENTA: D2D1_COLOR_F = color(255, 32, 200, 255);
    const PURPLE: D2D1_COLOR_F = color(160, 82, 255, 255);
    const GREEN: D2D1_COLOR_F = color(75, 255, 116, 255);
    const TEXT_LIGHT: D2D1_COLOR_F = color(250, 255, 255, 255);
    const TEXT_PRIMARY: D2D1_COLOR_F = color(214, 236, 255, 255);
    const TEXT_SECONDARY: D2D1_COLOR_F = color(112, 150, 210, 255);
}

const fn color(r: u8, g: u8, b: u8, a: u8) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: a as f32 / 255.0,
    }
}

fn card(rt: &ID2D1HwndRenderTarget, rect: UiRect, radius: f32, brushes: &Brushes) {
    fill_round(rt, rect, radius, &brushes.panel);
    draw_line(
        rt,
        rect.x + radius,
        rect.y,
        rect.right() - radius,
        rect.y,
        &brushes.accent_dark,
        1.0,
    );
    stroke_round(rt, rect, radius, &brushes.border, 1.0);
}

fn solid(rt: &ID2D1HwndRenderTarget, color: D2D1_COLOR_F) -> Option<ID2D1SolidColorBrush> {
    unsafe { rt.CreateSolidColorBrush(&color, None).ok() }
}

fn fill_rect(rt: &ID2D1HwndRenderTarget, rect: UiRect, brush: &ID2D1SolidColorBrush) {
    unsafe {
        rt.FillRectangle(&rect.d2d(), brush);
    }
}

fn fill_round(rt: &ID2D1HwndRenderTarget, rect: UiRect, radius: f32, brush: &ID2D1SolidColorBrush) {
    let rr = D2D1_ROUNDED_RECT {
        rect: rect.d2d(),
        radiusX: radius,
        radiusY: radius,
    };
    unsafe {
        rt.FillRoundedRectangle(&rr, brush);
    }
}

fn stroke_round(
    rt: &ID2D1HwndRenderTarget,
    rect: UiRect,
    radius: f32,
    brush: &ID2D1SolidColorBrush,
    width: f32,
) {
    let rr = D2D1_ROUNDED_RECT {
        rect: rect.d2d(),
        radiusX: radius,
        radiusY: radius,
    };
    unsafe {
        rt.DrawRoundedRectangle(
            &rr,
            brush,
            width,
            Option::<&windows::Win32::Graphics::Direct2D::ID2D1StrokeStyle>::None,
        );
    }
}

fn draw_line(
    rt: &ID2D1HwndRenderTarget,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    brush: &ID2D1SolidColorBrush,
    width: f32,
) {
    unsafe {
        rt.DrawLine(
            Vector2 { X: x0, Y: y0 },
            Vector2 { X: x1, Y: y1 },
            brush,
            width,
            Option::<&windows::Win32::Graphics::Direct2D::ID2D1StrokeStyle>::None,
        );
    }
}

fn fill_circle(
    rt: &ID2D1HwndRenderTarget,
    x: f32,
    y: f32,
    radius: f32,
    brush: &ID2D1SolidColorBrush,
) {
    let ellipse = D2D1_ELLIPSE {
        point: Vector2 { X: x, Y: y },
        radiusX: radius,
        radiusY: radius,
    };
    unsafe {
        rt.FillEllipse(&ellipse, brush);
    }
}

fn stroke_circle(
    rt: &ID2D1HwndRenderTarget,
    x: f32,
    y: f32,
    radius: f32,
    brush: &ID2D1SolidColorBrush,
    width: f32,
) {
    let ellipse = D2D1_ELLIPSE {
        point: Vector2 { X: x, Y: y },
        radiusX: radius,
        radiusY: radius,
    };
    unsafe {
        rt.DrawEllipse(
            &ellipse,
            brush,
            width,
            Option::<&windows::Win32::Graphics::Direct2D::ID2D1StrokeStyle>::None,
        );
    }
}

fn draw_arc(
    rt: &ID2D1HwndRenderTarget,
    cx: f32,
    cy: f32,
    radius: f32,
    start: f32,
    end: f32,
    brush: &ID2D1SolidColorBrush,
    width: f32,
) {
    let steps = 40;
    let mut prev = None;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let angle = start + (end - start) * t;
        let point = (cx + radius * angle.cos(), cy + radius * angle.sin());
        if let Some((px, py)) = prev {
            draw_line(rt, px, py, point.0, point.1, brush, width);
        }
        prev = Some(point);
    }
}

fn draw_knob(
    rt: &ID2D1HwndRenderTarget,
    cx: f32,
    cy: f32,
    radius: f32,
    norm: f32,
    accent: &ID2D1SolidColorBrush,
    brushes: &Brushes,
    s: f32,
) {
    let norm = norm.clamp(0.0, 1.0);
    let angle = ARC_START + ARC_SWEEP * norm;
    fill_circle(rt, cx, cy + 1.5 * s, radius + 2.0 * s, &brushes.bg);
    fill_circle(rt, cx, cy, radius, &brushes.control);
    stroke_circle(rt, cx, cy, radius, &brushes.border, 1.0 * s);
    draw_arc(
        rt,
        cx,
        cy,
        radius * 0.72,
        ARC_START,
        ARC_START + ARC_SWEEP,
        &brushes.border,
        3.3 * s,
    );
    draw_arc(rt, cx, cy, radius * 0.72, ARC_START, angle, accent, 3.0 * s);
    let dot_x = cx + radius * 0.52 * angle.cos();
    let dot_y = cy + radius * 0.52 * angle.sin();
    fill_circle(rt, dot_x, dot_y, 4.2 * s, accent);
    fill_circle(rt, dot_x, dot_y, 1.7 * s, &brushes.text_light);
}

fn draw_note_ring(
    rt: &ID2D1HwndRenderTarget,
    cx: f32,
    cy: f32,
    radius: f32,
    brushes: &Brushes,
    s: f32,
) {
    let labels = ["1/64", "1/32", "1/16", "1/8", "1/4", "1/2", "1/1"];
    for (idx, _) in labels.iter().enumerate() {
        let t = idx as f32 / (labels.len() - 1) as f32;
        let angle = ARC_START + ARC_SWEEP * t;
        let x = cx + radius * angle.cos();
        let y = cy + radius * angle.sin();
        fill_circle(rt, x, y, 1.8 * s, &brushes.text_secondary);
    }
}

fn draw_button(
    rt: &ID2D1HwndRenderTarget,
    rect: UiRect,
    label: &str,
    active: bool,
    brushes: &Brushes,
    formats: &TextFormats,
    s: f32,
) {
    let fill = if active {
        &brushes.accent
    } else {
        &brushes.control
    };
    let text = if active {
        &brushes.bg
    } else {
        &brushes.text_primary
    };
    fill_round(rt, rect, 4.0 * s, fill);
    stroke_round(
        rt,
        rect,
        4.0 * s,
        if active {
            &brushes.accent_light
        } else {
            &brushes.border
        },
        1.0 * s,
    );
    draw_text(rt, label, rect, &formats.small, text, Align::Center);
}

fn draw_value_box(
    rt: &ID2D1HwndRenderTarget,
    rect: UiRect,
    label: &str,
    brushes: &Brushes,
    formats: &TextFormats,
) {
    fill_round(rt, rect, 4.0, &brushes.bg);
    stroke_round(rt, rect, 4.0, &brushes.border, 1.0);
    draw_text(
        rt,
        label,
        rect,
        &formats.small,
        &brushes.text_primary,
        Align::Center,
    );
}

enum Align {
    Leading,
    Center,
    Trailing,
}

fn draw_text(
    rt: &ID2D1HwndRenderTarget,
    text: &str,
    rect: UiRect,
    format: &IDWriteTextFormat,
    brush: &ID2D1SolidColorBrush,
    align: Align,
) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    if wide.is_empty() {
        return;
    }
    let alignment = match align {
        Align::Leading => DWRITE_TEXT_ALIGNMENT_LEADING,
        Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
        Align::Trailing => DWRITE_TEXT_ALIGNMENT_TRAILING,
    };
    let _ = unsafe { format.SetTextAlignment(alignment) };
    let _ = unsafe { format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER) };
    unsafe {
        rt.DrawText(
            &wide,
            format,
            &rect.d2d(),
            brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }
}

fn channel_zones(zones: &mut Vec<HitZone>, layout: &ChannelLayout, ch: Channel) {
    let (delay, dev, input, note, feedback, cross, fb_phase, cf_phase, halve, double) = match ch {
        Channel::Left => (
            FloatControl::DelayTimeL,
            FloatControl::DeviationL,
            EnumControl::InputL,
            EnumControl::NoteL,
            FloatControl::FeedbackL,
            FloatControl::CrossfeedLr,
            BoolControl::FeedbackPhaseL,
            BoolControl::CrossfeedPhaseLr,
            Action::DelayScale(Channel::Left, 0.5),
            Action::DelayScale(Channel::Left, 2.0),
        ),
        Channel::Right => (
            FloatControl::DelayTimeR,
            FloatControl::DeviationR,
            EnumControl::InputR,
            EnumControl::NoteR,
            FloatControl::FeedbackR,
            FloatControl::CrossfeedRl,
            BoolControl::FeedbackPhaseR,
            BoolControl::CrossfeedPhaseRl,
            Action::DelayScale(Channel::Right, 0.5),
            Action::DelayScale(Channel::Right, 2.0),
        ),
    };
    zones.push(HitZone::new(layout.input, Action::Dropdown(input)));
    zones.push(HitZone::new(layout.note, Action::Dropdown(note)));
    zones.push(HitZone::new(
        knob_rect(
            layout.delay_cx,
            layout.delay_cy,
            layout.delay_r + 18.0 * layout.s,
        ),
        Action::Float(delay),
    ));
    zones.push(HitZone::new(layout.halve, halve));
    zones.push(HitZone::new(layout.double, double));
    zones.push(HitZone::new(layout.deviation, Action::FloatField(dev)));

    let filters = match ch {
        Channel::Left => [
            FloatControl::LowCutL,
            FloatControl::LowCutSlopeL,
            FloatControl::HighCutL,
            FloatControl::HighCutSlopeL,
        ],
        Channel::Right => [
            FloatControl::LowCutR,
            FloatControl::LowCutSlopeR,
            FloatControl::HighCutR,
            FloatControl::HighCutSlopeR,
        ],
    };
    for (idx, control) in filters.iter().enumerate() {
        zones.push(HitZone::new(
            knob_rect(layout.filter_x[idx], layout.filter_y, 33.0 * layout.s),
            Action::Float(*control),
        ));
        zones.push(HitZone::new(
            UiRect::new(
                layout.filter_x[idx] - 37.0 * layout.s,
                layout.filter_y + 29.0 * layout.s,
                74.0 * layout.s,
                18.0 * layout.s,
            ),
            Action::FloatField(*control),
        ));
    }
    zones.push(HitZone::new(
        knob_rect(layout.feedback_cx, layout.feedback_cy, 43.0 * layout.s),
        Action::Float(feedback),
    ));
    zones.push(HitZone::new(
        UiRect::new(
            layout.feedback_cx - 44.0 * layout.s,
            layout.feedback_cy + 39.0 * layout.s,
            88.0 * layout.s,
            18.0 * layout.s,
        ),
        Action::FloatField(feedback),
    ));
    zones.push(HitZone::new(layout.feedback_phase, Action::Bool(fb_phase)));
    zones.push(HitZone::new(
        knob_rect(layout.crossfeed_cx, layout.crossfeed_cy, 43.0 * layout.s),
        Action::Float(cross),
    ));
    zones.push(HitZone::new(
        UiRect::new(
            layout.crossfeed_cx - 44.0 * layout.s,
            layout.crossfeed_cy + 39.0 * layout.s,
            88.0 * layout.s,
            18.0 * layout.s,
        ),
        Action::FloatField(cross),
    ));
    zones.push(HitZone::new(layout.crossfeed_phase, Action::Bool(cf_phase)));
}

fn knob_rect(cx: f32, cy: f32, radius: f32) -> UiRect {
    UiRect::new(cx - radius, cy - radius, radius * 2.0, radius * 2.0)
}

fn meter_rail(rect: UiRect, s: f32) -> UiRect {
    UiRect::new(
        rect.center_x() - 10.0 * s,
        rect.y + 74.0 * s,
        20.0 * s,
        rect.h - 150.0 * s,
    )
}

fn value_rect_for_control(
    control: FloatControl,
    layout: &ChannelLayout,
    ch: Channel,
) -> Option<UiRect> {
    let s = layout.s;
    match (control, ch) {
        (FloatControl::DeviationL, Channel::Left) | (FloatControl::DeviationR, Channel::Right) => {
            Some(layout.deviation)
        }
        (FloatControl::FeedbackL, Channel::Left) | (FloatControl::FeedbackR, Channel::Right) => {
            Some(UiRect::new(
                layout.feedback_cx - 44.0 * s,
                layout.feedback_cy + 39.0 * s,
                88.0 * s,
                18.0 * s,
            ))
        }
        (FloatControl::CrossfeedLr, Channel::Left)
        | (FloatControl::CrossfeedRl, Channel::Right) => Some(UiRect::new(
            layout.crossfeed_cx - 44.0 * s,
            layout.crossfeed_cy + 39.0 * s,
            88.0 * s,
            18.0 * s,
        )),
        _ => {
            let controls = match ch {
                Channel::Left => [
                    FloatControl::LowCutL,
                    FloatControl::LowCutSlopeL,
                    FloatControl::HighCutL,
                    FloatControl::HighCutSlopeL,
                ],
                Channel::Right => [
                    FloatControl::LowCutR,
                    FloatControl::LowCutSlopeR,
                    FloatControl::HighCutR,
                    FloatControl::HighCutSlopeR,
                ],
            };
            controls.iter().position(|c| *c == control).map(|idx| {
                UiRect::new(
                    layout.filter_x[idx] - 37.0 * s,
                    layout.filter_y + 29.0 * s,
                    74.0 * s,
                    18.0 * s,
                )
            })
        }
    }
}

fn format_float(param: &FloatParam) -> String {
    param.normalized_value_to_string(param.modulated_normalized_value(), true)
}

fn dropdown_items(control: EnumControl) -> Vec<(usize, &'static str)> {
    match control {
        EnumControl::InputL | EnumControl::InputR => vec![
            (0, "Off"),
            (1, "Left"),
            (2, "Right"),
            (3, "L+R"),
            (4, "L-R"),
        ],
        EnumControl::NoteL | EnumControl::NoteR => note_variants()
            .into_iter()
            .map(|(variant, label)| (note_to_index(variant), label))
            .collect(),
        EnumControl::Routing => vec![
            (0, "Customized"),
            (1, "Straight"),
            (2, "Crossfeed"),
            (3, "90/10"),
            (4, "10/90"),
            (5, "Ping Pong L"),
            (8, "Ping Pong R"),
            (6, "Pan L to R"),
            (9, "Pan R to L"),
            (7, "Rotate L"),
            (10, "Rotate R"),
        ],
        EnumControl::Oversampling => vec![(0, "Off"), (1, "2x"), (2, "4x"), (3, "6x"), (4, "8x")],
    }
}

fn input_mode_label(value: InputModeParam) -> &'static str {
    match value {
        InputModeParam::Off => "Off",
        InputModeParam::Left => "Left",
        InputModeParam::Right => "Right",
        InputModeParam::LeftPlusRight => "L+R",
        InputModeParam::LeftMinusRight => "L-R",
    }
}

fn note_label(value: NoteValueParam) -> &'static str {
    note_variants()
        .iter()
        .find(|(variant, _)| *variant == value)
        .map(|(_, label)| *label)
        .unwrap_or("1/4")
}

fn oversampling_label(value: OversamplingParam) -> &'static str {
    match value {
        OversamplingParam::Off => "Off",
        OversamplingParam::TwoX => "2x",
        OversamplingParam::FourX => "4x",
        OversamplingParam::SixX => "6x",
        OversamplingParam::EightX => "8x",
    }
}

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
        12 => NoteValueParam::HalfDotted,
        13 => NoteValueParam::QuarterDotted,
        14 => NoteValueParam::EighthDotted,
        15 => NoteValueParam::SixteenthDotted,
        16 => NoteValueParam::ThirtySecondDotted,
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
        NoteValueParam::HalfDotted => 12,
        NoteValueParam::QuarterDotted => 13,
        NoteValueParam::EighthDotted => 14,
        NoteValueParam::SixteenthDotted => 15,
        NoteValueParam::ThirtySecondDotted => 16,
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
        8 => RoutingModeParam::PingPongR,
        9 => RoutingModeParam::PanRl,
        10 => RoutingModeParam::RotateR,
        _ => RoutingModeParam::Customized,
    }
}

fn routing_label(value: RoutingModeParam) -> &'static str {
    match value {
        RoutingModeParam::Customized => "Customized",
        RoutingModeParam::Straight => "Straight",
        RoutingModeParam::Crossfeed => "Crossfeed",
        RoutingModeParam::NinetyTen => "90/10",
        RoutingModeParam::TenNinety => "10/90",
        RoutingModeParam::PingPong => "Ping Pong L",
        RoutingModeParam::Pan => "Pan L to R",
        RoutingModeParam::Rotate => "Rotate L",
        RoutingModeParam::PingPongR => "Ping Pong R",
        RoutingModeParam::PanRl => "Pan R to L",
        RoutingModeParam::RotateR => "Rotate R",
    }
}

fn oversampling_from_index(idx: usize) -> OversamplingParam {
    match idx {
        0 => OversamplingParam::Off,
        1 => OversamplingParam::TwoX,
        2 => OversamplingParam::FourX,
        3 => OversamplingParam::SixX,
        4 => OversamplingParam::EightX,
        _ => OversamplingParam::Off,
    }
}

fn note_variants() -> [(NoteValueParam, &'static str); 17] {
    [
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
        (NoteValueParam::HalfDotted, "1/2."),
        (NoteValueParam::QuarterDotted, "1/4."),
        (NoteValueParam::EighthDotted, "1/8."),
        (NoteValueParam::SixteenthDotted, "1/16."),
        (NoteValueParam::ThirtySecondDotted, "1/32."),
    ]
}

fn sync_knob_normalized(note: NoteValueParam, deviation: f32) -> f32 {
    let max = (note_variants().len() - 1) as f32;
    let idx = note_variants()
        .iter()
        .position(|(variant, _)| *variant == note)
        .unwrap_or(3) as f32;
    ((idx + deviation.clamp(-100.0, 100.0) / 200.0) / max).clamp(0.0, 1.0)
}

fn sync_from_norm(norm: f32) -> (NoteValueParam, f32) {
    let max = (note_variants().len() - 1) as f32;
    let pos = norm.clamp(0.0, 1.0) * max;
    let idx = pos.round().clamp(0.0, max) as usize;
    let deviation = ((pos - idx as f32) * 200.0).clamp(-100.0, 100.0);
    (note_variants()[idx].0, deviation)
}

fn synced_delay_ms(note: NoteValueParam, deviation: f32) -> f32 {
    let base = NoteValue::from(note).duration_seconds(120.0) as f32;
    let factor = 2.0_f32.powf(deviation.clamp(-100.0, 100.0) / 1200.0);
    (base * factor * 1000.0).clamp(5.0, 2000.0)
}

fn sync_from_ms(target_ms: f32) -> (NoteValueParam, f32) {
    let target_ms = target_ms.clamp(5.0, 2000.0);
    let mut best = (NoteValueParam::Quarter, 0.0_f32);
    let mut best_error = f32::INFINITY;
    for (variant, _) in note_variants() {
        let base_ms = NoteValue::from(variant).duration_seconds(120.0) as f32 * 1000.0;
        let cents = 1200.0 * (target_ms / base_ms.max(0.000_001)).log2();
        let deviation = cents.clamp(-100.0, 100.0);
        let candidate_ms = synced_delay_ms(variant, deviation);
        let error = (target_ms / candidate_ms.max(0.000_001)).ln().abs();
        if error < best_error {
            best_error = error;
            best = (variant, deviation);
        }
    }
    best
}

fn linked_float_target_norm(
    other: &FloatParam,
    old_plain: f32,
    new_plain: f32,
    delta_norm: f32,
) -> f32 {
    let other_plain = other.value();
    if old_plain.abs() > 0.000_001
        && old_plain.is_finite()
        && new_plain.is_finite()
        && other_plain.is_finite()
        && old_plain >= 0.0
        && new_plain >= 0.0
        && other_plain >= 0.0
    {
        let ratio = (new_plain / old_plain).clamp(0.0, 1000.0);
        other.preview_normalized(other_plain * ratio)
    } else {
        (other.modulated_normalized_value() + delta_norm).clamp(0.0, 1.0)
    }
}

fn set_float_plain(setter: &ParamSetter<'_>, param: &FloatParam, value: f32) {
    setter.begin_set_parameter(param);
    setter.set_parameter(param, value);
    setter.end_set_parameter(param);
}

fn set_bool_value(setter: &ParamSetter<'_>, param: &BoolParam, value: bool) {
    setter.begin_set_parameter(param);
    setter.set_parameter(param, value);
    setter.end_set_parameter(param);
}

fn set_input_value(
    setter: &ParamSetter<'_>,
    param: &nih_plug::params::enums::EnumParam<InputModeParam>,
    value: InputModeParam,
) {
    setter.begin_set_parameter(param);
    setter.set_parameter(param, value);
    setter.end_set_parameter(param);
}

fn set_note_dev(
    setter: &ParamSetter<'_>,
    note: &nih_plug::params::enums::EnumParam<NoteValueParam>,
    dev: &FloatParam,
    note_value: NoteValueParam,
    dev_value: f32,
) {
    setter.begin_set_parameter(note);
    setter.set_parameter(note, note_value);
    setter.end_set_parameter(note);
    setter.begin_set_parameter(dev);
    setter.set_parameter(dev, dev_value.clamp(-100.0, 100.0));
    setter.end_set_parameter(dev);
}

fn set_routing_value(
    setter: &ParamSetter<'_>,
    param: &nih_plug::params::enums::EnumParam<RoutingModeParam>,
    value: RoutingModeParam,
) {
    setter.begin_set_parameter(param);
    setter.set_parameter(param, value);
    setter.end_set_parameter(param);
}

fn set_oversampling_value(
    setter: &ParamSetter<'_>,
    param: &nih_plug::params::enums::EnumParam<OversamplingParam>,
    value: OversamplingParam,
) {
    setter.begin_set_parameter(param);
    setter.set_parameter(param, value);
    setter.end_set_parameter(param);
}

fn set_standard_inputs(setter: &ParamSetter<'_>, params: &NebulaStereoDelayParams) {
    set_input_value(setter, &params.input_mode_l, InputModeParam::Left);
    set_input_value(setter, &params.input_mode_r, InputModeParam::Right);
}

fn set_normal_phases(setter: &ParamSetter<'_>, params: &NebulaStereoDelayParams) {
    set_bool_value(setter, &params.feedback_phase_l, false);
    set_bool_value(setter, &params.feedback_phase_r, false);
    set_bool_value(setter, &params.crossfeed_phase_lr, false);
    set_bool_value(setter, &params.crossfeed_phase_rl, false);
}

fn mean2(a: f32, b: f32) -> f32 {
    ((a + b) * 0.5).clamp(0.0, 1.0)
}

fn mean3(a: f32, b: f32, c: f32) -> f32 {
    ((a + b + c) / 3.0).clamp(0.0, 1.0)
}

fn audible_crossfeed(value: f32) -> f32 {
    if value <= ROUTE_EPS {
        0.5
    } else {
        value.clamp(0.01, 1.0)
    }
}

fn pan_crossfeed(current: f32, feedback: f32) -> f32 {
    if feedback <= ROUTE_EPS {
        0.0
    } else if current > ROUTE_EPS && current <= feedback + ROUTE_EPS {
        current.clamp(0.0, feedback)
    } else {
        feedback
    }
}

fn rounded_tenth_amount(source: f32) -> f32 {
    let pct = (source.clamp(0.0, 1.0) * 100.0).round() as i32;
    let rounded_pct = match pct {
        0..=4 => 0,
        5..=13 => 1,
        14..=22 => 2,
        23..=31 => 3,
        32..=40 => 4,
        41..=49 => 5,
        50..=58 => 6,
        59..=67 => 7,
        68..=76 => 8,
        77..=82 => 9,
        83..=91 => 10,
        _ => 11,
    };
    rounded_pct as f32 / 100.0
}

fn classify_routing_shape(params: &NebulaStereoDelayParams) -> RoutingModeParam {
    let im_l = params.input_mode_l.value();
    let im_r = params.input_mode_r.value();
    let fb_l = params.feedback_l.value();
    let fb_r = params.feedback_r.value();
    let cf_lr = params.crossfeed_lr.value();
    let cf_rl = params.crossfeed_rl.value();
    let fpl = params.feedback_phase_l.value();
    let fpr = params.feedback_phase_r.value();
    let cplr = params.crossfeed_phase_lr.value();
    let cprl = params.crossfeed_phase_rl.value();
    let standard_inputs = im_l == InputModeParam::Left && im_r == InputModeParam::Right;
    let left_sum_only = im_l == InputModeParam::LeftPlusRight && im_r == InputModeParam::Off;
    let right_sum_only = im_l == InputModeParam::Off && im_r == InputModeParam::LeftPlusRight;
    let normal_phase = !fpl && !fpr && !cplr && !cprl;
    let feedback_equal = close(fb_l, fb_r);
    let crossfeed_equal = close(cf_lr, cf_rl);
    let feedback_zero = is_zero(fb_l) && is_zero(fb_r);
    let crossfeed_zero = is_zero(cf_lr) && is_zero(cf_rl);

    if right_sum_only
        && is_zero(fb_l)
        && close(fb_r, cf_lr)
        && close(fb_r, cf_rl)
        && !fpl
        && fpr
        && cplr
        && !cprl
    {
        return RoutingModeParam::Rotate;
    }
    if left_sum_only
        && is_zero(fb_r)
        && close(fb_l, cf_lr)
        && close(fb_l, cf_rl)
        && fpl
        && !fpr
        && !cplr
        && cprl
    {
        return RoutingModeParam::RotateR;
    }
    if normal_phase && left_sum_only && feedback_zero && crossfeed_equal && !is_zero(cf_lr) {
        return RoutingModeParam::PingPong;
    }
    if normal_phase && right_sum_only && feedback_zero && crossfeed_equal && !is_zero(cf_lr) {
        return RoutingModeParam::PingPongR;
    }
    if normal_phase
        && left_sum_only
        && feedback_equal
        && !is_zero(cf_lr)
        && cf_lr <= fb_l + ROUTE_EPS
        && is_zero(cf_rl)
    {
        return RoutingModeParam::Pan;
    }
    if normal_phase
        && right_sum_only
        && feedback_equal
        && !is_zero(cf_rl)
        && cf_rl <= fb_l + ROUTE_EPS
        && is_zero(cf_lr)
    {
        return RoutingModeParam::PanRl;
    }
    if normal_phase && standard_inputs && feedback_equal && crossfeed_zero {
        return RoutingModeParam::Straight;
    }
    if normal_phase
        && standard_inputs
        && feedback_equal
        && crossfeed_equal
        && !is_zero(cf_lr)
        && close(cf_lr, rounded_tenth_amount(fb_l))
    {
        return RoutingModeParam::NinetyTen;
    }
    if normal_phase
        && standard_inputs
        && feedback_equal
        && crossfeed_equal
        && !is_zero(fb_l)
        && close(fb_l, rounded_tenth_amount(cf_lr))
    {
        return RoutingModeParam::TenNinety;
    }
    if normal_phase && standard_inputs && feedback_zero && crossfeed_equal && !is_zero(cf_lr) {
        return RoutingModeParam::Crossfeed;
    }
    RoutingModeParam::Customized
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= ROUTE_EPS
}

fn is_zero(value: f32) -> bool {
    value.abs() <= ROUTE_EPS
}

fn apply_snapshot(
    params: &NebulaStereoDelayParams,
    setter: &ParamSetter<'_>,
    snap: &ParamSnapshot,
) {
    setter.set_parameter(&params.input_level, snap.input_level_db);
    setter.set_parameter(&params.output_level, snap.output_level_db);
    setter.set_parameter(
        &params.input_mode_l,
        input_mode_from_index(snap.input_mode_l),
    );
    setter.set_parameter(
        &params.input_mode_r,
        input_mode_from_index(snap.input_mode_r),
    );
    setter.set_parameter(&params.delay_time_l, snap.delay_time_l);
    setter.set_parameter(&params.delay_time_r, snap.delay_time_r);
    setter.set_parameter(&params.note_l, note_from_index(snap.note_l));
    setter.set_parameter(&params.note_r, note_from_index(snap.note_r));
    setter.set_parameter(&params.deviation_l, snap.deviation_l);
    setter.set_parameter(&params.deviation_r, snap.deviation_r);
    setter.set_parameter(&params.halve_l, snap.halve_l);
    setter.set_parameter(&params.halve_r, snap.halve_r);
    setter.set_parameter(&params.double_l, snap.double_l);
    setter.set_parameter(&params.double_r, snap.double_r);
    setter.set_parameter(&params.low_cut_l, snap.low_cut_l);
    setter.set_parameter(&params.low_cut_r, snap.low_cut_r);
    setter.set_parameter(&params.low_cut_slope_l, snap.low_cut_slope_l);
    setter.set_parameter(&params.low_cut_slope_r, snap.low_cut_slope_r);
    setter.set_parameter(&params.high_cut_l, snap.high_cut_l);
    setter.set_parameter(&params.high_cut_r, snap.high_cut_r);
    setter.set_parameter(&params.high_cut_slope_l, snap.high_cut_slope_l);
    setter.set_parameter(&params.high_cut_slope_r, snap.high_cut_slope_r);
    setter.set_parameter(&params.feedback_l, snap.feedback_l);
    setter.set_parameter(&params.feedback_r, snap.feedback_r);
    setter.set_parameter(&params.feedback_phase_l, snap.feedback_phase_l);
    setter.set_parameter(&params.feedback_phase_r, snap.feedback_phase_r);
    setter.set_parameter(&params.crossfeed_lr, snap.crossfeed_lr);
    setter.set_parameter(&params.crossfeed_rl, snap.crossfeed_rl);
    setter.set_parameter(&params.crossfeed_phase_lr, snap.crossfeed_phase_lr);
    setter.set_parameter(&params.crossfeed_phase_rl, snap.crossfeed_phase_rl);
    setter.set_parameter(&params.routing, routing_from_index(snap.routing));
    setter.set_parameter(
        &params.oversampling,
        oversampling_from_index(snap.oversampling),
    );
    setter.set_parameter(&params.tempo_sync, snap.tempo_sync);
    setter.set_parameter(&params.stereo_link, snap.stereo_link);
    setter.set_parameter(&params.output_mix_l, snap.output_mix_l);
    setter.set_parameter(&params.output_mix_r, snap.output_mix_r);
}

fn preset_values_from_snapshot(snap: &ParamSnapshot) -> PresetValues {
    PresetValues {
        input_level_db: snap.input_level_db,
        output_level_db: snap.output_level_db,
        input_mode_l: snap.input_mode_l as u8,
        input_mode_r: snap.input_mode_r as u8,
        delay_time_l: snap.delay_time_l,
        delay_time_r: snap.delay_time_r,
        note_l: snap.note_l as u8,
        note_r: snap.note_r as u8,
        deviation_l: snap.deviation_l,
        deviation_r: snap.deviation_r,
        halve_l: snap.halve_l,
        halve_r: snap.halve_r,
        double_l: snap.double_l,
        double_r: snap.double_r,
        low_cut_l: snap.low_cut_l,
        low_cut_r: snap.low_cut_r,
        low_cut_slope_l: snap.low_cut_slope_l,
        low_cut_slope_r: snap.low_cut_slope_r,
        high_cut_l: snap.high_cut_l,
        high_cut_r: snap.high_cut_r,
        high_cut_slope_l: snap.high_cut_slope_l,
        high_cut_slope_r: snap.high_cut_slope_r,
        feedback_l: snap.feedback_l,
        feedback_r: snap.feedback_r,
        feedback_phase_l: snap.feedback_phase_l,
        feedback_phase_r: snap.feedback_phase_r,
        crossfeed_lr: snap.crossfeed_lr,
        crossfeed_rl: snap.crossfeed_rl,
        crossfeed_phase_lr: snap.crossfeed_phase_lr,
        crossfeed_phase_rl: snap.crossfeed_phase_rl,
        routing: snap.routing as u8,
        oversampling: snap.oversampling as u8,
        tempo_sync: snap.tempo_sync,
        stereo_link: snap.stereo_link,
        output_mix_l: snap.output_mix_l,
        output_mix_r: snap.output_mix_r,
    }
}

#[derive(Clone)]
struct PresetItem {
    rect: UiRect,
    kind: PresetItemKind,
    action: PresetAction,
}

#[derive(Clone)]
enum PresetItemKind {
    Header(&'static str),
    Button(&'static str),
    Preset(String, bool),
}

#[derive(Clone)]
enum PresetAction {
    SaveCurrent,
    SaveA,
    SaveB,
    LoadFactory(usize),
    LoadUser(usize),
    None,
}

fn preset_name_rect(layout: &Layout) -> UiRect {
    UiRect::new(
        layout.preset_popup.x + 12.0 * layout.s,
        layout.preset_popup.y + 38.0 * layout.s,
        layout.preset_popup.w - 24.0 * layout.s,
        25.0 * layout.s,
    )
}

fn preset_popup_items(
    layout: &Layout,
    manager: &PresetManager,
    _params: &NebulaStereoDelayParams,
) -> Vec<PresetItem> {
    let mut items = Vec::new();
    let s = layout.s;
    let x = layout.preset_popup.x + 12.0 * s;
    let w = layout.preset_popup.w - 24.0 * s;
    let mut y = layout.preset_popup.y + 72.0 * s;
    let h = 22.0 * s;
    let gap = 6.0 * s;
    let bw = (w - gap * 2.0) / 3.0;
    let buttons = [
        ("Save Current", PresetAction::SaveCurrent),
        ("Save A", PresetAction::SaveA),
        ("Save B", PresetAction::SaveB),
    ];
    for (idx, (label, action)) in buttons.into_iter().enumerate() {
        items.push(PresetItem {
            rect: UiRect::new(x + idx as f32 * (bw + gap), y, bw, h),
            kind: PresetItemKind::Button(label),
            action,
        });
    }
    y += h + 14.0 * s;
    items.push(PresetItem {
        rect: UiRect::new(x, y, w, h),
        kind: PresetItemKind::Header("Factory Presets"),
        action: PresetAction::None,
    });
    y += h;
    for (idx, preset) in manager.factory_presets().iter().enumerate().take(10) {
        items.push(PresetItem {
            rect: UiRect::new(x, y, w, h),
            kind: PresetItemKind::Preset(preset.name.clone(), true),
            action: PresetAction::LoadFactory(idx),
        });
        y += h;
    }
    y += 8.0 * s;
    items.push(PresetItem {
        rect: UiRect::new(x, y, w, h),
        kind: PresetItemKind::Header("User Presets"),
        action: PresetAction::None,
    });
    y += h;
    if let Ok(presets) = manager.user_presets() {
        for (idx, preset) in presets.iter().enumerate().take(6) {
            items.push(PresetItem {
                rect: UiRect::new(x, y, w, h),
                kind: PresetItemKind::Preset(preset.name.clone(), false),
                action: PresetAction::LoadUser(idx),
            });
            y += h;
        }
    }
    items
}

#[derive(Clone)]
struct MidiItem {
    rect: UiRect,
    kind: MidiItemKind,
    action: MidiAction,
}

#[derive(Clone)]
enum MidiItemKind {
    Header(&'static str),
    Button(&'static str, bool),
    Mapping(String),
}

#[derive(Clone)]
enum MidiAction {
    ToggleGlobal,
    Clean(String),
    ClearAll,
    Rollback,
    Save,
    None,
}

fn midi_popup_items(layout: &Layout, params: &NebulaStereoDelayParams) -> Vec<MidiItem> {
    let mut items = Vec::new();
    let s = layout.s;
    let x = layout.midi_popup.x + 12.0 * s;
    let w = layout.midi_popup.w - 24.0 * s;
    let mut y = layout.midi_popup.y + 40.0 * s;
    let h = 23.0 * s;
    let global_on = params
        .midi_learn
        .read()
        .map(|learn| learn.is_global_enabled())
        .unwrap_or(true);
    items.push(MidiItem {
        rect: UiRect::new(x, y, w, h),
        kind: MidiItemKind::Button(if global_on { "MIDI On" } else { "MIDI Off" }, global_on),
        action: MidiAction::ToggleGlobal,
    });
    y += h + 8.0 * s;
    items.push(MidiItem {
        rect: UiRect::new(x, y, w, h),
        kind: MidiItemKind::Header("Clean Up"),
        action: MidiAction::None,
    });
    y += h;
    if let Ok(learn) = params.midi_learn.read() {
        for mapping in learn.mappings().iter().take(7) {
            let label = format!(
                "{}  Ch {} CC {}",
                mapping.param_id,
                mapping.channel + 1,
                mapping.cc
            );
            items.push(MidiItem {
                rect: UiRect::new(x, y, w, h),
                kind: MidiItemKind::Mapping(label),
                action: MidiAction::Clean(mapping.param_id.clone()),
            });
            y += h;
        }
    }
    items.push(MidiItem {
        rect: UiRect::new(x, y, w, h),
        kind: MidiItemKind::Button("Clear All", false),
        action: MidiAction::ClearAll,
    });
    y += h + 8.0 * s;
    items.push(MidiItem {
        rect: UiRect::new(x, y, w * 0.48, h),
        kind: MidiItemKind::Button("Roll Back", false),
        action: MidiAction::Rollback,
    });
    items.push(MidiItem {
        rect: UiRect::new(x + w * 0.52, y, w * 0.48, h),
        kind: MidiItemKind::Button("Save", false),
        action: MidiAction::Save,
    });
    items
}

fn client_size(hwnd: HWND) -> Option<(u32, u32)> {
    let mut rect = RECT::default();
    unsafe { GetClientRect(hwnd, &mut rect).ok()? };
    let width = (rect.right - rect.left).max(1) as u32;
    let height = (rect.bottom - rect.top).max(1) as u32;
    Some((width, height))
}

struct DpiAwarenessScope {
    previous: Option<DPI_AWARENESS_CONTEXT>,
}

impl DpiAwarenessScope {
    fn enter() -> Self {
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        Self {
            previous: (!previous.0.is_null()).then_some(previous),
        }
    }
}

impl Drop for DpiAwarenessScope {
    fn drop(&mut self) {
        if let Some(previous) = self.previous {
            let _ = unsafe { SetThreadDpiAwarenessContext(previous) };
        }
    }
}

fn dpi_for_window(hwnd: HWND) -> u32 {
    let dpi = if hwnd.0.is_null() {
        0
    } else {
        unsafe { GetDpiForWindow(hwnd) }
    };
    if dpi == 0 {
        unsafe { GetDpiForSystem() }.max(DEFAULT_DPI)
    } else {
        dpi.max(DEFAULT_DPI)
    }
}

fn dpi_from_wparam(wparam: WPARAM) -> u32 {
    let dpi_x = (wparam.0 as u32) & 0xffff;
    dpi_x.max(DEFAULT_DPI)
}

fn dpi_scale(dpi: u32) -> f32 {
    (dpi.max(DEFAULT_DPI) as f32 / DEFAULT_DPI as f32).clamp(0.5, 3.0)
}

fn invalidate(hwnd: HWND) {
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

fn class_name() -> PCWSTR {
    w!("NebulaStereoDelayDirect2DEditor")
}

fn module_instance() -> Option<HINSTANCE> {
    unsafe {
        GetModuleHandleW(None)
            .ok()
            .map(|module| HINSTANCE(module.0))
    }
}

fn register_window_class() -> bool {
    static REGISTER_ONCE: Once = Once::new();
    static REGISTERED: AtomicBool = AtomicBool::new(false);

    REGISTER_ONCE.call_once(|| {
        let Some(instance) = module_instance() else {
            return;
        };
        let cursor = unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() };
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: Default::default(),
            hCursor: cursor,
            hbrBackground: HBRUSH::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: class_name(),
        };
        let atom = unsafe { RegisterClassW(&wc) };
        if atom != 0 || unsafe { GetLastError() } == ERROR_CLASS_ALREADY_EXISTS {
            REGISTERED.store(true, Ordering::Release);
        }
    });

    REGISTERED.load(Ordering::Acquire)
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let create = lparam.0 as *const CREATESTRUCTW;
            if !create.is_null() {
                let state = (*create).lpCreateParams.cast::<NativeWindowState>();
                if !state.is_null() {
                    (*state).hwnd = hwnd;
                    (*state).dpi = dpi_for_window(hwnd);
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
                    let _ = SetTimer(Some(hwnd), TIMER_ID, TIMER_MS, None);
                    return LRESULT(1);
                }
            }
            return LRESULT(0);
        }

        let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut NativeWindowState;
        if msg == WM_NCDESTROY {
            let _ = KillTimer(Some(hwnd), TIMER_ID);
            if !state_ptr.is_null() {
                (*state_ptr).persist_midi_mapping();
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(state_ptr));
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        if state_ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let state = &mut *state_ptr;

        match msg {
            WM_GETDLGCODE => {
                if state.numeric_input.is_some() || state.text_input.is_some() {
                    LRESULT((DLGC_WANTALLKEYS | DLGC_WANTCHARS) as isize)
                } else {
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SIZE => {
                state.render_target = None;
                state.text_formats = None;
                invalidate(hwnd);
                LRESULT(0)
            }
            WM_DPICHANGED => {
                state.handle_dpi_changed(dpi_from_wparam(wparam));
                LRESULT(0)
            }
            WM_DPICHANGED_AFTERPARENT => {
                state.handle_dpi_changed(dpi_for_window(hwnd));
                LRESULT(0)
            }
            WM_DPICHANGED_BEFOREPARENT => LRESULT(0),
            WM_TIMER => {
                state.resize_to_parent();
                invalidate(hwnd);
                LRESULT(0)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                BeginPaint(hwnd, &mut ps);
                state.paint();
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let (x, y) = point_from_lparam(lparam);
                let (x, y) = state.logical_point(x, y);
                state.mouse_down(x, y);
                LRESULT(0)
            }
            WM_LBUTTONDBLCLK => {
                let (x, y) = point_from_lparam(lparam);
                let (x, y) = state.logical_point(x, y);
                state.mouse_double_click(x, y);
                LRESULT(0)
            }
            WM_RBUTTONDOWN => {
                let (x, y) = point_from_lparam(lparam);
                let (x, y) = state.logical_point(x, y);
                state.mouse_right_down(x, y);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if state.drag.is_some() {
                    let (x, y) = point_from_lparam(lparam);
                    let (x, y) = state.logical_point(x, y);
                    state.mouse_move(x, y);
                    LRESULT(0)
                } else {
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
            }
            WM_LBUTTONUP => {
                let (x, y) = point_from_lparam(lparam);
                let (x, y) = state.logical_point(x, y);
                state.mouse_up(x, y);
                LRESULT(0)
            }
            WM_CHAR => {
                if let Some(ch) = char::from_u32(wparam.0 as u32) {
                    state.char_input(ch);
                }
                LRESULT(0)
            }
            WM_KEYDOWN => {
                state.key_down(wparam.0 as u32);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn point_from_lparam(lparam: LPARAM) -> (f32, f32) {
    let raw = lparam.0 as u32;
    let x = (raw & 0xffff) as u16 as i16 as f32;
    let y = ((raw >> 16) & 0xffff) as u16 as i16 as f32;
    (x, y)
}
