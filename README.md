# meili

[![ci](https://github.com/sonirico/meili/actions/workflows/ci.yml/badge.svg)](https://github.com/sonirico/meili/actions/workflows/ci.yml)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Pronounced like 美丽 (měilì) in Mandarin, roughly "may-lee". It means
"beautiful", and it sounds like "mail".

Gmail from the terminal. A k9s-style TUI with vim keys for day-to-day
reading, and a non-interactive command mode for the things a TUI is bad
at: emptying a label, trashing everything that matches a query, pulling
every attachment a sender ever mailed you, sending a file from a script.

```
meili                                   # the TUI
meili mail delete --label newsletters   # 312 messages trashed
meili mail attachments --query 'from:ceo@acme.example filename:pdf' --out ~/reports
```

![The TUI: moving through the inbox, reading a message, saving an attachment](demo/tui.gif)

Gmail is the first provider. Both front ends talk to the `Provider` trait
in `src/provider.rs`, and nothing Gmail-specific leaves `src/gmail.rs`.

## Try it without an account

```
meili --demo
meili --demo mail list --query 'from:kestrel has:attachment'
```

`--demo` swaps Gmail for an invented mailbox that lives in memory. It
needs no login, touches no network, and forgets every change when the
process exits. Every command and every key works. Search understands a
small subset of the Gmail syntax: `from:`, `has:attachment`, `in:trash`
and plain words. Every recording in this README was made with it.

## Install

You need Rust 1.85 or newer (edition 2024).

```
cargo install --path .
```

The binary is `meili`. It keeps its config in `~/.config/meili/` and a
metadata cache in `~/.cache/meili/gmail.sqlite`.

## Login

Google does not hand out a shared OAuth client for terminal apps, so you
bring your own. It takes five minutes and you do it once.

1. Enable the Gmail API: https://console.cloud.google.com/apis/library/gmail.googleapis.com
2. Set up the OAuth consent screen: https://console.cloud.google.com/auth/overview
   Then either publish it ("In production") or add your own address as a
   test user. Unpublished apps get refresh tokens that expire after 7 days.
3. Create an OAuth client of type "Desktop app": https://console.cloud.google.com/auth/clients
4. Download its JSON and save it as `~/.config/meili/client_secret.json`.
5. Run `meili login`. The browser opens, you consent, and the terminal
   prints the account it logged in as.

If you skipped step 2 Google shows "Access blocked: ... has not completed
the verification process". That is the consent screen in Testing mode
without you as a tester. Fix it there, not in meili.

Only `meili login` ever opens a browser. Every other command fails fast
with `not logged in: run 'meili login'` when there is no token, so a
script never hangs waiting for a redirect.

The token asks for the full Gmail scope (`https://mail.google.com/`).
Permanent delete and send need it, and asking once beats asking twice.

## The TUI

```
meili
```

![Labels, search, jumping to a label, archive, trash and delete](demo/organize.gif)

Unread messages are bold. The first column shows one icon per attachment
kind for the first two attachments: image, PDF, document, spreadsheet,
slides, archive, audio, video, calendar, or a paperclip for anything else.

| Key | Does |
| --- | --- |
| `j` `k` | move |
| `gg` `G` | first / last row (`G` loads every remaining page) |
| `C-d` `C-u` `C-f` `C-b` | half page down / up |
| `Enter` | read the message |
| `o` | attachments picker: `Enter` saves to `~/Downloads/meili/` and opens it with the system handler, `s` only saves |
| `a` | archive |
| `d` | trash |
| `D` | delete for good, after `y/n` |
| `l` | label picker; `Enter` or `Space` toggles a label |
| `/` | search with Gmail query syntax; `Esc` goes back to the label |
| `:` | jump to a label, filtering as you type |
| `Tab` `[` `]` | next / previous label |
| `r` `R` | refresh; `R` also drops the cached rows and re-reads the labels |
| `q` `C-c` | quit |

Inside a message `j` `k` scroll, `g` `G` jump to the top or the end, and
`o` `a` `d` `D` `l` act on it. `q` or `Esc` goes back to the list.

The list refreshes itself every 30 seconds while you are on it. The
cursor stays on the same message.

Reading never marks a message as read. Only `D` asks for confirmation,
because everything else is undoable from Trash or by re-labelling.

The TUI calls Gmail synchronously and shows `working...` meanwhile. Pages
of 50 load as you scroll past the last row.

## Command mode

![Command mode: list, read, save attachments, bulk delete, reply](demo/cli.gif)

Every mutating command takes exactly one selection:

| Selection | Meaning |
| --- | --- |
| `--label NAME` | every message under that label, exact name |
| `--query QUERY` | every message matching a Gmail search, passed through verbatim |
| `ID...` | the ids `mail list` printed |

An unknown label is an error, never an empty selection. The one exception
is the label `mail label` adds: it is created when missing. A blank query is
rejected at parse time, because it would select the whole mailbox.

```
meili mail list    [--label NAME | --query QUERY] [--limit N | --all]
meili mail read    ID
meili mail archive (--label NAME | --query QUERY | ID...)
meili mail delete  [--permanent] (--label NAME | --query QUERY | ID...)
meili mail label   LABEL [--remove] (--label NAME | --query QUERY | ID...)
meili mail attachments --out DIR (--label NAME | --query QUERY | ID...)
meili mail send    --to ADDR... --subject S --body FILE|- [--attach FILE...]
meili mail send    --reply-to ID [--to ADDR...] [--subject S] --body FILE|- [--attach FILE...]
```

`list` prints TSV to stdout: `id  date  from  subject`, 50 rows unless
you pass `--limit` or `--all`. Mutations print `N messages trashed` and
the like to stderr and exit 0 even when N is 0, so re-running a cleanup
is a no-op, not a failure. `read` prints the body to stdout and the
attachment names and sizes to stderr. `send` prints the new message id.
With `--reply-to` the mail joins the thread of that message, and goes to
its sender with "Re: " plus its subject unless you pass `--to` or
`--subject`.
A body file ending in `.html` is sent as HTML, anything else as
plain text.

`delete` moves to Trash. `delete --permanent` does what it says.

`attachments` writes each file as `DIR/<id>-<name>`, prints `id  path`
per file, and skips a file that already exists, so re-running it only
fetches what is new.

Queries starting with `-` need the `=` form: `--query='-has:attachment'`.

### Recipes

```sh
# empty a label, then make sure
meili mail delete --label newsletters
meili mail list --label newsletters      # prints nothing

# archive newsletters older than a month
meili mail archive --query 'category:promotions older_than:30d'

# every PDF a sender ever mailed you
meili mail attachments --query 'from:ceo@acme.example has:attachment' --out ~/reports

# read a whole label
meili mail list --label work --all | cut -f1 | xargs -n1 meili mail read

# mail a report from a script
echo 'Attached.' | meili mail send --to boss@acme.example --subject 'Q3 numbers' --body - --attach q3.xlsx

# really empty the trash of one sender
meili mail delete --permanent --query 'in:trash from:brightdeals.example'
```

Spam and Trash are excluded from every selection unless the query names
them (`in:trash`, `in:spam`, `in:anywhere`), exactly like Gmail's search
bar. A bulk action by query therefore never reaches Trash by accident.

## How it talks to Gmail

- Bulk actions collect every id first (500 per request), then mutate in
  chunks of 1000 through `batchModify` / `batchDelete`. Mutating while
  paginating would shift the set and skip messages.
- There are no retries. Every action is idempotent, so on a quota or
  network error you re-run the command and it finishes the job.
- Gmail's list call returns ids only. Subject, sender, date, labels and
  attachment names cost one `messages.get` each, at 5 quota units.
  meili caches them in SQLite, so a listing only fetches ids it has not
  seen, and a second run of the same list is instant. Any mutation drops
  the affected rows.
- The quota is 6000 units per minute per user, about 1200 never-seen
  messages per minute. `meili mail list --all` on a huge label will hit
  it and stop with a one-line error; wait a minute and run it again, the
  cache keeps what it got.

## Layout

```
src/main.rs      clap entry point and wiring
src/cli.rs       command mode
src/tui.rs       the TUI
src/provider.rs  the trait both front ends talk to
src/gmail.rs     the Gmail provider, the only file that knows Gmail types
src/demo.rs      the in-memory provider behind --demo
src/auth.rs      OAuth delegates: open the browser, or refuse to
src/cache.rs     SQLite metadata cache
src/mail.rs      domain types shared by both front ends
```

## Development

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The recordings are scripted with [vhs](https://github.com/charmbracelet/vhs).
The tapes live in `demo/` and run against `--demo`, so they never show a
real mailbox. To record them again, with `meili` on your `PATH`:

```
vhs demo/tui.tape
vhs demo/organize.tape
vhs demo/cli.tape
```

## Not here

Multiple accounts, HTML rendering, CC/BCC, a local search index,
offline mode. Open an issue if one of them is blocking you.

## License

MIT. See [LICENSE](LICENSE).
