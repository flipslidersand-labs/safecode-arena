//! 候補を一時環境で実行する検証ランナー。
//!
//! MVP: プロセスベースの隔離（一時ディレクトリ + タイムアウト kill）。
//! 発展: Wasmtime/WASI による capability ベースのサンドボックス。

use crate::model::StageOutcome;
use std::io::{Read, Seek, SeekFrom};
use std::process::Stdio;
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;

/// 取り込む出力の最大バイト数。
///
/// 候補コードが改行なしで大量に出力してもメモリを使い切らないための上限。
/// cargo / clippy の通常出力や `cargo audit --json` は数百 KiB 程度に収まるため、
/// 4 MiB あれば実用上は全文が入る。超えた場合は先頭と末尾を半分ずつ残す。
/// 先頭は最初のエラーや警告、末尾はコンパイラやテストの最終的な失敗理由
/// (summary 行や panic) が出やすいので、どちらも落とさない。
const MAX_CAPTURE_BYTES: usize = 4 * 1024 * 1024;

/// 失敗理由 1 行あたりの最大文字数。改行なしの長大な出力を表示用に切り詰める。
const MAX_LINE_CHARS: usize = 500;

/// 出力ファイルの許容サイズ(バイト)。超えたら子を kill して中断する。
///
/// 読み込み側の上限だけではディスクが膨らむため、タイムアウト前でも打ち切る。
/// RLIMIT_FSIZE は cargo のビルド成果物の書き込みにも効いてしまうので使わない。
const MAX_OUTPUT_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// 出力ファイルのサイズを確認する間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// `reader` から最大 `max_bytes` バイトだけ読む。
///
/// fstat 後にファイルが伸びても(孫プロセスが書き続けても)読み込み量が上限を超えない。
fn read_capped<R: Read>(reader: &mut R, max_bytes: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = reader.take(max_bytes as u64).read_to_end(&mut buf);
    buf
}

/// キャプチャ先ファイルを上限付きで読み取る。
///
/// 上限を超える場合は先頭と末尾を残し、間に切り詰めの印を入れる。
/// UTF-8 の途中で切れても lossy 変換で安全に扱う。
fn read_bounded(mut file: std::fs::File, max_bytes: usize) -> String {
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(0));
    if len <= max_bytes as u64 {
        return String::from_utf8_lossy(&read_capped(&mut file, max_bytes)).into_owned();
    }
    let half = max_bytes / 2;
    let mut head = vec![0u8; half];
    let mut tail = vec![0u8; half];
    let head_ok = file.read_exact(&mut head).is_ok();
    let tail_ok =
        file.seek(SeekFrom::Start(len - half as u64)).is_ok() && file.read_exact(&mut tail).is_ok();
    if !head_ok {
        head.clear();
    }
    if !tail_ok {
        tail.clear();
    }
    format!(
        "{}\n…(出力が {} KiB を超えたため中間 {} KiB を切り詰め)…\n{}",
        String::from_utf8_lossy(&head),
        max_bytes / 1024,
        (len - 2 * half as u64) / 1024,
        String::from_utf8_lossy(&tail)
    )
}

/// キャプチャ先ファイルの内容を上限付きで読み取る。
fn read_all(file: std::fs::File) -> String {
    read_bounded(file, MAX_CAPTURE_BYTES)
}

/// 1 行を `MAX_LINE_CHARS` 文字に切り詰める。
fn clip_line(line: &str) -> String {
    match line.char_indices().nth(MAX_LINE_CHARS) {
        Some((idx, _)) => format!("{}…(行を {MAX_LINE_CHARS} 文字で切り詰め)", &line[..idx]),
        None => line.to_string(),
    }
}

