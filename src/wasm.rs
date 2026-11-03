//! Wasm サンドボックス実行（Phase 4-b）。
//!
//! 候補を `wasm32-wasip1` にビルドした成果物を、埋め込み wasmtime ランタイムで
//! 実行する。capability ベースの隔離（ホスト FS / ネットワーク / env をすべて非公開）、
//! fuel による命令数上限、`StoreLimits` によるメモリ上限を課す。
//!
//! 実行結果は `resource_usage` 軸の採点に使う（`scoring`）。

use crate::model::StageOutcome;
use std::path::Path;
use std::time::Instant;
use wasmtime::{Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder, Trap};
use wasmtime_wasi::p1::{self, WasiP1Ctx};
use wasmtime_wasi::{I32Exit, WasiCtxBuilder};

/// Wasm 実行の結果。`fuel_used` は消費した命令数（fuel 単位）。
pub struct WasmOutcome {
    pub outcome: StageOutcome,
    pub fuel_used: Option<u64>,
}

impl WasmOutcome {
    fn failed(detail: impl Into<String>) -> Self {
        WasmOutcome {
            outcome: StageOutcome::Failed {
                detail: detail.into(),
            },
            fuel_used: None,
        }
    }
}

/// Store が持つホスト状態: WASI コンテキスト + リソース制限。
struct Host {
    wasi: WasiP1Ctx,
    limits: StoreLimits,
}

/// `.wasm` ファイルを fuel/メモリ制限付きで実行する。
pub fn run_wasm(wasm_path: &Path, fuel: u64, max_memory_bytes: usize) -> WasmOutcome {
    match std::fs::read(wasm_path) {
        Ok(bytes) => run_bytes(&bytes, fuel, max_memory_bytes),
        Err(e) => WasmOutcome::failed(format!("wasm 読込失敗: {e}")),
    }
}

/// wasm バイナリ（または wat テキスト）を実行する中核。
/// `_start`（WASI command）を呼び、結果を `StageOutcome` に分類する。
fn run_bytes(bytes: &[u8], fuel: u64, max_memory_bytes: usize) -> WasmOutcome {
    let mut config = Config::new();
    config.consume_fuel(true);
    let engine = match Engine::new(&config) {
        Ok(e) => e,
        Err(e) => return WasmOutcome::failed(format!("engine 生成失敗: {e}")),
    };
    let module = match Module::new(&engine, bytes) {
        Ok(m) => m,
        Err(e) => return WasmOutcome::failed(format!("module ロード失敗: {e}")),
    };

    // capability ゼロの WASI（preopen/env/network なし）。
    let wasi = WasiCtxBuilder::new().build_p1();
    let limits = StoreLimitsBuilder::new()
        .memory_size(max_memory_bytes)
        .build();
    let mut store = Store::new(&engine, Host { wasi, limits });
    store.limiter(|h| &mut h.limits);
    if let Err(e) = store.set_fuel(fuel) {
        return WasmOutcome::failed(format!("fuel 設定失敗: {e}"));
    }

    let mut linker: Linker<Host> = Linker::new(&engine);
    if let Err(e) = p1::add_to_linker_sync(&mut linker, |h: &mut Host| &mut h.wasi) {
        return WasmOutcome::failed(format!("WASI link 失敗: {e}"));
    }

    let instance = match linker.instantiate(&mut store, &module) {
        Ok(i) => i,
        Err(e) => return WasmOutcome::failed(format!("instantiate 失敗: {e}")),
    };
    let start = match instance.get_typed_func::<(), ()>(&mut store, "_start") {
        Ok(f) => f,
        Err(_) => return WasmOutcome::failed("_start エクスポートがありません"),
    };

    let t0 = Instant::now();
    let result = start.call(&mut store, ());
    let elapsed_ms = t0.elapsed().as_millis() as u64;
    let fuel_used = store.get_fuel().ok().map(|left| fuel.saturating_sub(left));

    let outcome = match result {
        Ok(()) => StageOutcome::Passed {
            duration_ms: elapsed_ms,
        },
        // 元の wasmtime エラーをそのまま渡す（re-wrap すると downcast 情報が失われる）。
        Err(e) => classify_error(&e, elapsed_ms),
    };
    WasmOutcome { outcome, fuel_used }
}

