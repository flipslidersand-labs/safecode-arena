//! 実行条件キー（実行条件の比較キー）の組み立て。

use crate::config::{Config, Rubric};
use crate::model::{Candidate, Language};
use crate::pipeline::{self, EvalOptions};

/// スコアに影響する実行条件（比較キーの材料）。
pub struct RunConditions<'a> {
    pub llm_review: bool,
    pub mutation: bool,
    pub wasm_entry: Option<&'a str>,
    pub tests: bool,
    pub prop_tests: bool,
    pub wasm_fuel: u64,
    pub wasm_max_memory_mb: usize,
    /// 今回評価した候補の (言語名, 適用ルーブリック)。
    pub rubrics: Vec<(&'a str, Rubric)>,
    /// 今回評価した候補の言語。対応しないオプションをキーから除くために使う。
    pub languages: Vec<Language>,
}

/// スコアに影響する実行条件を正規化した比較キー（#191, #243）。
///
/// 含めるもの: --llm-review / --mutation / --wasm-entry / --wasm-fuel / --wasm-max-memory-mb（上限超過で resource_usage が変わる）、--tests・--prop-tests を使ったか（有無のみ、パスは含めない）、
/// 評価した言語ごとの適用ルーブリック（重み）。ハッシュは使わず、言語名で整列・重複排除した
/// 文字列にするため決定的で、不一致時に原因を目視できる。
/// ただし候補の言語が対応しないオプション（`capabilities`）は、どの候補も対応しない場合キーから除く。
/// 含めないもの: --out / --format / --timeout-secs / --db / --regression-threshold（採点に無関係、または判定側の設定）。
pub fn conditions_key(c: &RunConditions) -> String {
    let mut rubrics: Vec<(&str, Rubric)> = c.rubrics.clone();
    rubrics.sort_by(|a, b| a.0.cmp(b.0));
    rubrics.dedup_by(|a, b| a.0 == b.0);
    let weights: Vec<String> = rubrics
        .iter()
        .map(|(lang, r)| {
            format!(
                "{lang}:{},{},{},{},{},{}",
                r.correctness,
                r.security,
                r.performance,
                r.maintainability,
                r.resource_usage,
                r.reasoning
            )
        })
        .collect();
    // どの候補の言語も対応しないオプションは実行結果に影響しないのでキーに含めない。
    // Rust 候補を含む run では全オプションが有効なので従来のキーと同一。
    let caps: Vec<_> = c
        .languages
        .iter()
        .map(|l| pipeline::capabilities(*l))
        .collect();
    let mutation = c.mutation && caps.iter().any(|k| k.mutation);
    let prop_tests = c.prop_tests && caps.iter().any(|k| k.prop_tests);
    let wasm = caps.iter().any(|k| k.wasm);
    let (wasm_entry, wasm_fuel, wasm_max_memory_mb) = if wasm {
        (
            c.wasm_entry.unwrap_or(""),
            c.wasm_fuel,
            c.wasm_max_memory_mb,
        )
    } else {
        ("", 0, 0)
    };
    format!(
        "llm_review={};mutation={};wasm_entry={};tests={};prop_tests={};wasm_fuel={};wasm_max_memory_mb={};rubric={}",
        c.llm_review,
        mutation,
        wasm_entry,
        c.tests,
        prop_tests,
        wasm_fuel,
        wasm_max_memory_mb,
        weights.join("|")
    )
}

/// `EvalOptions` と候補・設定から実行条件キーを導出する。
/// 評価失敗で脱落した候補も含めるため、`cands` には読み込んだ全候補を渡す
/// （脱落でキーが変わると #181 の脱落検出が効かなくなる）。
pub fn run_options_key(opts: &EvalOptions, cands: &[Candidate], cfg: &Config) -> String {
    conditions_key(&RunConditions {
        llm_review: opts.run_llm_review,
        mutation: opts.run_mutation,
        wasm_entry: opts.wasm_opts.entry,
        tests: opts.tests_dir.is_some(),
        prop_tests: opts.prop_tests_dir.is_some(),
        wasm_fuel: opts.wasm_opts.fuel,
        // EvalOptions は bytes、キーは MB 単位。
        wasm_max_memory_mb: opts.wasm_opts.max_memory_bytes / (1024 * 1024),
        rubrics: cands
            .iter()
            .map(|c| (c.language.name(), cfg.rubric_for(c.language.name())))
            .collect(),
        languages: cands.iter().map(|c| c.language).collect(),
    })
}
