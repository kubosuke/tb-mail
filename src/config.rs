use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    /// Default account email for list/read (set via `tb-mail use`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

pub fn config_dir() -> Result<PathBuf> {
    if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Ok(PathBuf::from(xdg).join("tb-mail"));
        }
    }
    let home = env::var("HOME").context("$HOME not set")?;
    Ok(PathBuf::from(home).join(".config/tb-mail"))
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn load() -> Result<Config> {
    let path = config_path()?;
    if !path.is_file() {
        // One-time migration from earlier JSON path, if present.
        let legacy = config_dir()?.join("config.json");
        if legacy.is_file() {
            let raw = fs::read_to_string(&legacy)
                .with_context(|| format!("read {}", legacy.display()))?;
            let cfg: Config = serde_json::from_str(&raw)
                .with_context(|| format!("parse {}", legacy.display()))?;
            let _ = save(&cfg);
            let _ = fs::remove_file(&legacy);
            return Ok(cfg);
        }
        return Ok(Config::default());
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

pub fn save(cfg: &Config) -> Result<PathBuf> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let body = match &cfg.email {
        Some(email) => format!(
            "# tb-mail — edit or run: tb-mail use ADDRESS\nemail = \"{}\"\n",
            escape_toml_string(email)
        ),
        None => "# tb-mail — no default email\n# email = \"you@example.com\"\n".to_string(),
    };
    fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

pub fn set_default_email(email: &str) -> Result<PathBuf> {
    let email = email.trim();
    if email.is_empty() {
        bail!("email is empty");
    }
    let mut cfg = load()?;
    cfg.email = Some(email.to_string());
    save(&cfg)
}

pub fn clear_default_email() -> Result<PathBuf> {
    let mut cfg = load()?;
    cfg.email = None;
    save(&cfg)
}

pub fn default_email() -> Option<String> {
    load().ok().and_then(|c| c.email)
}

fn escape_toml_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_toml_shape() {
        let raw = "email = \"a@b.com\"\n";
        let cfg: Config = toml::from_str(raw).unwrap();
        assert_eq!(cfg.email.as_deref(), Some("a@b.com"));
    }
}
