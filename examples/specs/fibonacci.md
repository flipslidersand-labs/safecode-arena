# Spec: fibonacci

Write a Rust function `fibonacci` that returns the nth Fibonacci number.

## Signature

```rust
pub fn fibonacci(n: u64) -> u64
```

## Requirements

- Must handle n=0 → 0, n=1 → 1
- Must not overflow for n ≤ 50 (use u64)
- No panics for valid input
- Must pass: `assert_eq!(fibonacci(10), 55)`
