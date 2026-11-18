//! 検証パイプライン駆動。候補を一時 Cargo プロジェクトへ展開し、
//! compile → test → 採点 までを実行する。

use crate::analysis::{
    count_eslint_findings, count_go_vet_findings, count_lint_warnings, count_ruff_findings,
    count_staticcheck_findings, has_bench, SourceMetrics,
};
use crate::config::Rubric;
use crate::model::{Candidate, Evaluation, Language, StageOutcome};
use crate::test_summary::{parse_test_summary, TestVerdict};
use crate::{runner, scoring, wasm};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

mod common;
pub(crate) mod go;
pub(crate) mod js;
pub(crate) mod python;
mod rust;
pub(crate) mod ts;

#[cfg(test)]
use common::test_verdict_failure;
pub(crate) use common::TRUNCATED_LINT_FLOOR;
use common::{
    copy_ext, copy_ext_keep_existing, copy_ext_no_clobber, floor_if_truncated, run_test_stage,
};
#[cfg(test)]
use go::run_go_stages;
#[cfg(test)]
use js::run_js_stages;
#[cfg(test)]
use python::run_python_stages;
pub(crate) use rust::run_rust_stages;
#[cfg(test)]
use ts::run_ts_stages;

/// Wasm 実行のオプション。`entry` 指定時のみ Wasm ステージを実行する。
#[derive(Debug, Clone, Copy)]
pub struct WasmOptions<'a> {
    pub entry: Option<&'a str>,
    pub fuel: u64,
    pub max_memory_bytes: usize,
}

impl Default for WasmOptions<'_> {
    fn default() -> Self {
        WasmOptions {
            entry: None,
            fuel: 100_000_000,
            max_memory_bytes: 64 * 1024 * 1024,
        }
    }
}