/// 失敗理由の表示用に先頭 20 行へ要約する。各行は長さも切り詰める。
fn summarize(file: std::fs::File) -> String {
    read_all(file)
        .lines()
        .take(20)
        .map(clip_line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// 子プロセスへ引き継ぐ環境変数の許可リスト（完全一致）。
///
/// 候補コードが API キー等の秘密を読んで出力しないよう、ツールチェーンの
/// 実行に必要な変数だけを通す。`*_TOKEN` / `*_API_KEY` 系は含めない。
const ALLOWED_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "LANG",
    "LANGUAGE",
    "TMPDIR",
    "TZ",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_JOBS",
    "RUSTFLAGS",
    "GOPATH",
    "GOCACHE",
    "GOFLAGS",
    "GOROOT",
    "GOMODCACHE",
    "GOENV",
    "GOTOOLCHAIN",
    "NODE_PATH",
    "VIRTUAL_ENV",
    "PYTHONPATH",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
];

/// 環境変数名が子プロセスへ渡してよいか判定する。`LC_*` は全て許可する。
fn is_allowed_env(name: &str) -> bool {
    name.starts_with("LC_") || ALLOWED_ENV.contains(&name)
}

/// 子の環境を許可リストだけに絞る。呼び出し側が `Command::env` で明示した値は、
/// 許可リストに含まれるものだけ残す。呼び出し側が `env_remove` した変数でも、
/// 許可リストにあれば親の値が入り直す。
fn apply_env_allowlist(cmd: &mut std::process::Command) {
    let explicit: Vec<(std::ffi::OsString, std::ffi::OsString)> = cmd
        .get_envs()
        .filter_map(|(k, v)| v.map(|v| (k.to_os_string(), v.to_os_string())))
        .collect();
    cmd.env_clear();
    for (k, v) in std::env::vars_os().chain(explicit) {
        if k.to_str().is_some_and(is_allowed_env) {
            cmd.env(k, v);
        }
    }
}

/// 子プロセスをプロセスグループごと SIGKILL して回収する。
///
/// 子は `process_group(0)` で自身がグループリーダー（pgid == pid）になっている。
fn kill_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    // SAFETY: killpg は引数が整数のみの単純なシステムコール。
    unsafe {
        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
    }
    // 非 unix、または killpg が失敗した場合の保険（直接の子だけでも止める）。
    let _ = child.kill();
    let _ = child.wait();
}

/// プロセスをタイムアウト付きで実行し、ステージ結果と stderr を返す。
///
/// [`run_stage`] と同じ動作だが、呼び出し元が stderr を検査できるよう
/// 文字列として返す（clippy warning 数のカウント等に使う）。
/// stdout と stderr は同じ一時ファイルへ書き出し、全文を返す
/// （`cargo audit --json` / ruff / Criterion は stdout に出力するため）。
pub fn run_stage_capture(
    label: &str,
    command: std::process::Command,
    limit: Duration,
) -> (StageOutcome, String) {
    run_stage_capture_with(label, command, limit, MAX_OUTPUT_FILE_BYTES)
}

fn run_stage_capture_with(
    label: &str,
    command: std::process::Command,
    limit: Duration,
    max_file_bytes: u64,
) -> (StageOutcome, String) {
    let err_file = match tempfile::tempfile() {
        Ok(f) => f,
        Err(e) => {
            return (
                StageOutcome::Failed {
                    detail: format!("{label}: 一時ファイル生成失敗: {e}"),
                },
                String::new(),
            );
        }
    };
    let err_for_child = match err_file.try_clone() {
        Ok(f) => f,
        Err(e) => {
            return (
                StageOutcome::Failed {
                    detail: format!("{label}: ファイル複製失敗: {e}"),
                },
                String::new(),
            );
        }
    };
    let out_for_child = match err_file.try_clone() {
        Ok(f) => f,
        Err(e) => {
            return (
                StageOutcome::Failed {
                    detail: format!("{label}: ファイル複製失敗: {e}"),
                },
                String::new(),
            );
        }
    };
    let mut cmd = command;
    apply_env_allowlist(&mut cmd);
    cmd.stdout(Stdio::from(out_for_child))
        .stderr(Stdio::from(err_for_child));

    // 孫プロセスごと止められるよう、子を新しいプロセスグループのリーダーにする。
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);

    let start = Instant::now();
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return (
                StageOutcome::Failed {
                    detail: format!("{label}: 起動失敗: {e}"),
                },
                String::new(),
            );
        }
    };

    // 出力ファイルが肥大化していないか定期的に確認しながら待つ。
    let deadline = start + limit;
    let waited = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match child.wait_timeout(remaining.min(POLL_INTERVAL)) {
            Ok(None) => {
                if err_file.metadata().map(|m| m.len()).unwrap_or(0) > max_file_bytes {
                    kill_group(&mut child);
                    break Err(None);
                }
                if remaining <= POLL_INTERVAL {
                    break Ok(None);
                }
            }
            Ok(Some(status)) => break Ok(Some(status)),
            Err(e) => break Err(Some(e)),
        }
    };

    let outcome = match waited {
        Err(None) => StageOutcome::Failed {
            detail: format!(
                "{label}: 出力が {} MiB を超えたため中断",
                max_file_bytes / (1024 * 1024)
            ),
        },
        Ok(Some(status)) => {
            let elapsed = start.elapsed();
            if status.success() {
                StageOutcome::Passed {
                    duration_ms: elapsed.as_millis() as u64,
                }
            } else {
                StageOutcome::Failed {
                    detail: summarize(
                        err_file
                            .try_clone()
                            .unwrap_or_else(|_| tempfile::tempfile().unwrap()),
                    ),
                }
            }
        }
        Ok(None) => {
            kill_group(&mut child);
            StageOutcome::TimedOut {
                limit_ms: limit.as_millis() as u64,
            }
        }
        Err(Some(e)) => {
            kill_group(&mut child);
            StageOutcome::Failed {
                detail: format!("{label}: 待機失敗: {e}"),
            }
        }
    };

    let captured = read_all(err_file);
    (outcome, captured)
}

