//! Locating `git` and running it with a fixed, machine-parseable environment.

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Windows process creation flag that keeps a console window from flashing up.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Variables that would make git work on another repository than the one it runs in.
const REPOSITORY_ENV: [&str; 3] = ["GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Oldest git this crate supports.
pub const MIN_VERSION: Version = Version {
    major: 2,
    minor: 31,
    patch: 0,
};

/// Parses the output of `git --version`, e.g. `git version 2.50.1.windows.1`.
pub fn parse_version(s: &str) -> Option<Version> {
    let rest = s.trim().strip_prefix("git version ")?;
    let mut parts = rest.split('.');
    let major = leading_number(parts.next()?)?;
    let minor = leading_number(parts.next()?)?;
    let patch = parts.next().and_then(leading_number).unwrap_or(0);
    Some(Version {
        major,
        minor,
        patch,
    })
}

fn leading_number(s: &str) -> Option<u32> {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    s[..end].parse().ok()
}

#[derive(thiserror::Error, Debug)]
pub enum GitError {
    #[error("git executable not found")]
    NotFound,
    #[error("git {0} is too old (minimum {MIN_VERSION})")]
    TooOld(Version),
    #[error("not a git repository")]
    NotARepo,
    #[error("repository is locked by another git process (index.lock)")]
    Locked,
    #[error("git {args} failed (exit code {code:?}): {stderr}")]
    Failed {
        args: String,
        code: Option<i32>,
        stderr: String,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug)]
pub struct GitExe {
    pub path: PathBuf,
    pub version: Version,
}

/// Finds a supported `git`: PATH first, then the standard Git for Windows install locations.
///
/// The first candidate that is at least [`MIN_VERSION`] wins. If candidates exist but all are
/// older, the first one's version is reported as [`GitError::TooOld`].
pub fn locate() -> Result<GitExe, GitError> {
    let mut candidates = Vec::new();
    if let Ok(on_path) = which::which("git") {
        candidates.push(on_path);
    }
    candidates.push(PathBuf::from(r"C:\Program Files\Git\cmd\git.exe"));
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join(r"Programs\Git\cmd\git.exe"));
    }

    let mut too_old = None;
    for path in candidates.into_iter().filter(|p| p.is_file()) {
        let Some(version) = probe_version(&path) else {
            continue;
        };
        if version >= MIN_VERSION {
            return Ok(GitExe { path, version });
        }
        too_old.get_or_insert(version);
    }
    Err(too_old.map_or(GitError::NotFound, GitError::TooOld))
}

fn probe_version(path: &Path) -> Option<Version> {
    let mut cmd = Command::new(path);
    cmd.arg("--version").stdin(Stdio::null());
    hide_window(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_version(&String::from_utf8_lossy(&out.stdout))
}

#[derive(Clone, Debug)]
pub struct Repo {
    pub exe: GitExe,
    pub root: PathBuf,
}

#[derive(Default)]
pub struct RunOpts<'a> {
    /// Set `GIT_OPTIONAL_LOCKS=0` so a read-only command never takes locks (e.g. refreshing the index).
    pub read_only: bool,
    /// Run against this index instead of the repository's own (`GIT_INDEX_FILE`).
    pub index_file: Option<&'a Path>,
    /// Bytes fed to git's standard input.
    pub stdin: Option<&'a [u8]>,
    /// Extra `-c key=value` settings, applied after the built-in ones.
    pub config: &'a [(&'a str, &'a str)],
}

impl RunOpts<'_> {
    /// Options for a command that only reads: no optional locks, nothing else set.
    pub fn read_only() -> Self {
        Self {
            read_only: true,
            ..Self::default()
        }
    }
}

impl Repo {
    /// Runs git in the repository root and returns its stdout. A non-zero exit is an `Err`.
    pub fn run(&self, args: &[&str], opts: &RunOpts) -> Result<Vec<u8>, GitError> {
        run_in(&self.exe, &self.root, args, opts)
    }
}

