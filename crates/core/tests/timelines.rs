use savestates_core::{
    OpenOutcome, Project, is_empty_app_timeline, list_timelines, open_project, read_older_saves,
    read_timelines, sweep_empty_timelines,
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
    assert_eq!(
        t.iter()
            .find(|t| t.branch == "savestates/wt")
            .unwrap()
            .elsewhere
            .is_some(),
        true
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
