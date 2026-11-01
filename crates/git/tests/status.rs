use savestates_git::{EntryKind, StatusEntry, parse_status_v2, status};
use testkit::TestRepo;

/// Joins records the way `git status --porcelain=v2 -z` does: every record ends in NUL.
fn nul_separated(records: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for record in records {
        out.extend_from_slice(record.as_bytes());
        out.push(0);
    }
    out
}

fn entry(path: &str, kind: EntryKind, orig_path: Option<&str>) -> StatusEntry {
    StatusEntry {
        path: path.to_owned(),
        kind,
        orig_path: orig_path.map(str::to_owned),
    }
}

#[test]
fn parses_fixture() {
    let bytes = nul_separated(&[
        "# branch.oid (initial)",
        "# branch.head main",
        "1 .M N... 100644 100644 100644 1111111 2222222 src/a.txt",
        "? new file.md",
        "2 R. N... 100644 100644 100644 1111111 1111111 R100 b.txt",
        "a.txt",
        "u UU N... 100644 100644 100644 100644 1111111 2222222 3333333 c.txt",
    ]);

    let status = parse_status_v2(&bytes);

    assert_eq!(status.head_oid, None);
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(
        status.entries,
        vec![
            entry("src/a.txt", EntryKind::Modified, None),
            entry("new file.md", EntryKind::Untracked, None),
            entry("b.txt", EntryKind::Renamed, Some("a.txt")),
            entry("c.txt", EntryKind::Unmerged, None),
        ]
    );
}

#[test]
fn detached_header() {
    let oid = "0123456789abcdef0123456789abcdef01234567";
    let bytes = nul_separated(&[&format!("# branch.oid {oid}"), "# branch.head (detached)"]);

    let status = parse_status_v2(&bytes);

    assert_eq!(status.branch, None);
    assert_eq!(status.head_oid.as_deref(), Some(oid));
    assert!(status.entries.is_empty());
}

#[test]
fn spaces_in_every_record_kind() {
    let bytes = nul_separated(&[
        "# branch.oid (initial)",
        "# branch.head main",
        "1 .M N... 100644 100644 100644 1111111 2222222 my dir/my file.txt",
        "2 R. N... 100644 100644 100644 1111111 1111111 R100 new name.txt",
        "old name.txt",
        "u UU N... 100644 100644 100644 100644 1111111 2222222 3333333 conflict file.txt",
        "? untracked dir/other file.md",
    ]);

    let status = parse_status_v2(&bytes);

    assert_eq!(
        status.entries,
        vec![
            entry("my dir/my file.txt", EntryKind::Modified, None),
            entry("new name.txt", EntryKind::Renamed, Some("old name.txt")),
            entry("conflict file.txt", EntryKind::Unmerged, None),
            entry("untracked dir/other file.md", EntryKind::Untracked, None),
        ]
    );
}

#[test]
fn ordinary_record_kind_precedence_is_delete_add_modify_typechange() {
    let kind_of = |xy: &str| {
        let record = format!("1 {xy} N... 100644 100644 100644 1111111 2222222 f.txt");
        let status = parse_status_v2(&nul_separated(&[&record]));
        assert_eq!(status.entries.len(), 1, "record for {xy}");
        status.entries[0].kind
    };

    assert_eq!(kind_of(".M"), EntryKind::Modified);
    assert_eq!(kind_of("M."), EntryKind::Modified);
    assert_eq!(kind_of("MM"), EntryKind::Modified);
    assert_eq!(kind_of(".T"), EntryKind::TypeChanged);
    assert_eq!(kind_of("T."), EntryKind::TypeChanged);
    assert_eq!(kind_of("A."), EntryKind::Added);
    assert_eq!(kind_of(".A"), EntryKind::Added);
    assert_eq!(kind_of("AM"), EntryKind::Added);
    assert_eq!(kind_of("D."), EntryKind::Deleted);
    assert_eq!(kind_of(".D"), EntryKind::Deleted);
    assert_eq!(kind_of("AD"), EntryKind::Deleted);
    assert_eq!(kind_of("MD"), EntryKind::Deleted);
    assert_eq!(kind_of("TM"), EntryKind::Modified);
    assert_eq!(kind_of("TA"), EntryKind::Added);
}

#[test]
fn accented_paths_round_trip() {
    let repo = TestRepo::new();
    repo.write("Área de Trabalho/Ação 1.md", "texto");

    let status = status(&repo.repo()).unwrap();

    assert_eq!(status.head_oid, None);
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(
        status.entries,
        vec![entry(
            "Área de Trabalho/Ação 1.md",
            EntryKind::Untracked,
            None
        )]
    );
}