/// 候補ファイルを読み込んで `Candidate` を生成する。
/// `id` はファイル名 stem（拡張子なし）。
/// 拡張子なしは Rust、未対応の拡張子はエラー。
pub fn load_candidate(path: &Path) -> Result<Candidate> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("候補ファイルの読込に失敗: {}", path.display()))?;
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("candidate")
        .to_string();
    let language = match path.extension().and_then(|e| e.to_str()) {
        None => Language::Rust,
        Some(ext) => Language::try_from_extension(ext).ok_or_else(|| {
            anyhow::anyhow!(
                "未対応の拡張子 `.{ext}`: {}（対応: {}）",
                path.display(),
                Language::SUPPORTED_EXTENSIONS
                    .iter()
                    .map(|e| format!(".{e}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?,
    };
    Ok(Candidate {
        id,
        source,
        language,
    })
}

/// 複数の候補ファイルを読み込む。候補 ID（ファイル名 stem）が重複したらエラー。
pub fn load_candidates<P: AsRef<Path>>(paths: &[P]) -> Result<Vec<Candidate>> {
    let mut seen: HashMap<String, &Path> = HashMap::new();
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        let p = p.as_ref();
        let c = load_candidate(p)?;
        if let Some(prev) = seen.insert(c.id.clone(), p) {
            anyhow::bail!(
                "候補 ID `{}` が重複: {} と {}（ファイル名 stem を変えてください）",
                c.id,
                prev.display(),
                p.display()
            );
        }
        out.push(c);
    }
    Ok(out)
}

/// 各検証ステージの生結果。言語ごとのランナーが埋め、`assemble` が採点する。
/// `lint` / `lint_warnings` は Rust では cargo clippy、Python では ruff を指す
///
pub(crate) struct StageResults {
    compile: StageOutcome,
    test: StageOutcome,
    lint: StageOutcome,
    lint_warnings: usize,
    prop_test: StageOutcome,
    wasm: StageOutcome,
    wasm_fuel_used: Option<u64>,
    mutation: StageOutcome,
    mutation_caught: usize,
    mutation_total: usize,
    /// None = cargo audit が結果(JSON)を返せなかった（採点では audit 項を除外, #179）。
    audit_findings: Option<usize>,
    bench_ns: Option<u64>,
    reasoning_score: Option<f64>,
    reasoning_comment: Option<String>,
}

impl Default for StageResults {
    fn default() -> Self {
        StageResults {
            compile: StageOutcome::Skipped,
            test: StageOutcome::Skipped,
            lint: StageOutcome::Skipped,
            lint_warnings: 0,
            prop_test: StageOutcome::Skipped,
            wasm: StageOutcome::Skipped,
            wasm_fuel_used: None,
            mutation: StageOutcome::Skipped,
            mutation_caught: 0,
            mutation_total: 0,
            audit_findings: Some(0),
            bench_ns: None,
            reasoning_score: None,
            reasoning_comment: None,
        }
    }
}

impl StageResults {
    /// compile 失敗時の既定（後続ステージはすべて Skipped）。
    fn skipped_after_compile(compile: StageOutcome) -> Self {
        StageResults {
            compile,
            ..Default::default()
        }
    }
}

/// `evaluate_candidate` に渡すオプション群。
#[derive(Default)]
pub struct EvalOptions<'a> {
    pub tests_dir: Option<&'a Path>,
    pub prop_tests_dir: Option<&'a Path>,
    pub wasm_opts: WasmOptions<'a>,
    pub run_mutation: bool,
    pub run_llm_review: bool,
    /// LLM レビュー取得関数 `(source, test_passed) -> Option<LlmReview>`。
    /// `run_llm_review` が true のときだけ呼ばれる。`None` なら未取得扱い。
    /// `None` のときは `run_llm_review` が true でもレビューを呼ばない
    /// (以前は `llm::review` を直接呼んでいた。ライブラリ利用側は reviewer を渡すこと)。
    /// 実行条件キーには影響しない。
    pub reviewer: Option<&'a Reviewer<'a>>,
}

/// LLM レビューを取得する関数の型（`crate::llm::review` が本番実装）。
pub type Reviewer<'a> = dyn Fn(&str, bool) -> Option<crate::llm::LlmReview> + 'a;

/// `run_*_stages` に共通で渡す実行コンテキスト。言語が使わない項目は無視される。
#[derive(Clone, Copy)]
pub(crate) struct StageContext<'a> {
    timeout: Duration,
    tests_dir: Option<&'a Path>,
    prop_tests_dir: Option<&'a Path>,
    wasm_opts: &'a WasmOptions<'a>,
    run_mutation: bool,
}

/// 言語ごとに対応する `EvalOptions` の機能（`run_*_stages` が受け取る引数に対応）。
/// 実行条件キー（`main.rs` の `run_options_key`）と警告の両方がここを参照する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub mutation: bool,
    pub prop_tests: bool,
    pub wasm: bool,
}

/// 言語が対応するオプションの一覧表（唯一の定義箇所）。
pub fn capabilities(lang: Language) -> Capabilities {
    match lang {
        Language::Rust => Capabilities {
            mutation: true,
            prop_tests: true,
            wasm: true,
        },
        Language::Python | Language::Go => Capabilities {
            mutation: true,
            prop_tests: false,
            wasm: false,
        },
        Language::JavaScript | Language::TypeScript => Capabilities {
            mutation: false,
            prop_tests: false,
            wasm: false,
        },
    }
}

/// 指定されたが候補の言語が対応せず無視されるオプションの警告文を返す。
pub fn ignored_option_warnings(candidate: &Candidate, opts: &EvalOptions) -> Vec<String> {
    let cap = capabilities(candidate.language);
    let mut ignored = Vec::new();
    if opts.run_mutation && !cap.mutation {
        ignored.push("--mutation");
    }
    if opts.prop_tests_dir.is_some() && !cap.prop_tests {
        ignored.push("--prop-tests");
    }
    if opts.wasm_opts.entry.is_some() && !cap.wasm {
        ignored.push("--wasm-entry");
    }
    ignored
        .into_iter()
        .map(|o| {
            format!(
                "警告: {} ({}) は {} に対応しないため無視します",
                candidate.id,
                candidate.language.name(),
                o
            )
        })
        .collect()
}

