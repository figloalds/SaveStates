//! Custom widgets: painting and raw input.

use crate::cstr;
use raven::{R_INVALID_ID, RPainter};
use std::marker::PhantomData;

/// A color: red, green, blue, alpha, from 0 to 1.
#[derive(Clone, Copy)]
pub struct Rgba(pub f32, pub f32, pub f32, pub f32);

/// Draws into a custom widget during its paint callback. Coordinates are in logical units from
/// the top-left corner of the widget.
pub struct Painter<'a> {
    raw: *mut RPainter,
    _paint: PhantomData<&'a mut RPainter>,
}

impl Painter<'_> {
    /// Anti-aliased line with round caps.
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, thickness: f32, c: Rgba) {
        // SAFETY: `raw` is the painter of the paint callback running now.
        unsafe { raven::Rdraw_line(self.raw, x0, y0, x1, y1, thickness, c.0, c.1, c.2, c.3) };
    }

    /// Filled circle.
    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, c: Rgba) {
        // SAFETY: `raw` is the painter of the paint callback running now.
        unsafe { raven::Rdraw_circle(self.raw, cx, cy, r, c.0, c.1, c.2, c.3) };
    }

    /// Filled rectangle with rounded corners (`radius` 0 = sharp).
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, c: Rgba) {
        // SAFETY: `raw` is the painter of the paint callback running now.
        unsafe { raven::Rdraw_rect(self.raw, x, y, w, h, radius, c.0, c.1, c.2, c.3) };
    }

    /// Outline of a rounded rectangle, drawn inside its bounds.
    #[allow(clippy::too_many_arguments)]
    pub fn rect_outline(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        thickness: f32,
        c: Rgba,
    ) {
        // SAFETY: `raw` is the painter of the paint callback running now.
        unsafe {
            raven::Rdraw_rect_outline(self.raw, x, y, w, h, radius, thickness, c.0, c.1, c.2, c.3)
        };
    }

    /// Anti-aliased cubic Bézier curve from `from` to `to`, pulled by two control points.
    pub fn curve(
        &mut self,
        from: (f32, f32),
        control0: (f32, f32),
        control1: (f32, f32),
        to: (f32, f32),
        thickness: f32,
        c: Rgba,
    ) {
        // SAFETY: `raw` is the painter of the paint callback running now.
        unsafe {
            raven::Rdraw_curve(
                self.raw, from.0, from.1, control0.0, control0.1, control1.0, control1.1, to.0,
                to.1, thickness, c.0, c.1, c.2, c.3,
            )
        };
    }

    /// Text in the theme font; (`x`, `y`) is the top-left corner of the first line.
    pub fn text(&mut self, size: f32, x: f32, y: f32, s: &str, c: Rgba) {
        let s = cstr(s);
        // SAFETY: `raw` is the painter of the paint callback running now; `s` outlives the call.
        unsafe {
            raven::Rdraw_text(
                self.raw,
                R_INVALID_ID,
                size,
                x,
                y,
                s.as_ptr(),
                c.0,
                c.1,
                c.2,
                c.3,
            )
        };
    }

    /// Text in Raven's bundled fixed-width font (Roboto Mono); (`x`, `y`) is the top-left corner
    /// of the first line.
    pub fn mono_text(&mut self, size: f32, x: f32, y: f32, s: &str, c: Rgba) {
        let s = cstr(s);
        // SAFETY: `raw` is the painter of the paint callback running now; `s` outlives the call.
        // The monospace font is Raven's own and lives as long as Raven does.
        unsafe {
            raven::Rdraw_text(
                self.raw,
                raven::Rfont_get_monospace(),
                size,
                x,
                y,
                s.as_ptr(),
                c.0,
                c.1,
                c.2,
                c.3,
            )
        };
    }

    /// [`Painter::mono_text`] in bold. Raven draws the bold glyphs with the same advances, so
    /// [`mono_text_size`] measures it too.
    pub fn mono_bold_text(&mut self, size: f32, x: f32, y: f32, s: &str, c: Rgba) {
        let s = cstr(s);
        // SAFETY: `raw` is the painter of the paint callback running now; `s` outlives the call.
        // The bold variant of Raven's monospace font lives as long as that font does.
        unsafe {
            let bold = raven::Rfont_get_variant(raven::Rfont_get_monospace(), raven::R_FONT_BOLD);
            raven::Rdraw_text(self.raw, bold, size, x, y, s.as_ptr(), c.0, c.1, c.2, c.3)
        };
    }

    /// Restricts what follows to a rectangle, intersected with the current one.
    pub fn push_clip(&mut self, x: f32, y: f32, w: f32, h: f32) {
        // SAFETY: `raw` is the painter of the paint callback running now.
        unsafe { raven::Rdraw_push_clip_rect(self.raw, x, y, w, h) };
    }

    pub fn pop_clip(&mut self) {
        // SAFETY: `raw` is the painter of the paint callback running now.
        unsafe { raven::Rdraw_pop_clip_rect(self.raw) };
    }
}

