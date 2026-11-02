//! The app's colors (spec §5): dark and lightly sci-fi, a cyan line, amber for unsaved work,
//! green and red for added and removed lines.

use savestates_ui_raven::Rgba;

/// An opaque color from its `0xRRGGBB` code.
const fn hex(rgb: u32) -> Rgba {
    Rgba(
        ((rgb >> 16) & 0xff) as f32 / 255.0,
        ((rgb >> 8) & 0xff) as f32 / 255.0,
        (rgb & 0xff) as f32 / 255.0,
        1.0,
    )
}

/// The timeline and the accent: `#38bdf8`.
pub const LINE: Rgba = hex(0x38bdf8);
/// Unsaved changes, the "Now" station: `#fbbf24`.
pub const UNSAVED: Rgba = hex(0xfbbf24);
/// Added lines: `#34d399`.
pub const ADDED: Rgba = hex(0x34d399);
/// Removed lines: `#f87171`.
pub const REMOVED: Rgba = hex(0xf87171);
/// Secondary text (dates, counts, notes): `#66739a`.
pub const MUTED_TEXT: Rgba = hex(0x66739a);
/// Main text: `#dbe4ff`.
pub const TEXT: Rgba = hex(0xdbe4ff);
/// The background: `#0c1120`.
pub const BG: Rgba = hex(0x0c1120);
/// Surfaces raised over the background, such as tooltips: `#151d33`.
pub const SURFACE: Rgba = hex(0x151d33);
/// Thin outlines around surfaces: `#2a3654`.
pub const BORDER: Rgba = hex(0x2a3654);
/// Behind the lines of a file in the comparison view, a shade darker than the background:
/// `#0a0f1c` (the theme's `Header`).
pub const CODE_BG: Rgba = hex(0x0a0f1c);
/// A surface under the pointer: `#1c2642` (the theme's `ControlHovered`).
pub const HOVERED: Rgba = hex(0x1c2642);

/// `c` with its opacity set to `alpha`.
pub const fn with_alpha(c: Rgba, alpha: f32) -> Rgba {
    Rgba(c.0, c.1, c.2, alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_codes_become_fractions() {
        let Rgba(r, g, b, a) = LINE;
        assert_eq!(
            (r, g, b, a),
            (56.0 / 255.0, 189.0 / 255.0, 248.0 / 255.0, 1.0)
        );
        let Rgba(r, g, b, _) = BG;
        assert_eq!((r, g, b), (12.0 / 255.0, 17.0 / 255.0, 32.0 / 255.0));
    }
}