/// 1 候補を一時ディレクトリへ展開し、言語に応じた検証ステージを実行して採点する。
///
/// compile が通らなかった場合、後続ステージはすべて `Skipped`。
pub fn evaluate_candidate(
    candidate: &Candidate,
    timeout: Duration,
    rubric: &Rubric,
    opts: &EvalOptions,
) -> Result<Evaluation> {
    for w in ignored_option_warnings(candidate, opts) {
        eprintln!("{w}");
    }
    let dir = tempfile::tempdir().context("一時ディレクトリの生成に失敗")?;
    let root = dir.path();

    let ctx = StageContext {
        timeout,
        tests_dir: opts.tests_dir,
        prop_tests_dir: opts.prop_tests_dir,
        wasm_opts: &opts.wasm_opts,
        run_mutation: opts.run_mutation,
    };
    let results = crate::language::runner_for(candidate.language).run(root, candidate, &ctx)?;

    // LLM セマンティックレビュー（オプション）
    let review = match (opts.run_llm_review, opts.reviewer) {
        (true, Some(f)) => f(&candidate.source, results.test.is_passed()),
        _ => None,
    };
    let (reasoning_score, reasoning_comment) = match review {
        Some(r) => (Some(r.score), Some(r.comment)),
        None => (None, None),
    };

    let results = StageResults {
        reasoning_score,
        reasoning_comment,
        ..results
    };

    Ok(assemble(candidate, results, rubric))
}

/// ステージ結果を採点して `Evaluation` を組み立てる（言語非依存）。
fn assemble(candidate: &Candidate, r: StageResults, rubric: &Rubric) -> Evaluation {
    let metrics = SourceMetrics::analyze(&candidate.source);
    let axes = scoring::axes_with_audit_status(
        &r.compile,
        &r.test,
        &r.prop_test,
        &r.lint,
        r.lint_warnings,
        &r.wasm,
        &metrics,
        rubric,
        r.mutation_caught,
        r.mutation_total,
        r.audit_findings,
    );
    let score = axes.total();
    Evaluation {
        candidate_id: candidate.id.clone(),
        compile: r.compile,
        test: r.test,
        lint: r.lint,
        lint_warnings: r.lint_warnings,
        prop_test: r.prop_test,
        wasm: r.wasm,
        wasm_fuel_used: r.wasm_fuel_used,
        mutation: r.mutation,
        mutation_caught: r.mutation_caught,
        mutation_total: r.mutation_total,
        audit_findings: r.audit_findings.unwrap_or(0),
        bench_ns: r.bench_ns,
        reasoning: if r.reasoning_score.is_some() {
            StageOutcome::Passed { duration_ms: 0 }
        } else {
            StageOutcome::Skipped
        },
        reasoning_score: r.reasoning_score.unwrap_or(0.0),
        reasoning_comment: r.reasoning_comment,
        axes,
        score,
    }
}

