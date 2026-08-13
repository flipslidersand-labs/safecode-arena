//! spec（自然言語）→ LLM → N 候補生成（phase15）。
//!
//! `llm.rs` の HTTP クライアント基盤を再利用して Rust コード候補を生成する。
//! LLM 呼び出し失敗・パース失敗は空 Vec を返す（呼び出し元でエラー扱い）。

const GENERATE_PROMPT: &str = r#"You are an expert Rust programmer.
Generate exactly {N} distinct Rust implementations for the following specification.

## Specification
{SPEC}

## Requirements
- Each implementation must be valid, compilable Rust code
- Each must be a complete `lib.rs` (no main function)
- Implementations must differ in approach (e.g., iterative vs recursive, different algorithms)
- Include doc comments on public functions

Output format — repeat exactly {N} times, separated by `---CANDIDATE---`:

---CANDIDATE---
```rust
// implementation 1
pub fn ...
```
---CANDIDATE---
```rust
// implementation 2
pub fn ...
```

Output ONLY the candidates in this format. No other text."#;

fn build_generate_prompt(spec: &str, n: usize) -> String {
    GENERATE_PROMPT
        .replace("{N}", &n.to_string())
        .replace("{SPEC}", spec)
}

/// LLM の出力から Rust コードブロックを抽出する。
///
/// `---CANDIDATE---` セパレータで分割し、各セクションから ` ```rust ... ``` ` を取り出す。
/// コードブロックが見つからないセクションはスキップする。
pub fn parse_candidates(output: &str) -> Vec<String> {
    output
        .split("---CANDIDATE---")
        .filter_map(|section| {
            let s = section.trim();
            // ```rust ... ``` ブロックを探す
            let start = s.find("```rust").map(|i| i + 7)?;
            let rest = &s[start..];
            let end = rest.find("```")?;
            let code = rest[..end].trim().to_string();
            if code.is_empty() {
                None
            } else {
                Some(code)
            }
        })
        .collect()
}

/// spec テキストから N 候補の Rust ソースを生成する。
///
/// `llm_timeout`: LLM 呼び出し 1 回あたりのタイムアウト。
/// 生成に失敗した場合や候補数が足りない場合でも取得できた分を返す。
pub fn generate(spec: &str, n: usize, llm_timeout: std::time::Duration) -> Vec<String> {
    let prompt = build_generate_prompt(spec, n);
    let raw = call_llm(&prompt, llm_timeout);
    match raw {
        Some(text) => {
            let candidates = parse_candidates(&text);
            candidates.into_iter().take(n).collect()
        }
        None => vec![],
    }
}

/// LLM バックエンドを選択してテキスト生成する（review と同じ環境変数）。
fn call_llm(prompt: &str, timeout: std::time::Duration) -> Option<String> {
    let backend = std::env::var("SAFECODE_LLM_BACKEND").unwrap_or_else(|_| "claude".into());
    match backend.as_str() {
        "ollama" => call_ollama_generate(prompt, timeout),
        _ => call_claude_generate(prompt, timeout),
    }
}

fn call_claude_generate(prompt: &str, timeout: std::time::Duration) -> Option<String> {
    let api_key = std::env::var("ANTHROPIC_API_KEY").ok()?;
    let model =
        std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-haiku-4-5-20251001".into());

    let body = serde_json::json!({
        "model": model,
        "max_tokens": 4096,
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

fn call_ollama_generate(prompt: &str, timeout: std::time::Duration) -> Option<String> {
    let host = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".into());
    let model = std::env::var("OLLAMA_MODEL").unwrap_or_else(|_| "qwen2.5-coder:7b".into());

    let body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "stream": false
    });

    let resp = ureq::post(&format!("{host}/api/generate"))
        .set("content-type", "application/json")
        .timeout(timeout)
        .send_json(body)
        .ok()?;

    let json: serde_json::Value = resp.into_json().ok()?;
    json["response"].as_str().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_candidates_extracts_rust_blocks() {
        let output = r#"
---CANDIDATE---
```rust
pub fn add(a: i32, b: i32) -> i32 { a + b }
```
---CANDIDATE---
```rust
pub fn add(a: i32, b: i32) -> i32 { b + a }
```
"#;
        let candidates = parse_candidates(output);
        assert_eq!(candidates.len(), 2);
        assert!(candidates[0].contains("pub fn add"));
        assert!(candidates[1].contains("pub fn add"));
    }

    #[test]
    fn parse_candidates_skips_empty_sections() {
        let output = "---CANDIDATE---\n\n---CANDIDATE---\n```rust\npub fn f() {}\n```\n";
        let candidates = parse_candidates(output);
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn parse_candidates_returns_empty_on_no_fences() {
        assert!(parse_candidates("no code here").is_empty());
        assert!(parse_candidates("").is_empty());
    }

    #[test]
    fn parse_candidates_trims_whitespace() {
        let output = "---CANDIDATE---\n```rust\n\n  pub fn f() {}\n\n```\n";
        let candidates = parse_candidates(output);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0], "pub fn f() {}");
    }
}