/// Runs git in `cwd` and returns its stdout. A non-zero exit is an `Err`.
pub fn run_in(
    exe: &GitExe,
    cwd: &Path,
    args: &[&str],
    opts: &RunOpts,
) -> Result<Vec<u8>, GitError> {
    let mut child = command(exe, cwd, args, opts).spawn()?;
    let stdin = child.stdin.take();
    // Feed stdin from a separate thread so a large payload cannot deadlock against a full stdout pipe.
    let output = std::thread::scope(|scope| {
        if let (Some(mut pipe), Some(bytes)) = (stdin, opts.stdin) {
            scope.spawn(move || {
                // Git may exit without reading everything (BrokenPipe); its exit status tells the story.
                let _ = pipe.write_all(bytes);
            });
        }
        child.wait_with_output()
    })?;

    if output.status.success() {
        return Ok(output.stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if stderr.contains("not a git repository") {
        Err(GitError::NotARepo)
    } else if stderr.contains("index.lock") {
        Err(GitError::Locked)
    } else {
        Err(GitError::Failed {
            args: args.join(" "),
            code: output.status.code(),
            stderr,
        })
    }
}

/// The git command [`run_in`] spawns: the fixed settings and environment, then `args`.
fn command(exe: &GitExe, cwd: &Path, args: &[&str], opts: &RunOpts) -> Command {
    let mut cmd = Command::new(&exe.path);
    cmd.current_dir(cwd)
        .args(["-c", "core.quotepath=off", "-c", "color.ui=never"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env("LANGUAGE", "en")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if opts.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    for (key, value) in opts.config {
        cmd.arg("-c").arg(format!("{key}={value}"));
    }
    cmd.args(args);
    if opts.read_only {
        cmd.env("GIT_OPTIONAL_LOCKS", "0");
    }
    // Git finds the repository from `cwd` alone: an inherited environment (a terminal or a
    // hook with these set) must not send the command to another repository or index.
    for key in REPOSITORY_ENV {
        cmd.env_remove(key);
    }
    match opts.index_file {
        Some(index) => cmd.env("GIT_INDEX_FILE", index),
        None => cmd.env_remove("GIT_INDEX_FILE"),
    };
    hide_window(&mut cmd);
    cmd
}

#[cfg(windows)]
fn hide_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_window(_cmd: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    /// The value `cmd` gives `key`: `Some(None)` when it takes the variable away from git,
    /// `None` when it leaves the inherited one alone.
    fn env_of<'a>(cmd: &'a Command, key: &str) -> Option<Option<&'a OsStr>> {
        cmd.get_envs()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value)
    }

    fn exe() -> GitExe {
        GitExe {
            path: PathBuf::from("git"),
            version: MIN_VERSION,
        }
    }

    #[test]
    fn an_inherited_environment_cannot_point_git_at_another_repository() {
        let cmd = command(&exe(), Path::new("."), &["status"], &RunOpts::default());
        for key in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_COMMON_DIR",
            "GIT_INDEX_FILE",
        ] {
            assert_eq!(env_of(&cmd, key), Some(None), "{key} must be removed");
        }
    }

    #[test]
    fn a_scratch_index_is_still_passed() {
        let index = Path::new("scratch-index");
        let opts = RunOpts {
            index_file: Some(index),
            ..RunOpts::read_only()
        };
        let cmd = command(&exe(), Path::new("."), &["add", "-A"], &opts);
        assert_eq!(
            env_of(&cmd, "GIT_INDEX_FILE"),
            Some(Some(index.as_os_str()))
        );
        assert_eq!(env_of(&cmd, "GIT_DIR"), Some(None));
        assert_eq!(
            env_of(&cmd, "GIT_OPTIONAL_LOCKS"),
            Some(Some(OsStr::new("0")))
        );
    }
}
