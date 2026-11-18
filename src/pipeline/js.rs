//! JavaScript 候補のステージ (node --check / node --test / ESLint)。

use super::*;

/// `eslint` が PATH に存在するか確認する。
pub(super) fn eslint_available() -> bool {
    tool_available("eslint", &["--version"])
}

/// ESLint を `file` に対して実行し (lint 結果, 警告数) を返す。ESLint が無ければ Skipped。
pub(super) fn run_eslint(root: &Path, file: &str, timeout: Duration) -> (StageOutcome, usize) {
    if !eslint_available() {
        return (StageOutcome::Skipped, 0);
    }
    let mut l = Command::new("sh");
    l.arg("-c").arg(eslint_cmd(file)).current_dir(root);
    let (outcome, out, truncated) = runner::run_stage_capture_flagged("eslint", l, timeout);
    lint_outcome(
        outcome,
        &out,
        truncated,
        eslint_run_failed,
        count_eslint_findings,
    )
}

/// ESLint の出力が「実行失敗」(設定なし・未知オプション・フォーマッタ欠落)を示すか判定する純関数。
///
/// `eslint ... 2>&1; true` は終了コードを捨てるため、失敗が「指摘 0 件」に化けるのを防ぐ (#152)。
pub(super) fn eslint_run_failed(output: &str) -> bool {
    const MARKERS: [&str; 5] = [
        "Oops! Something went wrong",
        "Invalid option '--",
        "formatter is no longer part of core ESLint",
        "couldn't find an eslint.config",
        // flat config の既定は .js 系のみ。.ts は lint されず 1 warning に化ける
        "File ignored because no matching configuration",
    ];
    MARKERS.iter().any(|m| output.contains(m))
}

/// 設定ファイルなしで最小ルールだけを当てる ESLint 9+ (flat config) 向けコマンド。
///
/// 出力は既定の stylish (`✖ N problems`)。ESLint 8 は `--no-config-lookup` を
/// `Invalid option` で拒否するので `eslint_run_failed` で Skipped になる (#152)。
/// 設定なしの flat config は Node の globals を持たないため、`--global` で渡して
/// CommonJS の `module.exports` 等が no-undef に化けないようにする。
pub(super) fn eslint_cmd(file: &str) -> String {
    const NODE_GLOBALS: &str = "require,module,exports,__dirname,__filename,console,process,\
        Buffer,setTimeout,clearTimeout,setInterval,clearInterval,setImmediate,clearImmediate,\
        queueMicrotask,structuredClone,URL,URLSearchParams,TextEncoder,TextDecoder,fetch";
    format!(
        "eslint --no-config-lookup --no-inline-config --global {NODE_GLOBALS} --rule 'no-undef: error' --rule 'no-unused-vars: warn' {file} 2>&1; true"
    )
}

/// JS のテストコマンドを決める。`*.test.js` が無ければ `None`（test は Skipped）。
/// candidate.js 自体は `node --test` に渡さない（top-level 副作用で誤判定するため, #159）。
fn js_test_command(root: &Path) -> Option<String> {
    let has_tests = root
        .read_dir()
        .map(|rd| {
            rd.flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".test.js"))
        })
        .unwrap_or(false);
    has_tests.then(|| format!("{NODE_ISOLATION_PROBE} node --test $ISO *.test.js 1>&2"))
}

/// `node --test` を同一プロセスで実行させるためのフラグ検出 (#226)。
/// 既定の子プロセス隔離ではテストファイル自体が 1 件の合格に数えられ、`process.exit(0)` でも
/// 結果行が出てしまう。`--test-isolation=none` なら exit で結果行が出ず NoSummary になる。
/// `--help` に `--test-isolation` が出ない Node ではフラグを付けると起動に失敗するため、出る場合のみ付ける
/// (そのような Node では exit(0) 偽装を検出できない既知の制約)。
pub(super) const NODE_ISOLATION_PROBE: &str =
    "ISO=; node --help 2>/dev/null | grep -q -- --test-isolation && ISO=--test-isolation=none;";

