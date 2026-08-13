//! 共通 LLM HTTP クライアント。
//!
//! `llm.rs`（レビュー）と `generator.rs`（候補生成）が共有する
//! Claude / Ollama への HTTP 呼び出し基盤。
//! 環境変数の読み取りと JSON の送受信のみ担当し、プロンプト構築・レスポンス解析は
//! 各モジュールに委ねる。

use std::time::Duration;

/// `SAFECODE_LLM_BACKEND` 環境変数が示すバックエンド名（小文字）を返す。
/// 未設定の場合は `"claude"` を返す。
pub fn backend() -> String {
    std::env::var("SAFECODE_LLM_BACKEND")
        .unwrap_or_else(|_| "claude".to_string())
        .to_lowercase()
}

/// Claude (Anthropic Messages API) にテキストを送信し、応答テキストを返す。
///
/// - `max_tokens`: 応答の最大トークン数。レビューは 256、候補生成は 4096 が目安。
/// - `ANTHROPIC_API_KEY` が未設定の場合 `None` を返す。
pub fn post_claude(prompt: &str, max_tokens: u32, timeout: Duration) -> Option<String> {
    let api_key = std::env::var("ANTHROPIC_API_KEY").ok()?;
    let model = std::env::var("ANTHROPIC_MODEL")
        .unwrap_or_else(|_| "claude-haiku-4-5-20251001".to_string());
    let body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": [{"role": "user", "content": prompt}]
    });
    let resp = ureq::post("https://api.anthropic.com/v1/messages")
        .set("x-api-key", &api_key)
        .set("anthropic-version", "2023-06-01")
        .set("content-type", "application/json")
        .timeout(timeout)
        .send_json(body)
        .ok()?;
    let json: serde_json::Value = resp.into_json().ok()?;
    json["content"][0]["text"].as_str().map(|s| s.to_string())
}

/// Ollama `/api/generate` にテキストを送信し、応答テキストを返す。
///
/// - `format_json`: `true` のとき `"format": "json"` を付与する（レビュー用）。
///   候補生成では自然言語レスポンスが必要なため `false` にする。
pub fn post_ollama(prompt: &str, format_json: bool, timeout: Duration) -> Option<String> {
    let host = std::env::var("OLLAMA_HOST")
        .unwrap_or_else(|_| "http://localhost:11434".to_string());
    let model = std::env::var("OLLAMA_MODEL")
        .unwrap_or_else(|_| "qwen2.5-coder:7b".to_string());
    let mut body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
    });
    if format_json {
        body["format"] = serde_json::json!("json");
    }
    let resp = ureq::post(&format!("{host}/api/generate"))
        .set("content-type", "application/json")
        .timeout(timeout)
        .send_json(body)
        .ok()?;
    let json: serde_json::Value = resp.into_json().ok()?;
    json["response"].as_str().map(|s| s.to_string())
}
