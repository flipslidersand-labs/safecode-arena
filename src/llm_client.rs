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

/// Ollama `/api/generate` の URL を組み立てる（host 末尾の `/` は除去）。
fn ollama_generate_url(host: &str) -> String {
    let host = host.trim_end_matches('/');
    format!("{host}/api/generate")
}

/// Ollama `/api/generate` のリクエストボディを組み立てる。
fn ollama_body(model: &str, prompt: &str, format_json: bool) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
    });
    if format_json {
        body["format"] = serde_json::json!("json");
    }
    body
}

#[allow(clippy::result_large_err)]
fn post_ollama_inner(prompt: &str, format_json: bool, timeout: Duration) -> Option<String> {
    let host =
        std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".to_string());
    let model = std::env::var("OLLAMA_MODEL").unwrap_or_else(|_| "qwen2.5-coder:7b".to_string());
    let body = ollama_body(&model, prompt, format_json);
    let url = ollama_generate_url(&host);
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
    retry_once_with_delay(f, Duration::from_secs(1))
}

/// `retry_once` の本体。待機時間を注入可能にしてテストで短縮する。
#[allow(clippy::result_large_err)]
fn retry_once_with_delay<F>(f: F, delay: Duration) -> Option<ureq::Response>
where
    F: Fn() -> Result<ureq::Response, ureq::Error>,
{
    match f() {
        Ok(r) => Some(r),
        Err(ureq::Error::Status(code, _)) if code >= 500 => {
            eprintln!("[llm_client] HTTP {code} — retrying after 1s");
            std::thread::sleep(delay);
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

#[cfg(test)]
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::Mutex;

    /// 環境変数を触るテストの直列化用。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const NO_DELAY: Duration = Duration::from_millis(0);

    fn with_backend_env(value: Option<&str>) -> BackendKind {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("SAFECODE_LLM_BACKEND").ok();
        match value {
            Some(v) => std::env::set_var("SAFECODE_LLM_BACKEND", v),
            None => std::env::remove_var("SAFECODE_LLM_BACKEND"),
        }
        let kind = BackendKind::from_env();
        match prev {
            Some(v) => std::env::set_var("SAFECODE_LLM_BACKEND", v),
            None => std::env::remove_var("SAFECODE_LLM_BACKEND"),
        }
        kind
    }

    fn status(code: u16) -> Result<ureq::Response, ureq::Error> {
        let resp = ureq::Response::new(code, "X", "{}").unwrap();
        if code >= 400 {
            Err(ureq::Error::Status(code, resp))
        } else {
            Ok(resp)
        }
    }

    #[test]
    fn from_env_unset_is_claude() {
        assert_eq!(with_backend_env(None), BackendKind::Claude);
    }

    #[test]
    fn from_env_ollama_case_insensitive() {
        assert_eq!(with_backend_env(Some("ollama")), BackendKind::Ollama);
        assert_eq!(with_backend_env(Some("OLLAMA")), BackendKind::Ollama);
        assert_eq!(with_backend_env(Some("Ollama")), BackendKind::Ollama);
    }

    #[test]
    fn from_env_unknown_or_empty_is_claude() {
        assert_eq!(with_backend_env(Some("gpt")), BackendKind::Claude);
        assert_eq!(with_backend_env(Some("")), BackendKind::Claude);
        assert_eq!(with_backend_env(Some("claude")), BackendKind::Claude);
    }

    #[test]
    fn retry_success_calls_once() {
        let n = Cell::new(0u32);
        let r = retry_once_with_delay(
            || {
                n.set(n.get() + 1);
                status(200)
            },
            NO_DELAY,
        );
        assert!(r.is_some());
        assert_eq!(n.get(), 1);
    }

    #[test]
    fn retry_5xx_then_success_calls_twice() {
        let n = Cell::new(0u32);
        let r = retry_once_with_delay(
            || {
                n.set(n.get() + 1);
                if n.get() == 1 {
                    status(503)
                } else {
                    status(200)
                }
            },
            NO_DELAY,
        );
        assert!(r.is_some());
        assert_eq!(n.get(), 2);
    }

    #[test]
    fn retry_5xx_twice_returns_none_after_two_calls() {
        let n = Cell::new(0u32);
        let r = retry_once_with_delay(
            || {
                n.set(n.get() + 1);
                status(503)
            },
            NO_DELAY,
        );
        assert!(r.is_none());
        assert_eq!(n.get(), 2);
    }

    #[test]
    fn retry_boundary_500_retries_499_does_not() {
        let n = Cell::new(0u32);
        let _ = retry_once_with_delay(
            || {
                n.set(n.get() + 1);
                status(500)
            },
            NO_DELAY,
        );
        assert_eq!(n.get(), 2);

        n.set(0);
        let _ = retry_once_with_delay(
            || {
                n.set(n.get() + 1);
                status(499)
            },
            NO_DELAY,
        );
        assert_eq!(n.get(), 1);
    }

    #[test]
    fn retry_4xx_is_not_retried() {
        for code in [400u16, 401, 404, 429] {
            let n = Cell::new(0u32);
            let r = retry_once_with_delay(
                || {
                    n.set(n.get() + 1);
                    status(code)
                },
                NO_DELAY,
            );
            assert!(r.is_none(), "code {code}");
            assert_eq!(n.get(), 1, "code {code}");
        }
    }

    #[test]
    fn retry_network_error_is_not_retried() {
        let n = Cell::new(0u32);
        let r = retry_once_with_delay(
            || {
                n.set(n.get() + 1);
                Err(ureq::Error::from(std::io::Error::other("boom")))
            },
            NO_DELAY,
        );
        assert!(r.is_none());
        assert_eq!(n.get(), 1);
    }

    #[test]
    fn ollama_url_strips_trailing_slashes() {
        assert_eq!(
            ollama_generate_url("http://h:11434/"),
            "http://h:11434/api/generate"
        );
        assert_eq!(
            ollama_generate_url("http://h:11434"),
            "http://h:11434/api/generate"
        );
        // trim_end_matches は連続する `/` もすべて除去する
        assert_eq!(
            ollama_generate_url("http://h:11434//"),
            "http://h:11434/api/generate"
        );
    }

    #[test]
    fn ollama_body_json_format_sets_format() {
        let b = ollama_body("m", "p", true);
        assert_eq!(b["format"], "json");
        assert_eq!(b["model"], "m");
        assert_eq!(b["prompt"], "p");
        assert_eq!(b["stream"], false);
    }

    #[test]
    fn ollama_body_text_has_no_format() {
        let b = ollama_body("m", "p", false);
        assert!(b.get("format").is_none());
        assert_eq!(b["stream"], false);
    }
}
