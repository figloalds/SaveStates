//! The rows of a change list (spec addendum §5), as pure code: which symbol a change shows, how
//! its path splits into a name and a folder, how a long folder is shortened in the middle, and
//! which rows a view shows and what is under the pointer.
//!
//! Widths come from a `measure` function, so that the text fitting is tested here with one unit
//! per character and painted by the widget with the real font.

use std::ops::Range;

use savestates_core::ChangeKind;

use crate::tr_key;

/// The ellipsis that stands for what a shortened text leaves out.
const ELLIPSIS: &str = "…";

/// What a row's symbol says about its change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Symbol {
    Added,
    Deleted,
    Changed,
    Renamed,
}

/// The symbol of a change.
pub fn symbol(kind: &ChangeKind) -> Symbol {
    match kind {
        ChangeKind::New => Symbol::Added,
        ChangeKind::Deleted => Symbol::Deleted,
        ChangeKind::Changed => Symbol::Changed,
        ChangeKind::Renamed { .. } => Symbol::Renamed,
    }
}

/// The text of a symbol, as git shows it. Deleted is the minus sign (U+2212), as wide as "+".
pub fn symbol_text(s: Symbol) -> &'static str {
    match s {
        Symbol::Added => "+",
        Symbol::Deleted => "\u{2212}",
        Symbol::Changed => "M",
        Symbol::Renamed => "R",
    }
}

/// The text key of a symbol's tooltip. The renamed one has a `{0}` for the file's old path.
pub fn tooltip_key(s: Symbol) -> &'static str {
    match s {
        Symbol::Added => tr_key("change.tip.new"),
        Symbol::Deleted => tr_key("change.tip.deleted"),
        Symbol::Changed => tr_key("change.tip.changed"),
        Symbol::Renamed => tr_key("change.tip.renamed"),
    }
}

/// A path as its file name and its folder, with the folder's trailing `/` ("" at the top).
pub fn split_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(slash) => (&path[slash + 1..], &path[..=slash]),
        None => (path, ""),
    }
}

/// `folder` (ending in `/`) as it fits in `max_px` by `measure`. It stays whole when it fits.
/// Otherwise its first and last folders stay and the ones between give way to `…/`, from the
/// left, until it fits ("src/…/botoes/"). When even "first/…/last/" is too wide, that is cut
/// and ends in `…`; "" when not even `…` fits. Characters are kept whole.
pub fn shorten_middle(folder: &str, max_px: f64, measure: impl Fn(&str) -> f64) -> String {
    if measure(folder) <= max_px {
        return folder.to_owned();
    }
    let segments: Vec<&str> = folder.trim_end_matches('/').split('/').collect();
    let mut compact = folder.to_owned();
    if let [first, middle @ .., last] = segments.as_slice()
        && !middle.is_empty()
    {
        for dropped in 1..=middle.len() {
            let kept: String = middle[dropped..].iter().map(|s| format!("{s}/")).collect();
            compact = format!("{first}/{ELLIPSIS}/{kept}{last}/");
            if measure(&compact) <= max_px {
                return compact;
            }
        }
    }
    fit_end(&compact, max_px, measure)
}

/// `text` as it fits in `max_px` by `measure`: whole, or cut and ended with `…`; "" when not
/// even `…` fits. Characters are kept whole.
pub fn fit_end(text: &str, max_px: f64, measure: impl Fn(&str) -> f64) -> String {
    if measure(text) <= max_px {
        return text.to_owned();
    }
    let ends: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    for &end in ends.iter().rev() {
        // A cut right after an ellipsis would show two of them.
        let cut = format!("{}{ELLIPSIS}", text[..end].trim_end_matches(ELLIPSIS));
        if measure(&cut) <= max_px {
            return cut;
        }
    }
    String::new()
}

