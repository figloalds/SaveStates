use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use savestates_core::settings::{
    MAX_RECENT, NamingMode, Settings, WindowGeom, default_path, load, save,
};
use savestates_i18n::LanguageChoice;

#[test]
fn defaults_match_spec() {
    let s = Settings::default();
    assert_eq!(s.naming, NamingMode::AskMe);
    assert_eq!(s.ai_model, "haiku");
    assert_eq!(s.ai_timeout_secs, 30);
    assert_eq!(s.language, LanguageChoice::System);
    assert_eq!(s.large_file_mb, 25);
    assert!(!s.ai_notice_shown);
    assert!(s.open_tabs.is_empty());
    assert_eq!(s.active_tab, 0);
    assert!(s.recent.is_empty());
    assert_eq!(s.window, None);
    assert!(s.timeline_day_px.is_empty());
    assert_eq!(MAX_RECENT, 10);
}

#[test]
fn round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    let mut day_px = BTreeMap::new();
    day_px.insert("C:\\Users\\ana\\Meu Projeto".to_owned(), 140.0_f32);
    day_px.insert("D:\\outro".to_owned(), 57_600.0_f32);
    let original = Settings {
        naming: NamingMode::Ai,
        ai_model: "sonnet".to_owned(),
        ai_timeout_secs: 45,
        language: LanguageChoice::PtBr,
        large_file_mb: 50,
        ai_notice_shown: true,
        open_tabs: vec![
            PathBuf::from("C:\\Users\\ana\\Meu Projeto"),
            PathBuf::from("D:\\outro"),
        ],
        active_tab: 1,
        recent: vec![PathBuf::from("D:\\outro"), PathBuf::from("E:\\antigo")],
        window: Some(WindowGeom {
            x: -8,
            y: 12,
            width: 1280,
            height: 720,
            maximized: true,
        }),
        timeline_day_px: day_px,
    };

    save(&path, &original).unwrap();
    let loaded = load(&path);

    assert_eq!(loaded, original);
}

#[test]
fn day_px_replaces_old_zoom_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    // A wave-1 zoom meant something else: it is not read as a day width.
    fs::write(&path, "timeline_zoom = { \"C:/p\" = 1.5 }\n").unwrap();
    let loaded = load(&path);
    assert!(loaded.timeline_day_px.is_empty());
    assert_eq!(loaded, Settings::default());
    assert!(path.exists(), "a readable file is not moved aside");

    // The day width round-trips, and the old key is gone from the file once saved.
    let mut settings = loaded;
    settings
        .timeline_day_px
        .insert("C:/p".to_owned(), 1_440.0_f32);
    save(&path, &settings).unwrap();
    let written = fs::read_to_string(&path).unwrap();
    assert!(!written.contains("timeline_zoom"), "{written}");
    assert!(written.contains("timeline_day_px"), "{written}");
    assert_eq!(load(&path).timeline_day_px.get("C:/p"), Some(&1_440.0_f32));
}

#[test]
fn language_names_are_pinned_in_the_file() {
    let parsed: Settings = toml::from_str("language = \"PtBr\"\n").unwrap();
    assert_eq!(parsed.language, LanguageChoice::PtBr);
    let parsed: Settings = toml::from_str("language = \"En\"\n").unwrap();
    assert_eq!(parsed.language, LanguageChoice::En);
    let parsed: Settings = toml::from_str("language = \"System\"\n").unwrap();
    assert_eq!(parsed.language, LanguageChoice::System);

    let written = toml::to_string(&Settings {
        language: LanguageChoice::PtBr,
        naming: NamingMode::Ai,
        ..Settings::default()
    })
    .unwrap();
    assert!(written.contains("language = \"PtBr\""), "{written}");
    assert!(written.contains("naming = \"Ai\""), "{written}");
}

#[test]
fn missing_file_gives_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nope").join("settings.toml");

    assert_eq!(load(&path), Settings::default());
    assert!(!path.exists());
}

#[test]
fn corrupt_file_backed_up() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    fs::write(&path, "not = [toml").unwrap();

    assert_eq!(load(&path), Settings::default());

    assert!(!path.exists());
    let bad = dir.path().join("settings.toml.bad");
    assert_eq!(fs::read_to_string(bad).unwrap(), "not = [toml");
}

