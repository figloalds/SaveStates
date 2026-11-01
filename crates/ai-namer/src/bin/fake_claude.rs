//! Test stand-in for the `claude` CLI. Its behaviour comes from `FAKE_CLAUDE_MODE`:
//! `ok` (default), `slow`, `error`, `empty` or `login`. If `FAKE_CLAUDE_LOG` is set, it writes
//! its args, cwd and stdin there as JSON, whatever the mode.
use std::io::Read;
use std::process::ExitCode;
use std::time::Duration;

fn main() -> ExitCode {
    // Read all of stdin first so the parent's write never blocks, whatever the mode.
    let mut stdin = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut stdin);

    if let Some(log_path) = std::env::var_os("FAKE_CLAUDE_LOG") {
        let args: Vec<String> = std::env::args_os()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let cwd = std::env::current_dir()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default();
        let log = serde_json::json!({
            "args": args,
            "cwd": cwd,
            "stdin": String::from_utf8_lossy(&stdin),
        });
        if let Err(e) = std::fs::write(&log_path, log.to_string()) {
            eprintln!("fake_claude: cannot write log: {e}");
            return ExitCode::from(3);
        }
    }

    let mode = std::env::var("FAKE_CLAUDE_MODE").unwrap_or_else(|_| "ok".to_string());
    match mode.as_str() {
        "ok" => {
            println!(r#"{{"type":"result","is_error":false,"result":"\"Corrige o menu.\""}}"#);
        }
        "slow" => std::thread::sleep(Duration::from_secs(10)),
        "error" => {
            eprint!("boom");
            return ExitCode::from(1);
        }
        "empty" => println!(r#"{{"is_error":false,"result":"  "}}"#),
        "login" => println!(r#"{{"is_error":true,"result":"Please run /login"}}"#),
        other => {
            eprintln!("fake_claude: unknown mode {other:?}");
            return ExitCode::from(2);
        }
    }
    ExitCode::SUCCESS
}
