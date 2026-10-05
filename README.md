# tb-mail

CLI to list and read local Thunderbird IMAP mail (mbox). JSON-first output for automation and AI agents.

## Install

```bash
cargo install --path . --force
```

Requires `tb-mail` on your `PATH` (e.g. `~/.cargo/bin`).

## Usage

```bash
tb-mail use foobar@gmail.com   # set default account
tb-mail use                               # show current default
tb-mail folders
tb-mail list --limit 20
tb-mail read --last
tb-mail read --seq 42 --max-body 10000
```

### Selecting an account

**`tb-mail use EMAIL`** saves the default to `~/.config/tb-mail/config.toml` (editable by hand). After that, `list` / `read` use that account unless overridden.

```bash
tb-mail use foobar@gmail.com
tb-mail list --limit 5
tb-mail use foobar@test.com
tb-mail list --folder Sent --limit 5
tb-mail use --clear
```

One-off override with **`--email`** (takes precedence over the saved default):

```bash
tb-mail list --email foobar@gmail.com --limit 5
```

Low-level alternative: `--account` = folder name under `ImapMail/` (e.g. `imap.gmail.com`, `outlook.office365.com`).

### Nested folders (Gmail)

Use `/` for nested folders (Thunderbird `.sbd` on disk):

```bash
tb-mail list --email foobar@gmail.com --folder "[Gmail]/Sent Mail" --limit 5
```

### Config file

`~/.config/tb-mail/config.toml` (or `$XDG_CONFIG_HOME/tb-mail/config.toml`):

```toml
# tb-mail — edit or run: tb-mail use ADDRESS
email = "foobar@gmail.com"
```

### Environment

| Variable | Purpose |
|----------|---------|
| `TB_PROFILE` | Thunderbird profile directory |
| `TB_EMAIL` | Account email (same as `--email`) |
| `TB_ACCOUNT` | ImapMail host folder (same as `--account`) |

Precedence: `--email` / `TB_EMAIL` → `--account` / `TB_ACCOUNT` → saved `tb-mail use` / config.toml → first IMAP account.

Defaults on macOS: active profile from `~/Library/Thunderbird/profiles.ini`, folder `INBOX`.

## License

MIT OR Apache-2.0 — see repository defaults; add `LICENSE` if you need a formal grant.
