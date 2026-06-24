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

fn clippy_cell(o: &StageOutcome, warnings: usize) -> String {
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

fn render_summary_table(out: &mut String, evals: &[Evaluation]) {
    out.push_str("## 比較サマリー\n\n");
    out.push_str(
        "| 順位 | 候補 | 合計 | 正誤 | 安全 | 性能 | 保守 | 資源 | コンパイル | テスト | Clippy | PropTest | Wasm |\n",
    );
    out.push_str(
        "| ---- | ---- | ---- | ---- | ---- | ---- | ---- | ---- | ---------- | ------ | ------ | -------- | ---- |\n",
    );
    for (i, e) in evals.iter().enumerate() {
        out.push_str(&format!(
            "| {} {} | {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {} | {} | {} | {} | {} |\n",
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
            clippy_cell(&e.clippy, e.clippy_warnings),
            stage_cell(&e.prop_test),
            wasm_cell(&e.wasm, e.wasm_fuel_used),
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
            clippy_cell(&e.clippy, e.clippy_warnings)
        ));
        out.push_str(&format!("- PropTest: {}\n", stage_cell(&e.prop_test)));
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
            ("Clippy", &e.clippy),
            ("PropTest", &e.prop_test),
        ] {
            if let StageOutcome::Failed { detail } = stage {
                if !has_failures {
                    out.push_str("\n**失敗詳細**\n\n");
                    has_failures = true;
                }
                let snippet = detail.lines().take(8).collect::<Vec<_>>().join("\n");
                out.push_str(&format!(
                    "<details><summary>{label} エラー</summary>\n\n```\n{snippet}\n```\n\n</details>\n\n"
                ));
            }
        }
        out.push('\n');
    }
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
    let warns: Vec<usize> = evals.iter().map(|e| e.clippy_warnings).collect();
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
            clippy_cell(&e.clippy, e.clippy_warnings),
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
            clippy_cell(&e.clippy, e.clippy_warnings)
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