/// `cmd args...` を実行して成功終了するかでツールの有無を判定する。
///
/// 起動失敗（PATH にない等）・非 0 終了は false。stdin/stdout/stderr は使わない。
fn tool_available(cmd: &str, args: &[&str]) -> bool {
    std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// run_failed 判定と件数から lint の (結果, 件数) を決める。
///
/// 切り詰め時は出力の先頭/末尾だけを見た `run_failed` 判定を信用しない
/// (": not found" を混ぜて Skipped=係数 1.0 にされるのを防ぐ)。
fn lint_outcome(
    outcome: StageOutcome,
    out: &str,
    truncated: bool,
    run_failed: fn(&str) -> bool,
    count: fn(&str) -> usize,
) -> (StageOutcome, usize) {
    if !truncated && run_failed(out) {
        (StageOutcome::Skipped, 0)
    } else {
        (outcome, floor_if_truncated(count(out), truncated))
    }
}

#[cfg(test)]
mod load_tests {
    use super::*;

    fn write(dir: &Path, rel: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn unknown_extension_is_error_listing_supported() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "foo.java", "class A {}");
        let err = load_candidate(&p).unwrap_err().to_string();
        assert!(err.contains(".java"), "{err}");
        for ext in ["rs", "py", "go", "js", "ts"] {
            assert!(err.contains(ext), "{err}");
        }
        assert!(load_candidate(&write(d.path(), "foo.txt", "x")).is_err());
    }

    #[test]
    fn no_extension_defaults_to_rust() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "foo", "pub fn f() {}");
        assert_eq!(load_candidate(&p).unwrap().language, Language::Rust);
    }

    #[test]
    fn known_extensions_unchanged() {
        let d = tempfile::tempdir().unwrap();
        for (f, l) in [
            ("a.rs", Language::Rust),
            ("a.py", Language::Python),
            ("a.go", Language::Go),
            ("a.js", Language::JavaScript),
            ("A.TS", Language::TypeScript),
        ] {
            let c = load_candidate(&write(d.path(), f, "x")).unwrap();
            assert_eq!(c.language, l);
        }
    }

    #[test]
    fn duplicate_candidate_id_is_error_with_paths() {
        let d = tempfile::tempdir().unwrap();
        let a = write(d.path(), "a/foo.rs", "pub fn f() {}");
        let b = write(d.path(), "b/foo.rs", "pub fn g() {}");
        let err = load_candidates(&[a.clone(), b.clone()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("foo"), "{err}");
        assert!(err.contains(&a.display().to_string()), "{err}");
        assert!(err.contains(&b.display().to_string()), "{err}");
    }

    #[test]
    fn distinct_ids_load_in_order() {
        let d = tempfile::tempdir().unwrap();
        let a = write(d.path(), "a.rs", "pub fn f() {}");
        let b = write(d.path(), "b.py", "x = 1");
        let cs = load_candidates(&[a, b]).unwrap();
        assert_eq!(
            cs.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
    }
}

#[cfg(test)]
mod copy_assemble_tests {
    use super::*;

    fn write(dir: &Path, rel: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, body).unwrap();
        p
    }

    fn cand(source: &str) -> Candidate {
        Candidate {
            id: "c1".into(),
            source: source.into(),
            language: Language::Rust,
        }
    }

    fn passed() -> StageOutcome {
        StageOutcome::Passed { duration_ms: 7 }
    }

    // ---- load_candidate / load_candidates ----

    #[test]
    fn load_candidate_missing_file_is_error_with_path() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("nope.rs");
        let err = format!("{:#}", load_candidate(&p).unwrap_err());
        assert!(err.contains("候補ファイルの読込に失敗"), "{err}");
        assert!(err.contains(&p.display().to_string()), "{err}");
    }

    #[test]
    fn load_candidate_directory_is_error() {
        let d = tempfile::tempdir().unwrap();
        assert!(load_candidate(d.path()).is_err());
    }

    #[test]
    fn load_candidate_id_is_stem_and_source_is_preserved() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "x.test.py", "print(1)\n");
        let c = load_candidate(&p).unwrap();
        assert_eq!(c.id, "x.test");
        assert_eq!(c.source, "print(1)\n");
        assert_eq!(c.language, Language::Python);
    }

    #[test]
    fn load_candidate_extension_is_case_insensitive() {
        let d = tempfile::tempdir().unwrap();
        for (f, l) in [
            ("a.RS", Language::Rust),
            ("a.Py", Language::Python),
            ("a.GO", Language::Go),
            ("a.JS", Language::JavaScript),
        ] {
            assert_eq!(
                load_candidate(&write(d.path(), f, "x")).unwrap().language,
                l
            );
        }
    }

    #[test]
    fn load_candidates_empty_is_ok_and_missing_path_propagates() {
        let none: [&Path; 0] = [];
        assert!(load_candidates(&none).unwrap().is_empty());
        let d = tempfile::tempdir().unwrap();
        let ok = write(d.path(), "a.rs", "x");
        let missing = d.path().join("missing.rs");
        assert!(load_candidates(&[ok, missing]).is_err());
    }

    // ---- copy_ext ----

    #[test]
    fn copy_ext_copies_only_matching_extension() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        write(&src, "a.rs", "A");
        write(&src, "b.rs", "B");
        write(&src, "c.txt", "C");
        write(&src, "noext", "N");
        write(&src, "D.RS", "D");
        let dest = d.path().join("dest");
        copy_ext(&src, &dest, "rs").unwrap();
        let mut names: Vec<_> = fs::read_dir(&dest)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        // 拡張子比較は大文字小文字を区別する（D.RS はコピーされない）
        assert_eq!(names, ["a.rs", "b.rs"]);
        assert_eq!(fs::read_to_string(dest.join("a.rs")).unwrap(), "A");
        assert_eq!(fs::read_to_string(dest.join("b.rs")).unwrap(), "B");
    }

    #[test]
    fn copy_ext_creates_nested_dest_and_handles_no_match() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        write(&src, "c.txt", "C");
        let dest = d.path().join("x/y/z");
        copy_ext(&src, &dest, "rs").unwrap();
        assert!(dest.is_dir());
        assert_eq!(fs::read_dir(&dest).unwrap().count(), 0);
    }

    #[test]
    fn copy_ext_keep_existing_renames_colliding_file_with_prefix() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        write(&src, "props.rs", "prop");
        write(&src, "other.rs", "other");
        let dest = d.path().join("dest");
        write(&dest, "props.rs", "user");
        copy_ext_keep_existing(&src, &dest, "rs", "prop_").unwrap();
        assert_eq!(fs::read_to_string(dest.join("props.rs")).unwrap(), "user");
        assert_eq!(
            fs::read_to_string(dest.join("prop_props.rs")).unwrap(),
            "prop"
        );
        assert_eq!(fs::read_to_string(dest.join("other.rs")).unwrap(), "other");
    }

    #[test]
    fn copy_ext_keep_existing_errors_when_renamed_also_collides() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        write(&src, "props.rs", "prop");
        let dest = d.path().join("dest");
        write(&dest, "props.rs", "user");
        write(&dest, "prop_props.rs", "user2");
        let err = format!(
            "{:#}",
            copy_ext_keep_existing(&src, &dest, "rs", "prop_").unwrap_err()
        );
        assert!(err.contains("prop_props.rs"), "{err}");
        assert_eq!(fs::read_to_string(dest.join("props.rs")).unwrap(), "user");
        assert_eq!(
            fs::read_to_string(dest.join("prop_props.rs")).unwrap(),
            "user2"
        );
    }

    /// テストディレクトリに候補ファイルと同名のファイルがあっても候補を上書きさせない。
    fn assert_collision_is_error(
        name: &str,
        run: fn(&Path, &Candidate, &StageContext) -> Result<StageResults>,
    ) {
        let d = tempfile::tempdir().unwrap();
        let tests = d.path().join("tests");
        write(&tests, name, "EVIL");
        let root = d.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let wasm_opts = WasmOptions::default();
        let ctx = StageContext {
            timeout: Duration::from_secs(5),
            tests_dir: Some(&tests),
            prop_tests_dir: None,
            wasm_opts: &wasm_opts,
            run_mutation: false,
        };
        let res = run(&root, &cand("CANDIDATE"), &ctx);
        let err = format!("{:#}", res.err().expect("衝突はエラーになるべき"));
        assert!(err.contains(name), "{err}");
        assert_eq!(fs::read_to_string(root.join(name)).unwrap(), "CANDIDATE");
    }

    #[test]
    fn python_tests_dir_candidate_name_collision_is_error() {
        assert_collision_is_error("candidate.py", run_python_stages);
    }

    #[test]
    fn go_tests_dir_candidate_name_collision_is_error() {
        assert_collision_is_error("candidate.go", run_go_stages);
    }

    #[test]
    fn js_tests_dir_candidate_name_collision_is_error() {
        assert_collision_is_error("candidate.js", run_js_stages);
    }

    #[test]
    fn ts_tests_dir_candidate_name_collision_is_error() {
        assert_collision_is_error("candidate.ts", run_ts_stages);
    }

    #[test]
    fn copy_ext_does_not_recurse_and_overwrites_existing() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        write(&src, "sub/inner.rs", "I");
        write(&src, "a.rs", "new");
        let dest = d.path().join("dest");
        write(&dest, "a.rs", "old");
        copy_ext(&src, &dest, "rs").unwrap();
        assert!(!dest.join("inner.rs").exists());
        assert!(!dest.join("sub").exists());
        assert_eq!(fs::read_to_string(dest.join("a.rs")).unwrap(), "new");
    }

    #[test]
    fn copy_ext_missing_src_is_error_but_dest_already_created() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("absent");
        let dest = d.path().join("dest");
        let err = format!("{:#}", copy_ext(&src, &dest, "rs").unwrap_err());
        assert!(err.contains("テストディレクトリの読込に失敗"), "{err}");
        assert!(err.contains(&src.display().to_string()), "{err}");
        // 現状仕様: create_dir_all が read_dir より先に走る
        assert!(dest.is_dir());
    }

    #[test]
    fn copy_ext_directory_named_with_ext_is_copy_error() {
        // 現状仕様を固定: 拡張子一致のディレクトリは is_file 判定がなく fs::copy が失敗する
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        fs::create_dir_all(src.join("weird.rs")).unwrap();
        let err = format!(
            "{:#}",
            copy_ext(&src, &d.path().join("dest"), "rs").unwrap_err()
        );
        assert!(err.contains("テストのコピーに失敗"), "{err}");
    }

    // ---- assemble ----

    #[test]
    fn assemble_without_reasoning_is_skipped_and_score_equals_axes_total() {
        let c = cand("pub fn f() {}");
        let r = StageResults {
            compile: passed(),
            test: passed(),
            ..Default::default()
        };
        let e = assemble(&c, r, &Rubric::default());
        assert_eq!(e.candidate_id, "c1");
        assert!(matches!(e.reasoning, StageOutcome::Skipped));
        assert_eq!(e.reasoning_score, 0.0);
        assert_eq!(e.reasoning_comment, None);
        assert_eq!(e.score, e.axes.total());
        // compile+test のみ: correctness = 50 * 0.8
        assert!((e.axes.correctness - 40.0).abs() < 1e-9, "{:?}", e.axes);
        assert_eq!(e.axes.performance, 0.0);
        assert_eq!(e.axes.resource_usage, 0.0);
    }

    #[test]
    fn assemble_with_reasoning_score_marks_passed_but_axis_is_not_assigned_here() {
        let c = cand("pub fn f() {}");
        let rubric = Rubric {
            reasoning: 10.0,
            ..Rubric::default()
        };
        let r = StageResults {
            compile: passed(),
            test: passed(),
            reasoning_score: Some(0.8),
            reasoning_comment: Some("ok".into()),
            ..Default::default()
        };
        let e = assemble(&c, r, &rubric);
        assert!(matches!(
            e.reasoning,
            StageOutcome::Passed { duration_ms: 0 }
        ));
        assert_eq!(e.reasoning_score, 0.8);
        assert_eq!(e.reasoning_comment.as_deref(), Some("ok"));
        // 現状仕様: axes.reasoning は assign_reasoning() で後から付与されるため 0 のまま
        assert_eq!(e.axes.reasoning, 0.0);
        assert_eq!(e.score, e.axes.total());
    }

    #[test]
    fn assemble_some_zero_reasoning_is_passed_not_skipped() {
        let r = StageResults {
            reasoning_score: Some(0.0),
            ..Default::default()
        };
        let e = assemble(&cand("x"), r, &Rubric::default());
        assert!(matches!(
            e.reasoning,
            StageOutcome::Passed { duration_ms: 0 }
        ));
        assert_eq!(e.reasoning_score, 0.0);
    }

    #[test]
    fn assemble_compile_failure_zeroes_axes_and_passes_fields_through() {
        let r = StageResults {
            compile: StageOutcome::Failed {
                detail: "boom".into(),
            },
            lint_warnings: 3,
            mutation_caught: 2,
            mutation_total: 5,
            audit_findings: Some(1),
            bench_ns: Some(42),
            wasm_fuel_used: Some(9),
            ..Default::default()
        };
        let e = assemble(&cand("pub fn f() {}"), r, &Rubric::default());
        assert!(matches!(e.compile, StageOutcome::Failed { .. }));
        assert_eq!(e.axes.security, 0.0);
        assert_eq!(e.axes.maintainability, 0.0);
        assert_eq!(e.score, e.axes.total());
        assert_eq!(
            (e.lint_warnings, e.mutation_caught, e.mutation_total),
            (3, 2, 5)
        );
        assert_eq!(
            (e.audit_findings, e.bench_ns, e.wasm_fuel_used),
            (1, Some(42), Some(9))
        );
    }
}