/// Width of `s` in the theme font at `size`, in logical units.
pub fn text_width(size: f32, s: &str) -> f32 {
    let s = cstr(s);
    let (mut width, mut height) = (0.0, 0.0);
    // SAFETY: `s` outlives the call; both outputs are valid.
    unsafe { raven::Rfont_measure_text(R_INVALID_ID, size, s.as_ptr(), &mut width, &mut height) };
    width
}

/// Width and height of `s` in the fixed-width font of [`Painter::mono_text`] at `size`, in
/// logical units.
pub fn mono_text_size(size: f32, s: &str) -> (f32, f32) {
    let s = cstr(s);
    let (mut width, mut height) = (0.0, 0.0);
    // SAFETY: `s` outlives the call; both outputs are valid; the font is Raven's own.
    unsafe {
        raven::Rfont_measure_text(
            raven::Rfont_get_monospace(),
            size,
            s.as_ptr(),
            &mut width,
            &mut height,
        )
    };
    (width, height)
}

/// Widgets painted and driven by the app (`<Custom Name="..."/>` in XML, or [`custom::create`]).
pub mod custom {
    use super::Painter;
    use crate::event::Event;
    use crate::widget::Widget;
    use crate::{leak, with_callback};
    use raven::{R_WIDGET_CUSTOM, REvent, RPainter, Rid};
    use std::ffi::{c_int, c_void};
    use std::marker::PhantomData;

    type PaintFn = Box<dyn FnMut(&mut Painter, f32, f32)>;
    type EventFn = Box<dyn FnMut(&Event) -> bool>;

    /// A new custom widget, without a parent yet. It asks for no size of its own: give it one
    /// in its parent (`Slot.Fill`), or prefer `<Custom>` in the window's XML.
    pub fn create() -> Widget {
        // SAFETY: plain call.
        Widget(unsafe { raven::Rwidget_create(R_WIDGET_CUSTOM) })
    }

    /// Paints the widget: the closure gets a painter and the widget's width and height.
    pub fn on_paint(w: Widget, f: impl FnMut(&mut Painter, f32, f32) + 'static) {
        let Some(user) = leak::<PaintFn>(Box::new(f)) else {
            return;
        };
        // SAFETY: `user` is a `PaintFn` box that is never freed.
        unsafe { raven::Rcustom_set_paint_callback(w.0, Some(paint_trampoline), user) };
    }

    /// Receives the widget's input events first: true consumes the event.
    pub fn on_event(w: Widget, f: impl FnMut(&Event) -> bool + 'static) {
        let Some(user) = leak::<EventFn>(Box::new(f)) else {
            return;
        };
        // SAFETY: `user` is an `EventFn` box that is never freed.
        unsafe { raven::Rwidget_set_event_callback(w.0, Some(event_trampoline), user) };
    }

    unsafe extern "C" fn paint_trampoline(
        _widget: Rid,
        raw: *mut RPainter,
        width: f32,
        height: f32,
        user: *mut c_void,
    ) {
        // The painter cannot leave the closure: it is borrowed for this call only.
        let mut painter = Painter {
            raw,
            _paint: PhantomData,
        };
        // SAFETY: Raven passes back the `PaintFn` box given in `on_paint`, on the UI thread.
        unsafe { with_callback::<PaintFn, _>(user, |f| f(&mut painter, width, height)) };
    }

    unsafe extern "C" fn event_trampoline(
        _widget: Rid,
        raw: *const REvent,
        user: *mut c_void,
    ) -> c_int {
        // SAFETY: Raven passes a valid event for the duration of the call.
        let Some(event) = Event::from_raw(unsafe { &*raw }) else {
            return 0;
        };
        // SAFETY: Raven passes back the `EventFn` box given in `on_event`, on the UI thread.
        // A nested call does not consume the event.
        let consumed = unsafe { with_callback::<EventFn, _>(user, |f| f(&event)) }.unwrap_or(false);
        c_int::from(consumed)
    }
}
