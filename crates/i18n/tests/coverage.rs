use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use savestates_i18n::TRANSLATIONS_CSV;

fn app_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../app")
}

/// Every file with the given extension under `dir`. A missing directory holds no files.
fn files_with_extension(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return found;
    };
    for entry in entries {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            found.extend(files_with_extension(&path, extension));
        } else if path.extension().is_some_and(|e| e == extension) {
            found.push(path);
        }
    }
    found
}

fn csv_keys() -> HashSet<String> {
    csv::Reader::from_reader(TRANSLATIONS_CSV.as_bytes())
        .records()
        .map(|r| r.expect("record").get(0).expect("key column").to_string())
        .collect()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// Keys a layout file refers to as `"@some.key"`.
fn xml_keys(text: &str) -> Vec<String> {
    let reference = Regex::new(r#""@([a-z0-9_.]+)""#).expect("regex");
    reference
        .captures_iter(text)
        .map(|cap| cap[1].to_string())
        .collect()
}

/// Table keys a Rust source needs for its `tr`, `tr_fmt` and `tr_plural` calls, and for the keys
/// it marks with `tr_key` (looked up later, by a widget or from a table of keys).
/// A plural call needs both its `.one` and its `.other` form.
fn rust_keys(text: &str) -> Vec<String> {
    let call = Regex::new(r#"\btr(_fmt|_plural|_key)?\(\s*"([a-z0-9_.]+)""#).expect("regex");
    let mut keys = Vec::new();
    for cap in call.captures_iter(text) {
        let key = &cap[2];
        if cap.get(1).is_some_and(|m| m.as_str() == "_plural") {
            keys.push(format!("{key}.one"));
            keys.push(format!("{key}.other"));
        } else {
            keys.push(key.to_string());
        }
    }
    keys
}

#[test]
fn every_key_the_app_uses_exists_in_the_table() {
    let table = csv_keys();
    let mut missing = Vec::new();

    for path in files_with_extension(&app_dir().join("ui"), "xml") {
        for key in xml_keys(&read(&path)) {
            if !table.contains(&key) {
                missing.push(format!("{}: {key}", path.display()));
            }
        }
    }
    for path in files_with_extension(&app_dir().join("src"), "rs") {
        for key in rust_keys(&read(&path)) {
            if !table.contains(&key) {
                missing.push(format!("{}: {key}", path.display()));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "keys missing from translations.csv:\n{}",
        missing.join("\n")
    );
}

#[test]
fn the_scanner_finds_keys_in_layouts_and_sources() {
    let xml =
        r#"<Label text="@timeline.title"/> <Button label="@project.open"/> <Label text="plain"/>"#;
    assert_eq!(xml_keys(xml), ["timeline.title", "project.open"]);

    let rs = r#"
        let a = tr("save.quick");
        let b = tr_fmt( "peek.banner", &["x", "y"]);
        let c = tr_plural(
            "changes.unsaved", n);
        let d = str("not.a.key");
        let e = tr_key("timeline.title");
        let f = attr_key("not.a.key.either");
    "#;
    assert_eq!(
        rust_keys(rs),
        [
            "save.quick",
            "peek.banner",
            "changes.unsaved.one",
            "changes.unsaved.other",
            "timeline.title"
        ]
    );
}

#[test]
fn the_scanner_reports_a_key_that_is_not_in_the_table() {
    let table = csv_keys();
    assert!(
        rust_keys(r#"tr("no.such.key")"#)
            .iter()
            .any(|k| !table.contains(k))
    );
    assert!(
        rust_keys(r#"tr_key("no.such.key")"#)
            .iter()
            .any(|k| !table.contains(k))
    );
    assert!(
        rust_keys(r#"tr_plural("save.noun", 2)"#)
            .iter()
            .all(|k| table.contains(k))
    );
}

#[test]
fn a_missing_app_directory_counts_as_no_files() {
    assert!(files_with_extension(&app_dir().join("no-such-dir"), "xml").is_empty());
}