#[cfg(test)]
mod test_verdict_failure_tests {
    use super::*;

    #[test]
    fn missing_summary_is_failure() {
        assert!(test_verdict_failure(Language::Rust, "", false).is_some());
        assert!(test_verdict_failure(Language::Python, "", false).is_some());
    }

    #[test]
    fn zero_run_is_allowed() {
        let o = "     Running unittests src/lib.rs (a)\n\
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n";
        assert!(test_verdict_failure(Language::Rust, o, false).is_none());
        assert!(test_verdict_failure(Language::Go, "?   \tx\t[no test files]\n", false).is_none());
    }

    #[test]
    fn go_ok_with_no_tests_marker_is_allowed() {
        let marked =
            "testing: warning: no tests to run\nPASS\nok  \tcandidate\t0.002s [no tests to run]\n";
        assert!(test_verdict_failure(Language::Go, marked, false).is_none());
        let unmarked = "ok  \tcandidate\t0.002s\n";
        assert!(test_verdict_failure(Language::Go, unmarked, false).is_some());
        let with_run = "=== RUN   TestA\n--- PASS: TestA (0.00s)\nPASS\nok  \tcandidate\t0.002s\n";
        assert!(test_verdict_failure(Language::Go, with_run, false).is_none());
    }

    #[test]
    fn go_with_test_funcs_requires_run_even_with_no_tests_marker() {
        // 偽装 init が "[no tests to run]" を出して exit(0) しても、Test 関数があれば合格にしない。
        let forged = "ok  \tcandidate\t0.002s [no tests to run]\n";
        assert!(test_verdict_failure(Language::Go, forged, true).is_some());
        let real = "=== RUN   TestA\n--- PASS: TestA (0.00s)\nPASS\nok  \tcandidate\t0.002s\n";
        assert!(test_verdict_failure(Language::Go, real, true).is_none());
    }

