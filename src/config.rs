//! 採点ルーブリックの設定。`safecode.toml` から重みを上書きできる。
//!
//! ```toml
//! [weights]
//! correctness = 50
//! security = 20
//! performance = 15
//! maintainability = 10
//! resource_usage = 5
//!
//! # 言語別上書き（省略時は [weights] の値を使用）
//! [weights.go]
//! correctness = 55
//! security = 15
//! performance = 15
//! maintainability = 10
//! resource_usage = 5
//!
//! [weights.javascript]
//! correctness = 60
//! security = 15
//! performance = 10
//! maintainability = 10
//! resource_usage = 5
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// 採点の重み。合計 100 を想定（強制はしない）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Rubric {
    pub correctness: f64,
    pub security: f64,
    pub performance: f64,
    pub maintainability: f64,
    pub resource_usage: f64,
    /// LLM セマンティックレビュー軸の重み（既定 0 = 無効）。
    #[serde(default)]
    pub reasoning: f64,
}

impl Default for Rubric {
    fn default() -> Self {
        Rubric {
            correctness: 50.0,
            security: 20.0,
            performance: 15.0,
            maintainability: 10.0,
            resource_usage: 5.0,
            reasoning: 0.0,
        }
    }
}

/// `safecode.toml` の全設定。言語別ルーブリックを含む。
#[derive(Debug, Default)]
pub struct Config {
    pub default: Rubric,
    pub rust: Option<Rubric>,
    pub python: Option<Rubric>,
    pub go: Option<Rubric>,
    pub javascript: Option<Rubric>,
    pub typescript: Option<Rubric>,
}

