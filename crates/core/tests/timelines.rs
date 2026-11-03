use savestates_core::{
    HISTORY_PAGE, OpenOutcome, Project, SaveId, is_empty_app_timeline, list_timelines,
    open_project, read_older_saves, read_timelines, snapshot, sweep_empty_timelines,
};
use savestates_git::config_get;
use testkit::TestRepo;

fn open(repo: &TestRepo) -> Project {
    match open_project(&repo.exe(), repo.path()).unwrap() {
        OpenOutcome::Opened(project) => project,
        other => panic!("expected Opened, got {other:?}"),
    }
}

fn save(repo: &TestRepo, file: &str, msg: &str) -> String {
    repo.write(file, msg);
    repo.commit_all(msg)
}

fn main_lines(repo: &TestRepo) -> Vec<String> {
    list_timelines(&open(repo))
        .unwrap()
        .into_iter()
        .filter(|t| t.main_line)
        .map(|t| t.branch)
        .collect()
}

#[test]
fn the_main_line_is_remembered_at_first_read() {
    let repo = TestRepo::new();
    save(&repo, "a", "one");
    repo.git(&["branch", "savestates/x"]);
    assert_eq!(main_lines(&repo), ["main"]);
    assert_eq!(
        config_get(&repo.repo(), "savestates.main-line")
            .unwrap()
            .as_deref(),
        Some("main")
    );
}

#[test]
fn opened_on_an_app_branch_the_main_line_is_main() {
    let repo = TestRepo::new();
    save(&repo, "a", "one");
    repo.git(&["switch", "-q", "-c", "savestates/x"]);
    save(&repo, "b", "two");
    assert_eq!(main_lines(&repo), ["main"]);
}

#[test]
fn without_main_or_master_the_longest_line_is_the_main_line() {
    let repo = TestRepo::new();
    repo.git(&["switch", "-q", "-c", "a"]);
    for i in 0..3 {
        save(&repo, &format!("a{i}"), &format!("a{i}"));
    }
    repo.git(&["switch", "-q", "-c", "b"]);
    for i in 0..2 {
        save(&repo, &format!("b{i}"), &format!("b{i}"));
    }
    repo.git(&["switch", "-q", "-c", "savestates/x"]);
    assert_eq!(main_lines(&repo), ["b"]);
}

#[test]
fn a_missing_main_line_falls_back_without_rewriting() {
    let repo = TestRepo::new();
    save(&repo, "a", "one");
    repo.git(&["config", "savestates.main-line", "gone"]);
    assert_eq!(main_lines(&repo), ["main"]);
    assert_eq!(
        config_get(&repo.repo(), "savestates.main-line")
            .unwrap()
            .as_deref(),
        Some("gone")
    );
}

#[test]
fn an_app_timeline_is_named_after_its_first_own_save() {
    let repo = TestRepo::new();
    save(&repo, "a", "base");
    repo.git(&["switch", "-q", "-c", "savestates/x"]);
    save(&repo, "b", "Versão com menu lateral");
    save(&repo, "c", "Ajustes");
    repo.git(&["switch", "-q", "main"]);
    repo.git(&["switch", "-q", "-c", "develop"]);
    save(&repo, "d", "dev work");
    repo.git(&["switch", "-q", "main"]);
    repo.git(&["switch", "-q", "-c", "savestates/named"]);
    save(&repo, "e", "ignored subject");
    repo.git(&["config", "branch.savestates/named.description", "Mine"]);
    repo.git(&["switch", "-q", "main"]);

    let timelines = list_timelines(&open(&repo)).unwrap();
    let name = |b: &str| {
        timelines
            .iter()
            .find(|t| t.branch == b)
            .unwrap()
            .display_name
            .clone()
    };
    assert_eq!(
        name("savestates/x").as_deref(),
        Some("Versão com menu lateral")
    );
    assert_eq!(name("develop"), None);
    assert_eq!(name("savestates/named").as_deref(), Some("Mine"));
    assert_eq!(
        config_get(&repo.repo(), "branch.savestates/x.description")
            .unwrap()
            .as_deref(),
        Some("Versão com menu lateral")
    );
    assert_eq!(
        config_get(&repo.repo(), "branch.develop.description").unwrap(),
        None
    );
}

#[test]
fn history_comes_in_pages() {
    let repo = TestRepo::new();
    for i in 0..7 {
        save(&repo, &format!("f{i}"), &format!("s{i}"));
    }
    let p = open(&repo);
    let view = read_timelines(&p, 5).unwrap();
    assert_eq!(view.saves.len(), 5);
    assert!(view.more_available);
    assert!(view.timelines[0].current);
    let older = read_older_saves(&p, 5, 5).unwrap();
    let names: Vec<_> = older.iter().map(|g| g.save.name.as_str()).collect();
    assert_eq!(names, ["s1", "s0"]);
}

