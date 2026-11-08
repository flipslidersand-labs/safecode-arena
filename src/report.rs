//! Markdown / HTML レポート生成。

use crate::model::{Evaluation, StageOutcome};

/// 軸スコアを取り出す関数の型（HTML レポートの軸テーブル用）。
type AxisGetter = fn(&Evaluation) -> f64;

// ── 共通ヘルパー ──────────────────────────────────────────────────────────────

fn stage_cell(o: &StageOutcome) -> String {
    match o {
        StageOutcome::Passed { duration_ms } => format!("✅ {duration_ms}ms"),
        StageOutcome::Failed { .. } => "❌ failed".to_string(),
        StageOutcome::TimedOut { limit_ms } => format!("⏱ timeout({limit_ms}ms)"),
        StageOutcome::Skipped => "— skipped".to_string(),
    }
}

fn lint_cell(o: &StageOutcome, warnings: usize) -> String {
    match o {
        StageOutcome::Passed { .. } if warnings == 0 => "✅ 0 warn".to_string(),
        StageOutcome::Passed { .. } => format!("⚠️ {warnings} warn"),
        StageOutcome::Failed { .. } => "❌ failed".to_string(),
        StageOutcome::TimedOut { limit_ms } => format!("⏱ timeout({limit_ms}ms)"),
        StageOutcome::Skipped => "— skipped".to_string(),
    }
}

/// Wasm サンドボックスの結果セル。fuel 消費量を併記する。
fn wasm_cell(o: &StageOutcome, fuel_used: Option<u64>) -> String {
    let fuel = fuel_used.map(|f| format!(" {f}f")).unwrap_or_default();
    match o {
        StageOutcome::Passed { .. } => format!("✅{fuel}"),
        StageOutcome::Failed { .. } => "❌ failed".to_string(),
        StageOutcome::TimedOut { .. } => format!("⏱ fuel切れ{fuel}"),
        StageOutcome::Skipped => "— skipped".to_string(),
    }
}

fn stage_label(o: &StageOutcome) -> &'static str {
    match o {
        StageOutcome::Passed { .. } => "passed",
        StageOutcome::Failed { .. } => "failed",
        StageOutcome::TimedOut { .. } => "timeout",
        StageOutcome::Skipped => "skipped",
    }
}

fn medal(rank: usize) -> &'static str {
    match rank {
        0 => "🥇",
        1 => "🥈",
        2 => "🥉",
        _ => "  ",
    }
}

// ── Markdown ─────────────────────────────────────────────────────────────────

