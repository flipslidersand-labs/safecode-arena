//! 候補を一時環境で実行する検証ランナー。
//!
//! MVP: プロセスベースの隔離（一時ディレクトリ + タイムアウト kill）。
//! 発展: Wasmtime/WASI による capability ベースのサンドボックス。

use crate::model::StageOutcome;
use std::io::{Read, Seek, SeekFrom};
use std::process::Stdio;
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;

/// stderr を読み取り、先頭 20 行に要約する。
fn summarize(mut file: std::fs::File) -> String {
    let mut buf = String::new();
    let _ = file.seek(SeekFrom::Start(0));
    let _ = file.read_to_string(&mut buf);
    buf.lines().take(20).collect::<Vec<_>>().join("\n")
}

/// プロセスをタイムアウト付きで実行し、ステージ結果と stderr を返す。
///
/// [`run_stage`] と同じ動作だが、呼び出し元が stderr を検査できるよう
/// 文字列として返す（clippy warning 数のカウント等に使う）。
pub fn run_stage_capture(
    label: &str,
    command: std::process::Command,
    limit: Duration,
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
    let mut cmd = command;
    cmd.stdout(Stdio::null()).stderr(Stdio::from(err_for_child));

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

    let outcome = match child.wait_timeout(limit) {
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
            let _ = child.kill();
            let _ = child.wait();
            StageOutcome::TimedOut {
                limit_ms: limit.as_millis() as u64,
            }
        }
        Err(e) => StageOutcome::Failed {
            detail: format!("{label}: 待機失敗: {e}"),
        },
    };

    let captured = summarize(err_file);
    (outcome, captured)
}

/// プロセスをタイムアウト付きで実行し、ステージ結果へ変換する。
///
/// 出力（stdout/stderr）は一時ファイルへリダイレクトする。パイプ
/// バッファ溢れによるデッドロックを避けるためで、cargo の大量出力でも安全。
/// 制限時間を超えたら子プロセスを kill して `TimedOut` を返す。
pub fn run_stage(label: &str, mut command: std::process::Command, limit: Duration) -> StageOutcome {
    // stderr/stdout を一時ファイルへ。stdout は捨て、stderr は失敗時に要約する。
    let err_file = match tempfile::tempfile() {
        Ok(f) => f,
        Err(e) => {
            return StageOutcome::Failed {
                detail: format!("{label}: 一時ファイル生成失敗: {e}"),
            }
        }
    };
    let err_for_child = match err_file.try_clone() {
        Ok(f) => f,
        Err(e) => {
            return StageOutcome::Failed {
                detail: format!("{label}: ファイル複製失敗: {e}"),
            }
        }
    };
    command
        .stdout(Stdio::null())
        .stderr(Stdio::from(err_for_child));

    let start = Instant::now();
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            return StageOutcome::Failed {
                detail: format!("{label}: 起動失敗: {e}"),
            }
        }
    };

    match child.wait_timeout(limit) {
        // 制限時間内に終了
        Ok(Some(status)) => {
            let elapsed = start.elapsed();
            if status.success() {
                StageOutcome::Passed {
                    duration_ms: elapsed.as_millis() as u64,
                }
            } else {
                StageOutcome::Failed {
                    detail: summarize(err_file),
                }
            }
        }
        // タイムアウト → kill
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            StageOutcome::TimedOut {
                limit_ms: limit.as_millis() as u64,
            }
        }
        Err(e) => StageOutcome::Failed {
            detail: format!("{label}: 待機失敗: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn long_command_times_out_and_is_killed() {
        let mut cmd = Command::new("sleep");
        cmd.arg("10");
        let outcome = run_stage("sleep", cmd, Duration::from_millis(300));
        assert!(matches!(outcome, StageOutcome::TimedOut { .. }));
    }

    #[test]
    fn quick_success_passes() {
        let outcome = run_stage("true", Command::new("true"), Duration::from_secs(5));
        assert!(outcome.is_passed());
    }

    #[test]
    fn nonzero_exit_fails() {
        let outcome = run_stage("false", Command::new("false"), Duration::from_secs(5));
        assert!(matches!(outcome, StageOutcome::Failed { .. }));
    }
}
