//! 共通 LLM HTTP クライアント。
//!
//! `llm.rs`（レビュー）と `generator.rs`（候補生成）が共有する
//! Claude / Ollama への HTTP 呼び出し基盤。
//! 環境変数の読み取りと JSON の送受信のみ担当し、プロンプト構築・レスポンス解析は
//! 各モジュールに委ねる。

use std::time::Duration;

/// LLM バックエンドの種別。
/// `SAFECODE_LLM_BACKEND` 環境変数で切り替える（未設定時は `Claude`）。
/// 第三のバックエンド追加時に compiler がマッチ漏れを検出できる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackendKind {
    Ollama,
    Claude,
}

impl BackendKind {
    /// 環境変数からバックエンドを決定する。
    pub(crate) fn from_env() -> Self {
        match std::env::var("SAFECODE_LLM_BACKEND")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "ollama" => BackendKind::Ollama,
            _ => BackendKind::Claude,
        }
    }
}

/// Claude (Anthropic Messages API) にテキストを送信し、応答テキストを返す。
///
/// - `max_tokens`: 応答の最大トークン数。レビューは 256、候補生成は 4096 が目安。
/// - `ANTHROPIC_API_KEY` が未設定の場合 `None` を返す。
/// - HTTP 5xx を受け取った場合、1 秒待機して 1 回リトライする。
#[allow(clippy::result_large_err)]
pub(crate) fn post_claude(prompt: &str, max_tokens: u32, timeout: Duration) -> Option<String> {
    let api_key = std::env::var("ANTHROPIC_API_KEY").ok()?;
    let model = std::env::var("ANTHROPIC_MODEL")
        .unwrap_or_else(|_| "claude-haiku-4-5-20251001".to_string());
    let body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": [{"role": "user", "content": prompt}]
    });
    let send = || {
        ureq::post("https://api.anthropic.com/v1/messages")
            .set("x-api-key", &api_key)
            .set("anthropic-version", "2023-06-01")
            .set("content-type", "application/json")
            .timeout(timeout)
            .send_json(body.clone())
    };
    let resp = retry_once(send)?;
    let json: serde_json::Value = resp.into_json().ok()?;
    json["content"][0]["text"].as_str().map(|s| s.to_string())
}

/// Ollama `/api/generate` に JSON フォーマット指定でテキストを送信する（レビュー用）。
///
/// - HTTP 5xx を受け取った場合、1 秒待機して 1 回リトライする。
pub(crate) fn post_ollama_json(prompt: &str, timeout: Duration) -> Option<String> {
    post_ollama_inner(prompt, true, timeout)
}

/// Ollama `/api/generate` に自由形式テキストとして送信する（候補生成用）。
///
/// - HTTP 5xx を受け取った場合、1 秒待機して 1 回リトライする。
pub(crate) fn post_ollama_text(prompt: &str, timeout: Duration) -> Option<String> {
    post_ollama_inner(prompt, false, timeout)
}

#[allow(clippy::result_large_err)]
fn post_ollama_inner(prompt: &str, format_json: bool, timeout: Duration) -> Option<String> {
    let host = std::env::var("OLLAMA_HOST")
        .unwrap_or_else(|_| "http://localhost:11434".to_string());
    let host = host.trim_end_matches('/');
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
    let url = format!("{host}/api/generate");
    let send = || {
        ureq::post(&url)
            .set("content-type", "application/json")
            .timeout(timeout)
            .send_json(body.clone())
    };
    let resp = retry_once(send)?;
    let json: serde_json::Value = resp.into_json().ok()?;
    json["response"].as_str().map(|s| s.to_string())
}

/// HTTP 5xx 時に 1 秒待機して 1 回リトライする。
/// 成功または 4xx/ネットワークエラー時はそのまま返す。
/// HTTP 5xx 時に 1 秒待機して 1 回リトライする。
/// 成功または 4xx/ネットワークエラー時はそのまま返す。
// ureq::Error は 272 bytes だが内部ヘルパーのため boxing を避け lint を抑制する。
#[allow(clippy::result_large_err)]
fn retry_once<F>(f: F) -> Option<ureq::Response>
where
    F: Fn() -> Result<ureq::Response, ureq::Error>,
{
    match f() {
        Ok(r) => Some(r),
        Err(ureq::Error::Status(code, _)) if code >= 500 => {
            eprintln!("[llm_client] HTTP {code} — retrying after 1s");
            std::thread::sleep(Duration::from_secs(1));
            match f() {
                Ok(r) => Some(r),
                Err(e) => {
                    eprintln!("[llm_client] retry failed: {e}");
                    None
                }
            }
        }
        Err(e) => {
            eprintln!("[llm_client] request failed: {e}");
            None
        }
    }
}
