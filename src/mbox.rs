use anyhow::{Context, Result};
use mail_parser::MessageParser;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const CHUNK: usize = 8 * 1024 * 1024;
const HEADER_SCAN: u64 = 256 * 1024;

/// Byte offsets of each message in an mbox file (Thunderbird `From ` separator lines).
pub struct MboxIndex {
    pub offsets: Vec<u64>,
}

impl MboxIndex {
    pub fn build(path: &Path) -> Result<Self> {
        let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let file_len = file.metadata()?.len();
        let mut offsets = Vec::new();
        let mut buf = vec![0u8; CHUNK];
        let mut bytes_read: u64 = 0;
        let mut pending: Vec<u8> = Vec::new();

        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }

            let mut combined = pending;
            let combined_base = bytes_read - combined.len() as u64;
            combined.extend_from_slice(&buf[..n]);
            scan_mbox_starts(&combined, combined_base, &mut offsets);

            let keep = combined.len().saturating_sub(6);
            pending = combined[keep..].to_vec();
            bytes_read += n as u64;

            if n < CHUNK {
                break;
            }
        }

        if offsets.is_empty() && file_len > 0 {
            bail_if_not_mbox(path)?;
        }

        Ok(Self { offsets })
    }

    pub fn count(&self) -> usize {
        self.offsets.len()
    }

    pub fn read_raw_message(&self, path: &Path, seq: usize) -> Result<Vec<u8>> {
        let idx = seq.checked_sub(1).context("seq must be >= 1")?;
        let start = *self
            .offsets
            .get(idx)
            .with_context(|| format!("seq {seq} out of range (1..={})", self.count()))?;
        let end = self.offsets.get(idx + 1).copied().unwrap_or_else(|| {
            std::fs::metadata(path).map(|m| m.len()).unwrap_or(start)
        });
        read_range(path, start, end)
    }
}

fn bail_if_not_mbox(path: &Path) -> Result<()> {
    let mut f = File::open(path)?;
    let mut head = [0u8; 5];
    f.read_exact(&mut head)?;
    if &head == b"From " {
        Ok(())
    } else {
        anyhow::bail!("{} does not look like an mbox file", path.display())
    }
}

fn scan_mbox_starts(data: &[u8], base_offset: u64, out: &mut Vec<u64>) {
    if base_offset == 0 && data.starts_with(b"From ") {
        out.push(0);
    }
    for i in 1..data.len().saturating_sub(5) {
        if data[i - 1] == b'\n' && &data[i..][..5] == b"From " {
            out.push(base_offset + i as u64);
        }
    }
}

fn read_range(path: &Path, start: u64, end: u64) -> Result<Vec<u8>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let len = (end - start) as usize;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf)?;
    strip_mbox_from_line(&mut buf);
    Ok(buf)
}

fn strip_mbox_from_line(raw: &mut Vec<u8>) {
    if raw.starts_with(b"From ") {
        if let Some(i) = raw.iter().position(|&b| b == b'\n') {
            raw.drain(..=i);
        }
    }
}

pub struct HeaderSummary {
    pub from: Option<String>,
    pub to: Option<String>,
    pub subject: Option<String>,
    pub date: Option<String>,
    pub message_id: Option<String>,
}

pub fn parse_headers(raw: &[u8]) -> Result<HeaderSummary> {
    let msg = MessageParser::default()
        .parse(raw)
        .context("parse message headers")?;
    Ok(HeaderSummary {
        from: msg.from().and_then(format_address_field),
        to: msg.to().and_then(format_address_field),
        subject: msg.subject().map(|s| s.to_string()),
        date: msg.date().map(|d| d.to_rfc822()),
        message_id: msg.message_id().map(|s| s.to_string()),
    })
}

pub fn read_header_block(path: &Path, start: u64, file_len: u64) -> Result<Vec<u8>> {
    let end = (start + HEADER_SCAN).min(file_len);
    let mut raw = read_range(path, start, end)?;
    if let Some(sep) = find_header_body_sep(&raw) {
        raw.truncate(sep);
    }
    Ok(raw)
}

fn find_header_body_sep(raw: &[u8]) -> Option<usize> {
    raw.windows(4)
        .position(|w| w == b"\r\n\r\n" || w == b"\n\n")
        .map(|i| i + if raw[i..].starts_with(b"\r\n\r\n") { 4 } else { 2 })
}

fn format_address_field(field: &mail_parser::Address<'_>) -> Option<String> {
    let parts: Vec<String> = match field {
        mail_parser::Address::List(addrs) => addrs.iter().map(format_addr).collect(),
        mail_parser::Address::Group(groups) => groups
            .iter()
            .flat_map(|g| g.addresses.iter().map(format_addr))
            .collect(),
    };
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

fn format_addr(addr: &mail_parser::Addr<'_>) -> String {
    let email = addr.address.as_deref().unwrap_or("");
    if let Some(name) = addr.name.as_deref() {
        if email.is_empty() {
            name.to_string()
        } else {
            format!("{name} <{email}>")
        }
    } else {
        email.to_string()
    }
}

pub struct ParsedBody {
    pub text_plain: Option<String>,
    pub text_html: Option<String>,
}

pub fn parse_body(raw: &[u8], include_html: bool) -> Result<ParsedBody> {
    let msg = MessageParser::default()
        .parse(raw)
        .context("parse full message")?;
    let text_plain = msg.body_text(0).map(|s| s.to_string());
    let text_html = if include_html {
        msg.body_html(0).map(|s| s.to_string())
    } else {
        None
    };
    Ok(ParsedBody {
        text_plain,
        text_html,
    })
}

pub fn truncate(s: Option<String>, max: usize) -> Option<String> {
    s.map(|mut t| {
        if t.len() > max {
            t.truncate(max);
            t.push_str("… [truncated]");
        }
        t
    })
}