/// プロセスをタイムアウト付きで実行し、ステージ結果へ変換する。
///
/// 出力（stdout/stderr）は一時ファイルへリダイレクトする。パイプ
/// バッファ溢れによるデッドロックを避けるためで、cargo の大量出力でも安全。
/// 制限時間を超えたら子プロセスを kill して `TimedOut` を返す。
pub fn run_stage(label: &str, command: std::process::Command, limit: Duration) -> StageOutcome {
    run_stage_capture(label, command, limit).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn secrets_are_not_allowed_but_toolchain_vars_are() {
        for name in [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GITHUB_TOKEN",
            "CARGO_REGISTRY_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
        ] {
            assert!(!is_allowed_env(name), "{name} は許可されてはならない");
        }
        for name in [
            "PATH",
            "HOME",
            "CARGO_HOME",
            "RUSTUP_HOME",
            "LC_ALL",
            "GOPATH",
        ] {
            assert!(is_allowed_env(name), "{name} は許可される必要がある");
        }
    }

    #[test]
    fn child_does_not_inherit_non_allowlisted_env() {
        // 親の環境は変更せず、Command 側で明示設定した値も許可リスト外なら消えることを確認する。
        // (親由来の継承は apply_env_allowlist が vars_os を同じ判定で濾過する。)
        let mut cmd = Command::new("sh");
        cmd.env("ANTHROPIC_API_KEY", "dummy-secret").args([
            "-c",
            "echo key=${ANTHROPIC_API_KEY:-unset} path=${PATH:+set}",
        ]);
        let (outcome, captured) = run_stage_capture("env", cmd, Duration::from_secs(5));
        assert!(outcome.is_passed());
        assert!(captured.contains("key=unset"), "{captured}");
        assert!(captured.contains("path=set"), "{captured}");
    }

    #[test]
    fn long_command_times_out_and_is_killed() {
        let mut cmd = Command::new("sleep");
        cmd.arg("10");
        let outcome = run_stage("sleep", cmd, Duration::from_millis(300));
        assert!(matches!(outcome, StageOutcome::TimedOut { .. }));
    }

    /// /proc/<pid> が存在し、ゾンビでなければ生存とみなす。
    #[cfg(target_os = "linux")]
    fn pid_alive(pid: i32) -> bool {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(s) => !s
                .rsplit(')')
                .next()
                .unwrap_or("")
                .trim_start()
                .starts_with('Z'),
            Err(_) => false,
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn timeout_kills_grandchild_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("grandchild.pid");
        let mut cmd = Command::new("sh");
        // 孫（一意な引数の sleep）をバックグラウンドで起動し、PID を書いて待つ。
        cmd.arg("-c")
            .arg("sleep 30.162 & echo $! > \"$1\"; wait")
            .arg("sh")
            .arg(&pid_file);
        let outcome = run_stage("grandchild", cmd, Duration::from_millis(800));
        assert!(matches!(outcome, StageOutcome::TimedOut { .. }));

        let pid: i32 = std::fs::read_to_string(&pid_file)
            .expect("孫の PID ファイル")
            .trim()
            .parse()
            .unwrap();
        // SIGKILL 後の回収を最大 3 秒待つ。
        let mut alive = true;
        for _ in 0..30 {
            alive = pid_alive(pid);
            if !alive {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if alive {
            // 後始末（テスト失敗時に孫を残さない）
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
        assert!(!alive, "孫プロセス {pid} がタイムアウト後も残っている");
    }

    #[test]
    fn quick_success_passes() {
        let outcome = run_stage("true", Command::new("true"), Duration::from_secs(5));
        assert!(outcome.is_passed());
    }

    #[test]
    fn capture_returns_stdout_and_stderr_in_full() {
        let mut cmd = Command::new("sh");
        cmd.args([
            "-c",
            "echo out-line; for i in $(seq 1 30); do echo warn-$i >&2; done",
        ]);
        let (outcome, captured) = run_stage_capture("sh", cmd, Duration::from_secs(5));
        assert!(outcome.is_passed());
        assert!(captured.contains("out-line"));
        assert_eq!(captured.matches("warn-").count(), 30);
    }

    #[test]
    fn failed_detail_is_summarized_to_20_lines() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "for i in $(seq 1 30); do echo e-$i >&2; done; exit 1"]);
        let (outcome, captured) = run_stage_capture("sh", cmd, Duration::from_secs(5));
        let StageOutcome::Failed { detail } = outcome else {
            panic!("Failed を期待");
        };
        assert_eq!(detail.lines().count(), 20);
        assert_eq!(captured.lines().count(), 30);
    }

    #[test]
    fn spawn_failure_is_failed_with_empty_capture() {
        let (outcome, captured) =
            run_stage_capture("x", Command::new("/no/such/cmd"), Duration::from_secs(1));
        assert!(matches!(outcome, StageOutcome::Failed { .. }));
        assert!(captured.is_empty());
    }

    #[test]
    fn nonzero_exit_fails() {
        let outcome = run_stage("false", Command::new("false"), Duration::from_secs(5));
        assert!(matches!(outcome, StageOutcome::Failed { .. }));
    }

    #[test]
    fn huge_single_line_output_is_bounded() {
        // 改行なしで 12 MiB 出力する。取り込み・要約ともに上限内に収まること。
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "head -c 12582912 /dev/zero | tr '\\0' a; exit 1"]);
        let (outcome, captured) = run_stage_capture("big", cmd, Duration::from_secs(30));
        assert!(
            captured.len() <= MAX_CAPTURE_BYTES + 1024,
            "{}",
            captured.len()
        );
        assert!(captured.contains("切り詰め"));
        let StageOutcome::Failed { detail } = outcome else {
            panic!("Failed を期待");
        };
        assert!(
            detail.chars().count() <= 20 * (MAX_LINE_CHARS + 64),
            "{}",
            detail.len()
        );
    }

    #[test]
    fn runaway_output_is_killed_before_timeout() {
        // 上限 1 MiB で、無限出力する子を止められること(タイムアウトの 30 秒より十分早い)。
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "yes"]);
        let start = Instant::now();
        let (outcome, captured) =
            run_stage_capture_with("yes", cmd, Duration::from_secs(30), 1024 * 1024);
        assert!(start.elapsed() < Duration::from_secs(15));
        let StageOutcome::Failed { detail } = outcome else {
            panic!("Failed を期待: {outcome:?}");
        };
        assert!(detail.contains("中断"), "{detail}");
        assert!(captured.len() <= MAX_CAPTURE_BYTES + 1024);
    }

    #[test]
    fn read_capped_stops_at_limit_even_if_source_grows() {
        // fstat 後に伸びたファイルを模して、上限より大きい入力を渡す。
        let mut src = std::io::repeat(b'a').take(10 * 1024 * 1024);
        let buf = read_capped(&mut src, 1024);
        assert_eq!(buf.len(), 1024);
    }

    #[test]
    fn truncation_replaces_only_cut_chars_with_replacement_char() {
        let mut f = tempfile::tempfile().unwrap();
        std::io::Write::write_all(&mut f, "あ".repeat(1000).as_bytes()).unwrap();
        // 上限 98 → 先頭 49 バイト(16 文字 + 1 バイト)・末尾 49 バイト(1 バイト + 16 文字)。
        let s = read_bounded(f, 98);
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 3, "{s}");
        assert!(lines[1].contains("切り詰め"), "{s}");
        assert_eq!(lines[0], format!("{}\u{FFFD}", "あ".repeat(16)));
        assert_eq!(lines[2], format!("\u{FFFD}{}", "あ".repeat(16)));
        assert_eq!(s.matches('\u{FFFD}').count(), 2);
    }

    #[test]
    fn clip_line_limits_chars() {
        let long = "x".repeat(MAX_LINE_CHARS + 10);
        assert!(clip_line(&long).contains("切り詰め"));
        assert_eq!(clip_line("short"), "short");
    }
}
