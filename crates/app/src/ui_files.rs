//! The layout and theme files in `crates/app/ui`.
//!
//! Release builds use the copies embedded at build time. Debug builds read the file from disk
//! each time it is asked for, so an edited layout shows on the next start (or the next tab)
//! without a rebuild; they fall back to the embedded copy when the source tree is not there.

/// Every file, embedded.
const FILES: &[(&str, &str)] = &[
    ("main.xml", include_str!("../ui/main.xml")),
    ("start.xml", include_str!("../ui/start.xml")),
    ("project_tab.xml", include_str!("../ui/project_tab.xml")),
    ("theme.xml", include_str!("../ui/theme.xml")),
    ("dialog_name.xml", include_str!("../ui/dialog_name.xml")),
    (
        "dialog_warnings.xml",
        include_str!("../ui/dialog_warnings.xml"),
    ),
    (
        "dialog_identity.xml",
        include_str!("../ui/dialog_identity.xml"),
    ),
    ("ai_spinner.xml", include_str!("../ui/ai_spinner.xml")),
    (
        "dialog_recovery.xml",
        include_str!("../ui/dialog_recovery.xml"),
    ),
    (
        "dialog_settings.xml",
        include_str!("../ui/dialog_settings.xml"),
    ),
    ("menu_changes.xml", include_str!("../ui/menu_changes.xml")),
    ("menu_lane.xml", include_str!("../ui/menu_lane.xml")),
    ("dialog_rename.xml", include_str!("../ui/dialog_rename.xml")),
    (
        "dialog_archived.xml",
        include_str!("../ui/dialog_archived.xml"),
    ),
    ("menu_tabs.xml", include_str!("../ui/menu_tabs.xml")),
];

/// The text of `crates/app/ui/<name>`, or "" for a name that is not one of the files.
pub fn source(name: &str) -> String {
    #[cfg(debug_assertions)]
    {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("ui")
            .join(name);
        if let Ok(text) = std::fs::read_to_string(path) {
            return text;
        }
    }
    FILES
        .iter()
        .find(|(file, _)| *file == name)
        .map_or_else(String::new, |(_, text)| (*text).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_is_found_and_unknown_names_give_nothing() {
        for (name, embedded) in FILES {
            assert!(!embedded.is_empty(), "{name} is empty");
            assert_eq!(
                source(name),
                *embedded,
                "{name} on disk is the embedded one"
            );
        }
        assert_eq!(source("nope.xml"), "");
    }
}
