mod config;
mod mbox;
mod profile;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use mbox::{truncate, MboxIndex, ParsedBody};
use profile::{
    account_for_email, default_account, list_accounts, mbox_path, thunderbird_root,
};
use serde::Serialize;
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "tb-mail", about = "List/read Thunderbird local mbox mail (JSON-first, for agents)")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Thunderbird profile directory (default: TB_PROFILE or active profile from profiles.ini)
    #[arg(long, env = "TB_PROFILE", global = true)]
    profile: Option<PathBuf>,

    /// Email address of the Thunderbird account (from prefs.js; env: TB_EMAIL). Takes precedence over --account.
    #[arg(long, env = "TB_EMAIL", global = true)]
    email: Option<String>,

    /// IMAP account host folder under ImapMail/ (env: TB_ACCOUNT; default: first account)
    #[arg(long, env = "TB_ACCOUNT", global = true)]
    account: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Set or show the default account email (saved in ~/.config/tb-mail/config.toml)
    Use {
        /// Email to make the default (omit to show current default)
        email: Option<String>,
        /// Clear the saved default email
        #[arg(long)]
        clear: bool,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        format: OutputFormat,
    },
    /// List folders for each IMAP account
    Folders {
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        format: OutputFormat,
    },
    /// List messages (newest last in output)
    List {
        #[arg(long, default_value = "INBOX")]
        folder: String,
        /// How many messages to return (from the end / newest)
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        format: OutputFormat,
    },
    /// Read one message by sequence number (1 = oldest in folder)
    Read {
        #[arg(long, default_value = "INBOX")]
        folder: String,
        /// Message sequence number (see `list`)
        #[arg(long, group = "pick")]
        seq: Option<usize>,
        /// Read the newest message in the folder
        #[arg(long, group = "pick")]
        last: bool,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        format: OutputFormat,
        /// Max characters for text/plain and text/html bodies
        #[arg(long, default_value_t = 100_000)]
        max_body: usize,
        /// Include text/html part (can be large)
        #[arg(long, default_value_t = false)]
        html: bool,
    },
}

#[derive(Clone, Copy, ValueEnum, Default)]
enum OutputFormat {
    #[default]
    Json,
    Text,
}

fn resolve_profile(cli: &Cli) -> Result<PathBuf> {
    if let Some(p) = &cli.profile {
        return Ok(p.clone());
    }
    thunderbird_root()
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Use {
            ref email,
            clear,
            format,
        } => cmd_use(&cli, email.as_deref(), clear, format),
        Command::Folders { format } => cmd_folders(&cli, format),
        Command::List {
            ref folder,
            limit,
            format,
        } => cmd_list(&cli, folder, limit, format),
        Command::Read {
            ref folder,
            seq,
            last,
            format,
            max_body,
            html,
        } => cmd_read(&cli, folder, seq, last, format, max_body, html),
    }
}

#[derive(Serialize)]
struct UseResponse {
    email: Option<String>,
    account: Option<String>,
    config: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    known: Option<Vec<String>>,
}

fn cmd_use(cli: &Cli, email: Option<&str>, clear: bool, format: OutputFormat) -> Result<()> {
    if clear && email.is_some() {
        anyhow::bail!("use either EMAIL or --clear, not both");
    }

    let profile = resolve_profile(cli)?;
    let path = if clear {
        config::clear_default_email()?
    } else if let Some(email) = email {
        // Validate against Thunderbird prefs before saving.
        let account = account_for_email(&profile, email)?;
        let path = config::set_default_email(email)?;
        let resp = UseResponse {
            email: Some(email.to_string()),
            account: Some(account),
            config: path.display().to_string(),
            known: None,
        };
        return print_use(resp, format);
    } else {
        config::config_path()?
    };

    let saved = config::default_email();
    let account = saved
        .as_deref()
        .and_then(|e| account_for_email(&profile, e).ok());
    let known = list_accounts(&profile)?
        .into_iter()
        .filter_map(|a| a.email)
        .collect::<Vec<_>>();

    print_use(
        UseResponse {
            email: saved,
            account,
            config: path.display().to_string(),
            known: Some(known),
        },
        format,
    )
}

fn print_use(resp: UseResponse, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&resp)?),
        OutputFormat::Text => match &resp.email {
            Some(email) => {
                if let Some(account) = &resp.account {
                    println!("{email} ({account})");
                } else {
                    println!("{email}");
                }
            }
            None => println!("(no default email; use: tb-mail use ADDRESS)"),
        },
    }
    Ok(())
}

#[derive(Serialize)]
struct FolderAccount {
    account: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    default: bool,
    folders: Vec<String>,
}