    #[test]
    fn go_ok_without_run_lines_is_failure() {
        assert!(test_verdict_failure(Language::Go, "ok  \tx\t0.003s\n", false).is_some());
        let o = "=== RUN   TestA\n--- PASS: TestA (0.00s)\nPASS\nok  \tx\t0.003s\n";
        assert!(test_verdict_failure(Language::Go, o, false).is_none());
    }
}

#[cfg(test)]
mod truncated_lint_tests {
    use super::python::ruff_run_failed;
    use super::*;

    #[test]
    fn truncated_output_floors_lint_count_to_zero_score() {
        assert_eq!(floor_if_truncated(3, false), 3);
        assert_eq!(floor_if_truncated(3, true), TRUNCATED_LINT_FLOOR);
        assert_eq!(floor_if_truncated(5000, true), 5000);
        // 床値で security / maintainability の係数が 0 になる(scoring.rs の閾値 30 件以上)。
        let floored = floor_if_truncated(0, true);
        assert!(floored >= 30, "床値が採点の閾値未満: {floored}");
    }

    /// head/tail に ": not found" を含む切り詰め出力でも lint が最低点になる。
    #[test]
    fn truncated_output_with_not_found_still_scores_zero() {
        use crate::model::StageOutcome;
        let out = "sh: 1: ruff: not found\nsh: staticcheck: command not found\n";
        assert!(ruff_run_failed(out));
        let passed = StageOutcome::Passed { duration_ms: 1 };
        let (lint, n) = lint_outcome(
            passed.clone(),
            out,
            true,
            ruff_run_failed,
            count_ruff_findings,
        );
        assert!(matches!(lint, StageOutcome::Passed { .. }), "{lint:?}");
        assert_eq!(crate::scoring::lint_security_ratio(&lint, n), 0.0);
        assert_eq!(crate::scoring::lint_maintainability_ratio(n), 0.0);
        // 切り詰めなしなら従来どおり Skipped。
        let (lint, n) = lint_outcome(passed, out, false, ruff_run_failed, count_ruff_findings);
        assert!(matches!(lint, StageOutcome::Skipped));
        assert_eq!(n, 0);
    }
}