#[test]
fn corrupt_file_replaces_an_older_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    let bad = dir.path().join("settings.toml.bad");
    fs::write(&bad, "old garbage").unwrap();
    fs::write(&path, "also = [broken").unwrap();

    assert_eq!(load(&path), Settings::default());

    assert!(!path.exists());
    assert_eq!(fs::read_to_string(bad).unwrap(), "also = [broken");
}

#[test]
fn wrong_field_type_counts_as_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    fs::write(&path, "active_tab = \"two\"\n").unwrap();

    assert_eq!(load(&path), Settings::default());
    assert!(dir.path().join("settings.toml.bad").exists());
}

#[test]
fn unreadable_file_gives_defaults_without_panicking() {
    let dir = tempfile::tempdir().unwrap();
    // A folder where the file should be cannot be read as a file.
    let path = dir.path().join("settings.toml");
    fs::create_dir(&path).unwrap();

    assert_eq!(load(&path), Settings::default());
}

#[test]
fn unknown_and_missing_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    fs::write(&path, "ai_model = \"sonnet\"\nfoo = 1\n").unwrap();

    let loaded = load(&path);

    let expected = Settings {
        ai_model: "sonnet".to_owned(),
        ..Settings::default()
    };
    assert_eq!(loaded, expected);
    assert!(path.exists(), "a readable file is not moved aside");
    assert!(!dir.path().join("settings.toml.bad").exists());
}

#[test]
fn recent_dedupes_and_caps() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Settings::default();
    let mut folders = Vec::new();
    for i in 0..12 {
        let folder = dir.path().join(format!("proj{i}"));
        fs::create_dir(&folder).unwrap();
        folders.push(folder);
    }
    for folder in &folders {
        s.push_recent(folder.clone());
    }
    // proj5 is still in the list (the newest ten are proj2..proj11); push it again.
    s.push_recent(folders[5].clone());

    assert_eq!(s.recent.len(), MAX_RECENT);
    assert_eq!(s.recent[0], folders[5]);
    let expected_tail: Vec<&PathBuf> = [11, 10, 9, 8, 7, 6, 4, 3, 2]
        .iter()
        .map(|&i| &folders[i])
        .collect();
    assert_eq!(s.recent[1..].iter().collect::<Vec<_>>(), expected_tail);
    let mut unique = s.recent.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), s.recent.len());
}

#[test]
fn recent_treats_different_spellings_of_one_folder_as_the_same() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("proj");
    fs::create_dir(&folder).unwrap();
    let other = dir.path().join("other");
    fs::create_dir(&other).unwrap();
    let roundabout = dir.path().join("other").join("..").join("proj");

    let mut s = Settings::default();
    s.push_recent(folder.clone());
    s.push_recent(other.clone());
    s.push_recent(roundabout.clone());

    assert_eq!(s.recent, vec![roundabout, other]);
}

#[test]
fn recent_keeps_paths_that_do_not_exist() {
    let dir = tempfile::tempdir().unwrap();
    let gone = dir.path().join("gone");
    let mut s = Settings::default();

    s.push_recent(gone.clone());
    s.push_recent(gone.clone());

    assert_eq!(s.recent, vec![gone]);
}

#[test]
fn save_leaves_no_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("SaveStates").join("settings.toml");

    save(&path, &Settings::default()).unwrap();
    save(
        &path,
        &Settings {
            ai_model: "sonnet".to_owned(),
            ..Settings::default()
        },
    )
    .unwrap();

    let names: Vec<String> = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["settings.toml".to_owned()]);
    assert_eq!(load(&path).ai_model, "sonnet");
}

#[test]
fn failed_save_reports_the_error_and_leaves_no_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    // A folder where the file should go cannot be replaced by a file.
    let path = dir.path().join("settings.toml");
    fs::create_dir(&path).unwrap();

    assert!(save(&path, &Settings::default()).is_err());

    assert!(path.is_dir());
    assert!(!dir.path().join("settings.toml.tmp").exists());
}

#[test]
fn default_path_is_settings_toml_under_savestates() {
    let path = default_path();

    assert_eq!(path.file_name().unwrap(), "settings.toml");
    assert!(
        path.parent().unwrap().ends_with("SaveStates"),
        "{}",
        path.display()
    );
}

#[cfg(windows)]
#[test]
fn default_path_is_in_appdata_savestates() {
    let appdata = std::env::var_os("APPDATA").expect("APPDATA is set on Windows");

    assert_eq!(
        default_path(),
        PathBuf::from(appdata)
            .join("SaveStates")
            .join("settings.toml")
    );
}