#[test]
fn the_sweep_removes_only_empty_app_timelines() {
    let repo = TestRepo::new();
    save(&repo, "a", "m1");
    let mid = save(&repo, "b", "m2");
    save(&repo, "c", "m3");
    repo.git(&["branch", "savestates/empty", &mid]);
    repo.git(&["branch", "backup", &mid]);
    repo.git(&["branch", "savestates/wt", &mid]);
    repo.git(&["branch", "savestates/here", &mid]);
    repo.git(&["switch", "-q", "-c", "savestates/kept", &mid]);
    save(&repo, "k", "own");
    repo.git(&["switch", "-q", "-c", "savestates/merged", "main"]);
    save(&repo, "m", "merged work");
    repo.git(&["switch", "-q", "main"]);
    repo.write("z", "z");
    repo.commit_all("m4");
    repo.git(&["merge", "-q", "--no-ff", "-m", "join", "savestates/merged"]);
    let wt = repo.path().parent().unwrap().join(format!(
        "wt-{}",
        repo.path().file_name().unwrap().to_string_lossy()
    ));
    repo.git(&[
        "worktree",
        "add",
        "-q",
        wt.to_str().unwrap(),
        "savestates/wt",
    ]);
    repo.git(&["switch", "-q", "savestates/here"]);

    let p = open(&repo);
    assert!(is_empty_app_timeline(&p, "savestates/empty").unwrap());
    assert!(!is_empty_app_timeline(&p, "backup").unwrap());
    let removed = sweep_empty_timelines(&p).unwrap();
    assert_eq!(removed, ["savestates/empty"]);
    let left = repo.git(&["branch", "--format=%(refname:short)"]);
    for b in [
        "savestates/kept",
        "savestates/merged",
        "backup",
        "savestates/wt",
        "savestates/here",
    ] {
        assert!(left.lines().any(|l| l == b), "{b} should remain");
    }
    let t = list_timelines(&p).unwrap();
    assert!(
        t.iter()
            .find(|t| t.branch == "savestates/wt")
            .unwrap()
            .elsewhere
            .is_some(),
    );
    let _ = std::fs::remove_dir_all(wt);
}

#[test]
fn a_crash_after_load_is_tidied_by_the_next_sweep() {
    let repo = TestRepo::new();
    save(&repo, "a", "one");
    let mid = save(&repo, "b", "two");
    save(&repo, "c", "three");
    repo.git(&["switch", "-q", "-c", "savestates/x", &mid]);
    repo.git(&["switch", "-q", "main"]);
    assert_eq!(
        sweep_empty_timelines(&open(&repo)).unwrap(),
        ["savestates/x"]
    );
}

#[test]
fn a_bare_repository_shows_its_timelines_and_writes_nothing() {
    let source = TestRepo::new();
    let first = save(&source, "a", "one");
    save(&source, "a", "two");
    let tip = save(&source, "a", "three");
    source.git(&["switch", "-q", "-c", "savestates/x"]);
    save(&source, "x", "x1");
    source.git(&["switch", "-q", "main"]);
    let bare = TestRepo::empty_dir();
    let from = source.path().to_string_lossy().into_owned();
    bare.git(&["clone", "-q", "--bare", &from, "."]);
    let p = open(&bare);

    let view = read_timelines(&p, HISTORY_PAGE).unwrap();
    let branches: Vec<(&str, bool, bool)> = view
        .timelines
        .iter()
        .map(|t| (t.branch.as_str(), t.main_line, t.current))
        .collect();
    assert_eq!(
        branches,
        [("main", true, true), ("savestates/x", false, false)]
    );
    assert_eq!(view.head, Some(SaveId(tip.clone())));
    assert!(!view.detached);
    let names: Vec<&str> = view.saves.iter().map(|s| s.save.name.as_str()).collect();
    assert_eq!(names, ["x1", "three", "two", "one"]);
    // Only looked at: no main-line setting and no first-sight name are written.
    let repo = bare.repo();
    assert_eq!(config_get(&repo, "savestates.main-line").unwrap(), None);
    assert_eq!(
        config_get(&repo, "branch.savestates/x.description").unwrap(),
        None
    );
    // The first own save still names the timeline on screen.
    assert_eq!(view.timelines[1].display_name.as_deref(), Some("x1"));

    // The timelines key follows branches moved (a push into the bare repository, say).
    let before = snapshot(&p).timelines_key;
    assert!(!before.is_empty());
    assert_eq!(snapshot(&p).timelines_key, before);
    bare.git(&["update-ref", "refs/heads/main", &first]);
    let moved = snapshot(&p).timelines_key;
    assert_ne!(moved, before);
    bare.git(&["branch", "other", &tip]);
    assert_ne!(snapshot(&p).timelines_key, moved);
}
