# Spec: reverse_string

Write a Rust function `reverse_string` that reverses a string without heap allocation.

## Signature

```rust
pub fn reverse_string(s: &str) -> String
```

## Requirements

- Must handle Unicode correctly (reverse by char, not byte)
- No `unsafe` blocks
- Must pass: `assert_eq!(reverse_string("hello"), "olleh")`
- Must pass: `assert_eq!(reverse_string(""), "")`
- Must pass: `assert_eq!(reverse_string("日本語"), "語本日")`
