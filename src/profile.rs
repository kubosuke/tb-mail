use anyhow::{bail, Context, Result};
use std::collections::HashMap;
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
        .context(
            "Thunderbird profiles.ini not found (macOS: ~/Library/Thunderbird/profiles.ini)",
        )?;

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
    let accounts = list_accounts(profile)?;
    accounts
        .into_iter()
        .next()
        .map(|a| a.account)
        .context("no IMAP accounts under ImapMail/")
}

/// Resolve ImapMail account folder from an email address (prefs.js identities / server userName).
pub fn account_for_email(profile: &Path, email: &str) -> Result<String> {
    let want = email.trim().to_ascii_lowercase();
    if want.is_empty() {
        bail!("email is empty");
    }

    let accounts = list_accounts(profile)?;
    let matches: Vec<&AccountFolders> = accounts
        .iter()
        .filter(|a| {
            a.email
                .as_deref()
                .map(|e| e.eq_ignore_ascii_case(&want))
                .unwrap_or(false)
        })
        .collect();

    match matches.as_slice() {
        [one] => Ok(one.account.clone()),
        [] => {
            let known: Vec<String> = accounts.iter().filter_map(|a| a.email.clone()).collect();
            if known.is_empty() {
                bail!("no account matched email {email:?}; could not read emails from prefs.js");
            }
            bail!(
                "no account matched email {email:?}; known: {}",
                known.join(", ")
            );
        }
        many => {
            let names: Vec<&str> = many.iter().map(|a| a.account.as_str()).collect();
            bail!(
                "email {email:?} matched multiple accounts: {}",
                names.join(", ")
            );
        }
    }
}

/// Thunderbird stores nested folders as `Parent.sbd/Child` on disk.
/// CLI folder names use `/` (e.g. `[Gmail]/Sent Mail`).
pub fn mbox_path(profile: &Path, account: &str, folder: &str) -> PathBuf {
    let mut path = profile.join("ImapMail").join(account);
    let parts: Vec<&str> = folder.split('/').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        [] => path,
        [name] => path.join(name),
        [parents @ .., name] => {
            for p in parents {
                path = path.join(format!("{p}.sbd"));
            }
            path.join(name)
        }
    }
}

pub fn list_accounts(profile: &Path) -> Result<Vec<AccountFolders>> {
    let email_by_dir = emails_by_imap_dir(profile).unwrap_or_default();
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
        let mut folders = Vec::new();
        collect_folders(&entry.path(), "", &mut folders)?;
        folders.sort();
        let email = email_by_dir.get(&name).cloned();
        out.push(AccountFolders {
            account: name,
            email,
            folders,
        });
    }
    out.sort_by(|a, b| a.account.cmp(&b.account));
    Ok(out)
}

#[derive(Debug, serde::Serialize)]
pub struct AccountFolders {
    pub account: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub folders: Vec<String>,
}

fn collect_folders(dir: &Path, prefix: &str, out: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let fname = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };

        if path.is_dir() {
            if let Some(parent) = fname.strip_suffix(".sbd") {
                let nested = if prefix.is_empty() {
                    parent.to_string()
                } else {
                    format!("{prefix}/{parent}")
                };
                collect_folders(&path, &nested, out)?;
            }
            continue;
        }

        if !path.is_file() {
            continue;
        }
        if fname.ends_with(".msf") || fname == "msgFilterRules.dat" || fname.ends_with(".bak") {
            continue;
        }
        // Skip empty placeholder files that are not mbox bodies (e.g. Notes.msf companions already filtered)
        let folder = if prefix.is_empty() {
            fname
        } else {
            format!("{prefix}/{fname}")
        };
        out.push(folder);
    }
    Ok(())
}

/// Map ImapMail directory basename → email from prefs.js.
fn emails_by_imap_dir(profile: &Path) -> Result<HashMap<String, String>> {
    let prefs_path = profile.join("prefs.js");
    let content = fs::read_to_string(&prefs_path)
        .with_context(|| format!("read {}", prefs_path.display()))?;
    Ok(parse_email_map(&content))
}

