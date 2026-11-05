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
    // 信頼できない source を最後に埋め込み、その中の {TEST_RESULT} が再置換されないようにする
    PROMPT_TEMPLATE
        .replace("{TEST_RESULT}", test_result)
        .replace("{SOURCE}", source)
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
    let text = match llm_client::BackendKind::from_env() {
        llm_client::BackendKind::Ollama => {
            llm_client::post_ollama_json(&prompt, std::time::Duration::from_secs(60))?
        }
        llm_client::BackendKind::Claude => {
            llm_client::post_claude(&prompt, 256, std::time::Duration::from_secs(30))?
        }
    };
    parse_review(&text)
}

#[cfg(test)]
mod build_prompt_tests {
    use super::*;

    #[test]
    fn source_containing_test_result_placeholder_is_kept_verbatim() {
        let source = "let s = \"{TEST_RESULT}\";";
        let p = build_prompt(source, true);
        assert!(p.contains(source), "source was rewritten: {p}");
        // テンプレート側の {TEST_RESULT} だけが置換される
        assert_eq!(p.matches("PASSED").count(), 1);
    }
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
        assert_eq!(r.comment.chars().count(), 120);
    }

    #[test]
    fn score_negative_clamped_to_zero() {
        let r = parse_review(r#"{"score": -0.5, "comment": "bad"}"#).unwrap();
        assert_eq!(r.score, 0.0);
    }

    #[test]
    fn parse_bare_fence_without_json_tag() {
        let text = "```\n{\"score\": 0.7, \"comment\": \"ok\"}\n```";
        let r = parse_review(text).unwrap();
        assert!((r.score - 0.7).abs() < 1e-9);
        assert_eq!(r.comment, "ok");
    }

    #[test]
    fn parse_with_preamble_returns_none() {
        // 現状の挙動を固定: 前置説明文があると strip されずパース失敗 -> None
        let text = "Here is the review:\n```json\n{\"score\": 0.5, \"comment\": \"x\"}\n```";
        assert!(parse_review(text).is_none());
    }

    #[test]
    fn comment_truncated_by_chars_not_bytes_for_japanese() {
        let long = "あ".repeat(200);
        let text = format!(r#"{{"score": 0.9, "comment": "{long}"}}"#);
        let r = parse_review(&text).unwrap();
        assert_eq!(r.comment.chars().count(), 120);
        assert_eq!(r.comment.len(), 360);
    }

    #[test]
    fn score_wrong_type_or_missing_returns_none() {
        assert!(parse_review(r#"{"score": "0.5", "comment": "x"}"#).is_none());
        assert!(parse_review(r#"{"comment": "x"}"#).is_none());
        assert!(parse_review(r#"{"score": null, "comment": "x"}"#).is_none());
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

    #[test]
    fn build_prompt_marks_failed_when_test_failed() {
        let p = build_prompt("fn f() {}", false);
        assert!(p.contains("FAILED"));
        assert!(!p.contains("PASSED"));
    }
}
