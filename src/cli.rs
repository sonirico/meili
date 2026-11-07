use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};

use crate::gmail::{Client, resolve};
use crate::mail::{Action, Label, Selection};

#[derive(Args)]
#[group(required = true, multiple = false)]
pub struct SelArgs {
    #[arg(long)]
    label: Option<String>,
    #[arg(long, value_parser = non_blank)]
    query: Option<String>,
    ids: Vec<String>,
}

#[derive(Args)]
pub struct ListArgs {
    #[arg(long, conflicts_with = "query")]
    label: Option<String>,
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
    List(ListArgs),
    Read {
        id: String,
    },
    Archive(SelArgs),
    Delete {
        #[arg(long)]
        permanent: bool,
        #[command(flatten)]
        sel: SelArgs,
    },
    Label {
        label: String,
        #[arg(long)]
        remove: bool,
        #[command(flatten)]
        sel: SelArgs,
    },
    /// Save every attachment of the selection as OUT/<id>-<name>; prints id<TAB>path
    Attachments {
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
        MailCmd::List(_) | MailCmd::Read { .. } | MailCmd::Attachments { .. } => {
            unreachable!("not a mutation")
        }
    })
}

pub async fn run(client: &Client, cmd: MailCmd) -> Result<()> {
    match cmd {
        MailCmd::List(args) => list(client, args).await,
        MailCmd::Read { id } => {
            println!("{}", client.message(&id).await?);
            Ok(())
        }
        MailCmd::Attachments { out, sel } => attachments(client, &out, sel.into()).await,
        cmd => mutate(client, cmd).await,
    }
}

async fn list(client: &Client, args: ListArgs) -> Result<()> {
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

async fn mutate(client: &Client, cmd: MailCmd) -> Result<()> {
    let labels = client.labels().await?;
    let Plan { action, sel, verb } = plan(cmd, &labels)?;
    let ids = ids_of(client, &labels, sel).await?;
    client.apply(&action, &ids).await?;
    eprintln!("{} messages {}", ids.len(), verb);
    Ok(())
}

async fn attachments(client: &Client, out: &Path, sel: Sel) -> Result<()> {
    let labels = client.labels().await?;
    let ids = ids_of(client, &labels, sel).await?;
    std::fs::create_dir_all(out).with_context(|| format!("create {}", out.display()))?;
    let mut saved = 0;
    for id in ids {
        for a in client.attachments(&id).await? {
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

async fn ids_of(client: &Client, labels: &[Label], sel: Sel) -> Result<Vec<String>> {
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

/// Keeps the name a single path component; an empty name becomes "attachment".
fn safe_name(name: &str) -> String {
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