/// `text` broken into lines no wider than `max_px` by `measure`, after a space, a `/` (a path
/// breaks between its folders), a `-` or a `_` (a file name between its words). A piece wider
/// than a line is cut between characters.
pub fn wrap_text(text: &str, max_px: f64, measure: impl Fn(&str) -> f64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for piece in text.split_inclusive([' ', '/', '-', '_']) {
        let longer = format!("{line}{piece}");
        if measure(longer.trim_end()) <= max_px {
            line = longer;
            continue;
        }
        if !line.is_empty() {
            lines.push(line.trim_end().to_owned());
            line.clear();
        }
        if measure(piece.trim_end()) <= max_px {
            line = piece.to_owned();
            continue;
        }
        for c in piece.chars() {
            let longer = format!("{line}{c}");
            if line.is_empty() || measure(longer.trim_end()) <= max_px {
                line = longer;
            } else {
                lines.push(std::mem::replace(&mut line, c.to_string()));
            }
        }
    }
    if !line.trim_end().is_empty() {
        lines.push(line.trim_end().to_owned());
    }
    lines
}

/// What part of a row a point is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Symbol(usize),
    Name(usize),
    Folder(usize),
}

/// The rows of a list view, all `row_h` high, scrolled down by `scroll_y`. Heights are in the
/// view's units, from the top of its first row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowsModel {
    pub count: usize,
    pub row_h: f64,
    pub scroll_y: f64,
}

impl RowsModel {
    /// The rows that show, partly or wholly, in a view `viewport_h` high: only those are
    /// painted, however many there are.
    pub fn visible(&self, viewport_h: f64) -> Range<usize> {
        if self.row_h <= 0.0 {
            return 0..0;
        }
        let start = ((self.scroll_y / self.row_h).floor().max(0.0) as usize).min(self.count);
        let end = (((self.scroll_y + viewport_h.max(0.0)) / self.row_h).ceil() as usize)
            .clamp(start, self.count);
        start..end
    }

    /// The row at height `y` from the top of the view, if any.
    pub fn row_at(&self, y: f64) -> Option<usize> {
        if y < 0.0 || self.row_h <= 0.0 {
            return None;
        }
        let row = ((y + self.scroll_y) / self.row_h).floor() as usize;
        (row < self.count).then_some(row)
    }

    /// The part of a row at (`x`, `y`): its symbol left of `symbol_w`, its name up to
    /// `name_end` (where that row's name ends), its folder beyond.
    pub fn zone_at(&self, x: f64, y: f64, symbol_w: f64, name_end: f64) -> Option<Zone> {
        if x < 0.0 {
            return None;
        }
        let row = self.row_at(y)?;
        Some(if x < symbol_w {
            Zone::Symbol(row)
        } else if x < name_end {
            Zone::Name(row)
        } else {
            Zone::Folder(row)
        })
    }

    /// How far down the view can go: until the last row reaches its bottom.
    pub fn max_scroll(&self, viewport_h: f64) -> f64 {
        (self.count as f64 * self.row_h - viewport_h).max(0.0)
    }

    /// Scrolls down by `dy` (up when negative), between the top and the point where the last
    /// row reaches the bottom of a view `viewport_h` high.
    pub fn scroll_by(&mut self, dy: f64, viewport_h: f64) {
        let target = self.scroll_y + dy;
        let target = if target.is_nan() {
            self.scroll_y
        } else {
            target
        };
        self.scroll_y = target.clamp(0.0, self.max_scroll(viewport_h));
    }

