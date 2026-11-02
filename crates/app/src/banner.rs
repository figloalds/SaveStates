//! The read-only banner of a project tab: which texts explain the project's condition.

use savestates_core::{Blocker, RepoCondition};

use crate::tr_key;

/// The translation keys of the texts that explain `c`, one per reason, in the order given.
/// A normal project has none.
pub fn banner_keys(c: &RepoCondition) -> Vec<&'static str> {
    match c {
        RepoCondition::Normal => Vec::new(),
        RepoCondition::ReadOnly(blockers) => blockers.iter().map(blocker_key).collect(),
        RepoCondition::Broken { .. } => vec![tr_key("condition.broken")],
    }
}

fn blocker_key(b: &Blocker) -> &'static str {
    match b {
        Blocker::Merge => tr_key("condition.merge"),
        Blocker::Rebase => tr_key("condition.rebase"),
        Blocker::CherryPick => tr_key("condition.cherry_pick"),
        Blocker::Revert => tr_key("condition.revert"),
        Blocker::Bisect => tr_key("condition.bisect"),
        Blocker::Detached => tr_key("condition.detached"),
        Blocker::Conflicts => tr_key("condition.conflicts"),
        Blocker::Bare => tr_key("condition.bare"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use savestates_core::Blocker;

    #[test]
    fn each_blocker_gives_its_text_in_order() {
        let c = RepoCondition::ReadOnly(vec![Blocker::Merge, Blocker::Conflicts]);
        assert_eq!(banner_keys(&c), ["condition.merge", "condition.conflicts"]);
    }

    #[test]
    fn a_normal_project_has_no_banner() {
        assert!(banner_keys(&RepoCondition::Normal).is_empty());
    }

    #[test]
    fn every_reason_has_its_own_text() {
        let all = [
            (Blocker::Merge, "condition.merge"),
            (Blocker::Rebase, "condition.rebase"),
            (Blocker::CherryPick, "condition.cherry_pick"),
            (Blocker::Revert, "condition.revert"),
            (Blocker::Bisect, "condition.bisect"),
            (Blocker::Detached, "condition.detached"),
            (Blocker::Conflicts, "condition.conflicts"),
            (Blocker::Bare, "condition.bare"),
        ];
        for (blocker, key) in all {
            assert_eq!(banner_keys(&RepoCondition::ReadOnly(vec![blocker])), [key]);
        }
        let broken = RepoCondition::Broken {
            details: "fatal: bad object HEAD".to_owned(),
        };
        assert_eq!(banner_keys(&broken), ["condition.broken"]);
    }
}
