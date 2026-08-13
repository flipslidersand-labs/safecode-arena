//! spec（自然言語）→ LLM → N 候補生成（phase15）。
//!
//! HTTP 基盤は `llm_client` モジュールを共有。
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
    let raw = match crate::llm_client::backend().as_str() {
        "ollama" => crate::llm_client::post_ollama(&prompt, false, llm_timeout),
        _ => crate::llm_client::post_claude(&prompt, 4096, llm_timeout),
    };
    match raw {
        Some(text) => parse_candidates(&text).into_iter().take(n).collect(),
        None => vec![],
    }
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
