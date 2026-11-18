//! 言語レジストリ。言語ごとの名前・拡張子・検証ステージ実行を `LanguageRunner` に集約する。
//!
//! 言語を追加するときは `Language` の variant、ランナー実装、`RUNNERS` への登録、
//! `Language::SUPPORTED_EXTENSIONS` の更新が必要（網羅はテストで検査する）。

use crate::model::{Candidate, Language};
use crate::pipeline::go::run_go_stages;
use crate::pipeline::js::run_js_stages;
use crate::pipeline::python::run_python_stages;
use crate::pipeline::ts::run_ts_stages;
use crate::pipeline::{run_rust_stages, StageContext, StageResults};
use anyhow::Result;
use std::path::Path;

/// 1 言語分の検証ランナー。
pub(crate) trait LanguageRunner: Sync {
    fn language(&self) -> Language;
    /// 小文字の名前（serde の lowercase rename と一致）。
    fn name(&self) -> &'static str;
    /// 対応する拡張子（小文字、ドットなし）。
    fn extensions(&self) -> &'static [&'static str];
    /// 候補を `root` に展開して検証ステージを実行する。
    fn run(&self, root: &Path, candidate: &Candidate, ctx: &StageContext) -> Result<StageResults>;
}

macro_rules! runner {
    ($ty:ident, $lang:expr, $name:expr, $exts:expr, $run:path) => {
        struct $ty;
        impl LanguageRunner for $ty {
            fn language(&self) -> Language {
                $lang
            }
            fn name(&self) -> &'static str {
                $name
            }
            fn extensions(&self) -> &'static [&'static str] {
                $exts
            }
            fn run(
                &self,
                root: &Path,
                candidate: &Candidate,
                ctx: &StageContext,
            ) -> Result<StageResults> {
                $run(root, candidate, ctx)
            }
        }
    };
}

runner!(RustRunner, Language::Rust, "rust", &["rs"], run_rust_stages);
runner!(
    PythonRunner,
    Language::Python,
    "python",
    &["py"],
    run_python_stages
);
runner!(GoRunner, Language::Go, "go", &["go"], run_go_stages);
runner!(
    JsRunner,
    Language::JavaScript,
    "javascript",
    &["js"],
    run_js_stages
);
runner!(
    TsRunner,
    Language::TypeScript,
    "typescript",
    &["ts"],
    run_ts_stages
);

/// 全言語のランナー。
pub(crate) static RUNNERS: [&dyn LanguageRunner; 5] =
    [&RustRunner, &PythonRunner, &GoRunner, &JsRunner, &TsRunner];

/// 言語に対応するランナー。`RUNNERS` が全言語を網羅することはテストで担保する。
pub(crate) fn runner_for(lang: Language) -> &'static dyn LanguageRunner {
    RUNNERS
        .iter()
        .copied()
        .find(|r| r.language() == lang)
        .expect("RUNNERS must cover every Language (see tests)")
}

/// 拡張子（大文字小文字を区別しない）から言語を引く。
pub(crate) fn from_extension(ext: &str) -> Option<Language> {
    let ext = ext.to_ascii_lowercase();
    RUNNERS
        .iter()
        .find(|r| r.extensions().contains(&ext.as_str()))
        .map(|r| r.language())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全 variant を列挙する網羅 match。variant を足すとここでコンパイルが落ちる。
    fn ordinal(l: Language) -> usize {
        match l {
            Language::Rust => 0,
            Language::Python => 1,
            Language::Go => 2,
            Language::JavaScript => 3,
            Language::TypeScript => 4,
        }
    }

    fn all_languages() -> [Language; 5] {
        [
            Language::Rust,
            Language::Python,
            Language::Go,
            Language::JavaScript,
            Language::TypeScript,
        ]
    }

    #[test]
    fn all_languages_lists_every_variant() {
        let mut ords: Vec<usize> = all_languages().into_iter().map(ordinal).collect();
        ords.sort_unstable();
        assert_eq!(ords, (0..all_languages().len()).collect::<Vec<_>>());
    }

    #[test]
    fn runners_cover_every_language_exactly_once() {
        for lang in all_languages() {
            let n = RUNNERS.iter().filter(|r| r.language() == lang).count();
            assert_eq!(n, 1, "{lang:?}");
            assert_eq!(runner_for(lang).language(), lang);
        }
        assert_eq!(RUNNERS.len(), all_languages().len());
    }

    #[test]
    fn runner_extensions_match_supported_extensions() {
        let mut from_runners: Vec<&str> = RUNNERS
            .iter()
            .flat_map(|r| r.extensions().iter().copied())
            .collect();
        let mut supported: Vec<&str> = Language::SUPPORTED_EXTENSIONS.to_vec();
        from_runners.sort_unstable();
        supported.sort_unstable();
        assert_eq!(from_runners, supported);
    }
}
