mod mbox;
mod profile;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use mbox::{truncate, MboxIndex, ParsedBody};
use profile::{list_accounts, default_account, mbox_path, thunderbird_root};
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

    /// IMAP account host folder under ImapMail/ (default: TB_ACCOUNT or first account)
    #[arg(long, env = "TB_ACCOUNT", global = true)]
    account: Option<String>,
}

#[derive(Subcommand)]
enum Command {
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

fn cmd_folders(cli: &Cli, format: OutputFormat) -> Result<()> {
    let profile = resolve_profile(cli)?;
    let accounts = list_accounts(&profile)?;
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&accounts)?);
        }
        OutputFormat::Text => {
            for a in &accounts {
                println!("{}", a.account);
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
    let account = resolve_account(cli, &profile)?;
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
    let account = resolve_account(cli, &profile)?;
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

fn resolve_account(cli: &Cli, profile: &PathBuf) -> Result<String> {
    if let Some(a) = &cli.account {
        return Ok(a.clone());
    }
    default_account(profile)
}
