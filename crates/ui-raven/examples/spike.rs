//! THROWAWAY Raven go/no-go spike (Task 1). Raw `raven::*` calls, no tests, not production code.
//!
//! A custom-painted "metro line" of 10,000 stations. Only the visible range is drawn.
//! Drag = pan, Ctrl+wheel = zoom around the cursor, FPS in the window title.
//!
//!   cargo run -p savestates-ui-raven --example spike --release
//!
//! `--selftest <prefix>` drives the window with injected input (drag, Ctrl+wheel), prints
//! PASS/FAIL lines, writes `<prefix>-zoom1.bmp` / `<prefix>-zoom2_5.bmp` screenshots read back
//! from the GPU frame, then quits. It exists only so the spike can be checked without a human.

#![allow(unsafe_op_in_unsafe_fn)]

// Pulls in the ui-raven rlib so the system libs its build.rs declares (winhttp) reach the linker.
extern crate savestates_ui_raven as _;

use raven::*;
use std::cell::Cell;
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::time::{Duration, Instant};

const STATIONS: usize = 10_000;
/// World units between two stations.
const STEP: f32 = 96.0;
const BASE_Y: f32 = 190.0;
const WAVE: f32 = 60.0;
const MIN_ZOOM: f32 = 0.15;
const MAX_ZOOM: f32 = 6.0;
const ZOOM_PER_NOTCH: f32 = 1.1;

const XML: &str = r#"
<Window Title="SaveStates spike" Width="1100" Height="420">
    <VBox Spacing="0">
        <Label Text="drag = pan, Ctrl+wheel = zoom" Slot.Padding="8"/>
        <Custom Name="canvas" Slot.Fill="1"/>
    </VBox>
</Window>"#;

const LINE_COLORS: [[f32; 3]; 6] = [
    [0.90, 0.30, 0.30],
    [0.25, 0.65, 0.95],
    [0.35, 0.80, 0.45],
    [0.95, 0.75, 0.20],
    [0.70, 0.45, 0.90],
    [0.95, 0.55, 0.20],
];

/// Everything the callbacks share. All of it is touched on the UI thread only.
struct State {
    window: Cell<Rid>,
    /// World coordinate shown at the top-left corner of the canvas.
    origin_x: Cell<f32>,
    origin_y: Cell<f32>,
    zoom: Cell<f32>,
    dragging: Cell<bool>,
    /// Paints since the last timer tick.
    paints: Cell<u32>,
    total_paints: Cell<u64>,
    /// Stations drawn by the last paint.
    drawn: Cell<u32>,
    last_tick: Cell<Instant>,
}

fn station_y(i: usize) -> f32 {
    BASE_Y + WAVE * (i as f32 * 0.35).sin()
}

unsafe extern "C" fn on_measure(_widget: Rid, width: *mut f32, height: *mut f32, _user: *mut c_void) {
    *width = 400.0;
    *height = 200.0;
}

unsafe extern "C" fn on_paint(_widget: Rid, painter: *mut RPainter, width: f32, height: f32, user: *mut c_void) {
    let st = &*(user as *const State);
    let (ox, oy, z) = (st.origin_x.get(), st.origin_y.get(), st.zoom.get());

    Rdraw_push_clip_rect(painter, 0.0, 0.0, width, height);
    Rdraw_rect(painter, 0.0, 0.0, width, height, 0.0, 0.10, 0.11, 0.14, 1.0);

    // Visible station range, with one station of slack on each side so segments and labels
    // that straddle the edge still draw.
    let first = ((ox / STEP).floor() as i64 - 1).max(0);
    let last = (((ox + width / z) / STEP).ceil() as i64 + 1).min(STATIONS as i64 - 1);

    let thickness = (5.0 * z).clamp(2.0, 16.0);
    let label_size = 13.0 * z;
    let mut drawn = 0u32;
    for i in first..=last {
        let i = i as usize;
        let sx = (i as f32 * STEP - ox) * z;
        let sy = (station_y(i) - oy) * z;
        let [r, g, b] = LINE_COLORS[(i / 50) % LINE_COLORS.len()];

        if i + 1 < STATIONS {
            let nx = ((i + 1) as f32 * STEP - ox) * z;
            let ny = (station_y(i + 1) - oy) * z;
            Rdraw_line(painter, sx, sy, nx, ny, thickness, r, g, b, 1.0);
        }
        Rdraw_circle(painter, sx, sy, 9.0 * z, 1.0, 1.0, 1.0, 1.0);
        Rdraw_circle(painter, sx, sy, 6.0 * z, 0.10, 0.11, 0.14, 1.0);

        if label_size >= 6.0 {
            let ty = if i % 2 == 0 { sy - 16.0 * z - label_size } else { sy + 14.0 * z };
            let label = cstr(&format!("Save #{}", i + 1));
            Rdraw_text(painter, R_INVALID_ID, label_size, sx - 24.0 * z, ty, label.as_ptr(), 0.92, 0.93, 0.95, 1.0);
        }
        drawn += 1;
    }
    Rdraw_pop_clip_rect(painter);

    st.drawn.set(drawn);
    st.paints.set(st.paints.get() + 1);
    st.total_paints.set(st.total_paints.get() + 1);
}

