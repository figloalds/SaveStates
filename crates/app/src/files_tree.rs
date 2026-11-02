//! The folder tree of the "All files" tab of peek: a save's files as folders and files, and the
//! filter box over them.

use std::cmp::Ordering;

/// A folder or a file of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeNode {
    /// The folder's or the file's own name.
    pub name: String,
    /// A file's path, as git gives it (`css/estilo.css`); `None` for a folder.
    pub path: Option<String>,
    /// What a folder holds, sorted like the top level.
    pub children: Vec<TreeNode>,
}

/// The tree of `paths` (files, as git gives them: relative, with `/`). In every folder, the
/// folders come first, then the files, each in alphabetical order without regard to letter case
/// (names that differ only in case: lower case first).
pub fn build_tree(paths: &[String]) -> Vec<TreeNode> {
    let mut top = Vec::new();
    for path in paths {
        let mut level = &mut top;
        let mut parts = path.split('/').filter(|part| !part.is_empty()).peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                level.push(TreeNode {
                    name: part.to_owned(),
                    path: Some(path.clone()),
                    children: Vec::new(),
                });
                break;
            }
            let at = match level
                .iter()
                .position(|node| node.path.is_none() && node.name == part)
            {
                Some(at) => at,
                None => {
                    level.push(TreeNode {
                        name: part.to_owned(),
                        path: None,
                        children: Vec::new(),
                    });
                    level.len() - 1
                }
            };
            level = &mut level[at].children;
        }
    }
    sort(&mut top);
    top
}

fn sort(nodes: &mut [TreeNode]) {
    nodes.sort_by(|a, b| {
        let folders_first = b.path.is_none().cmp(&a.path.is_none());
        folders_first.then_with(|| by_name(&a.name, &b.name))
    });
    for node in nodes {
        sort(&mut node.children);
    }
}

/// Alphabetical without regard to letter case; between names that differ only in case, lower
/// case first (the reverse of the byte order, where upper case comes first).
fn by_name(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| b.cmp(a))
}

/// The paths that contain `query`, without regard to letter case, in their order. An empty
/// query keeps them all.
pub fn filter_paths(paths: &[String], query: &str) -> Vec<String> {
    let query = query.to_lowercase();
    paths
        .iter()
        .filter(|path| path.to_lowercase().contains(&query))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| (*p).to_owned()).collect()
    }

    fn file(name: &str, path: &str) -> TreeNode {
        TreeNode {
            name: name.to_owned(),
            path: Some(path.to_owned()),
            children: Vec::new(),
        }
    }

    fn folder(name: &str, children: Vec<TreeNode>) -> TreeNode {
        TreeNode {
            name: name.to_owned(),
            path: None,
            children,
        }
    }

    #[test]
    fn folders_come_first_then_files_in_case_insensitive_order() {
        let tree = build_tree(&strings(&["b.md", "a/x.rs", "A/y.rs", "c/d/e.txt"]));
        assert_eq!(
            tree,
            [
                folder("a", vec![file("x.rs", "a/x.rs")]),
                folder("A", vec![file("y.rs", "A/y.rs")]),
                folder("c", vec![folder("d", vec![file("e.txt", "c/d/e.txt")])]),
                file("b.md", "b.md"),
            ]
        );
    }

    #[test]
    fn inside_a_folder_too_and_whatever_order_git_gives() {
        let tree = build_tree(&strings(&[
            "src/Zeta.rs",
            "src/alpha.rs",
            "src/util/mod.rs",
            "README.md",
            "LICENSE",
        ]));
        assert_eq!(
            tree,
            [
                folder(
                    "src",
                    vec![
                        folder("util", vec![file("mod.rs", "src/util/mod.rs")]),
                        file("alpha.rs", "src/alpha.rs"),
                        file("Zeta.rs", "src/Zeta.rs"),
                    ]
                ),
                file("LICENSE", "LICENSE"),
                file("README.md", "README.md"),
            ]
        );
        // Names that differ only in letter case: lower case first, as git's order would not.
        let tree = build_tree(&strings(&["A/y.rs", "a/x.rs"]));
        assert_eq!(tree[0].name, "a");
        assert_eq!(tree[1].name, "A");
        assert!(build_tree(&[]).is_empty());
    }

    #[test]
    fn the_filter_ignores_letter_case_and_an_empty_one_keeps_everything() {
        let paths = strings(&["b.md", "a/x.rs", "A/y.rs", "c/d/e.txt"]);
        assert_eq!(filter_paths(&paths, "X.R"), ["a/x.rs"]);
        assert_eq!(filter_paths(&paths, "c/D"), ["c/d/e.txt"]);
        assert_eq!(filter_paths(&paths, ".RS"), ["a/x.rs", "A/y.rs"]);
        assert_eq!(filter_paths(&paths, ""), paths);
        assert!(filter_paths(&paths, "nada").is_empty());
        // Accented letters too.
        let paths = strings(&["paginas/Ação.html", "paginas/sobre.html"]);
        assert_eq!(filter_paths(&paths, "AÇÃO"), ["paginas/Ação.html"]);
    }
}
