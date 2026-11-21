use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};

use crate::gmail::resolve;
use crate::mail::{Action, Label, Selection};
use crate::provider::Provider;

#[derive(Args)]
#[group(required = true, multiple = false)]
pub struct SelArgs {
    /// Every message under this label (exact name)
    #[arg(long)]
    label: Option<String>,
    /// Every message matching this Gmail search query
    #[arg(long, value_parser = non_blank)]
    query: Option<String>,
    /// Explicit message ids, as printed by `mail list`
    ids: Vec<String>,
}

#[derive(Args)]
pub struct ListArgs {
    /// Messages under this label (exact name) [default: INBOX]
    #[arg(long, conflicts_with = "query")]
    label: Option<String>,
    /// Messages matching this Gmail search query
    #[arg(long, value_parser = non_blank)]
    query: Option<String>,
    /// Stop after this many messages
    #[arg(long, default_value_t = 50, conflicts_with = "all")]
    limit: usize,
    /// Print the whole selection
    #[arg(long)]
    all: bool,
}

#[derive(Subcommand)]
pub enum MailCmd {
    /// Print id, date, from and subject as TSV, one message per line
    List(ListArgs),
    /// Print the body text of one message; attachments are listed on stderr
    Read { id: String },
    /// Remove the selection from the inbox
    Archive(SelArgs),
    /// Move the selection to Trash
    Delete {
        /// Delete for good instead of moving to Trash
        #[arg(long)]
        permanent: bool,
        #[command(flatten)]
        sel: SelArgs,
    },
    /// Add a label to the selection, creating the label if it does not exist
    Label {
        /// Label to add (exact name)
        #[arg(id = "name", value_name = "LABEL")]
        label: String,
        /// Remove the label instead of adding it
        #[arg(long)]
        remove: bool,
        #[command(flatten)]
        sel: SelArgs,
    },
    /// Send a mail from the logged-in account (HTML when the body file ends in .html); prints the new message id
    Send {
        /// Recipient; repeat for several. Defaults to the sender of --reply-to
        #[arg(long, required_unless_present = "reply_to")]
        to: Vec<String>,
        /// Defaults to "Re: " plus the subject of --reply-to
        #[arg(long, required_unless_present = "reply_to")]
        subject: Option<String>,
        /// Message to answer; the mail joins its thread
        #[arg(long, value_name = "ID")]
        reply_to: Option<String>,
        /// File with the body text, or '-' for stdin
        #[arg(long)]
        body: PathBuf,
        /// File to attach; repeat for several
        #[arg(long)]
        attach: Vec<PathBuf>,
    },
    /// Save every attachment of the selection as OUT/<id>-<name>; prints id<TAB>path
    Attachments {
        /// Directory to write into (created if missing)
        #[arg(long)]
        out: PathBuf,
        #[command(flatten)]
        sel: SelArgs,
    },
}

enum Sel {
    Label(String),
    Query(String),
    Ids(Vec<String>),
}

struct Plan {
    action: Action,
    sel: Sel,
    verb: &'static str,
}

fn non_blank(s: &str) -> Result<String, String> {
    if s.trim().is_empty() {
        Err("query must not be blank".to_string())
    } else {
        Ok(s.to_string())
    }
}

impl From<SelArgs> for Sel {
    fn from(a: SelArgs) -> Sel {
        match (a.label, a.query) {
            (Some(l), _) => Sel::Label(l),
            (_, Some(q)) => Sel::Query(q),
            _ => Sel::Ids(a.ids),
        }
    }
}

fn plan(cmd: MailCmd, labels: &[Label]) -> Result<Plan> {
    Ok(match cmd {
        MailCmd::Archive(sel) => Plan {
            action: Action::Archive,
            sel: sel.into(),
            verb: "archived",
        },
        MailCmd::Delete {
            permanent: true,
            sel,
        } => Plan {
            action: Action::Delete,
            sel: sel.into(),
            verb: "deleted",
        },
        MailCmd::Delete {
            permanent: false,
            sel,
        } => Plan {
            action: Action::Trash,
            sel: sel.into(),
            verb: "trashed",
        },
        MailCmd::Label { label, remove, sel } => Plan {
            action: Action::Label {
                id: resolve(labels, &label)?,
                remove,
            },
            sel: sel.into(),
            verb: if remove { "unlabelled" } else { "labelled" },
        },
        MailCmd::List(_)
        | MailCmd::Read { .. }
        | MailCmd::Attachments { .. }
        | MailCmd::Send { .. } => {
            unreachable!("not a mutation")
        }
    })
}

pub async fn run(client: &impl Provider, cmd: MailCmd) -> Result<()> {
    match cmd {
        MailCmd::List(args) => list(client, args).await,
        MailCmd::Read { id } => {
            let msg = client.message(&id).await?;
            println!("{}", msg.body);
            for a in &msg.attachments {
                eprintln!("attachment: {} ({})", a.name, human_size(a.data.len()));
            }
            Ok(())
        }
        MailCmd::Attachments { out, sel } => attachments(client, &out, sel.into()).await,
        MailCmd::Send {
            to,
            subject,
            reply_to,
            body,
            attach,
        } => send(client, to, subject, reply_to.as_deref(), &body, &attach).await,
        cmd => mutate(client, cmd).await,
    }
}

