mod auth;
mod cache;
mod cli;
mod gmail;
mod mail;
mod tui;

use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::cache::Cache;
use crate::cli::MailCmd;
use crate::gmail::{AuthMode, Client};

#[derive(Parser)]
#[command(
    name = "meili",
    about = "Gmail from the terminal: a TUI, and commands for bulk work",
    after_help = "Run without a subcommand to open the TUI."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Open the browser once and store the token
    Login,
    /// Non-interactive commands over the mailbox
    Mail {
        #[command(subcommand)]
        cmd: MailCmd,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let msg = format!("{e:#}");
            if msg.contains("rateLimitExceeded") {
                eprintln!(
                    "Gmail quota exceeded (6000 units per minute per user); wait a minute and retry"
                );
            } else {
                eprintln!("{msg}");
            }
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let rt = tokio::runtime::Runtime::new()?;
    let cfg = dirs::config_dir().context("no config dir")?.join("meili");
    let cache = Cache::open(
        &dirs::cache_dir()
            .context("no cache dir")?
            .join("meili")
            .join("gmail.sqlite"),
    )?;
    match cli.command {
        Some(Command::Login) => {
            let client = rt.block_on(Client::new(&cfg, AuthMode::Login, cache))?;
            let email = rt.block_on(client.email())?;
            eprintln!(
                "logged in as {email}; token saved to {}",
                cfg.join("token.json").display()
            );
            Ok(())
        }
        Some(Command::Mail { cmd }) => {
            let client = rt.block_on(Client::new(&cfg, AuthMode::Existing, cache))?;
            rt.block_on(cli::run(&client, cmd))
        }
        None => {
            let client = rt.block_on(Client::new(&cfg, AuthMode::Existing, cache))?;
            tui::run(&client, &rt)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("meili").chain(args.iter().copied()))
    }

    #[test]
    fn mutation_without_selection_fails() {
        assert!(parse(&["mail", "delete"]).is_err());
    }

    #[test]
    fn two_selections_fail() {
        assert!(parse(&["mail", "delete", "--label=a", "--query=b"]).is_err());
        assert!(parse(&["mail", "delete", "--query=b", "ID"]).is_err());
    }

    #[test]
    fn list_limit_combines_with_selection() {
        assert!(parse(&["mail", "list", "--query=a", "--limit=3"]).is_ok());
        assert!(parse(&["mail", "list", "--label=a", "--all"]).is_ok());
        assert!(parse(&["mail", "list", "--label=a", "--query=b"]).is_err());
        assert!(parse(&["mail", "list", "--limit=3", "--all"]).is_err());
    }

    #[test]
    fn attachments_needs_selection_and_out() {
        assert!(parse(&["mail", "attachments", "--out=x", "--query=a"]).is_ok());
        assert!(parse(&["mail", "attachments", "--out=x", "ID1", "ID2"]).is_ok());
        assert!(parse(&["mail", "attachments", "--out=x"]).is_err());
        assert!(parse(&["mail", "attachments", "--query=a"]).is_err());
    }

    #[test]
    fn send_needs_to_subject_and_body() {
        assert!(parse(&["mail", "send", "--to=a@b", "--subject=s", "--body=-"]).is_ok());
        assert!(
            parse(&[
                "mail",
                "send",
                "--to=a@b",
                "--to=c@d",
                "--subject=s",
                "--body=x",
                "--attach=f.pdf"
            ])
            .is_ok()
        );
        assert!(parse(&["mail", "send", "--subject=s", "--body=-"]).is_err());
        assert!(parse(&["mail", "send", "--to=a@b", "--body=-"]).is_err());
        assert!(parse(&["mail", "send", "--to=a@b", "--subject=s"]).is_err());
    }

    #[test]
    fn label_name_and_label_selection_coexist() {
        assert!(parse(&["mail", "label", "work", "--label=medium"]).is_ok());
        assert!(parse(&["mail", "label", "work", "--remove", "--query=from:x"]).is_ok());
        assert!(parse(&["mail", "label", "work", "ID1"]).is_ok());
        assert!(parse(&["mail", "label", "work"]).is_err());
    }

    #[test]
    fn blank_query_fails() {
        assert!(parse(&["mail", "delete", "--query="]).is_err());
        assert!(parse(&["mail", "delete", "--query=  "]).is_err());
    }
}