fn cmd_folders(cli: &Cli, format: OutputFormat) -> Result<()> {
    let profile = resolve_profile(cli)?;
    let accounts = list_accounts(&profile)?;
    let default_email = config::default_email();
    let rows: Vec<FolderAccount> = accounts
        .into_iter()
        .map(|a| {
            let is_default = match (&default_email, &a.email) {
                (Some(d), Some(e)) => d.eq_ignore_ascii_case(e),
                _ => false,
            };
            FolderAccount {
                account: a.account,
                email: a.email,
                default: is_default,
                folders: a.folders,
            }
        })
        .collect();

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&rows)?);
        }
        OutputFormat::Text => {
            for a in &rows {
                let mark = if a.default { "*" } else { " " };
                match &a.email {
                    Some(email) => println!("{mark} {} ({email})", a.account),
                    None => println!("{mark} {}", a.account),
                }
                for f in &a.folders {
                    println!("  {f}");
                }
            }
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct ListResponse {
    profile: String,
    account: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    folder: String,
    total: usize,
    messages: Vec<ListItem>,
}

#[derive(Serialize)]
struct ListItem {
    seq: usize,
    from: Option<String>,
    to: Option<String>,
    subject: Option<String>,
    date: Option<String>,
    message_id: Option<String>,
}

fn cmd_list(cli: &Cli, folder: &str, limit: usize, format: OutputFormat) -> Result<()> {
    let profile = resolve_profile(cli)?;
    let ResolvedAccount { account, email } = resolve_account(cli, &profile)?;
    let path = mbox_path(&profile, &account, folder);
    if !path.is_file() {
        anyhow::bail!("mbox not found: {}", path.display());
    }

    let index = MboxIndex::build(&path)?;
    let total = index.count();
    let file_len = fs::metadata(&path)?.len();
    let start_seq = total.saturating_sub(limit).max(0) + 1;

    let mut messages = Vec::new();
    for seq in start_seq..=total {
        let off = index.offsets[seq - 1];
        let header_raw = mbox::read_header_block(&path, off, file_len)?;
        let h = mbox::parse_headers(&header_raw)?;
        messages.push(ListItem {
            seq,
            from: h.from,
            to: h.to,
            subject: h.subject,
            date: h.date,
            message_id: h.message_id,
        });
    }

    match format {
        OutputFormat::Json => {
            let resp = ListResponse {
                profile: profile.display().to_string(),
                account,
                email,
                folder: folder.to_string(),
                total,
                messages,
            };
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            for m in &messages {
                println!(
                    "[{}] {} | {} | {}",
                    m.seq,
                    m.date.as_deref().unwrap_or(""),
                    m.from.as_deref().unwrap_or(""),
                    m.subject.as_deref().unwrap_or("")
                );
            }
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct ReadResponse {
    profile: String,
    account: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    folder: String,
    seq: usize,
    total: usize,
    from: Option<String>,
    to: Option<String>,
    subject: Option<String>,
    date: Option<String>,
    message_id: Option<String>,
    text_plain: Option<String>,
    text_html: Option<String>,
}

fn cmd_read(
    cli: &Cli,
    folder: &str,
    seq: Option<usize>,
    last: bool,
    format: OutputFormat,
    max_body: usize,
    html: bool,
) -> Result<()> {
    if seq.is_some() && last {
        anyhow::bail!("use either --seq or --last, not both");
    }
    if seq.is_none() && !last {
        anyhow::bail!("specify --seq N or --last");
    }

    let profile = resolve_profile(cli)?;
    let ResolvedAccount { account, email } = resolve_account(cli, &profile)?;
    let path = mbox_path(&profile, &account, folder);
    let index = MboxIndex::build(&path)?;
    let total = index.count();
    let seq = if last { total } else { seq.unwrap() };
    if seq == 0 || seq > total {
        anyhow::bail!("seq {seq} out of range (1..={total})");
    }

    let raw = index.read_raw_message(&path, seq)?;
    let headers = mbox::parse_headers(&raw)?;
    let ParsedBody {
        text_plain,
        text_html,
    } = mbox::parse_body(&raw, html)?;

    let text_plain = truncate(text_plain, max_body);
    let text_html = truncate(text_html, max_body);

    match format {
        OutputFormat::Json => {
            let resp = ReadResponse {
                profile: profile.display().to_string(),
                account,
                email,
                folder: folder.to_string(),
                seq,
                total,
                from: headers.from,
                to: headers.to,
                subject: headers.subject,
                date: headers.date,
                message_id: headers.message_id,
                text_plain,
                text_html,
            };
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            println!("From: {}", headers.from.as_deref().unwrap_or(""));
            println!("To: {}", headers.to.as_deref().unwrap_or(""));
            println!("Subject: {}", headers.subject.as_deref().unwrap_or(""));
            println!("Date: {}", headers.date.as_deref().unwrap_or(""));
            println!();
            if let Some(t) = text_plain {
                println!("{t}");
            }
        }
    }
    Ok(())
}

struct ResolvedAccount {
    account: String,
    email: Option<String>,
}

fn resolve_account(cli: &Cli, profile: &PathBuf) -> Result<ResolvedAccount> {
    let accounts = list_accounts(profile)?;
    let email_for = |account: &str| -> Option<String> {
        accounts
            .iter()
            .find(|a| a.account == account)
            .and_then(|a| a.email.clone())
    };

    // Precedence: --email / TB_EMAIL → --account / TB_ACCOUNT → saved `tb-mail use` → first account
    if let Some(email) = &cli.email {
        let account = account_for_email(profile, email)?;
        return Ok(ResolvedAccount {
            email: Some(email.clone()),
            account,
        });
    }
    if let Some(a) = &cli.account {
        return Ok(ResolvedAccount {
            email: email_for(a),
            account: a.clone(),
        });
    }
    if let Some(email) = config::default_email() {
        let account = account_for_email(profile, &email)?;
        return Ok(ResolvedAccount {
            email: Some(email),
            account,
        });
    }
    let account = default_account(profile)?;
    Ok(ResolvedAccount {
        email: email_for(&account),
        account,
    })
}