    /// Scrolls as little as it takes for `row` to show whole in a view `viewport_h` high (its
    /// top, in a view lower than a row).
    pub fn ensure_visible(&mut self, row: usize, viewport_h: f64) {
        if row >= self.count {
            return;
        }
        let top = row as f64 * self.row_h;
        let bottom = top + self.row_h;
        let target = if top < self.scroll_y || viewport_h < self.row_h {
            top
        } else if bottom > self.scroll_y + viewport_h {
            bottom - viewport_h
        } else {
            self.scroll_y
        };
        self.scroll_y = target;
        self.scroll_by(0.0, viewport_h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use savestates_core::ChangeKind;

    /// One unit of width per character.
    fn chars(s: &str) -> f64 {
        s.chars().count() as f64
    }

    #[test]
    fn symbols_and_tips() {
        let renamed = ChangeKind::Renamed {
            from: "css/velho.css".to_owned(),
        };
        let cases = [
            (ChangeKind::New, Symbol::Added, "+", "change.tip.new"),
            (
                ChangeKind::Deleted,
                Symbol::Deleted,
                "\u{2212}",
                "change.tip.deleted",
            ),
            (
                ChangeKind::Changed,
                Symbol::Changed,
                "M",
                "change.tip.changed",
            ),
            (renamed, Symbol::Renamed, "R", "change.tip.renamed"),
        ];
        for (kind, sym, text, key) in cases {
            assert_eq!(symbol(&kind), sym, "{kind:?}");
            assert_eq!(symbol_text(sym), text, "{sym:?}");
            assert_eq!(tooltip_key(sym), key, "{sym:?}");
        }
        // The minus sign, not a hyphen.
        assert_ne!(symbol_text(Symbol::Deleted), "-");
    }

    #[test]
    fn split_path_cases() {
        assert_eq!(split_path("css/estilo.css"), ("estilo.css", "css/"));
        assert_eq!(split_path("a.txt"), ("a.txt", ""));
        assert_eq!(
            split_path("src/app/componentes/botoes/primário.tsx"),
            ("primário.tsx", "src/app/componentes/botoes/")
        );
    }

    #[test]
    fn shorten_middle_keeps_ends() {
        let folder = "src/app/componentes/botoes/";
        assert_eq!(shorten_middle(folder, 20.0, chars), "src/…/botoes/");
        // Wide enough: unchanged.
        assert_eq!(shorten_middle(folder, 27.0, chars), folder);
        assert_eq!(shorten_middle(folder, 100.0, chars), folder);
        // The middle goes from its left first: the deepest folders that fit stay.
        assert_eq!(
            shorten_middle(folder, 25.0, chars),
            "src/…/componentes/botoes/"
        );
        // Not even "first/…/last/" fits: it is cut, and ends in "…".
        assert_eq!(shorten_middle(folder, 8.0, chars), "src/…/b…");
        // One or two folders have no middle to take out.
        assert_eq!(shorten_middle("componentes/", 8.0, chars), "compone…");
        assert_eq!(
            shorten_middle("src/componentes/", 10.0, chars),
            "src/compo…"
        );
        assert_eq!(shorten_middle("", 0.0, chars), "");
        assert_eq!(shorten_middle(folder, 0.0, chars), "");

        // Accented folders are never split inside a character.
        let accented = "açúcar/doces/configuração/";
        assert_eq!(
            shorten_middle(accented, 22.0, chars),
            "açúcar/…/configuração/"
        );
        assert_eq!(shorten_middle(accented, 12.0, chars), "açúcar/…/co…");
        // Measured in bytes, "ç" is wider than "a", and still kept whole or left out.
        let bytes = |s: &str| s.len() as f64;
        assert_eq!(shorten_middle("ação/x/", 4.0, bytes), "a…");
        assert_eq!(shorten_middle("ação/x/", 6.0, bytes), "aç…");
    }

    #[test]
    fn long_names_end_in_an_ellipsis() {
        assert_eq!(fit_end("estilo.css", 10.0, chars), "estilo.css");
        assert_eq!(fit_end("estilo.css", 7.0, chars), "estilo…");
        assert_eq!(fit_end("configuração.json", 13.0, chars), "configuração…");
        assert_eq!(fit_end("estilo.css", 0.5, chars), "");
    }

    #[test]
    fn tooltips_wrap_between_folders_and_words() {
        assert_eq!(
            wrap_text("src/app/componentes/botoes/", 12.0, chars),
            ["src/app/", "componentes/", "botoes/"]
        );
        assert_eq!(
            wrap_text("Renomeado de css/velho.css", 14.0, chars),
            ["Renomeado de", "css/velho.css"]
        );
        assert_eq!(
            wrap_text("texto/campo-de-texto_grande.tsx", 16.0, chars),
            ["texto/campo-de-", "texto_grande.tsx"]
        );
        // A piece wider than a line is cut between characters.
        assert_eq!(
            wrap_text("configuração", 5.0, chars),
            ["confi", "guraç", "ão"]
        );
        assert_eq!(wrap_text("curto", 40.0, chars), ["curto"]);
        assert!(wrap_text("", 40.0, chars).is_empty());
    }

    #[test]
    fn rows_virtualize() {
        let model = RowsModel {
            count: 5_000,
            row_h: 24.0,
            scroll_y: 24_000.0,
        };
        let visible = model.visible(480.0);
        assert_eq!(visible.start, 1000);
        assert!((1020..=1021).contains(&visible.end), "{visible:?}");
        // Half a row down, the row cut at the bottom shows too.
        let model = RowsModel {
            scroll_y: 12.0,
            ..model
        };
        assert_eq!(model.visible(480.0), 0..21);
        // Never past the last row; none at all for an empty list.
        let end = RowsModel {
            scroll_y: 5_000.0 * 24.0 - 480.0,
            ..model
        };
        assert_eq!(end.visible(480.0), 4980..5000);
        let empty = RowsModel {
            count: 0,
            row_h: 24.0,
            scroll_y: 0.0,
        };
        assert_eq!(empty.visible(480.0), 0..0);
    }

    #[test]
    fn zone_hit() {
        let model = RowsModel {
            count: 3,
            row_h: 24.0,
            scroll_y: 0.0,
        };
        // The symbol column ends at 28; on this row the name ends at 120.
        assert_eq!(
            model.zone_at(10.0, 30.0, 28.0, 120.0),
            Some(Zone::Symbol(1))
        );
        assert_eq!(model.zone_at(60.0, 30.0, 28.0, 120.0), Some(Zone::Name(1)));
        assert_eq!(
            model.zone_at(200.0, 30.0, 28.0, 120.0),
            Some(Zone::Folder(1))
        );
        // Under the last row, or outside the widget: nothing.
        assert_eq!(model.zone_at(10.0, 80.0, 28.0, 120.0), None);
        assert_eq!(model.zone_at(-1.0, 30.0, 28.0, 120.0), None);
        assert_eq!(model.row_at(-0.5), None);
        // Scrolled by half a row: the top of the view is the lower half of row 0.
        let scrolled = RowsModel {
            scroll_y: 12.0,
            ..model
        };
        assert_eq!(scrolled.row_at(11.0), Some(0));
        assert_eq!(
            scrolled.zone_at(10.0, 13.0, 28.0, 120.0),
            Some(Zone::Symbol(1))
        );
    }

    #[test]
    fn scroll_clamps() {
        let mut model = RowsModel {
            count: 100,
            row_h: 24.0,
            scroll_y: 0.0,
        };
        model.scroll_by(-50.0, 480.0);
        assert_eq!(model.scroll_y, 0.0);
        model.scroll_by(100.0, 480.0);
        assert_eq!(model.scroll_y, 100.0);
        model.scroll_by(1e9, 480.0);
        assert_eq!(model.scroll_y, 2400.0 - 480.0);
        // Fewer rows after a refresh: the next scroll brings the view back over them.
        model.count = 30;
        model.scroll_by(0.0, 480.0);
        assert_eq!(model.scroll_y, 720.0 - 480.0);
        model.scroll_by(f64::NAN, 480.0);
        assert_eq!(model.scroll_y, 240.0);
        // Every row fits: nothing scrolls.
        let mut short = RowsModel {
            count: 5,
            row_h: 24.0,
            scroll_y: 0.0,
        };
        short.scroll_by(30.0, 480.0);
        assert_eq!(short.scroll_y, 0.0);
    }

    #[test]
    fn ensure_visible_scrolls_as_little_as_it_can() {
        let mut model = RowsModel {
            count: 100,
            row_h: 24.0,
            scroll_y: 0.0,
        };
        // Already in view.
        model.ensure_visible(5, 480.0);
        assert_eq!(model.scroll_y, 0.0);
        // Below: it comes up to the bottom edge.
        model.ensure_visible(30, 480.0);
        assert_eq!(model.scroll_y, 31.0 * 24.0 - 480.0);
        // Above: it goes down to the top edge.
        model.ensure_visible(2, 480.0);
        assert_eq!(model.scroll_y, 48.0);
        model.ensure_visible(99, 480.0);
        assert_eq!(model.scroll_y, 2400.0 - 480.0);
        // No such row: nothing moves.
        model.ensure_visible(500, 480.0);
        assert_eq!(model.scroll_y, 2400.0 - 480.0);
        // A view lower than a row shows the row's top.
        model.ensure_visible(10, 10.0);
        assert_eq!(model.scroll_y, 240.0);
    }
}
