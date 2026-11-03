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

/// 先頭 `n` 件だけ残す（LLM が要求より多く返した場合の切り詰め）。
fn truncate_candidates(candidates: Vec<String>, n: usize) -> Vec<String> {
    candidates.into_iter().take(n).collect()
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
        Some(text) => truncate_candidates(parse_candidates(&text), n),
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

    fn block(body: &str) -> String {
        format!("---CANDIDATE---\n```rust\n{body}\n```\n")
    }

    #[test]
    fn parse_candidates_drops_unclosed_fence() {
        // 閉じ ``` が無い候補は破棄される（出力切れ想定）。
        assert!(parse_candidates("---CANDIDATE---\n```rust\npub fn f() {}\n").is_empty());
        // 閉じ済み + 未閉じ: 閉じ済みだけ残る。
        let out = format!(
            "{}---CANDIDATE---\n```rust\npub fn g() {{",
            block("pub fn f() {}")
        );
        assert_eq!(parse_candidates(&out), vec!["pub fn f() {}".to_string()]);
    }

    #[test]
    fn parse_candidates_tag_is_case_sensitive_rust_only() {
        // 現状の挙動: 小文字 ```rust のみ受理。別タグ・タグ無しは抽出されない。
        for tag in ["Rust", "RUST", "rs", ""] {
            let out = format!("---CANDIDATE---\n```{tag}\npub fn f() {{}}\n```\n");
            assert!(parse_candidates(&out).is_empty(), "tag={tag:?}");
        }
    }

    #[test]
    fn parse_candidates_rust_prefix_tag_leaks_into_code() {
        // 現状の挙動（要報告）: "```rustc" は "```rust" 前方一致し、残りの "c" がコード先頭に混入する。
        let out = "---CANDIDATE---\n```rustc\npub fn f() {}\n```\n";
        assert_eq!(parse_candidates(out), vec!["c\npub fn f() {}".to_string()]);
    }

    #[test]
    fn parse_candidates_no_fence_at_all_even_with_separator() {
        assert!(
            parse_candidates("---CANDIDATE---\npub fn f() {}\n---CANDIDATE---\nfoo").is_empty()
        );
    }

    #[test]
    fn parse_candidates_only_first_block_per_section() {
        let out = "---CANDIDATE---\n```rust\nfn a() {}\n```\ntext\n```rust\nfn b() {}\n```\n";
        assert_eq!(parse_candidates(out), vec!["fn a() {}".to_string()]);
    }

    #[test]
    fn parse_candidates_single_block_without_separator() {
        let out = "```rust\npub fn f() {}\n```";
        assert_eq!(parse_candidates(out), vec!["pub fn f() {}".to_string()]);
        // 前置きテキスト付きでも抽出される。
        let out = "Here you go:\n```rust\npub fn f() {}\n```\nDone.";
        assert_eq!(parse_candidates(out), vec!["pub fn f() {}".to_string()]);
    }

    #[test]
    fn parse_candidates_skips_blank_block_but_keeps_order() {
        let out = format!(
            "{}---CANDIDATE---\n```rust\n  \n```\n{}",
            block("fn a() {}"),
            block("fn c() {}")
        );
        assert_eq!(
            parse_candidates(&out),
            vec!["fn a() {}".to_string(), "fn c() {}".to_string()]
        );
    }

    #[test]
    fn build_generate_prompt_replaces_n_and_spec() {
        let p = build_generate_prompt("sum two ints", 3);
        assert!(p.contains("exactly 3 distinct"), "{p}");
        assert!(p.contains("repeat exactly 3 times"), "{p}");
        assert!(p.contains("## Specification\nsum two ints\n"), "{p}");
        assert!(!p.contains("{N}") && !p.contains("{SPEC}"), "{p}");
    }

    #[test]
    fn build_generate_prompt_does_not_double_replace_placeholders_in_spec() {
        // N を先に置換し SPEC を後で挿入するため、spec 内の {N} / {SPEC} はそのまま残る。
        let p = build_generate_prompt("repeat {N} times; {SPEC}", 4);
        assert!(
            p.contains("## Specification\nrepeat {N} times; {SPEC}\n"),
            "{p}"
        );
        assert!(p.contains("exactly 4 distinct"), "{p}");
    }

    #[test]
    fn build_generate_prompt_handles_empty_spec() {
        let p = build_generate_prompt("", 1);
        assert!(p.contains("## Specification\n\n\n## Requirements"), "{p}");
    }

    #[test]
    fn truncate_candidates_keeps_first_n() {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(truncate_candidates(v(&["a", "b", "c"]), 2), v(&["a", "b"]));
        assert_eq!(truncate_candidates(v(&["a"]), 3), v(&["a"]));
        assert!(truncate_candidates(v(&["a"]), 0).is_empty());
        assert!(truncate_candidates(vec![], 2).is_empty());
    }
}
