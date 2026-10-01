use anyhow::{bail, Context, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const TB_PROFILE_ENV: &str = "TB_PROFILE";
const TB_ACCOUNT_ENV: &str = "TB_ACCOUNT";

pub fn thunderbird_root() -> Result<PathBuf> {
    if let Ok(p) = env::var(TB_PROFILE_ENV) {
        let path = PathBuf::from(p);
        if path.is_dir() {
            return Ok(path);
        }
        bail!("{TB_PROFILE_ENV} is not a directory: {}", path.display());
    }

    let home = dirs_home().context("could not resolve home directory")?;
    let ini = home
        .join("Library/Thunderbird/profiles.ini")
        .canonicalize()
        .context("Thunderbird profiles.ini not found (macOS: ~/Library/Thunderbird/profiles.ini)")?;

    let tb_root = ini
        .parent()
        .context("invalid profiles.ini path")?
        .to_path_buf();

    let content = fs::read_to_string(&ini).context("read profiles.ini")?;
    let profile_rel = active_profile_path(&content).context("parse profiles.ini")?;
    Ok(tb_root.join(profile_rel))
}

pub fn default_account(profile: &Path) -> Result<String> {
    if let Ok(a) = env::var(TB_ACCOUNT_ENV) {
        return Ok(a);
    }
    let imap = profile.join("ImapMail");
    let mut accounts: Vec<String> = fs::read_dir(&imap)
        .with_context(|| format!("read {}", imap.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    accounts.sort();
    accounts
        .into_iter()
        .next()
        .context("no IMAP accounts under ImapMail/")
}

pub fn mbox_path(profile: &Path, account: &str, folder: &str) -> PathBuf {
    profile
        .join("ImapMail")
        .join(account)
        .join(folder)
}

pub fn list_accounts(profile: &Path) -> Result<Vec<AccountFolders>> {
    let imap = profile.join("ImapMail");
    let mut out = Vec::new();
    for entry in fs::read_dir(&imap).with_context(|| format!("read {}", imap.display()))? {
        let entry = entry?;
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("non-utf8 account name"))?;
        let mut folders: Vec<String> = fs::read_dir(entry.path())?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .filter_map(|p| p.file_name()?.to_str().map(|s| s.to_string()))
            .filter(|name| !name.ends_with(".msf") && name != "msgFilterRules.dat" && !name.ends_with(".bak"))
            .collect();
        folders.sort();
        out.push(AccountFolders { account: name, folders });
    }
    out.sort_by(|a, b| a.account.cmp(&b.account));
    Ok(out)
}

#[derive(Debug, serde::Serialize)]
pub struct AccountFolders {
    pub account: String,
    pub folders: Vec<String>,
}

fn dirs_home() -> Result<PathBuf> {
    env::var("HOME")
        .map(PathBuf::from)
        .context("$HOME not set")
}

fn active_profile_path(ini: &str) -> Result<String> {
    let mut default_rel: Option<String> = None;
    let mut install_default: Option<String> = None;
    let mut current_section: Option<&str> = None;
    let mut section_path: Option<String> = None;

    for line in ini.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            if current_section == Some("Profile0") {
                if let Some(p) = section_path.take() {
                    default_rel = Some(p);
                }
            }
            current_section = Some(&line[1..line.len() - 1]);
            section_path = None;
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            let v = v.trim();
            match (current_section, k) {
                (Some(s), "Path") if s.starts_with("Profile") => section_path = Some(v.to_string()),
                (Some("Install712DAF5A23ABECDC"), "Default") => {
                    install_default = Some(v.to_string())
                }
                _ => {}
            }
        }
    }
    if current_section == Some("Profile0") {
        if let Some(p) = section_path.take() {
            default_rel = Some(p);
        }
    }

    install_default
        .or(default_rel)
        .context("no Default/Profile path in profiles.ini")
}
