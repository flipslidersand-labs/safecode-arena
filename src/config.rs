//! 採点ルーブリックの設定。`safecode.toml` から重みを上書きできる。
//!
//! ```toml
//! [weights]
//! correctness = 50
//! security = 20
//! performance = 15
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
        // docs/spec.md の評価軸と一致。
        Rubric {
            correctness: 50.0,
            security: 20.0,
            performance: 15.0,
            maintainability: 10.0,
            resource_usage: 5.0,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    weights: Option<Rubric>,
}

impl Rubric {
    /// `safecode.toml` を読み込む。指定パスがあればそれを、
    /// なければカレントの `safecode.toml` を探す。どちらも無ければ既定値。
    pub fn load(explicit: Option<&str>) -> Result<Rubric> {
        let path = match explicit {
            Some(p) => Some(Path::new(p).to_path_buf()),
            None => {
                let default = Path::new("safecode.toml");
                default.exists().then(|| default.to_path_buf())
            }
        };

        let Some(path) = path else {
            return Ok(Rubric::default());
        };

        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("設定ファイルの読込に失敗: {}", path.display()))?;
        let parsed: ConfigFile = toml::from_str(&text)
            .with_context(|| format!("設定ファイルの解析に失敗: {}", path.display()))?;
        Ok(parsed.weights.unwrap_or_default())
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
}