unsafe extern "C" fn on_event(widget: Rid, event: *const REvent, user: *mut c_void) -> c_int {
    let st = &*(user as *const State);
    let ev = &*event;
    match ev.type_ {
        R_EVENT_MOUSE_DOWN if ev.button == R_MOUSE_LEFT => {
            st.dragging.set(true);
            R_TRUE
        }
        R_EVENT_MOUSE_UP if ev.button == R_MOUSE_LEFT => {
            st.dragging.set(false);
            R_TRUE
        }
        R_EVENT_MOUSE_MOVE if st.dragging.get() => {
            // A lost button-up (focus change, capture loss) must not leave the canvas stuck.
            if Rwindow_is_mouse_button_down(ev.window, R_MOUSE_LEFT) == 0 {
                st.dragging.set(false);
                return R_FALSE;
            }
            let z = st.zoom.get();
            st.origin_x.set(st.origin_x.get() - ev.delta_x / z);
            st.origin_y.set(st.origin_y.get() - ev.delta_y / z);
            Rwidget_invalidate(widget);
            R_TRUE
        }
        R_EVENT_MOUSE_WHEEL if ev.modifiers & R_MOD_CTRL as u32 != 0 => {
            let old = st.zoom.get();
            let new = (old * ZOOM_PER_NOTCH.powf(ev.wheel_y)).clamp(MIN_ZOOM, MAX_ZOOM);
            // Keep the world point under the cursor where it is.
            let wx = st.origin_x.get() + ev.local_x / old;
            let wy = st.origin_y.get() + ev.local_y / old;
            st.origin_x.set(wx - ev.local_x / new);
            st.origin_y.set(wy - ev.local_y / new);
            st.zoom.set(new);
            Rwidget_invalidate(widget);
            R_TRUE
        }
        R_EVENT_MOUSE_WHEEL => {
            // Plain wheel scrolls along the line.
            st.origin_x.set(st.origin_x.get() - (ev.wheel_y + ev.wheel_x) * 80.0 / st.zoom.get());
            Rwidget_invalidate(widget);
            R_TRUE
        }
        _ => R_FALSE,
    }
}

unsafe extern "C" fn on_timer(_timer: Rid, user: *mut c_void) {
    let st = &*(user as *const State);
    let now = Instant::now();
    let secs = now.duration_since(st.last_tick.get()).as_secs_f64().max(1e-3);
    st.last_tick.set(now);
    let fps = st.paints.replace(0) as f64 / secs;

    let mut stats: RFrameStats = std::mem::zeroed();
    Rwindow_get_frame_stats(st.window.get(), &mut stats);
    let title = format!(
        "SaveStates spike | {fps:.0} FPS | zoom {:.2} | {} of {STATIONS} stations drawn | paint {:.2} ms",
        st.zoom.get(),
        st.drawn.get(),
        stats.paint_ms,
    );
    Rwindow_set_title(st.window.get(), cstr(&title).as_ptr());
    println!("{title}");
}

/// Reads back the last rendered frame and writes it as a 24-bit BMP.
unsafe fn capture_bmp(window: Rid, path: &str) {
    let (mut w, mut h): (c_int, c_int) = (0, 0);
    let pixels = Rwindow_capture(window, &mut w, &mut h);
    if pixels.is_null() {
        println!("capture: backend cannot capture");
        return;
    }
    let (w, h) = (w as usize, h as usize);
    let rgba = std::slice::from_raw_parts(pixels, w * h * 4);
    let row = (w * 3 + 3) & !3;
    let mut out = Vec::with_capacity(54 + row * h);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + row * h) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    for y in (0..h).rev() {
        for x in 0..w {
            let p = &rgba[(y * w + x) * 4..];
            out.extend_from_slice(&[p[2], p[1], p[0]]);
        }
        out.resize(out.len() + row - w * 3, 0);
    }
    Rpixels_free(pixels);
    match std::fs::write(path, &out) {
        Ok(()) => println!("capture: {path} ({w}x{h})"),
        Err(e) => println!("capture: cannot write {path}: {e}"),
    }
}

unsafe fn pump(frames: u32) {
    for _ in 0..frames {
        Rwait_frame(0.016);
    }
}

fn verdict(ok: bool) -> &'static str {
    if ok { "PASS" } else { "FAIL" }
}

