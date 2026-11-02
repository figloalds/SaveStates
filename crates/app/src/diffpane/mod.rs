//! The comparison view (spec §6.4): a file side by side at two points in time.
//!
//! [`CompareTarget`] says which file to compare and between which sides; [`load`] reads and
//! compares them on the project's worker; [`widget::DiffPane`] shows the result.

pub mod model;
pub mod widget;

use savestates_core::{CoreError, FileContent, Project, Side, read_file, side_by_side};

use widget::PaneInput;

/// Which file to compare, between which two sides, and what names them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompareTarget {
    /// The older side and the file's path there, as git reports it. `None` when there is
    /// nothing before (a project without saves).
    pub before: Option<(Side, String)>,
    /// The newer side and the file's path there, as git reports it.
    pub after: (Side, String),
    /// The column headers.
    pub before_title: String,
    pub after_title: String,
    /// The text key shown in the After column when the file does not exist there.
    pub after_missing_key: &'static str,
}

impl CompareTarget {
    /// The file's path on the newer side: what the label above the comparison shows.
    pub fn path(&self) -> &str {
        &self.after.1
    }
}

/// Reads both sides of `target` and compares them. Runs on the project's worker.
///
/// With `force` ("Show anyway"), both files are read again whatever their size, and compared
/// whatever their length: a side read without it stays "too large" for the comparison.
pub fn load(p: &Project, target: &CompareTarget, force: bool) -> Result<PaneInput, CoreError> {
    let before = match &target.before {
        Some((side, path)) => read_file(p, side, path, force)?,
        None => FileContent::Missing,
    };
    let (side, path) = &target.after;
    let after = read_file(p, side, path, force)?;
    Ok(PaneInput {
        result: side_by_side(&before, &after, force),
        before_title: target.before_title.clone(),
        after_title: target.after_title.clone(),
        after_missing_key: target.after_missing_key,
        no_final_newline: (lacks_final_newline(&before), lacks_final_newline(&after)),
    })
}

/// Whether a text file ends without a line break after its last line.
fn lacks_final_newline(content: &FileContent) -> bool {
    matches!(content, FileContent::Text(text) if !text.is_empty() && !text.ends_with('\n'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use savestates_core::{DiffView, start_saving_folder};

    fn now_target(path: &str) -> CompareTarget {
        CompareTarget {
            before: None,
            after: (Side::Now, path.to_owned()),
            before_title: "Antes".to_owned(),
            after_title: "Depois · Agora".to_owned(),
            after_missing_key: "compare.was_deleted",
        }
    }

    #[test]
    fn show_anyway_reads_both_sides_again_and_compares_them() {
        let dir = tempfile::tempdir().unwrap();
        let git = savestates_git::locate().expect("git is installed");
        let project = start_saving_folder(&git, dir.path()).expect("a new project");
        // Over 1 MB, and over 5,000 lines.
        std::fs::write(dir.path().join("big.txt"), "linha\n".repeat(200_000)).unwrap();
        std::fs::write(dir.path().join("small.txt"), "a\nb").unwrap();

        let shown = load(&project, &now_target("big.txt"), false).unwrap();
        assert!(matches!(shown.result.view, DiffView::TooLarge { .. }));
        assert!(shown.result.before_missing);
        assert_eq!(shown.after_title, "Depois · Agora");
        let shown = load(&project, &now_target("big.txt"), true).unwrap();
        assert!(matches!(shown.result.view, DiffView::Blocks(_)));
        assert_eq!(shown.no_final_newline, (false, false));

        let shown = load(&project, &now_target("small.txt"), false).unwrap();
        assert!(matches!(shown.result.view, DiffView::Blocks(_)));
        assert_eq!(shown.no_final_newline, (false, true));

        let gone = load(&project, &now_target("gone.txt"), false).unwrap();
        assert!(gone.result.after_missing);
        assert_eq!(gone.result.view, DiffView::Identical);
    }
}
