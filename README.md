# tb-mail

CLI to list and read local Thunderbird IMAP mail (mbox). JSON-first output for automation and AI agents.

## Install

```bash
cargo install --path .   # or clone and run from repo root
```

Requires `tb-mail` on your `PATH` (e.g. `~/.cargo/bin`).

## Usage

```bash
tb-mail folders
tb-mail list --limit 20
tb-mail read --last
tb-mail read --seq 42 --max-body 10000
```

Optional env: `TB_PROFILE` (Thunderbird profile dir), `TB_ACCOUNT` (ImapMail account folder name).

Defaults on macOS: active profile from `~/Library/Thunderbird/profiles.ini`, folder `INBOX`.

## License

MIT OR Apache-2.0 — see repository defaults; add `LICENSE` if you need a formal grant.