impl Config {
    /// 言語名（"rust", "go", "python", "javascript", "typescript"）に合ったルーブリックを返す。
    pub fn rubric_for(&self, lang: &str) -> Rubric {
        match lang {
            "rust" => self.rust.unwrap_or(self.default),
            "python" => self.python.unwrap_or(self.default),
            "go" => self.go.unwrap_or(self.default),
            "javascript" => self.javascript.unwrap_or(self.default),
            "typescript" => self.typescript.unwrap_or(self.default),
            _ => self.default,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LangWeights {
    #[serde(default)]
    correctness: Option<f64>,
    #[serde(default)]
    security: Option<f64>,
    #[serde(default)]
    performance: Option<f64>,
    #[serde(default)]
    maintainability: Option<f64>,
    #[serde(default)]
    resource_usage: Option<f64>,
    #[serde(default)]
    reasoning: Option<f64>,
    #[serde(default)]
    rust: Option<PartialRubric>,
    #[serde(default)]
    python: Option<PartialRubric>,
    #[serde(default)]
    go: Option<PartialRubric>,
    #[serde(default)]
    javascript: Option<PartialRubric>,
    #[serde(default)]
    typescript: Option<PartialRubric>,
}

/// 言語別テーブル。省略したフィールドは default（`[weights]`）を継承する。
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialRubric {
    correctness: Option<f64>,
    security: Option<f64>,
    performance: Option<f64>,
    maintainability: Option<f64>,
    resource_usage: Option<f64>,
    reasoning: Option<f64>,
}

impl PartialRubric {
    fn apply(&self, base: Rubric) -> Rubric {
        Rubric {
            correctness: self.correctness.unwrap_or(base.correctness),
            security: self.security.unwrap_or(base.security),
            performance: self.performance.unwrap_or(base.performance),
            maintainability: self.maintainability.unwrap_or(base.maintainability),
            resource_usage: self.resource_usage.unwrap_or(base.resource_usage),
            reasoning: self.reasoning.unwrap_or(base.reasoning),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlConfig {
    #[serde(default)]
    weights: Option<LangWeights>,
}

impl Rubric {
    /// 各軸の重みが有限かつ 0 以上であることを検証する（`where_` はエラー表示用のセクション名）。
    fn validate(&self, where_: &str) -> Result<()> {
        let axes = [
            ("correctness", self.correctness),
            ("security", self.security),
            ("performance", self.performance),
            ("maintainability", self.maintainability),
            ("resource_usage", self.resource_usage),
            ("reasoning", self.reasoning),
        ];
        for (name, v) in axes {
            if !v.is_finite() || v < 0.0 {
                anyhow::bail!(
                    "重み {where_}.{name} は 0 以上の有限値で指定してください（指定値: {v}）"
                );
            }
        }
        Ok(())
    }

    /// 後方互換: グローバルルーブリックを読む。
    pub fn load(explicit: Option<&str>) -> Result<Rubric> {
        Ok(Config::load(explicit)?.default)
    }
}

impl Config {
    /// `safecode.toml` を読み込んで `Config` を返す。
    pub fn load(explicit: Option<&str>) -> Result<Config> {
        let path = match explicit {
            Some(p) => Some(Path::new(p).to_path_buf()),
            None => {
                let default = Path::new("safecode.toml");
                default.exists().then(|| default.to_path_buf())
            }
        };

        let Some(path) = path else {
            return Ok(Config::default());
        };

        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("設定ファイルの読込に失敗: {}", path.display()))?;
        let parsed: TomlConfig = toml::from_str(&text)
            .with_context(|| format!("設定ファイルの解析に失敗: {}", path.display()))?;

        let base = Rubric::default();
        let default_rubric = parsed
            .weights
            .as_ref()
            .map(|w| Rubric {
                correctness: w.correctness.unwrap_or(base.correctness),
                security: w.security.unwrap_or(base.security),
                performance: w.performance.unwrap_or(base.performance),
                maintainability: w.maintainability.unwrap_or(base.maintainability),
                resource_usage: w.resource_usage.unwrap_or(base.resource_usage),
                reasoning: w.reasoning.unwrap_or(base.reasoning),
            })
            .unwrap_or(base);

        let w = parsed.weights.unwrap_or_default();
        let resolve = |p: &Option<PartialRubric>| p.as_ref().map(|p| p.apply(default_rubric));

        let config = Config {
            default: default_rubric,
            rust: resolve(&w.rust),
            python: resolve(&w.python),
            go: resolve(&w.go),
            javascript: resolve(&w.javascript),
            typescript: resolve(&w.typescript),
        };
        config.default.validate("weights")?;
        for (lang, r) in [
            ("rust", &config.rust),
            ("python", &config.python),
            ("go", &config.go),
            ("javascript", &config.javascript),
            ("typescript", &config.typescript),
        ] {
            if let Some(r) = r {
                r.validate(&format!("weights.{lang}"))?;
            }
        }
        Ok(config)
    }
}

/// clap 用: `--regression-threshold` を 0 以上の有限値に検証する。
pub fn parse_regression_threshold(s: &str) -> Result<f64, String> {
    let v: f64 = s
        .parse()
        .map_err(|_| format!("`{s}` は数値ではありません"))?;
    if v.is_finite() && v >= 0.0 {
        Ok(v)
    } else {
        Err(format!(
            "回帰しきい値は 0 以上の有限値で指定してください（指定値: {s}）"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn default_when_no_file() {
        let r = Rubric::load(None).unwrap();
        assert_eq!(r.correctness, 50.0);
        assert_eq!(r.resource_usage, 5.0);
    }

    #[test]
    fn loads_weights_from_toml() {
        let mut f = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
        writeln!(
            f,
            "[weights]\ncorrectness = 70\nsecurity = 10\nperformance = 10\nmaintainability = 5\nresource_usage = 5"
        )
        .unwrap();
        let r = Rubric::load(Some(f.path().to_str().unwrap())).unwrap();
        assert_eq!(r.correctness, 70.0);
        assert_eq!(r.security, 10.0);
    }

    fn load_weights_err(body: &str) -> String {
        let mut f = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
        writeln!(f, "{body}").unwrap();
        format!(
            "{:#}",
            Config::load(Some(f.path().to_str().unwrap())).unwrap_err()
        )
    }

    #[test]
    fn nan_weight_is_rejected_with_axis_name() {
        let e = load_weights_err("[weights]\ncorrectness = nan");
        assert!(e.contains("correctness"), "{e}");
    }

    #[test]
    fn infinite_and_negative_weights_are_rejected() {
        assert!(load_weights_err("[weights]\nsecurity = inf").contains("security"));
        assert!(load_weights_err("[weights]\nperformance = -1").contains("performance"));
    }

    #[test]
    fn invalid_lang_weight_is_rejected_with_section() {
        let e = load_weights_err("[weights.go]\nreasoning = nan");
        assert!(e.contains("reasoning") && e.contains("go"), "{e}");
    }

    #[test]
    fn zero_weight_is_accepted() {
        let mut f = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
        writeln!(f, "[weights]\nreasoning = 0").unwrap();
        assert!(Config::load(Some(f.path().to_str().unwrap())).is_ok());
    }

    #[test]
    fn regression_threshold_parser_accepts_only_finite_non_negative() {
        assert_eq!(parse_regression_threshold("0").unwrap(), 0.0);
        assert_eq!(parse_regression_threshold("1.5").unwrap(), 1.5);
        for bad in ["-1", "-0.1", "nan", "inf", "-inf", "abc"] {
            assert!(parse_regression_threshold(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn missing_explicit_path_errors() {
        assert!(Rubric::load(Some("/no/such/safecode.toml")).is_err());
    }

    #[test]
    fn lang_specific_weights_override_default() {
        let mut f = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
        writeln!(
            f,
            r#"
[weights]
correctness = 50
security = 20
performance = 15
maintainability = 10
resource_usage = 5

[weights.go]
correctness = 55
security = 15
performance = 15
maintainability = 10
resource_usage = 5
"#
        )
        .unwrap();
        let c = Config::load(Some(f.path().to_str().unwrap())).unwrap();
        assert_eq!(c.default.correctness, 50.0);
        assert_eq!(c.rubric_for("go").correctness, 55.0);
        assert_eq!(c.rubric_for("go").security, 15.0);
        assert_eq!(c.rubric_for("rust").correctness, 50.0);
    }

    #[test]
    fn rubric_for_unknown_lang_returns_default() {
        let c = Config::default();
        assert_eq!(c.rubric_for("cobol").correctness, 50.0);
    }

    fn load_str(toml: &str) -> Config {
        let mut f = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
        write!(f, "{toml}").unwrap();
        Config::load(Some(f.path().to_str().unwrap())).unwrap()
    }

    #[test]
    fn partial_lang_override_inherits_default() {
        let c =
            load_str("[weights]\ncorrectness = 40\nsecurity = 30\n\n[weights.go]\nsecurity = 5\n");
        let g = c.rubric_for("go");
        assert_eq!(g.security, 5.0);
        // 省略フィールドは [weights]（default）を継承する
        assert_eq!(g.correctness, 40.0);
        assert_eq!(g.performance, 15.0);
    }

    #[test]
    fn typescript_override_applies() {
        let c = load_str("[weights.typescript]\ncorrectness = 65\n");
        assert_eq!(c.rubric_for("typescript").correctness, 65.0);
        assert_eq!(c.rubric_for("javascript").correctness, 50.0);
    }

    #[test]
    fn no_lang_tables_means_default_for_all() {
        let c = load_str("[weights]\ncorrectness = 70\n");
        for l in ["rust", "python", "go", "javascript", "typescript"] {
            assert_eq!(c.rubric_for(l).correctness, 70.0, "{l}");
        }
    }

    fn load_str_err(toml: &str) -> anyhow::Error {
        let mut f = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
        write!(f, "{toml}").unwrap();
        Config::load(Some(f.path().to_str().unwrap())).unwrap_err()
    }

    #[test]
    fn partial_weights_fall_back_to_rubric_default() {
        let r = load_str(
            "[weights]
correctness = 70
",
        )
        .default;
        let d = Rubric::default();
        assert_eq!(r.correctness, 70.0);
        assert_eq!(r.security, d.security);
        assert_eq!(r.performance, d.performance);
        assert_eq!(r.maintainability, d.maintainability);
        assert_eq!(r.resource_usage, d.resource_usage);
        assert_eq!(r.reasoning, 0.0);
    }

    #[test]
    fn reasoning_key_is_loaded_and_defaults_to_zero() {
        let c = load_str(
            "[weights]
reasoning = 12

[weights.go]
reasoning = 3
",
        );
        assert_eq!(c.default.reasoning, 12.0);
        assert_eq!(c.rubric_for("go").reasoning, 3.0);
        // 言語別で省略した reasoning は [weights] を継承する
        assert_eq!(c.rubric_for("rust").reasoning, 12.0);
        let c = load_str(
            "[weights.go]
correctness = 1
",
        );
        assert_eq!(c.rubric_for("go").reasoning, 0.0);
    }

    #[test]
    fn invalid_toml_syntax_errors_with_parse_message() {
        let e = load_str_err(
            "[weights
correctness = ",
        );
        assert!(e.to_string().contains("設定ファイルの解析に失敗"), "{e}");
    }

    #[test]
    fn wrong_value_type_errors_with_parse_message() {
        let e = load_str_err(
            "[weights]
correctness = \"high\"
",
        );
        assert!(e.to_string().contains("設定ファイルの解析に失敗"), "{e}");
        let e = load_str_err("[weights.go]\nsecurity = \"x\"\n");
        assert!(e.to_string().contains("設定ファイルの解析に失敗"), "{e}");
    }

    // Issue #131 (4): 言語別テーブルは部分指定を許す（PartialRubric）。
    // 必須キー欠落でも解析は失敗せず、欠けたキーは [weights] を継承する。
    #[test]
    fn lang_table_missing_keys_is_allowed_and_inherits() {
        let c = load_str("[weights.go]\nsecurity = 5\n");
        let g = c.rubric_for("go");
        let d = Rubric::default();
        assert_eq!(g.security, 5.0);
        assert_eq!(g.correctness, d.correctness);
        assert_eq!(g.resource_usage, d.resource_usage);
    }

    #[test]
    fn empty_lang_table_equals_default() {
        let c = load_str("[weights]\ncorrectness = 33\n\n[weights.python]\n");
        assert!(c.python.is_some());
        assert_eq!(c.rubric_for("python").correctness, 33.0);
        assert_eq!(c.rubric_for("python").security, 20.0);
    }

    #[test]
    fn python_javascript_rust_rubric_for() {
        let c = load_str(
            "[weights.python]\ncorrectness = 1\n[weights.javascript]\ncorrectness = 2\n[weights.rust]\ncorrectness = 3\n",
        );
        assert_eq!(c.rubric_for("python").correctness, 1.0);
        assert_eq!(c.rubric_for("javascript").correctness, 2.0);
        assert_eq!(c.rubric_for("rust").correctness, 3.0);
        // 指定のない言語は default
        assert_eq!(c.rubric_for("go").correctness, 50.0);
        assert_eq!(c.rubric_for("typescript").correctness, 50.0);
    }

    #[test]
    fn empty_file_yields_default() {
        let c = load_str("");
        let d = Rubric::default();
        assert_eq!(c.default.correctness, d.correctness);
        assert_eq!(c.default.reasoning, 0.0);
        assert!(c.rust.is_none() && c.python.is_none() && c.go.is_none());
        assert!(c.javascript.is_none() && c.typescript.is_none());
    }

    // 未知キー（タイポ）は黙って無視せず、解析エラーにしてキー名を示す。
    #[test]
    fn unknown_keys_are_rejected() {
        for (toml, key) in [
            ("[weights]\nsecurty = 99\ncorrectness = 60\n", "securty"),
            ("[weights.go]\nsecurty = 5\n", "securty"),
            ("[wieghts]\ncorrectness = 60\n", "wieghts"),
            ("[weights.cobol]\ncorrectness = 1\n", "cobol"),
        ] {
            let e = format!("{:#}", load_str_err(toml));
            assert!(e.contains("設定ファイルの解析に失敗"), "{e}");
            assert!(e.contains(key), "{e}");
        }
    }
}
