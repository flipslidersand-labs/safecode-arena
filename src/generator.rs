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

/// `--candidates` の上限。`max_tokens` の上限（8192）に収まる範囲。
pub const MAX_CANDIDATES: usize = 8;
const TOKENS_PER_CANDIDATE: u32 = 1024;
const MIN_MAX_TOKENS: u32 = 2048;
const MAX_MAX_TOKENS: u32 = 8192;

/// 候補数 `n` に応じた Claude の `max_tokens`（1024 * n を 2048..=8192 に丸める）。
pub fn max_tokens_for(n: usize) -> u32 {
    let n = u32::try_from(n).unwrap_or(u32::MAX);
    n.saturating_mul(TOKENS_PER_CANDIDATE)
        .clamp(MIN_MAX_TOKENS, MAX_MAX_TOKENS)
}

/// clap 用: `--candidates` を 1..=MAX_CANDIDATES に検証する（LLM 呼び出し前に弾く）。
pub fn parse_candidate_count(s: &str) -> Result<usize, String> {
    let n: usize = s
        .parse()
        .map_err(|_| format!("`{s}` は整数ではありません"))?;
    if (1..=MAX_CANDIDATES).contains(&n) {
        Ok(n)
    } else {
        Err(format!(
            "候補数は 1〜{MAX_CANDIDATES} で指定してください（指定値: {n}）"
        ))
    }
}

/// 要求数より取得数が少ない場合の警告文。足りていれば `None`。
pub fn shortfall_warning(requested: usize, got: usize) -> Option<String> {
    (got < requested).then(|| {
        format!(
            "要求 {requested} 候補のうち {got} 候補のみ取得（出力切れ・形式不正の可能性。閉じていないコードフェンスの候補は破棄されます）"
        )
    })
}

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
    let raw = match crate::llm_client::BackendKind::from_env() {
        crate::llm_client::BackendKind::Ollama => {
            crate::llm_client::post_ollama_text(&prompt, llm_timeout)
        }
        crate::llm_client::BackendKind::Claude => {
            crate::llm_client::post_claude(&prompt, max_tokens_for(n), llm_timeout)
        }
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
    fn max_tokens_scales_with_n_and_clamps() {
        assert_eq!(max_tokens_for(1), 2048);
        assert_eq!(max_tokens_for(3), 3072);
        assert_eq!(max_tokens_for(5), 5120);
        assert_eq!(max_tokens_for(8), 8192);
        assert_eq!(max_tokens_for(100), 8192);
    }

    #[test]
    fn parse_candidate_count_rejects_zero_and_over_max() {
        assert_eq!(parse_candidate_count("1"), Ok(1));
        assert_eq!(
            parse_candidate_count(&MAX_CANDIDATES.to_string()),
            Ok(MAX_CANDIDATES)
        );
        let zero = parse_candidate_count("0").unwrap_err();
        assert!(zero.contains("1"), "{zero}");
        assert!(parse_candidate_count(&(MAX_CANDIDATES + 1).to_string()).is_err());
        assert!(parse_candidate_count("abc").is_err());
        assert!(parse_candidate_count("-1").is_err());
    }

    #[test]
    fn shortfall_warning_only_when_fewer_than_requested() {
        assert!(shortfall_warning(3, 3).is_none());
        assert!(shortfall_warning(3, 4).is_none());
        let w = shortfall_warning(5, 2).unwrap();
        assert!(w.contains('5') && w.contains('2'), "{w}");
    }

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
