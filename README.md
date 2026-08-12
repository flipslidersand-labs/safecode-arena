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

# Python / Go / JavaScript candidates work too (auto-detected by extension)
safecode evaluate solution.py --tests py_tests/
safecode evaluate solution.go --tests tests/
safecode evaluate solution.js
safecode evaluate cand.rs cand.py cand.go    # cross-language comparison in one run
```

### Supported languages

| Language   | compile        | test                  | lint                     | wasm             | mutation (`--mutation`)      |
| ---------- | -------------- | --------------------- | ------------------------ | ---------------- | ---------------------------- |
| Rust       | `cargo build`  | `cargo test`          | `clippy`                 | ✅ wasm32-wasip1 | `cargo-mutants` (if in PATH) |
| Python     | `py_compile`   | `pytest`              | `ruff`                   | —                | `mutmut` (if in PATH)        |
| Go         | `go build`     | `go test`             | `staticcheck` → `go vet` | —                | `gremlins` (if in PATH)      |
| JavaScript | `node --check` | `node --test`         | `eslint` (if in PATH)    | —                | —                            |
| TypeScript | `tsc --noEmit` | `tsc` + `node --test` | `eslint` (if in PATH)    | —                | —                            |

## Scoring rubric

| Axis            | Weight | How it's computed                                                                       |
| --------------- | ------ | --------------------------------------------------------------------------------------- |
| correctness     | 50     | compile 40% + tests 40% + prop tests 20% (with `--mutation`: 30% + 30% + 15% + 25% mut) |
| security        | 20     | `unsafe` heuristics 50% + clippy 50%                                                    |
| performance     | 15     | relative compile+test time across candidates                                            |
| maintainability | 10     | function-length heuristics 60% + clippy 40%                                             |
| resource_usage  | 5      | pass/fail of sandboxed Wasm (wasm32-wasip1) execution                                   |

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

✅ Phases 1–11 complete. All 5 scoring axes measured. SQLite persistence + regression detection. Wasm sandbox. **Multi-language**: Rust / Python / Go / JavaScript auto-detected by extension. **Mutation testing**: Rust (`cargo-mutants`), Python (`mutmut`), Go (`gremlins`) integrated and weight-aware. **Criterion benchmarks** integrated for performance axis. GitHub Actions CI with regression detection.

## License

MIT
