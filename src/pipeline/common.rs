//! 共通ヘルパ (テストファイルのコピー・テストステージ実行・lint 件数の下限)。

use super::*;

/// テストディレクトリ内の指定拡張子ファイルを `dest` へコピーする。
pub(super) fn copy_ext(src_dir: &Path, dest: &Path, ext: &str) -> Result<()> {
    fs::create_dir_all(dest).context("コピー先ディレクトリの作成に失敗")?;
    for entry in fs::read_dir(src_dir)
        .with_context(|| format!("テストディレクトリの読込に失敗: {}", src_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            let name = path.file_name().unwrap();
            fs::copy(&path, dest.join(name))
                .with_context(|| format!("テストのコピーに失敗: {}", path.display()))?;
        }
    }
    Ok(())
}

/// `copy_ext` と同様だが、`dest` に同名ファイルが既にあればエラーにする。
/// 候補ソースを先に書き込む経路で、テストファイルが候補を上書きするのを防ぐ。
pub(super) fn copy_ext_no_clobber(src_dir: &Path, dest: &Path, ext: &str) -> Result<()> {
    fs::create_dir_all(dest).context("コピー先ディレクトリの作成に失敗")?;
    for entry in fs::read_dir(src_dir)
        .with_context(|| format!("テストディレクトリの読込に失敗: {}", src_dir.display()))?
    {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some(ext) {
            continue;
        }
        let name = path.file_name().unwrap();
        let target = dest.join(name);
        if target.exists() {
            anyhow::bail!(
                "テストファイル名が候補ファイルと衝突: {} ({} は候補が使用済み。リネームしてください)",
                path.display(),
                name.to_string_lossy()
            );
        }
        fs::copy(&path, &target)
            .with_context(|| format!("テストのコピーに失敗: {}", path.display()))?;
    }
    Ok(())
}

/// `copy_ext` と同様だが、`dest` に同名ファイルが既にあれば上書きせず
/// `prefix` 付きの名前でコピーする。それも存在すればエラー(黙って上書きしない)。
/// Rust の integration test は 1 ファイル 1 クレートなのでリネームしても安全。
pub(super) fn copy_ext_keep_existing(
    src_dir: &Path,
    dest: &Path,
    ext: &str,
    prefix: &str,
) -> Result<()> {
    fs::create_dir_all(dest).context("コピー先ディレクトリの作成に失敗")?;
    for entry in fs::read_dir(src_dir)
        .with_context(|| format!("テストディレクトリの読込に失敗: {}", src_dir.display()))?
    {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some(ext) {
            continue;
        }
        let name = path.file_name().unwrap();
        let mut target = dest.join(name);
        if target.exists() {
            let renamed = format!("{prefix}{}", name.to_string_lossy());
            target = dest.join(&renamed);
            if target.exists() {
                anyhow::bail!(
                    "テストファイル名が衝突: {} ({} も既に存在)",
                    path.display(),
                    renamed
                );
            }
        }
        fs::copy(&path, &target)
            .with_context(|| format!("テストのコピーに失敗: {}", path.display()))?;
    }
    Ok(())
}

/// テストステージを実行し、終了コードに加えて出力の結果行も確認する (#226)。
/// 合格は「exit 0 かつ結果行あり かつ失敗 0」。0 件実行は許可する。
pub(super) fn run_test_stage(
    label: &str,
    lang: Language,
    cmd: Command,
    timeout: Duration,
    go_has_tests: bool,
) -> StageOutcome {
    let (outcome, output) = runner::run_stage_capture(label, cmd, timeout);
    if !outcome.is_passed() {
        return outcome;
    }
    match test_verdict_failure(lang, &output, go_has_tests) {
        Some(detail) => StageOutcome::Failed { detail },
        None => outcome,
    }
}

/// 出力が合格と認められない場合に理由を返す。
pub(super) fn test_verdict_failure(
    lang: Language,
    output: &str,
    go_has_tests: bool,
) -> Option<String> {
    match parse_test_summary(lang, output) {
        TestVerdict::NoSummary => {
            Some("テスト結果行が見つからない(exit(0) 等で打ち切られた可能性)".to_string())
        }
        TestVerdict::Failed { reason } => Some(reason),
        TestVerdict::Ok { passed } => {
            // Go: テストが走った ok パッケージには `=== RUN` 行が出る。無ければ init 等での打ち切り。
            // `[no tests to run]` 付きの ok 行(テスト 0 件)は、Test 関数を持つ `*_test.go` が
            // 無い場合に限り 0 件実行として許可する(あるのに付くなら init の偽装)。
            let needs_run = output.lines().any(|l| {
                l.split_whitespace().next() == Some("ok")
                    && (go_has_tests || !l.contains("[no tests to run]"))
            });
            if lang == Language::Go && passed > 0 && needs_run && !output.contains("=== RUN") {
                Some("go test: ok だが実行ログ(=== RUN)が無い(os.Exit(0) 等の可能性)".to_string())
            } else {
                None
            }
        }
    }
}

/// 出力が取り込み上限で切り詰められた場合の lint 件数の下限。
///
/// 切り詰め後の出力で数えると過少になり、大量の警告を出すほど得をしてしまう。
/// 警告過多とみなし、security / maintainability の係数が 0 になる値へ引き上げる。
pub(crate) const TRUNCATED_LINT_FLOOR: usize = 1000;

/// 切り詰めが発生していれば件数を `TRUNCATED_LINT_FLOOR` 以上にする。
pub(super) fn floor_if_truncated(count: usize, truncated: bool) -> usize {
    if truncated {
        count.max(TRUNCATED_LINT_FLOOR)
    } else {
        count
    }
}

/// mutation ツール (cargo mutants / mutmut) の JSON 出力から (caught, total_mutants) を取り出す。
///
/// stdout の最後の JSON 行を探す（進捗ログが混在する場合があるため）。
pub(super) fn parse_mutation_json(stdout: &str) -> Option<(usize, usize)> {
    #[derive(serde::Deserialize)]
    struct MutationOutput {
        total_mutants: usize,
        caught: usize,
    }
    for line in stdout.lines().rev() {
        let line = line.trim();
        if line.starts_with('{') {
            if let Ok(m) = serde_json::from_str::<MutationOutput>(line) {
                return Some((m.caught, m.total_mutants));
            }
        }
    }
    None
}
