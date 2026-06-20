//! 検証パイプライン駆動。候補を一時 Cargo プロジェクトへ展開し、
//! compile → test → 採点 までを実行する。

use crate::analysis::{count_clippy_warnings, SourceMetrics};
use crate::config::Rubric;
use crate::model::{Candidate, Evaluation, Language, StageOutcome};
use crate::{runner, scoring};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// 一時プロジェクトの Cargo.toml。依存なしの最小構成。
const CARGO_TOML_TEMPLATE: &str = r#"[package]
name = "candidate"
version = "0.0.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#;

/// proptest を dev-dependency に持つ Cargo.toml。
const CARGO_TOML_WITH_PROPTEST: &str = r#"[package]
name = "candidate"
version = "0.0.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[dev-dependencies]
proptest = "1"
"#;

/// 候補ファイルを読み込んで `Candidate` を生成する。
/// `id` はファイル名 stem（拡張子なし）。
pub fn load_candidate(path: &Path) -> Result<Candidate> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("候補ファイルの読込に失敗: {}", path.display()))?;
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("candidate")
        .to_string();
    Ok(Candidate {
        id,
        source,
        language: Language::Rust,
    })
}

/// 外部テストディレクトリの `.rs` ファイルを一時プロジェクトの `tests/` へコピーする。
fn copy_tests(src_dir: &Path, dest_tests: &Path) -> Result<()> {
    fs::create_dir_all(dest_tests).context("tests ディレクトリの作成に失敗")?;
    for entry in fs::read_dir(src_dir)
        .with_context(|| format!("テストディレクトリの読込に失敗: {}", src_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            let name = path.file_name().unwrap();
            fs::copy(&path, dest_tests.join(name))
                .with_context(|| format!("テストのコピーに失敗: {}", path.display()))?;
        }
    }
    Ok(())
}

/// 1 候補を一時 Cargo プロジェクトへ展開し、compile → test → clippy → prop_test → 採点する。
///
/// - compile が通らなかった場合、test / clippy / prop_test はすべて `Skipped`。
/// - `tests_dir`: 統合テスト `.rs` を置いたディレクトリ（任意）。
/// - `prop_tests_dir`: proptest ファイルを置いたディレクトリ（任意）。指定時は
///   proptest を dev-dependency に追加して実行する。
pub fn evaluate_candidate(
    candidate: &Candidate,
    timeout: Duration,
    rubric: &Rubric,
    tests_dir: Option<&Path>,
    prop_tests_dir: Option<&Path>,
) -> Result<Evaluation> {
    let dir = tempfile::tempdir().context("一時ディレクトリの生成に失敗")?;
    let root = dir.path();
    fs::create_dir_all(root.join("src")).context("src ディレクトリの作成に失敗")?;

    // proptest を使う場合は専用 Cargo.toml を使う
    let toml = if prop_tests_dir.is_some() {
        CARGO_TOML_WITH_PROPTEST
    } else {
        CARGO_TOML_TEMPLATE
    };
    fs::write(root.join("Cargo.toml"), toml).context("Cargo.toml の書込に失敗")?;
    fs::write(root.join("src").join("lib.rs"), &candidate.source)
        .context("候補ソースの書込に失敗")?;

    if let Some(dir) = tests_dir {
        copy_tests(dir, &root.join("tests"))?;
    }

    // compile
    let mut build = Command::new("cargo");
    build.arg("build").current_dir(root);
    let compile = runner::run_stage("compile", build, timeout);

    // test（compile 成功時のみ）
    let test = if compile.is_passed() {
        let mut t = Command::new("cargo");
        t.arg("test").current_dir(root);
        runner::run_stage("test", t, timeout)
    } else {
        StageOutcome::Skipped
    };

    // clippy（compile 成功時のみ。stderr から warning 数を取得）
    let (clippy, clippy_warnings) = if compile.is_passed() {
        let mut c = Command::new("cargo");
        c.args(["clippy", "--", "-W", "clippy::all"])
            .current_dir(root);
        let (outcome, stderr) = runner::run_stage_capture("clippy", c, timeout);
        let warn_count = count_clippy_warnings(&stderr);
        (outcome, warn_count)
    } else {
        (StageOutcome::Skipped, 0)
    };

    // property test（--prop-tests 指定 + compile 成功時のみ）
    let prop_test = if compile.is_passed() {
        if let Some(prop_dir) = prop_tests_dir {
            copy_tests(prop_dir, &root.join("tests"))?;
            let mut pt = Command::new("cargo");
            pt.arg("test").current_dir(root);
            runner::run_stage("prop_test", pt, timeout)
        } else {
            StageOutcome::Skipped
        }
    } else {
        StageOutcome::Skipped
    };

    let metrics = SourceMetrics::analyze(&candidate.source);
    let axes = scoring::axes_without_performance(
        &compile,
        &test,
        &prop_test,
        &clippy,
        clippy_warnings,
        &metrics,
        rubric,
    );
    let score = axes.total();
    Ok(Evaluation {
        candidate_id: candidate.id.clone(),
        compile,
        test,
        clippy,
        clippy_warnings,
        prop_test,
        axes,
        score,
    })
}
