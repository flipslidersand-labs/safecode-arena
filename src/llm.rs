//! LLM セマンティックレビュークライアント（phase14）。
//!
//! バックエンドは環境変数 `SAFECODE_LLM_BACKEND` で切り替える:
//! - `claude` (既定): Anthropic API。`ANTHROPIC_API_KEY` 必須。
//! - `ollama`: ローカル Ollama。`OLLAMA_HOST` (既定 localhost:11434)、
//!             `OLLAMA_MODEL` (既定 qwen2.5-coder:7b)。
//!
//! LLM が利用不能 / タイムアウト / JSON パース失敗の場合は `None` を返す（採点に影響しない）。

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

// ── Claude backend ────────────────────────────────────────────────────────────

fn call_claude(prompt: &str, api_key: &str) -> Option<LlmReview> {
    let model = std::env::var("ANTHROPIC_MODEL")
        .unwrap_or_else(|_| "claude-haiku-4-5-20251001".to_string());
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 256,
        "messages": [{"role": "user", "content": prompt}]
    });
    let resp = ureq::post("https://api.anthropic.com/v1/messages")
        .set("x-api-key", api_key)
        .set("anthropic-version", "2023-06-01")
        .set("content-type", "application/json")
        .timeout(std::time::Duration::from_secs(30))
        .send_json(body)
        .ok()?;
    let json: serde_json::Value = resp.into_json().ok()?;
    let text = json["content"][0]["text"].as_str()?;
    parse_review(text)
}

// ── Ollama backend ────────────────────────────────────────────────────────────

fn call_ollama(prompt: &str) -> Option<LlmReview> {
    let host = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "localhost:11434".to_string());
    let model =
        std::env::var("OLLAMA_MODEL").unwrap_or_else(|_| "qwen2.5-coder:7b".to_string());
    let url = format!("http://{host}/api/generate");
    let body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "format": "json"
    });
    let resp = ureq::post(&url)
        .timeout(std::time::Duration::from_secs(60))
        .send_json(body)
        .ok()?;
    let json: serde_json::Value = resp.into_json().ok()?;
    let text = json["response"].as_str()?;
    parse_review(text)
}

// ── Public API ────────────────────────────────────────────────────────────────

/// ソースコードと test 結果を LLM に送り、レビュースコアを得る。
///
/// LLM が利用不能・タイムアウト・パース失敗のときは `None`（採点影響なし）。
pub fn review(source: &str, test_passed: bool) -> Option<LlmReview> {
    let prompt = build_prompt(source, test_passed);
    let backend = std::env::var("SAFECODE_LLM_BACKEND")
        .unwrap_or_else(|_| "claude".to_string())
        .to_lowercase();
    match backend.as_str() {
        "ollama" => call_ollama(&prompt),
        _ => {
            let key = std::env::var("ANTHROPIC_API_KEY").ok()?;
            call_claude(&prompt, &key)
        }
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