#[cfg(test)]
mod tool_available_tests {
    use super::tool_available;

    #[test]
    fn existing_command_with_success_exit_is_available() {
        assert!(tool_available("sh", &["-c", "exit 0"]));
    }

    #[test]
    fn missing_command_is_not_available() {
        assert!(!tool_available(
            "definitely-not-a-real-tool-xyz",
            &["--version"]
        ));
    }
}

#[cfg(test)]
mod ignored_option_tests {
    use super::*;

    fn cand(lang: Language) -> Candidate {
        Candidate {
            id: "c1".into(),
            source: String::new(),
            language: lang,
        }
    }

    fn loud<'a>(p: &'a Path) -> EvalOptions<'a> {
        EvalOptions {
            prop_tests_dir: Some(p),
            wasm_opts: WasmOptions {
                entry: Some("run"),
                ..Default::default()
            },
            run_mutation: true,
            ..Default::default()
        }
    }

    #[test]
    fn js_warns_for_all_unsupported_options() {
        let p = Path::new("x");
        let w = ignored_option_warnings(&cand(Language::JavaScript), &loud(p));
        assert_eq!(w.len(), 3);
        assert!(w.iter().any(|m| m.contains("--mutation")));
        assert!(w.iter().any(|m| m.contains("--prop-tests")));
        assert!(w.iter().any(|m| m.contains("--wasm-entry")));
    }

    #[test]
    fn python_warns_only_for_prop_tests_and_wasm() {
        let p = Path::new("x");
        let w = ignored_option_warnings(&cand(Language::Python), &loud(p));
        assert_eq!(w.len(), 2);
        assert!(!w.iter().any(|m| m.contains("--mutation")));
    }

    #[test]
    fn rust_and_unset_options_do_not_warn() {
        let p = Path::new("x");
        assert!(ignored_option_warnings(&cand(Language::Rust), &loud(p)).is_empty());
        assert!(
            ignored_option_warnings(&cand(Language::TypeScript), &EvalOptions::default())
                .is_empty()
        );
    }
}
