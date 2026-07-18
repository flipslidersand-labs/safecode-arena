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
}

impl Default for Rubric {
    fn default() -> Self {
        Rubric {
            correctness: 50.0,
            security: 20.0,
            performance: 15.0,
            maintainability: 10.0,
            resource_usage: 5.0,
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
}

impl Config {
    /// 言語名（"rust", "go", "python", "javascript"）に合ったルーブリックを返す。
    pub fn rubric_for(&self, lang: &str) -> Rubric {
        match lang {
            "rust" => self.rust.unwrap_or(self.default),
            "python" => self.python.unwrap_or(self.default),
            "go" => self.go.unwrap_or(self.default),
            "javascript" => self.javascript.unwrap_or(self.default),
            _ => self.default,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct WeightsSection {
    #[serde(flatten)]
    base: Option<FlatRubric>,
    rust: Option<Rubric>,
    python: Option<Rubric>,
    go: Option<Rubric>,
    javascript: Option<Rubric>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct FlatRubric {
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
}

impl FlatRubric {
    fn into_rubric(self, base: Rubric) -> Rubric {
        Rubric {
            correctness: self.correctness.unwrap_or(base.correctness),
            security: self.security.unwrap_or(base.security),
            performance: self.performance.unwrap_or(base.performance),
            maintainability: self.maintainability.unwrap_or(base.maintainability),
            resource_usage: self.resource_usage.unwrap_or(base.resource_usage),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    weights: Option<Rubric>,
    #[serde(rename = "weights")]
    weights_lang: Option<LangWeights>,
}

#[derive(Debug, Default, Deserialize)]
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
    rust: Option<Rubric>,
    #[serde(default)]
    python: Option<Rubric>,
    #[serde(default)]
    go: Option<Rubric>,
    #[serde(default)]
    javascript: Option<Rubric>,
}

#[derive(Debug, Default, Deserialize)]
struct TomlConfig {
    #[serde(default)]
    weights: Option<LangWeights>,
}

impl Rubric {
    /// 後方互換: グローバルルーブリックを読む（旧 API）。
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

        let base_default = Rubric::default();
        let default_rubric = parsed.weights.as_ref().map(|w| Rubric {
            correctness: w.correctness.unwrap_or(base_default.correctness),
            security: w.security.unwrap_or(base_default.security),
            performance: w.performance.unwrap_or(base_default.performance),
            maintainability: w.maintainability.unwrap_or(base_default.maintainability),
            resource_usage: w.resource_usage.unwrap_or(base_default.resource_usage),
        }).unwrap_or(base_default);

        let (rust, python, go, javascript) = parsed.weights.map(|w| {
            (w.rust, w.python, w.go, w.javascript)
        }).unwrap_or_default();

        Ok(Config {
            default: default_rubric,
            rust,
            python,
            go,
            javascript,
        })
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
        // rust falls back to default
        assert_eq!(c.rubric_for("rust").correctness, 50.0);
    }

    #[test]
    fn rubric_for_unknown_lang_returns_default() {
        let c = Config::default();
        assert_eq!(c.rubric_for("cobol").correctness, 50.0);
    }
}
