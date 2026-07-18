# SafeCode Arena

[日本語版 README](README.ja.md)

A verification runner that takes multiple AI-generated code candidates, **evaluates them automatically in an isolated sandbox, scores them, and picks the best one to adopt**.

> The point of this project is not code generation — it's the verification pipeline that turns generated code into something you can actually trust.

## Pipeline

```text
spec → N code candidates → compile → test → (property/fuzz/perf/static analysis) → score → adoption decision
```

## Usage

```bash
# Evaluate a single candidate
safecode evaluate candidate.rs --tests tests/

# Compare multiple candidates and emit a report
safecode evaluate cand_a.rs cand_b.rs cand_c.rs --tests tests/ --out report.md

# Run property tests (proptest) as well
safecode evaluate candidate.rs --prop-tests prop/

# JSON output / override the scoring rubric
safecode evaluate candidate.rs --format json --config safecode.toml

# Persist results to a DB (regression detection against past runs)
safecode evaluate candidate.rs --db history.db
safecode history --db history.db

# Isolated execution in a Wasm sandbox (candidate needs a pub fn run())
safecode evaluate candidate.rs --wasm-entry run --wasm-fuel 100000000

# Python candidates work too (auto-detected by extension), including mixed-language comparison
safecode evaluate solution.py --tests py_tests/
safecode evaluate cand.rs cand.py    # cross-language comparison in one run
```

### Supported languages

| Language | compile       | test         | lint     | wasm             |
| -------- | ------------- | ------------ | -------- | ---------------- |
| Rust     | `cargo build` | `cargo test` | `clippy` | ✅ wasm32-wasip1 |
| Python   | `py_compile`  | `pytest`     | `ruff`   | —                |

## Scoring rubric

| Axis            | Weight | How it's computed                                     |
| --------------- | ------ | ----------------------------------------------------- |
| correctness     | 50     | compile 40% + tests 40% + property tests 20%          |
| security        | 20     | `unsafe` heuristics 50% + clippy 50%                  |
| performance     | 15     | relative compile+test time across candidates          |
| maintainability | 10     | function-length heuristics 60% + clippy 40%           |
| resource_usage  | 5      | pass/fail of sandboxed Wasm (wasm32-wasip1) execution |

Weights can be overridden via `[weights]` in `safecode.toml`.

## Development

```bash
cargo build
cargo test
cargo clippy -- -D warnings
cargo fmt
```

## Documentation (Japanese)

- [Spec](docs/spec.md)
- [Tech stack](docs/tech-stack.md)
- [Data model](docs/data-model.md)
- [Implementation guide](docs/implementation-guide.md)
- [ADRs](docs/adr/)

## Status

✅ Phases 1–4 complete (all 5 axes measured / SQLite persistence + regression detection / Wasm sandbox). Phase 5 added **Python support** (py_compile / pytest / ruff, mixed comparison with Rust). Next up: Go/JS support, mutation testing. See Issue #1 for the roadmap.

## License

MIT