/// JavaScript 候補: node --check（構文検証）→ node --test → ESLint（あれば）。
/// prop_test / wasm は非対応のため Skipped。
pub(crate) fn run_js_stages(
    root: &Path,
    candidate: &Candidate,
    ctx: &StageContext,
) -> Result<StageResults> {
    let StageContext {
        timeout, tests_dir, ..
    } = *ctx;
    fs::write(root.join("candidate.js"), &candidate.source).context("候補ソースの書込に失敗")?;
    if let Some(dir) = tests_dir {
        copy_ext_no_clobber(dir, root, "js")?;
    }

    // compile: 構文チェック
    let mut c = Command::new("node");
    c.args(["--check", "candidate.js"]).current_dir(root);
    let compile = runner::run_stage("compile", c, timeout);
    if !compile.is_passed() {
        return Ok(StageResults::skipped_after_compile(compile));
    }

    // test: Node 組込みランナー（18+）で *.test.js のみ実行。無ければ Skipped。
    let test = match js_test_command(root) {
        Some(cmd) => {
            let mut t = Command::new("sh");
            t.arg("-c").arg(cmd).current_dir(root);
            run_test_stage("test", Language::JavaScript, t, timeout, false)
        }
        None => StageOutcome::Skipped,
    };

    // lint: ESLint があれば設定ファイルなし + 最小ルールで実行
    let (lint, lint_warnings) = run_eslint(root, "candidate.js", timeout);

    Ok(StageResults {
        compile,
        test,
        lint,
        lint_warnings,
        ..Default::default()
    })
}

#[cfg(test)]
mod eslint_run_failed_tests {
    use super::{eslint_cmd, eslint_run_failed};

    #[test]
    fn eslint_cmd_uses_flat_config_flags() {
        // ESLint 9+ で削除された --no-eslintrc と core 外の compact formatter を使わない (#152)
        let cmd = eslint_cmd("candidate.js");
        assert!(cmd.contains("--no-config-lookup"));
        // 候補の `/* eslint-disable */` で指摘を消されない (#237)
        assert!(cmd.contains("--no-inline-config"));
        assert!(cmd.contains("candidate.js"));
        assert!(cmd.contains("--global require,module,exports,"));
        assert!(!cmd.contains("--no-eslintrc"));
        assert!(!cmd.contains("compact"));
    }

    #[test]
    fn detects_invalid_option() {
        assert!(eslint_run_failed(
            "Invalid option '--eslintrc' - perhaps you meant '--ext'?"
        ));
    }

    #[test]
    fn detects_missing_config_oops() {
        assert!(eslint_run_failed(
            "\nOops! Something went wrong! :(\n\nESLint: 10.2.0\n\nESLint couldn't find an eslint.config.(js|mjs|cjs) file."
        ));
    }

    #[test]
    fn detects_missing_compact_formatter() {
        assert!(eslint_run_failed(
            "The compact formatter is no longer part of core ESLint. Install it manually with `npm install -D eslint-formatter-compact`"
        ));
    }

    #[test]
    fn detects_file_ignored_without_matching_config() {
        // ESLint 10.2.0 + --no-config-lookup で .ts を渡したときの実出力 (#152)
        assert!(eslint_run_failed(
            "\n/tmp/x/candidate.ts\n  0:0  warning  File ignored because no matching configuration was supplied\n\n\u{2716} 1 problem (0 errors, 1 warning)\n"
        ));
    }

    #[test]
    fn detects_eslint8_rejecting_no_config_lookup() {
        assert!(eslint_run_failed(
            "Invalid option '--config-lookup' - perhaps you meant '--config'?"
        ));
    }

    #[test]
    fn normal_outputs_are_not_failures() {
        assert!(!eslint_run_failed(""));
        assert!(!eslint_run_failed("candidate.js: line 1, col 7, Warning - 'a' is assigned a value but never used. (no-unused-vars)\n\n1 problem"));
        assert!(!eslint_run_failed(
            "\u{2716} 2 problems (1 error, 1 warning)"
        ));
    }
}

#[cfg(test)]
mod js_test_command_tests {
    use super::*;

    #[test]
    fn no_test_files_returns_none() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("candidate.js"), "module.exports = 1;").unwrap();
        assert!(js_test_command(d.path()).is_none());
    }

    #[test]
    fn with_test_file_runs_only_test_files() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("candidate.js"), "module.exports = 1;").unwrap();
        fs::write(d.path().join("a.test.js"), "").unwrap();
        let cmd = js_test_command(d.path()).expect("test file exists");
        assert!(cmd.contains("--test"));
        assert!(!cmd.contains("candidate.js"));
        assert!(cmd.contains("*.test.js"));
    }
}
