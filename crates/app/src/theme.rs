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
/// Renamed files in the change lists: a lighter cyan than the line, `#7dd3fc`.
pub const RENAMED: Rgba = hex(0x7dd3fc);
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
/// Behind lists and text fields: `#0b1020` (the theme's `Input`).
pub const FIELD: Rgba = hex(0x0b1020);
/// A surface under the pointer: `#1c2642` (the theme's `ControlHovered`).
pub const HOVERED: Rgba = hex(0x1c2642);

/// The colors of the timelines other than the main line (spec §6.4), clear of green, red and
/// amber, which mean added, removed and unsaved.
pub const LANE_PALETTE: [Rgba; 6] = [
    hex(0xa78bfa),
    hex(0xf472b6),
    hex(0x818cf8),
    hex(0xe879f9),
    hex(0xc4b5fd),
    hex(0x93c5fd),
];

/// A timeline's color: the main line's is [`LINE`]; any other's comes from its git name, the
/// same on every run and machine (FNV-1a over the name, not Rust's `DefaultHasher`).
pub fn lane_color(branch: &str, main_line: bool) -> Rgba {
    if main_line {
        return LINE;
    }
    let palette_len = LANE_PALETTE.len() as u64;
    LANE_PALETTE[(fnv1a64(branch) % palette_len) as usize]
}

/// The 64-bit FNV-1a hash of `s`'s UTF-8 bytes.
fn fnv1a64(s: &str) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    s.bytes().fold(OFFSET, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(PRIME)
    })
}

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

    #[test]
    fn lane_colors_are_stable_and_the_main_line_is_cyan() {
        assert_eq!(lane_color("main", true), LINE);
        // Even a name that would hash to another color: the main line is always cyan.
        assert_eq!(lane_color("savestates/graceful-elephant", true), LINE);

        // FNV-1a, 64 bits, over the name's UTF-8 bytes.
        let name = "savestates/graceful-elephant";
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in name.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        // Pinned values, so the hash itself (not only the loop above) cannot drift: FNV-1a
        // 64 of "" is the offset basis, of "a" the published 0xaf63dc4c8601ec8c.
        assert_eq!(fnv1a64(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64("a"), 0xaf63_dc4c_8601_ec8c);
        let expected = LANE_PALETTE[(hash % 6) as usize];
        assert_eq!(lane_color(name, false), expected);
        // The same name always gives the same color.
        for _ in 0..3 {
            assert_eq!(lane_color(name, false), expected);
            assert_eq!(lane_color("develop", false), lane_color("develop", false));
        }
        // The palette, in its order.
        let codes = [0xa78bfa, 0xf472b6, 0x818cf8, 0xe879f9, 0xc4b5fd, 0x93c5fd];
        for (color, code) in LANE_PALETTE.iter().zip(codes) {
            assert_eq!(*color, hex(code));
        }
    }
}