async fn list(client: &impl Provider, args: ListArgs) -> Result<()> {
    let sel = match (args.label, args.query) {
        (Some(name), _) => Selection::Label(resolve(&client.labels().await?, &name)?),
        (_, Some(q)) => Selection::Query(q),
        _ => Selection::Label("INBOX".to_string()),
    };
    let limit = if args.all { usize::MAX } else { args.limit };
    let mut printed = 0;
    let mut token = None;
    loop {
        let page = client.page(&sel, token.as_deref()).await?;
        for s in page.items {
            if printed == limit {
                return Ok(());
            }
            println!(
                "{}\t{}\t{}\t{}",
                s.id,
                tsv(&s.date),
                tsv(&s.from),
                tsv(&s.subject)
            );
            printed += 1;
        }
        match page.next {
            Some(_) if printed < limit => token = page.next,
            _ => return Ok(()),
        }
    }
}

async fn mutate(client: &impl Provider, cmd: MailCmd) -> Result<()> {
    let mut labels = client.labels().await?;
    if let MailCmd::Label {
        label,
        remove: false,
        ..
    } = &cmd
        && resolve(&labels, label).is_err()
    {
        labels.push(client.create_label(label).await?);
    }
    let Plan { action, sel, verb } = plan(cmd, &labels)?;
    let ids = ids_of(client, &labels, sel).await?;
    client.apply(&action, &ids).await?;
    eprintln!("{} messages {}", ids.len(), verb);
    Ok(())
}

async fn attachments(client: &impl Provider, out: &Path, sel: Sel) -> Result<()> {
    let labels = client.labels().await?;
    let ids = ids_of(client, &labels, sel).await?;
    std::fs::create_dir_all(out).with_context(|| format!("create {}", out.display()))?;
    let mut saved = 0;
    for id in ids {
        for a in client.message(&id).await?.attachments {
            let path = out.join(format!("{id}-{}", safe_name(&a.name)));
            if path.exists() {
                eprintln!("exists, skipped: {}", path.display());
                continue;
            }
            std::fs::write(&path, &a.data).with_context(|| format!("write {}", path.display()))?;
            println!("{id}\t{}", path.display());
            saved += 1;
        }
    }
    eprintln!("{saved} attachments saved");
    Ok(())
}

async fn send(
    client: &impl Provider,
    to: Vec<String>,
    subject: Option<String>,
    reply_to: Option<&str>,
    body: &Path,
    attach: &[PathBuf],
) -> Result<()> {
    let text = if body == Path::new("-") {
        std::io::read_to_string(std::io::stdin()).context("read body from stdin")?
    } else {
        std::fs::read_to_string(body).with_context(|| format!("read {}", body.display()))?
    };
    let from = client.email().await?;
    let parent = match reply_to {
        Some(id) => Some(client.parent(id).await?),
        None => None,
    };
    let to = match &parent {
        Some(p) if to.is_empty() => vec![p.from.clone()],
        _ => to,
    };
    let subject = match (subject, &parent) {
        (Some(s), _) => s,
        (None, Some(p)) => reply_subject(&p.subject),
        (None, None) => bail!("--subject is required without --reply-to"),
    };
    let mut msg = mail_builder::MessageBuilder::new()
        .from(from)
        .to(to)
        .subject(subject);
    msg = if body.extension().is_some_and(|e| e == "html") {
        msg.html_body(text)
    } else {
        msg.text_body(text)
    };
    if let Some(p) = &parent {
        let mut references = p.references.clone();
        references.push(p.message_id.clone());
        msg = msg.in_reply_to(p.message_id.clone()).references(references);
    }
    for path in attach {
        let data = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .context("attachment needs a file name")?
            .to_string();
        msg = msg.attachment(content_type(&name), name, data);
    }
    let raw = msg.write_to_vec().context("build message")?;
    let id = client.send(raw, parent.map(|p| p.thread)).await?;
    println!("{id}");
    Ok(())
}

fn reply_subject(subject: &str) -> String {
    if subject.to_ascii_lowercase().starts_with("re:") {
        subject.to_string()
    } else {
        format!("Re: {subject}")
    }
}

fn content_type(name: &str) -> &'static str {
    match name
        .rsplit_once('.')
        .map_or("", |(_, e)| e)
        .to_ascii_lowercase()
        .as_str()
    {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "txt" | "md" => "text/plain",
        "csv" => "text/csv",
        "zip" => "application/zip",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
}

async fn ids_of(client: &impl Provider, labels: &[Label], sel: Sel) -> Result<Vec<String>> {
    Ok(match sel {
        Sel::Label(n) => {
            client
                .all_ids(&Selection::Label(resolve(labels, &n)?))
                .await?
        }
        Sel::Query(q) => client.all_ids(&Selection::Query(q)).await?,
        Sel::Ids(v) => v,
    })
}

pub fn human_size(bytes: usize) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Keeps the name a single path component; an empty name becomes "attachment".
pub fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    if cleaned.is_empty() {
        "attachment".to_string()
    } else {
        cleaned
    }
}

fn tsv(field: &str) -> String {
    field.replace(['\t', '\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Wrap {
        #[command(subcommand)]
        cmd: MailCmd,
    }

    fn action_of(args: &[&str]) -> Action {
        let w = Wrap::try_parse_from(std::iter::once("mail").chain(args.iter().copied())).unwrap();
        plan(w.cmd, &[]).unwrap().action
    }

    #[test]
    fn safe_name_is_one_path_component() {
        assert_eq!(safe_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_name("a/b\\c.xlsx"), "a_b_c.xlsx");
        assert_eq!(safe_name("  "), "attachment");
        assert_eq!(safe_name("Informe Q3.xlsx"), "Informe Q3.xlsx");
    }

    #[test]
    fn delete_trashes_unless_permanent() {
        assert!(matches!(
            action_of(&["delete", "--label=medium"]),
            Action::Trash
        ));
        assert!(matches!(
            action_of(&["delete", "--permanent", "--label=medium"]),
            Action::Delete
        ));
    }
}
