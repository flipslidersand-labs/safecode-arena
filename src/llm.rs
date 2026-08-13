//! LLM セマンティックレビュークライアント（phase14）。
//!
//! HTTP 基盤は `llm_client` モジュールを共有。
//! バックエンドは環境変数 `SAFECODE_LLM_BACKEND` で切り替える:
//! - `claude` (既定): Anthropic API。`ANTHROPIC_API_KEY` 必須。
//! - `ollama`: ローカル Ollama。`OLLAMA_HOST` / `OLLAMA_MODEL` 参照。
//!
//! LLM が利用不能 / タイムアウト / JSON パース失敗の場合は `None` を返す（採点に影響しない）。

use crate::llm_client;
use serde::Deserialize;

const PROMPT_TEMPLATE: &str = r#"You are a strict code reviewer evaluating AI-generated code.
Analyze the source code below for correctness, safety, and code quality.

## Source
```
{SOURCE}
```

## Test result
{TEST_RESULT}

Respond with a single JSON object — no markdown fences, no explanation outside the JSON:
{"score": 0.85, "comment": "one or two sentence summary"}

- score: 0.0 (critical flaws) to 1.0 (excellent)
- comment: max 120 characters, English"#;

/// LLM が返す構造化レビュー。
#[derive(Debug, Clone)]
pub struct LlmReview {
    /// 0.0–1.0 のスコア。
    pub score: f64,
    /// 1–2 文のコメント（最大 120 字）。
    pub comment: String,
}

#[derive(Deserialize)]
struct RawReview {
    score: f64,
    comment: String,
}

fn build_prompt(source: &str, test_passed: bool) -> String {
    let test_result = if test_passed { "PASSED" } else { "FAILED" };
    PROMPT_TEMPLATE
        .replace("{SOURCE}", source)
        .replace("{TEST_RESULT}", test_result)
}

fn parse_review(text: &str) -> Option<LlmReview> {
    // モデルが ```json ... ``` で包むことがあるので strip する
    let trimmed = text.trim();
    let json_str = if let Some(inner) = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
    {
        inner.trim_end_matches("```").trim()
    } else {
        trimmed
    };

    let raw: RawReview = serde_json::from_str(json_str).ok()?;
    let score = raw.score.clamp(0.0, 1.0);
    let comment = raw.comment.chars().take(120).collect();
    Some(LlmReview { score, comment })
}

// ── Public API ────────────────────────────────────────────────────────────────

/// ソースコードと test 結果を LLM に送り、レビュースコアを得る。
///
/// LLM が利用不能・タイムアウト・パース失敗のときは `None`（採点影響なし）。
pub fn review(source: &str, test_passed: bool) -> Option<LlmReview> {
    let prompt = build_prompt(source, test_passed);
    let text = match llm_client::backend().as_str() {
        "ollama" => llm_client::post_ollama(&prompt, true, std::time::Duration::from_secs(60))?,
        _ => llm_client::post_claude(&prompt, 256, std::time::Duration::from_secs(30))?,
    };
    parse_review(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_json() {
        let text = r#"{"score": 0.85, "comment": "Looks good"}"#;
        let r = parse_review(text).unwrap();
        assert!((r.score - 0.85).abs() < 1e-9);
        assert_eq!(r.comment, "Looks good");
    }

    #[test]
    fn parse_fenced_json() {
        let text = "```json\n{\"score\": 0.5, \"comment\": \"Some issues\"}\n```";
        let r = parse_review(text).unwrap();
        assert!((r.score - 0.5).abs() < 1e-9);
    }

    #[test]
    fn score_clamped_to_range() {
        let text = r#"{"score": 1.5, "comment": "perfect"}"#;
        let r = parse_review(text).unwrap();
        assert_eq!(r.score, 1.0);
    }

    #[test]
    fn comment_truncated_at_120() {
        let long = "x".repeat(200);
        let text = format!(r#"{{"score": 0.9, "comment": "{long}"}}"#);
        let r = parse_review(&text).unwrap();
        assert_eq!(r.comment.len(), 120);
    }

    #[test]
    fn parse_invalid_returns_none() {
        assert!(parse_review("not json").is_none());
        assert!(parse_review("{}").is_none());
    }

    #[test]
    fn build_prompt_contains_source_and_result() {
        let p = build_prompt("fn add(a: i32, b: i32) -> i32 { a + b }", true);
        assert!(p.contains("fn add"));
        assert!(p.contains("PASSED"));
    }
}
