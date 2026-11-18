//! TypeScript 候補のステージ (tsc / node --test / ESLint)。

use super::js::{eslint_available, eslint_cmd, eslint_run_failed, NODE_ISOLATION_PROBE};
use super::*;

/// `tsc` が PATH に存在するか確認する。
fn tsc_available() -> bool {
    tool_available("tsc", &["--version"])
}

/// TypeScript 候補: tsc --noEmit（型検査）→ tsc + node --test（テスト）→ ESLint（あれば）。
/// prop_test / wasm / mutation は非対応のため Skipped。
/// `tsc` が PATH にない場合はコンパイル Skipped（減点なし）。
pub(crate) fn run_ts_stages(
    root: &Path,
    candidate: &Candidate,
    ctx: &StageContext,
) -> Result<StageResults> {
    let StageContext {
        timeout, tests_dir, ..
    } = *ctx;
    // 最小 tsconfig（strict + CommonJS 出力）
    let tsconfig = r#"{"compilerOptions":{"target":"ES2022","module":"CommonJS","strict":true,"outDir":"dist"},"include":["*.ts"]}"#;
    fs::write(root.join("tsconfig.json"), tsconfig).context("tsconfig.json の書込に失敗")?;
    fs::write(root.join("candidate.ts"), &candidate.source).context("候補ソースの書込に失敗")?;
    if let Some(dir) = tests_dir {
        copy_ext_no_clobber(dir, root, "ts")?;
    }

    if !tsc_available() {
        return Ok(StageResults::default());
    }

    // compile: 型検査のみ（JS 出力なし）
    let mut c = Command::new("tsc");
    c.args(["--noEmit"]).current_dir(root);
    let compile = runner::run_stage("compile", c, timeout);
    if !compile.is_passed() {
        return Ok(StageResults::skipped_after_compile(compile));
    }

    // test: JS にトランスパイルしてから node --test でテストファイルのみ実行
    // candidate.js は渡さない（node --test はテストファイルとして実行するため副作用が起きる）
    let test_files_exist = tests_dir.is_some()
        && tests_dir
            .map(|d| {
                d.read_dir()
                    .ok()
                    .map(|mut e| e.next().is_some())
                    .unwrap_or(false)
            })
            .unwrap_or(false);
    let test = if test_files_exist {
        let mut t = Command::new("sh");
        t.arg("-c")
            .arg(format!(
                "{NODE_ISOLATION_PROBE} tsc && node --test $ISO $(ls dist/*.test.js 2>/dev/null) 1>&2"
            ))
            .current_dir(root);
        run_test_stage("test", Language::TypeScript, t, timeout, false)
    } else {
        StageOutcome::Skipped
    };

    // lint: ESLint があれば TypeScript 向けルールで実行
    let (lint, lint_warnings) = if eslint_available() {
        let mut l = Command::new("sh");
        l.arg("-c")
            .arg(eslint_cmd("candidate.ts"))
            .current_dir(root);
        let (outcome, out, truncated) = runner::run_stage_capture_flagged("eslint", l, timeout);
        lint_outcome(
            outcome,
            &out,
            truncated,
            eslint_run_failed,
            count_eslint_findings,
        )
    } else {
        (StageOutcome::Skipped, 0)
    };

    Ok(StageResults {
        compile,
        test,
        lint,
        lint_warnings,
        ..Default::default()
    })
}