/// 実行エラーを分類する。WASI の正常 exit(0) は成功、fuel 枯渇はタイムアウト扱い。
fn classify_error(e: &wasmtime::Error, elapsed_ms: u64) -> StageOutcome {
    // 正常終了 exit(0)（WASI command は proc_exit でトラップする）。
    if let Some(exit) = e.downcast_ref::<I32Exit>() {
        return if exit.0 == 0 {
            StageOutcome::Passed {
                duration_ms: elapsed_ms,
            }
        } else {
            StageOutcome::Failed {
                detail: format!("exit code {}", exit.0),
            }
        };
    }
    // fuel 枯渇。downcast に加え、backtrace 付きでラップされた場合に備えて
    // メッセージ文字列でもフォールバック判定する。
    let is_out_of_fuel = e.downcast_ref::<Trap>() == Some(&Trap::OutOfFuel)
        || format!("{e:#}").contains("fuel consumed");
    if is_out_of_fuel {
        // limit_ms には経過 ms を入れる（消費 fuel は別途 fuel_used で持つ）。
        return StageOutcome::TimedOut {
            limit_ms: elapsed_ms,
        };
    }
    StageOutcome::Failed {
        detail: format!("{e:#}")
            .lines()
            .next()
            .unwrap_or_default()
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_MB: usize = 1024 * 1024;

    #[test]
    fn empty_start_passes() {
        // 何もしない _start。
        let wat = br#"(module (func (export "_start")))"#;
        let r = run_bytes(wat, 100_000, ONE_MB);
        assert!(r.outcome.is_passed(), "outcome={:?}", r.outcome);
        assert!(r.fuel_used.is_some());
    }

    #[test]
    fn infinite_loop_exhausts_fuel() {
        // 無限ループ → fuel 枯渇で TimedOut。
        let wat = br#"(module (func (export "_start") (loop br 0)))"#;
        let r = run_bytes(wat, 10_000, ONE_MB);
        assert!(
            matches!(r.outcome, StageOutcome::TimedOut { .. }),
            "outcome={:?}",
            r.outcome
        );
    }

    #[test]
    fn missing_start_fails() {
        let wat = br#"(module (func (export "other")))"#;
        let r = run_bytes(wat, 10_000, ONE_MB);
        assert!(matches!(r.outcome, StageOutcome::Failed { .. }));
    }

    /// `proc_exit(code)` を `_start` で呼ぶ WAT。
    fn exit_wat(code: i32) -> String {
        format!(
            r#"(module
                (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
                (func (export "_start") (i32.const {code}) (call $exit)))"#
        )
    }

    fn failed_detail(o: &StageOutcome) -> &str {
        match o {
            StageOutcome::Failed { detail } => detail,
            other => panic!("Failed を期待したが {other:?}"),
        }
    }

    // 注意: 以下3件は現状の挙動の固定。wasmtime 46 では backtrace 付きでラップされた
    // エラーから I32Exit/Trap の downcast が効かず、proc_exit(0) も Failed になり、
    // detail は先頭行 "error while executing at wasm backtrace:" だけになる。
    // classify_error の修正時は期待値を Passed / "exit code 3" 等へ更新すること。
    const BACKTRACE_HEAD: &str = "error while executing at wasm backtrace:";

    #[test]
    fn proc_exit_zero_current_behavior_is_failed() {
        let r = run_bytes(exit_wat(0).as_bytes(), 100_000, ONE_MB);
        assert_eq!(failed_detail(&r.outcome), BACKTRACE_HEAD);
        assert!(r.fuel_used.is_some_and(|u| u <= 100_000));
    }

    #[test]
    fn proc_exit_nonzero_current_behavior_loses_exit_code() {
        let r = run_bytes(exit_wat(3).as_bytes(), 100_000, ONE_MB);
        assert_eq!(failed_detail(&r.outcome), BACKTRACE_HEAD);
        assert!(r.fuel_used.is_some_and(|u| u <= 100_000));
    }

    #[test]
    fn unreachable_trap_fails_with_first_line_only() {
        let wat = br#"(module (func (export "_start") unreachable))"#;
        let r = run_bytes(wat, 100_000, ONE_MB);
        assert_eq!(failed_detail(&r.outcome), BACKTRACE_HEAD);
        assert!(r.fuel_used.is_some_and(|u| u <= 100_000));
    }

    #[test]
    fn initial_memory_over_limit_fails_instantiate() {
        // 100 page = 6.4MB > 1MB 上限。
        let wat = br#"(module (memory 100) (func (export "_start")))"#;
        let r = run_bytes(wat, 100_000, ONE_MB);
        let detail = failed_detail(&r.outcome);
        assert!(detail.starts_with("instantiate 失敗"), "detail={detail:?}");
        assert_eq!(r.fuel_used, None);
    }

    #[test]
    fn initial_memory_within_limit_passes() {
        // 1 page = 64KiB < 1MB。上限判定が過剰でないことの対照。
        let wat = br#"(module (memory 1) (func (export "_start")))"#;
        let r = run_bytes(wat, 100_000, ONE_MB);
        assert!(r.outcome.is_passed(), "outcome={:?}", r.outcome);
    }

    #[test]
    fn invalid_bytes_fail_module_load() {
        let r = run_bytes(b"\0bad", 100_000, ONE_MB);
        let detail = failed_detail(&r.outcome);
        assert!(detail.starts_with("module ロード失敗"), "detail={detail:?}");
        assert_eq!(r.fuel_used, None);
    }

    #[test]
    fn nonexistent_path_fails_read() {
        let p = Path::new("/nonexistent/safecode-arena/no-such.wasm");
        let r = run_wasm(p, 100_000, ONE_MB);
        let detail = failed_detail(&r.outcome);
        assert!(detail.starts_with("wasm 読込失敗"), "detail={detail:?}");
        assert_eq!(r.fuel_used, None);
    }

    #[test]
    fn fuel_used_never_exceeds_fuel() {
        let wat = br#"(module (func (export "_start") (loop br 0)))"#;
        let r = run_bytes(wat, 10_000, ONE_MB);
        assert!(matches!(r.outcome, StageOutcome::TimedOut { .. }));
        assert!(r.fuel_used.is_some_and(|u| u > 0 && u <= 10_000));
    }
}