fn parse_email_map(prefs: &str) -> HashMap<String, String> {
    // serverN → (directory basename under ImapMail, userName/name)
    let mut server_dir: HashMap<String, String> = HashMap::new();
    let mut server_user: HashMap<String, String> = HashMap::new();
    let mut server_type: HashMap<String, String> = HashMap::new();
    let mut identity_email: HashMap<String, String> = HashMap::new();
    // accountN → serverN
    let mut account_server: HashMap<String, String> = HashMap::new();
    // accountN → idN (may be comma-separated)
    let mut account_identities: HashMap<String, String> = HashMap::new();

    for line in prefs.lines() {
        let line = line.trim();
        let Some((key, value)) = parse_user_pref(line) else {
            continue;
        };

        if let Some(rest) = key.strip_prefix("mail.server.") {
            if let Some((server, field)) = rest.split_once('.') {
                match field {
                    "directory-rel" => {
                        if let Some(dir) = imap_dir_from_directory_rel(&value) {
                            server_dir.insert(server.to_string(), dir);
                        }
                    }
                    "directory" => {
                        if !server_dir.contains_key(server) {
                            if let Some(dir) = imap_dir_from_absolute(&value) {
                                server_dir.insert(server.to_string(), dir);
                            }
                        }
                    }
                    "userName" | "name" => {
                        server_user.entry(server.to_string()).or_insert(value);
                    }
                    "type" => {
                        server_type.insert(server.to_string(), value);
                    }
                    _ => {}
                }
            }
        } else if let Some(rest) = key.strip_prefix("mail.identity.") {
            if let Some((id, field)) = rest.split_once('.') {
                if field == "useremail" {
                    identity_email.insert(id.to_string(), value);
                }
            }
        } else if let Some(rest) = key.strip_prefix("mail.account.") {
            if let Some((acct, field)) = rest.split_once('.') {
                match field {
                    "server" => {
                        account_server.insert(acct.to_string(), value);
                    }
                    "identities" => {
                        account_identities.insert(acct.to_string(), value);
                    }
                    _ => {}
                }
            }
        }
    }

    let mut out: HashMap<String, String> = HashMap::new();

    // Prefer identity emails linked to each IMAP account.
    for (acct, server) in &account_server {
        if server_type.get(server).map(|t| t.as_str()) != Some("imap") {
            continue;
        }
        let Some(dir) = server_dir.get(server) else {
            continue;
        };
        if let Some(ids) = account_identities.get(acct) {
            for id in ids.split(',') {
                let id = id.trim();
                if let Some(email) = identity_email.get(id) {
                    out.insert(dir.clone(), email.clone());
                    break;
                }
            }
        }
        if !out.contains_key(dir) {
            if let Some(user) = server_user.get(server) {
                if user.contains('@') {
                    out.insert(dir.clone(), user.clone());
                }
            }
        }
    }

    // Also map any remaining IMAP servers by userName.
    for (server, dir) in &server_dir {
        if out.contains_key(dir) {
            continue;
        }
        if server_type.get(server).map(|t| t.as_str()) != Some("imap") {
            continue;
        }
        if let Some(user) = server_user.get(server) {
            if user.contains('@') {
                out.insert(dir.clone(), user.clone());
            }
        }
    }

    out
}

fn parse_user_pref(line: &str) -> Option<(String, String)> {
    // user_pref("key", "value");
    // user_pref("key", 123);
    // user_pref("key", true);
    let line = line.strip_prefix("user_pref(")?.strip_suffix(");")?.trim();
    let mut parts = line.splitn(2, ',');
    let key_raw = parts.next()?.trim();
    let val_raw = parts.next()?.trim();
    let key = unquote(key_raw)?;
    let value = if let Some(s) = unquote(val_raw) {
        s
    } else {
        val_raw.to_string()
    };
    Some((key, value))
}

fn unquote(s: &str) -> Option<String> {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let inner = &s[1..s.len() - 1];
        // prefs.js uses \" and \\ escapes lightly; emails / paths rarely need more.
        Some(inner.replace("\\\"", "\"").replace("\\\\", "\\"))
    } else {
        None
    }
}

fn imap_dir_from_directory_rel(rel: &str) -> Option<String> {
    // [ProfD]ImapMail/outlook.office365.com
    let rest = rel.strip_prefix("[ProfD]")?;
    let path = rest.trim_start_matches('/');
    let mut parts = path.split('/');
    if parts.next()? != "ImapMail" {
        return None;
    }
    let dir = parts.next()?;
    if dir.is_empty() || parts.next().is_some() {
        return None;
    }
    Some(dir.to_string())
}

fn imap_dir_from_absolute(path: &str) -> Option<String> {
    let p = Path::new(path);
    let name = p.file_name()?.to_str()?;
    let parent = p.parent()?.file_name()?.to_str()?;
    if parent == "ImapMail" {
        Some(name.to_string())
    } else {
        None
    }
}

fn dirs_home() -> Result<PathBuf> {
    env::var("HOME").map(PathBuf::from).context("$HOME not set")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_email_map_from_prefs() {
        let prefs = r#"
user_pref("mail.account.account1.identities", "id1");
user_pref("mail.account.account1.server", "server2");
user_pref("mail.account.account3.identities", "id2");
user_pref("mail.account.account3.server", "server4");
user_pref("mail.identity.id1.useremail", "foobar@test.com");
user_pref("mail.identity.id2.useremail", "foobar@gmail.com");
user_pref("mail.server.server2.directory-rel", "[ProfD]ImapMail/outlook.office365.com");
user_pref("mail.server.server2.type", "imap");
user_pref("mail.server.server2.userName", "foobar@test.com");
user_pref("mail.server.server4.directory-rel", "[ProfD]ImapMail/imap.gmail.com");
user_pref("mail.server.server4.type", "imap");
user_pref("mail.server.server4.userName", "foobar@gmail.com");
user_pref("mail.server.server3.directory-rel", "[ProfD]Mail/Local Folders");
user_pref("mail.server.server3.type", "none");
"#;
        let map = parse_email_map(prefs);
        assert_eq!(
            map.get("outlook.office365.com").map(String::as_str),
            Some("foobar@test.com")
        );
        assert_eq!(
            map.get("imap.gmail.com").map(String::as_str),
            Some("foobar@gmail.com")
        );
        assert!(!map.contains_key("Local Folders"));
    }

    #[test]
    fn mbox_path_nested_sbd() {
        let profile = Path::new("/tmp/profile");
        assert_eq!(
            mbox_path(profile, "imap.gmail.com", "INBOX"),
            PathBuf::from("/tmp/profile/ImapMail/imap.gmail.com/INBOX")
        );
        assert_eq!(
            mbox_path(profile, "imap.gmail.com", "[Gmail]/Sent Mail"),
            PathBuf::from("/tmp/profile/ImapMail/imap.gmail.com/[Gmail].sbd/Sent Mail")
        );
    }
}