/// ランク済みの評価から Markdown レポートを生成する。
pub fn render(evals: &[Evaluation]) -> String {
    let mut out = String::new();
    out.push_str("# SafeCode Arena 評価レポート\n\n");
    out.push_str(&format!(
        "生成日時: {}\n\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    ));
    render_summary_table(&mut out, evals);
    if evals.len() > 1 {
        render_candidate_details(&mut out, evals);
        render_comparison(&mut out, evals);
    }
    out
}

fn mutation_cell(caught: usize, total: usize, outcome: &StageOutcome) -> String {
    match outcome {
        StageOutcome::Skipped => "—".to_string(),
        StageOutcome::Failed { .. } | StageOutcome::TimedOut { .. } => "❌ failed".to_string(),
        StageOutcome::Passed { .. } => {
            if total == 0 {
                "✅ 0 mutants".to_string()
            } else {
                let pct = caught as f64 / total as f64 * 100.0;
                format!("{caught}/{total} ({pct:.0}%)")
            }
        }
    }
}

fn render_summary_table(out: &mut String, evals: &[Evaluation]) {
    let has_mutation = evals.iter().any(|e| e.mutation_total > 0);
    out.push_str("## 比較サマリー\n\n");
    let mutation_col = if has_mutation { " Mutation |" } else { "" };
    let mutation_sep = if has_mutation { " -------- |" } else { "" };
    out.push_str(&format!(
        "| 順位 | 候補 | 合計 | 正誤 | 安全 | 性能 | 保守 | 資源 | コンパイル | テスト | Clippy | PropTest | Wasm |{}\n",
        mutation_col
    ));
    out.push_str(&format!(
        "| ---- | ---- | ---- | ---- | ---- | ---- | ---- | ---- | ---------- | ------ | ------ | -------- | ---- |{}\n",
        mutation_sep
    ));
    for (i, e) in evals.iter().enumerate() {
        let mutation_cell_str = if has_mutation {
            format!(
                " {} |",
                mutation_cell(e.mutation_caught, e.mutation_total, &e.mutation)
            )
        } else {
            String::new()
        };
        out.push_str(&format!(
            "| {} {} | {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {} | {} | {} | {} | {} |{}\n",
            i + 1,
            medal(i),
            e.candidate_id,
            e.score,
            e.axes.correctness,
            e.axes.security,
            e.axes.performance,
            e.axes.maintainability,
            e.axes.resource_usage,
            stage_cell(&e.compile),
            stage_cell(&e.test),
            lint_cell(&e.lint, e.lint_warnings),
            stage_cell(&e.prop_test),
            wasm_cell(&e.wasm, e.wasm_fuel_used),
            mutation_cell_str,
        ));
    }
    if let Some(best) = evals.first() {
        out.push_str(&format!(
            "\n**採用候補**: `{}`（{:.1}点）\n\n",
            best.candidate_id, best.score
        ));
    }
}

fn render_candidate_details(out: &mut String, evals: &[Evaluation]) {
    out.push_str("---\n\n## 候補詳細\n\n");
    const RUBRIC: &[(&str, f64)] = &[
        ("correctness", 50.0),
        ("security", 20.0),
        ("performance", 15.0),
        ("maintainability", 10.0),
        ("resource_usage", 5.0),
    ];

    for (i, e) in evals.iter().enumerate() {
        out.push_str(&format!(
            "### {} {}位: `{}` — {:.1} 点\n\n",
            medal(i),
            i + 1,
            e.candidate_id,
            e.score
        ));

        out.push_str("**軸別スコア**\n\n");
        out.push_str("| 軸 | スコア | 上限 | 達成率 |\n");
        out.push_str("| -- | ------ | ---- | ------ |\n");
        let axis_scores = [
            e.axes.correctness,
            e.axes.security,
            e.axes.performance,
            e.axes.maintainability,
            e.axes.resource_usage,
        ];
        for ((name, limit), score) in RUBRIC.iter().zip(axis_scores.iter()) {
            let pct = if *limit > 0.0 {
                score / limit * 100.0
            } else {
                0.0
            };
            out.push_str(&format!(
                "| {name} | {score:.1} | {limit:.0} | {pct:.0}% |\n"
            ));
        }
        out.push('\n');

        out.push_str("**ステージ結果**\n\n");
        out.push_str(&format!("- コンパイル: {}\n", stage_cell(&e.compile)));
        out.push_str(&format!("- テスト: {}\n", stage_cell(&e.test)));
        out.push_str(&format!(
            "- Clippy: {}\n",
            lint_cell(&e.lint, e.lint_warnings)
        ));
        out.push_str(&format!("- PropTest: {}\n", stage_cell(&e.prop_test)));
        if !matches!(e.mutation, StageOutcome::Skipped) {
            out.push_str(&format!(
                "- Mutation: {}\n",
                mutation_cell(e.mutation_caught, e.mutation_total, &e.mutation)
            ));
        }
        if !matches!(e.wasm, StageOutcome::Skipped) {
            let wasm_line = if let Some(fuel) = e.wasm_fuel_used {
                format!("- Wasm: {} (fuel使用: {})\n", stage_cell(&e.wasm), fuel)
            } else {
                format!("- Wasm: {}\n", stage_cell(&e.wasm))
            };
            out.push_str(&wasm_line);
        }

        // Failure details
        let mut has_failures = false;
        for (label, stage) in [
            ("コンパイル", &e.compile),
            ("テスト", &e.test),
            ("Lint", &e.lint),
            ("PropTest", &e.prop_test),
        ] {
            if let StageOutcome::Failed { detail } = stage {
                if !has_failures {
                    out.push_str("\n**失敗詳細**\n\n");
                    has_failures = true;
                }
                let snippet = detail.lines().take(8).collect::<Vec<_>>().join("\n");
                let fence = fence_for(&snippet);
                out.push_str(&format!(
                    "<details><summary>{label} エラー</summary>\n\n{fence}\n{snippet}\n{fence}\n\n</details>\n\n"
                ));
            }
        }
        out.push('\n');
    }
}

/// `content` 中の最長のバッククォート連続より長いコードフェンス(最低 3 個)を返す。
/// 候補由来の文字列がフェンスを閉じて Markdown を差し込むのを防ぐ。
pub(crate) fn fence_for(content: &str) -> String {
    let mut longest = 0usize;
    let mut run = 0usize;
    for c in content.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

fn render_comparison(out: &mut String, evals: &[Evaluation]) {
    out.push_str("---\n\n## 候補間比較\n\n");

    let ids: Vec<&str> = evals.iter().map(|e| e.candidate_id.as_str()).collect();
    out.push_str("| 観点 |");
    for id in &ids {
        out.push_str(&format!(" {id} |"));
    }
    out.push('\n');
    out.push_str("| ---- |");
    for _ in &ids {
        out.push_str(" ---- |");
    }
    out.push('\n');

    // Compile time
    let compile_ms: Vec<Option<u64>> = evals.iter().map(|e| e.compile.duration_ms()).collect();
    let min_compile = compile_ms.iter().flatten().copied().min();
    out.push_str("| コンパイル時間 |");
    for ms in &compile_ms {
        match ms {
            Some(v) if Some(*v) == min_compile && compile_ms.iter().flatten().count() > 1 => {
                out.push_str(&format!(" **{v}ms** ⚡ |"))
            }
            Some(v) => out.push_str(&format!(" {v}ms |")),
            None => out.push_str(" — |"),
        }
    }
    out.push('\n');

    // Test time
    let test_ms: Vec<Option<u64>> = evals.iter().map(|e| e.test.duration_ms()).collect();
    let min_test = test_ms.iter().flatten().copied().min();
    out.push_str("| テスト時間 |");
    for ms in &test_ms {
        match ms {
            Some(v) if Some(*v) == min_test && test_ms.iter().flatten().count() > 1 => {
                out.push_str(&format!(" **{v}ms** ⚡ |"))
            }
            Some(v) => out.push_str(&format!(" {v}ms |")),
            None => out.push_str(" — |"),
        }
    }
    out.push('\n');

    // Clippy warnings
    let warns: Vec<usize> = evals.iter().map(|e| e.lint_warnings).collect();
    let min_warn = warns.iter().copied().min().unwrap_or(0);
    out.push_str("| Clippy警告 |");
    for w in &warns {
        if *w == min_warn && warns.iter().any(|x| x != w) {
            out.push_str(&format!(" **{w}件** ✅ |"));
        } else {
            out.push_str(&format!(" {w}件 |"));
        }
    }
    out.push('\n');

    // Axis scores
    for (axis, scores) in [
        (
            "正誤スコア",
            evals.iter().map(|e| e.axes.correctness).collect::<Vec<_>>(),
        ),
        (
            "安全スコア",
            evals.iter().map(|e| e.axes.security).collect::<Vec<_>>(),
        ),
        (
            "性能スコア",
            evals.iter().map(|e| e.axes.performance).collect::<Vec<_>>(),
        ),
        (
            "保守スコア",
            evals
                .iter()
                .map(|e| e.axes.maintainability)
                .collect::<Vec<_>>(),
        ),
    ] {
        let max = scores.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        out.push_str(&format!("| {axis} |"));
        for s in &scores {
            if (*s - max).abs() < 0.01 && scores.iter().any(|x| (x - s).abs() > 0.01) {
                out.push_str(&format!(" **{s:.1}** |"));
            } else {
                out.push_str(&format!(" {s:.1} |"));
            }
        }
        out.push('\n');
    }

    // Total
    let scores: Vec<f64> = evals.iter().map(|e| e.score).collect();
    let max_score = scores.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    out.push_str("| **総合スコア** |");
    for s in &scores {
        if (*s - max_score).abs() < 0.01 {
            out.push_str(&format!(" **{s:.1}** 🏆 |"));
        } else {
            out.push_str(&format!(" {s:.1} |"));
        }
    }
    out.push_str("\n\n");
}

// ── HTML ─────────────────────────────────────────────────────────────────────

/// ランク済みの評価からスタンドアロン HTML レポートを生成する。
pub fn render_html(evals: &[Evaluation]) -> String {
    let generated_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mut body = String::new();

    // Summary table
    body.push_str("<h2>比較サマリー</h2>\n<table>\n");
    body.push_str("<tr><th>順位</th><th>候補</th><th>合計</th><th>正誤</th><th>安全</th><th>性能</th><th>保守</th><th>コンパイル</th><th>テスト</th><th>Clippy</th><th>PropTest</th></tr>\n");
    for (i, e) in evals.iter().enumerate() {
        let row_class = if i == 0 { " class=\"best\"" } else { "" };
        body.push_str(&format!(
            "<tr{row_class}><td>{} {}</td><td><code>{}</code></td><td><strong>{:.1}</strong></td><td>{:.1}</td><td>{:.1}</td><td>{:.1}</td><td>{:.1}</td><td class=\"{}\">{}</td><td class=\"{}\">{}</td><td>{}</td><td class=\"{}\">{}</td></tr>\n",
            i + 1, medal(i),
            e.candidate_id, e.score,
            e.axes.correctness, e.axes.security, e.axes.performance, e.axes.maintainability,
            stage_label(&e.compile), stage_cell(&e.compile),
            stage_label(&e.test), stage_cell(&e.test),
            lint_cell(&e.lint, e.lint_warnings),
            stage_label(&e.prop_test), stage_cell(&e.prop_test),
        ));
    }
    body.push_str("</table>\n");

    // Score bars
    body.push_str("<h2>スコア可視化</h2>\n");
    for e in evals {
        let pct = e.score.min(100.0);
        let color = if pct >= 70.0 {
            "#4caf50"
        } else if pct >= 40.0 {
            "#ff9800"
        } else {
            "#f44336"
        };
        body.push_str(&format!(
            "<div class=\"bar-row\"><span class=\"bar-label\"><code>{}</code></span><div class=\"bar-wrap\"><div class=\"bar\" style=\"width:{pct:.1}%;background:{color}\"></div></div><span class=\"bar-score\">{:.1}</span></div>\n",
            e.candidate_id, e.score
        ));
    }

    // Per-candidate details
    body.push_str("<h2>候補詳細</h2>\n");
    const AXES: &[(&str, AxisGetter, f64)] = &[
        ("correctness", |e| e.axes.correctness, 50.0),
        ("security", |e| e.axes.security, 20.0),
        ("performance", |e| e.axes.performance, 15.0),
        ("maintainability", |e| e.axes.maintainability, 10.0),
        ("resource_usage", |e| e.axes.resource_usage, 5.0),
    ];
    for (i, e) in evals.iter().enumerate() {
        body.push_str(&format!(
            "<details open><summary><strong>{} {}位: <code>{}</code> — {:.1}点</strong></summary>\n",
            medal(i), i + 1, e.candidate_id, e.score
        ));
        body.push_str("<table>\n<tr><th>軸</th><th>スコア</th><th>上限</th><th>達成率</th></tr>\n");
        for (name, get, limit) in AXES {
            let score = get(e);
            let pct = if *limit > 0.0 {
                score / limit * 100.0
            } else {
                0.0
            };
            body.push_str(&format!("<tr><td>{name}</td><td>{score:.1}</td><td>{limit:.0}</td><td>{pct:.0}%</td></tr>\n"));
        }
        body.push_str("</table>\n<ul>\n");
        body.push_str(&format!(
            "<li>コンパイル: {}</li>\n",
            stage_cell(&e.compile)
        ));
        body.push_str(&format!("<li>テスト: {}</li>\n", stage_cell(&e.test)));
        body.push_str(&format!(
            "<li>Clippy: {}</li>\n",
            lint_cell(&e.lint, e.lint_warnings)
        ));
        body.push_str(&format!(
            "<li>PropTest: {}</li>\n",
            stage_cell(&e.prop_test)
        ));
        if !matches!(e.wasm, StageOutcome::Skipped) {
            body.push_str(&format!("<li>Wasm: {}</li>\n", stage_cell(&e.wasm)));
        }
        body.push_str("</ul>\n</details>\n");
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="ja">
<head>
<meta charset="UTF-8">
<title>SafeCode Arena レポート</title>
<style>
  body{{font-family:-apple-system,sans-serif;max-width:1024px;margin:2rem auto;padding:0 1rem;color:#222}}
  h1{{border-bottom:2px solid #1976d2;padding-bottom:.4rem}}
  h2{{border-left:4px solid #1976d2;padding-left:.6rem;margin-top:2rem}}
  table{{border-collapse:collapse;width:100%;margin-bottom:1rem}}
  th,td{{border:1px solid #ddd;padding:6px 10px;text-align:left;font-size:.9rem}}
  th{{background:#f0f4ff;font-weight:600}}
  tr.best{{background:#f0fff4}}
  .passed{{color:#2e7d32}}.failed{{color:#c62828}}.timeout{{color:#e65100}}.skipped{{color:#888}}
  .bar-row{{display:flex;align-items:center;gap:.8rem;margin:.4rem 0}}
  .bar-label{{width:180px;overflow:hidden;text-overflow:ellipsis}}
  .bar-wrap{{flex:1;background:#eee;border-radius:4px;height:18px}}
  .bar{{height:18px;border-radius:4px}}
  .bar-score{{width:50px;text-align:right;font-weight:600}}
  details{{border:1px solid #ddd;border-radius:6px;padding:.6rem 1rem;margin:.6rem 0}}
  details summary{{cursor:pointer;font-size:1rem}}
  code{{background:#f5f5f5;padding:2px 4px;border-radius:3px;font-size:.9em}}
  .meta{{color:#666;font-size:.85rem;margin-bottom:1.5rem}}
</style>
</head>
<body>
<h1>SafeCode Arena 評価レポート</h1>
<p class="meta">生成日時: {generated_at} | 候補数: {}</p>
{body}
</body>
</html>"#,
        evals.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AxisScores;

    fn eval(id: &str, score: f64) -> Evaluation {
        Evaluation {
            candidate_id: id.to_string(),
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
            audit_findings: 0,
            bench_ns: None,
            reasoning: StageOutcome::Skipped,
            reasoning_score: 0.0,
            reasoning_comment: None,
            axes: AxisScores::default(),
            score,
        }
    }

    const GREEN: &str = "#4caf50";
    const ORANGE: &str = "#ff9800";
    const RED: &str = "#f44336";

    /// スコア `score` のバーの `style` 属性断片を返す。
    fn bar_style(width: &str, color: &str) -> String {
        format!("width:{width}%;background:{color}")
    }

    #[test]
    fn bar_color_boundary_70_is_green() {
        let html = render_html(&[eval("a", 70.0)]);
        assert!(html.contains(&bar_style("70.0", GREEN)));
    }

    #[test]
    fn bar_color_just_below_70_is_orange() {
        let html = render_html(&[eval("a", 69.9)]);
        assert!(html.contains(&bar_style("69.9", ORANGE)));
    }

    #[test]
    fn bar_color_boundary_40_is_orange() {
        let html = render_html(&[eval("a", 40.0)]);
        assert!(html.contains(&bar_style("40.0", ORANGE)));
    }

    #[test]
    fn bar_color_just_below_40_is_red() {
        let html = render_html(&[eval("a", 39.9)]);
        assert!(html.contains(&bar_style("39.9", RED)));
    }

    #[test]
    fn bar_color_zero_is_red() {
        let html = render_html(&[eval("a", 0.0)]);
        assert!(html.contains(&bar_style("0.0", RED)));
    }

    #[test]
    fn bar_width_is_clamped_to_100_but_score_text_is_not() {
        let html = render_html(&[eval("a", 150.0)]);
        assert!(html.contains(&bar_style("100.0", GREEN)));
        assert!(!html.contains("width:150.0%"));
        // 数値表示側はクランプされない。
        assert!(html.contains("<span class=\"bar-score\">150.0</span>"));
    }

    #[test]
    fn only_first_row_has_best_class() {
        let html = render_html(&[eval("a", 90.0), eval("b", 80.0), eval("c", 10.0)]);
        // CSS 内の `tr.best` と区別するため、属性付きの行開始タグで数える。
        assert_eq!(html.matches("<tr class=\"best\">").count(), 1);
        let best_pos = html.find("<tr class=\"best\">").unwrap();
        let a_pos = html.find("<code>a</code>").unwrap();
        let b_pos = html.find("<code>b</code>").unwrap();
        assert!(best_pos < a_pos && a_pos < b_pos);
        // best 行は先頭候補の行であること。
        let row_end = html[best_pos..].find("</tr>").unwrap();
        assert!(html[best_pos..best_pos + row_end].contains("<code>a</code>"));
    }

    #[test]
    fn wasm_line_hidden_when_skipped() {
        let html = render_html(&[eval("a", 50.0)]);
        assert!(!html.contains("<li>Wasm:"));
    }

    #[test]
    fn wasm_line_shown_when_not_skipped() {
        let mut e = eval("a", 50.0);
        e.wasm = StageOutcome::Passed { duration_ms: 5 };
        let html = render_html(&[e]);
        assert!(html.contains("<li>Wasm:"));
    }

    #[test]
    fn empty_evals_renders_without_panic() {
        let html = render_html(&[]);
        assert!(html.contains("候補数: 0"));
        assert!(html.contains("<h2>比較サマリー</h2>"));
        assert!(html.contains("</html>"));
        assert!(!html.contains("<tr class=\"best\">"));
        assert!(!html.contains("<details"));
    }

    #[test]
    fn candidate_count_matches_input() {
        let html = render_html(&[eval("a", 1.0), eval("b", 2.0)]);
        assert!(html.contains("候補数: 2"));
    }

    /// 現状の挙動の固定（特性テスト）であり、仕様ではない。
    /// candidate_id は HTML エスケープされずそのまま埋め込まれる。
    /// エスケープを導入する際は、このテストを意図的に更新すること。
    #[test]
    fn characterization_candidate_id_is_not_html_escaped_current_behavior() {
        let html = render_html(&[eval("<b>&x", 50.0)]);
        assert!(html.contains("<code><b>&x</code>"));
        assert!(!html.contains("&lt;b&gt;"));
        assert!(!html.contains("&amp;x"));
    }

    /// 現状の挙動の固定（特性テスト）であり、仕様ではない。
    /// サマリー表のヘッダに resource 列は無い（Markdown 版とは列構成が異なる）。
    /// 詳細表には resource_usage 行がある。
    #[test]
    fn characterization_summary_table_has_no_resource_column_current_behavior() {
        let html = render_html(&[eval("a", 50.0)]);
        assert!(html.contains("<th>保守</th><th>コンパイル</th>"));
        assert!(!html.contains("<th>資源"));
        assert!(html.contains("<td>resource_usage</td>"));
    }

    // ── Markdown render() ────────────────────────────────────────────────────

    fn passed(ms: u64) -> StageOutcome {
        StageOutcome::Passed { duration_ms: ms }
    }

    fn failed(detail: &str) -> StageOutcome {
        StageOutcome::Failed {
            detail: detail.to_string(),
        }
    }

    /// 指定プレフィックスで始まる最初の行を返す。
    fn line_starting<'a>(md: &'a str, prefix: &str) -> &'a str {
        md.lines()
            .find(|l| l.starts_with(prefix))
            .unwrap_or_else(|| panic!("line starting with {prefix:?} not found in:\n{md}"))
    }

    fn pipes(line: &str) -> usize {
        line.matches('|').count()
    }

    fn with_mutation(mut e: Evaluation, caught: usize, total: usize) -> Evaluation {
        e.mutation = passed(1);
        e.mutation_caught = caught;
        e.mutation_total = total;
        e
    }

    // (1) Mutation 列

    #[test]
    fn md_summary_without_mutation_has_no_mutation_column() {
        let md = render(&[eval("a", 50.0)]);
        assert!(!md.contains("Mutation"));
        let header = line_starting(&md, "| 順位");
        assert!(header.ends_with("Wasm |"));
        // 13 列 -> パイプ 14 本
        assert_eq!(pipes(header), 14);
    }

    #[test]
    fn md_summary_with_mutation_adds_column_to_header_separator_and_rows() {
        let md = render(&[with_mutation(eval("a", 50.0), 3, 4), eval("b", 40.0)]);
        let header = line_starting(&md, "| 順位");
        let sep = line_starting(&md, "| ----");
        let row_a = line_starting(&md, "| 1 ");
        let row_b = line_starting(&md, "| 2 ");
        assert!(header.ends_with("Mutation |"));
        assert_eq!(pipes(header), 15);
        assert_eq!(pipes(sep), 15);
        assert_eq!(pipes(row_a), 15);
        assert_eq!(pipes(row_b), 15);
    }

    #[test]
    fn md_summary_mutation_column_shown_if_any_candidate_has_mutants() {
        // 先頭は total==0 でも、2 件目が total>0 なら列が付き、先頭行は「—」になる。
        let md = render(&[eval("a", 50.0), with_mutation(eval("b", 40.0), 1, 2)]);
        assert!(line_starting(&md, "| 1 ").ends_with("| — |"));
        assert!(line_starting(&md, "| 2 ").ends_with("| 1/2 (50%) |"));
    }

    #[test]
    fn md_summary_row_shows_candidate_id_and_one_decimal_scores() {
        let mut e = eval("cand-x", 87.65);
        e.axes.correctness = 42.0;
        let md = render(&[e]);
        let row = line_starting(&md, "| 1 ");
        assert!(row.contains("| cand-x | 87.7 | 42.0 |"), "{row}");
    }

    // (2) mutation_cell

    #[test]
    fn mutation_cell_skipped_is_dash_regardless_of_counts() {
        assert_eq!(mutation_cell(3, 4, &StageOutcome::Skipped), "—");
    }

    #[test]
    fn mutation_cell_failed_and_timed_out_are_failed() {
        assert_eq!(mutation_cell(0, 0, &failed("x")), "❌ failed");
        assert_eq!(
            mutation_cell(1, 2, &StageOutcome::TimedOut { limit_ms: 10 }),
            "❌ failed"
        );
    }

    #[test]
    fn mutation_cell_passed_with_zero_total_is_zero_mutants() {
        assert_eq!(mutation_cell(0, 0, &passed(1)), "✅ 0 mutants");
    }

    #[test]
    fn mutation_cell_passed_shows_caught_total_and_percentage() {
        assert_eq!(mutation_cell(3, 4, &passed(1)), "3/4 (75%)");
        assert_eq!(mutation_cell(0, 5, &passed(1)), "0/5 (0%)");
        assert_eq!(mutation_cell(5, 5, &passed(1)), "5/5 (100%)");
    }

    #[test]
    fn mutation_cell_percentage_is_rounded_to_integer() {
        assert_eq!(mutation_cell(1, 3, &passed(1)), "1/3 (33%)");
        assert_eq!(mutation_cell(2, 3, &passed(1)), "2/3 (67%)");
    }

    // (3) evals.len() > 1 の分岐

    #[test]
    fn md_single_candidate_has_no_details_or_comparison() {
        let md = render(&[eval("a", 50.0)]);
        assert!(md.contains("## 比較サマリー"));
        assert!(!md.contains("## 候補詳細"));
        assert!(!md.contains("## 候補間比較"));
    }

    #[test]
    fn md_two_candidates_have_details_and_comparison() {
        let md = render(&[eval("a", 50.0), eval("b", 40.0)]);
        assert!(md.contains("## 候補詳細"));
        assert!(md.contains("## 候補間比較"));
        assert!(md.contains("### 🥇 1位: `a` — 50.0 点"));
        assert!(md.contains("### 🥈 2位: `b` — 40.0 点"));
    }

    // (4) Failed detail の切り詰め

    #[test]
    fn md_failed_detail_is_truncated_to_first_8_lines() {
        let detail = (1..=10)
            .map(|i| format!("err-line-{i};"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut a = eval("a", 50.0);
        a.compile = failed(&detail);
        let md = render(&[a, eval("b", 40.0)]);
        assert!(md.contains("**失敗詳細**"));
        assert!(md.contains("<summary>コンパイル エラー</summary>"));
        assert!(md.contains("err-line-8;"));
        assert!(!md.contains("err-line-9;"));
        assert!(!md.contains("err-line-10;"));
    }

    #[test]
    fn md_failed_detail_of_exactly_8_lines_is_kept_whole() {
        let detail = (1..=8)
            .map(|i| format!("L{i};"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut a = eval("a", 50.0);
        a.test = failed(&detail);
        let md = render(&[a, eval("b", 40.0)]);
        assert!(md.contains("L1;\nL2;\nL3;\nL4;\nL5;\nL6;\nL7;\nL8;\n```"));
    }

    #[test]
    fn md_failed_detail_with_backtick_fence_cannot_close_outer_fence() {
        let mut a = eval("a", 50.0);
        a.compile = failed("error: x\n```\n# injected heading\n````\n[evil](http://e)");
        let md = render(&[a, eval("b", 40.0)]);
        let start = md.find("<summary>コンパイル エラー</summary>").unwrap();
        let body = &md[start..md[start..].find("</details>").map(|i| i + start).unwrap()];
        let lines: Vec<&str> = body.lines().collect();
        let open = lines.iter().position(|l| l.starts_with("```")).unwrap();
        let fence = lines[open];
        assert!(fence.len() >= 5, "fence too short: {fence}");
        // 外側フェンスと同一の行は開始と終了の 2 つだけ
        assert_eq!(lines.iter().filter(|l| **l == fence).count(), 2);
        let close = lines.iter().rposition(|l| *l == fence).unwrap();
        assert!(lines[open + 1..close].contains(&"# injected heading"));
        assert!(lines[close + 1..].iter().all(|l| l.trim().is_empty()));
    }

    #[test]
    fn fence_for_is_longer_than_longest_backtick_run() {
        assert_eq!(fence_for("no ticks"), "```");
        assert_eq!(fence_for("a ``` b"), "````");
        assert_eq!(fence_for("`` ````` `"), "``````");
    }

    #[test]
    fn md_failure_section_header_appears_once_for_multiple_failures() {
        let mut a = eval("a", 50.0);
        a.compile = failed("c");
        a.test = failed("t");
        let md = render(&[a, eval("b", 40.0)]);
        assert_eq!(md.matches("**失敗詳細**").count(), 1);
        assert_eq!(md.matches("<details><summary>").count(), 2);
    }

    #[test]
    fn md_no_failure_section_when_nothing_failed() {
        let md = render(&[eval("a", 50.0), eval("b", 40.0)]);
        assert!(!md.contains("**失敗詳細**"));
    }

    // (5) コンパイル時間の強調

    #[test]
    fn md_compile_time_only_fastest_is_highlighted() {
        let mut a = eval("a", 50.0);
        let mut b = eval("b", 40.0);
        a.compile = passed(300);
        b.compile = passed(100);
        let md = render(&[a, b]);
        assert_eq!(
            line_starting(&md, "| コンパイル時間"),
            "| コンパイル時間 | 300ms | **100ms** ⚡ |"
        );
    }

    #[test]
    fn md_compile_time_single_passed_is_not_highlighted() {
        let mut a = eval("a", 50.0);
        a.compile = passed(300);
        let mut b = eval("b", 40.0);
        b.compile = failed("x");
        let md = render(&[a, b]);
        assert_eq!(
            line_starting(&md, "| コンパイル時間"),
            "| コンパイル時間 | 300ms | — |"
        );
    }

    /// 現状の挙動の固定（特性テスト）。同タイムは両方強調される。
    #[test]
    fn characterization_md_compile_time_tie_highlights_all_tied_current_behavior() {
        let mut a = eval("a", 50.0);
        let mut b = eval("b", 40.0);
        a.compile = passed(100);
        b.compile = passed(100);
        let md = render(&[a, b]);
        assert_eq!(
            line_starting(&md, "| コンパイル時間"),
            "| コンパイル時間 | **100ms** ⚡ | **100ms** ⚡ |"
        );
    }

    #[test]
    fn md_test_time_fastest_is_highlighted() {
        let mut a = eval("a", 50.0);
        let mut b = eval("b", 40.0);
        a.test = passed(20);
        b.test = passed(90);
        let md = render(&[a, b]);
        assert_eq!(
            line_starting(&md, "| テスト時間"),
            "| テスト時間 | **20ms** ⚡ | 90ms |"
        );
    }

    // (6) Clippy 警告

    #[test]
    fn md_clippy_all_equal_warnings_not_highlighted() {
        let mut a = eval("a", 50.0);
        let mut b = eval("b", 40.0);
        a.lint_warnings = 2;
        b.lint_warnings = 2;
        let md = render(&[a, b]);
        assert_eq!(
            line_starting(&md, "| Clippy警告"),
            "| Clippy警告 | 2件 | 2件 |"
        );
    }

    #[test]
    fn md_clippy_fewest_warnings_highlighted_when_different() {
        let mut a = eval("a", 50.0);
        let mut b = eval("b", 40.0);
        a.lint_warnings = 3;
        b.lint_warnings = 1;
        let md = render(&[a, b]);
        assert_eq!(
            line_starting(&md, "| Clippy警告"),
            "| Clippy警告 | 3件 | **1件** ✅ |"
        );
    }

    // (7) 軸スコア・総合スコア

    #[test]
    fn md_axis_max_is_bold_when_scores_differ() {
        let mut a = eval("a", 50.0);
        let mut b = eval("b", 40.0);
        a.axes.correctness = 10.0;
        b.axes.correctness = 30.0;
        let md = render(&[a, b]);
        assert_eq!(
            line_starting(&md, "| 正誤スコア"),
            "| 正誤スコア | 10.0 | **30.0** |"
        );
    }

    #[test]
    fn md_axis_all_equal_is_not_bold() {
        // AxisScores::default() は全軸 0 -> 全員同点で強調なし。
        let md = render(&[eval("a", 50.0), eval("b", 40.0)]);
        assert_eq!(
            line_starting(&md, "| 安全スコア"),
            "| 安全スコア | 0.0 | 0.0 |"
        );
    }

    #[test]
    fn md_axis_tied_max_with_lower_third_highlights_both_tied() {
        let mut a = eval("a", 50.0);
        let mut b = eval("b", 40.0);
        let c = eval("c", 30.0);
        a.axes.security = 20.0;
        b.axes.security = 20.0;
        let md = render(&[a, b, c]);
        assert_eq!(
            line_starting(&md, "| 安全スコア"),
            "| 安全スコア | **20.0** | **20.0** | 0.0 |"
        );
    }

    #[test]
    fn md_axis_rows_cover_four_axes_without_resource() {
        let md = render(&[eval("a", 50.0), eval("b", 40.0)]);
        for label in [
            "| 正誤スコア",
            "| 安全スコア",
            "| 性能スコア",
            "| 保守スコア",
        ] {
            line_starting(&md, label);
        }
        assert!(!md.contains("| 資源スコア"));
    }

    #[test]
    fn md_total_score_max_gets_trophy() {
        let md = render(&[eval("a", 80.0), eval("b", 40.0)]);
        assert_eq!(
            line_starting(&md, "| **総合スコア**"),
            "| **総合スコア** | **80.0** 🏆 | 40.0 |"
        );
    }

    #[test]
    fn md_total_score_tie_gives_multiple_trophies() {
        let md = render(&[eval("a", 80.0), eval("b", 80.0), eval("c", 10.0)]);
        let line = line_starting(&md, "| **総合スコア**");
        assert_eq!(
            line,
            "| **総合スコア** | **80.0** 🏆 | **80.0** 🏆 | 10.0 |"
        );
    }

    /// 現状の挙動の固定（特性テスト）。軸スコアと違い、総合スコアは
    /// 全員同点でも 🏆 が全員に付く（「差があるときのみ」の条件が無い）。
    #[test]
    fn characterization_md_total_score_all_equal_all_get_trophy_current_behavior() {
        let md = render(&[eval("a", 50.0), eval("b", 50.0)]);
        assert_eq!(
            line_starting(&md, "| **総合スコア**"),
            "| **総合スコア** | **50.0** 🏆 | **50.0** 🏆 |"
        );
    }

    #[test]
    fn md_comparison_header_and_separator_match_candidate_count() {
        let md = render(&[eval("a", 3.0), eval("b", 2.0), eval("c", 1.0)]);
        assert_eq!(line_starting(&md, "| 観点"), "| 観点 | a | b | c |");
        let sep = md
            .lines()
            .find(|l| *l == "| ---- | ---- | ---- | ---- |")
            .is_some();
        assert!(sep);
    }

    // (8) 空 / 採用候補

    #[test]
    fn md_empty_evals_does_not_panic_and_has_no_best_line() {
        let md = render(&[]);
        assert!(md.contains("## 比較サマリー"));
        assert!(!md.contains("採用候補"));
        assert!(!md.contains("## 候補詳細"));
        assert!(!md.contains("## 候補間比較"));
    }

    #[test]
    fn md_best_candidate_line_uses_first_eval() {
        let md = render(&[eval("top", 91.26), eval("second", 10.0)]);
        assert!(md.contains("**採用候補**: `top`（91.3点）"));
    }

    // 詳細セクションの条件付き行

    #[test]
    fn md_details_mutation_and_wasm_lines_hidden_when_skipped() {
        let md = render(&[eval("a", 50.0), eval("b", 40.0)]);
        assert!(!md.contains("- Mutation:"));
        assert!(!md.contains("- Wasm:"));
    }

    #[test]
    fn md_details_show_wasm_with_fuel_and_mutation_when_present() {
        let mut a = with_mutation(eval("a", 50.0), 1, 2);
        a.wasm = passed(5);
        a.wasm_fuel_used = Some(1234);
        let md = render(&[a, eval("b", 40.0)]);
        assert!(md.contains("- Mutation: 1/2 (50%)"));
        assert!(md.contains("- Wasm: ✅ 5ms (fuel使用: 1234)"));
    }

    #[test]
    fn md_details_axis_table_shows_rate_against_rubric_limit() {
        let mut a = eval("a", 50.0);
        a.axes.correctness = 25.0;
        let md = render(&[a, eval("b", 40.0)]);
        assert!(md.contains("| correctness | 25.0 | 50 | 50% |"));
        assert!(md.contains("| resource_usage | 0.0 | 5 | 0% |"));
    }
}