/// Drives the window with injected input, then quits.
unsafe fn selftest(st: &State, window: Rid, canvas: Rid, prefix: &str) {
    let started = Instant::now();
    while st.total_paints.get() == 0 && started.elapsed() < Duration::from_secs(10) {
        Rwait_frame(0.016);
    }
    pump(10);
    println!("selftest first paint: {} paint(s), {} stations drawn, {}", st.total_paints.get(), st.drawn.get(), verdict(st.total_paints.get() > 0));

    let (mut x, mut y, mut w, mut h) = (0f32, 0f32, 0f32, 0f32);
    Rwidget_get_rect(canvas, &mut x, &mut y, &mut w, &mut h);
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    println!("selftest canvas rect: x={x} y={y} w={w} h={h}");
    capture_bmp(window, &format!("{prefix}-zoom1.bmp"));

    // Drag: hold the left button and sweep the mouse for 3 s, repainting as fast as the frame loop allows.
    Rwindow_inject_mouse_move(window, cx, cy);
    pump(2);
    Rwindow_inject_mouse_button(window, R_MOUSE_LEFT, 1);
    let (mut lo, mut hi) = (st.origin_x.get(), st.origin_x.get());
    let paints_before = st.total_paints.get();
    let sweep = Instant::now();
    while sweep.elapsed() < Duration::from_secs(3) {
        let t = sweep.elapsed().as_secs_f32();
        Rwindow_inject_mouse_move(window, cx + 300.0 * (t * 2.0).sin(), cy);
        Rwait_frame(0.0);
        lo = lo.min(st.origin_x.get());
        hi = hi.max(st.origin_x.get());
    }
    Rwindow_inject_mouse_button(window, R_MOUSE_LEFT, 0);
    let paints = st.total_paints.get() - paints_before;
    let secs = sweep.elapsed().as_secs_f64();
    println!(
        "selftest drag: origin_x swept {lo:.0}..{hi:.0}, {paints} paints in {secs:.2} s = {:.0} FPS, {}",
        paints as f64 / secs,
        verdict(hi - lo > 100.0)
    );

    // Ctrl+wheel: one event worth ln(2.5)/ln(1.1) notches takes zoom from 1.0 to 2.5 around the cursor.
    st.zoom.set(1.0);
    st.origin_x.set(1000.0);
    st.origin_y.set(0.0);
    Rwidget_invalidate(canvas);
    pump(3);
    Rwindow_inject_mouse_move(window, cx, cy);
    pump(2);
    Rwindow_inject_key(window, R_KEY_LEFT_CTRL, 1, R_MOD_CTRL as u32);
    pump(2);
    let (zx, zy) = (cx - x, cy - y);
    let world_before = (st.origin_x.get() + zx / st.zoom.get(), st.origin_y.get() + zy / st.zoom.get());
    Rwindow_inject_mouse_wheel(window, 0.0, 2.5f32.ln() / ZOOM_PER_NOTCH.ln());
    pump(5);
    Rwindow_inject_key(window, R_KEY_LEFT_CTRL, 0, 0);
    let world_after = (st.origin_x.get() + zx / st.zoom.get(), st.origin_y.get() + zy / st.zoom.get());
    println!(
        "selftest zoom: 1.00 -> {:.2}; world point under cursor {:.1},{:.1} -> {:.1},{:.1}, {}",
        st.zoom.get(),
        world_before.0,
        world_before.1,
        world_after.0,
        world_after.1,
        verdict((st.zoom.get() - 2.5).abs() < 0.05 && (world_before.0 - world_after.0).abs() < 1.0)
    );
    capture_bmp(window, &format!("{prefix}-zoom2_5.bmp"));
    println!("selftest done");
    Rquit(0);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let selftest_prefix = args.iter().position(|a| a == "--selftest").map(|i| args.get(i + 1).cloned().unwrap_or_else(|| "spike".into()));

    unsafe {
        if Rinit(R_BACKEND_VULKAN) == 0 {
            let mut detail: *const c_char = ptr::null();
            let mut severity: RSeverity = 0;
            let error = Rerror_get_last(&mut detail, &mut severity);
            eprintln!("Rinit failed: {} ({}): {}", text(Rerror_to_string(error)), error, text(detail));
            std::process::exit(2);
        }

        let window = window_from_xml(XML);
        let canvas = widget_find(window, "canvas");
        if window == R_INVALID_ID || canvas == R_INVALID_ID {
            let mut detail: *const c_char = ptr::null();
            let mut severity: RSeverity = 0;
            let error = Rerror_get_last(&mut detail, &mut severity);
            eprintln!("window setup failed (window={window} canvas={canvas}): {} {}", text(Rerror_to_string(error)), text(detail));
            Rshutdown();
            std::process::exit(3);
        }

        let st: &'static State = Box::leak(Box::new(State {
            window: Cell::new(window),
            origin_x: Cell::new(-40.0),
            origin_y: Cell::new(0.0),
            zoom: Cell::new(1.0),
            dragging: Cell::new(false),
            paints: Cell::new(0),
            total_paints: Cell::new(0),
            drawn: Cell::new(0),
            last_tick: Cell::new(Instant::now()),
        }));
        let user = st as *const State as *mut c_void;
        Rcustom_set_measure_callback(canvas, Some(on_measure), user);
        Rcustom_set_paint_callback(canvas, Some(on_paint), user);
        Rwidget_set_event_callback(canvas, Some(on_event), user);
        Rtimer_create(1.0, R_TRUE, Some(on_timer), user);

        println!("raven init ok (Raven {}, window {window}, canvas {canvas})", text(Rget_version()));

        match selftest_prefix {
            Some(prefix) => selftest(st, window, canvas, &prefix),
            None => while Rwait_frame(-1.0) != 0 {},
        }
        Rshutdown();
    }
}
