use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use futures::stream::{self, StreamExt, TryStreamExt};
use google_gmail1::api::{
    BatchDeleteMessagesRequest, BatchModifyMessagesRequest, Message, MessagePart,
};
use google_gmail1::hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use google_gmail1::hyper_util::client::legacy::{Client as HyperClient, connect::HttpConnector};
use google_gmail1::hyper_util::rt::TokioExecutor;
use google_gmail1::yup_oauth2::client::CustomHyperClientBuilder;
use google_gmail1::yup_oauth2::{InstalledFlowAuthenticator, InstalledFlowReturnMethod};
use google_gmail1::{Gmail, yup_oauth2};

use std::sync::Arc;

use tokio::sync::Notify;

use crate::auth::{BrowserDelegate, NoLoginDelegate};
use crate::cache::Cache;
use crate::mail::{Action, Attachment, Label, Message as Mail, Page, Parent, Selection, Summary};

const SCOPE: &str = "https://mail.google.com/";
const USER: &str = "me";
const CHUNK: usize = 1000;
const PAGE_SIZE: u32 = 50;
const ALL_PAGE_SIZE: u32 = 500;
const IN_FLIGHT: usize = 25;

pub enum AuthMode {
    Login,
    Existing,
}

pub struct Client {
    hub: Gmail<HttpsConnector<HttpConnector>>,
    cache: Cache,
}

impl Client {
    pub async fn new(cfg: &Path, mode: AuthMode, cache: Cache) -> Result<Client> {
        std::fs::create_dir_all(cfg).context("create config dir")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(cfg, std::fs::Permissions::from_mode(0o700))
                .context("restrict config dir")?;
        }
        let secret_path = cfg.join("client_secret.json");
        if !secret_path.exists() {
            bail!(
                "missing {path}\n\n\
                 meili needs your own Google OAuth client (Google does not allow a shared one):\n\
                 \x20 1. https://console.cloud.google.com/apis/library/gmail.googleapis.com  enable the Gmail API\n\
                 \x20 2. https://console.cloud.google.com/auth/overview  set up the consent screen, publish it (\"In production\")\n\
                 \x20 3. https://console.cloud.google.com/auth/clients  create an OAuth client, type \"Desktop app\"\n\
                 \x20 4. download its JSON and save it as {path}\n\
                 \x20 5. run: meili login",
                path = secret_path.display()
            );
        }
        let secret = yup_oauth2::read_application_secret(&secret_path)
            .await
            .with_context(|| format!("read {}", secret_path.display()))?;

        let connector = || {
            HttpsConnectorBuilder::new()
                .with_native_roots()
                .map(|b| b.https_only().enable_http2().build())
        };
        let auth_client = HyperClient::builder(TokioExecutor::new()).build(connector()?);
        let refused = Arc::new(Notify::new());
        let delegate: Box<dyn yup_oauth2::authenticator_delegate::InstalledFlowDelegate> =
            match mode {
                AuthMode::Login => Box::new(BrowserDelegate),
                AuthMode::Existing => Box::new(NoLoginDelegate(refused.clone())),
            };
        let auth = InstalledFlowAuthenticator::with_client(
            secret,
            InstalledFlowReturnMethod::HTTPRedirect,
            CustomHyperClientBuilder::from(auth_client),
        )
        .persist_tokens_to_disk(cfg.join("token.json"))
        .flow_delegate(delegate)
        .build()
        .await
        .context("build authenticator")?;
        tokio::select! {
            token = auth.token(&[SCOPE]) => {
                token.map_err(|e| anyhow!("{e}"))?;
            }
            _ = refused.notified() => bail!("not logged in: run 'meili login'"),
        }

        let http = HyperClient::builder(TokioExecutor::new()).build(connector()?);
        Ok(Client {
            hub: Gmail::new(http, auth),
            cache,
        })
    }

    pub async fn email(&self) -> Result<String> {
        let (_, profile) = self
            .hub
            .users()
            .get_profile(USER)
            .add_scope(SCOPE)
            .doit()
            .await?;
        profile.email_address.context("profile has no email")
    }

    pub async fn labels(&self) -> Result<Vec<Label>> {
        let (_, res) = self
            .hub
            .users()
            .labels_list(USER)
            .add_scope(SCOPE)
            .doit()
            .await?;
        let mut labels: Vec<Label> = res
            .labels
            .unwrap_or_default()
            .into_iter()
            .filter_map(|l| {
                Some(Label {
                    id: l.id?,
                    name: l.name?,
                })
            })
            .collect();
        labels.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(labels)
    }

    pub async fn page(&self, sel: &Selection, token: Option<&str>) -> Result<Page> {
        let (ids, next) = self.list(sel, token, PAGE_SIZE).await?;
        let mut cached = self.cache.get(&ids)?;
        let missing: Vec<String> = ids
            .iter()
            .filter(|id| !cached.contains_key(*id))
            .cloned()
            .collect();
        let fetched: Vec<Summary> = stream::iter(missing)
            .map(|id| self.summary(id))
            .buffered(IN_FLIGHT)
            .try_collect()
            .await?;
        self.cache.put(&fetched)?;
        cached.extend(fetched.into_iter().map(|s| (s.id.clone(), s)));
        let items = ids.iter().filter_map(|id| cached.remove(id)).collect();
        Ok(Page { items, next })
    }

    pub async fn all_ids(&self, sel: &Selection) -> Result<Vec<String>> {
        let mut all = Vec::new();
        let mut token = None;
        loop {
            let (ids, next) = self.list(sel, token.as_deref(), ALL_PAGE_SIZE).await?;
            all.extend(ids);
            match next {
                Some(_) => token = next,
                None => return Ok(all),
            }
        }
    }

    pub async fn message(&self, id: &str) -> Result<Mail> {
        let raw = self.raw(id).await?;
        let parsed = parse(&raw)?;
        let body = parsed
            .body_text(0)
            .map(|b| b.into_owned())
            .unwrap_or_default();
        let attachments = parsed
            .attachments()
            .map(|part| Attachment {
                name: tidy(part.attachment_name().unwrap_or("attachment")),
                data: part.decoded().into_owned(),
            })
            .collect();
        Ok(Mail { body, attachments })
    }

    async fn raw(&self, id: &str) -> Result<Vec<u8>> {
        let (_, msg) = self
            .hub
            .users()
            .messages_get(USER, id)
            .format("raw")
            .add_scope(SCOPE)
            .doit()
            .await?;
        msg.raw.context("message has no raw body")
    }

    pub async fn parent(&self, id: &str) -> Result<Parent> {
        let (_, msg) = self
            .hub
            .users()
            .messages_get(USER, id)
            .format("raw")
            .add_scope(SCOPE)
            .doit()
            .await?;
        let raw = msg.raw.context("message has no raw body")?;
        let parsed = parse(&raw)?;
        let from = parsed
            .reply_to()
            .or(parsed.from())
            .and_then(|list| list.first())
            .and_then(|mailbox| mailbox.address())
            .context("message has no sender address")?;
        Ok(Parent {
            thread: msg.thread_id.context("message has no thread")?,
            message_id: parsed
                .message_id()
                .context("message has no Message-ID")?
                .to_string(),
            references: parsed
                .references()
                .map(|list| list.iter().map(str::to_string).collect())
                .unwrap_or_default(),
            from: from.to_string(),
            subject: parsed.subject().unwrap_or_default().to_string(),
        })
    }

    /// Sends an RFC 2822 message as the logged-in account, into `thread` when given.
    pub async fn send(&self, raw: Vec<u8>, thread: Option<String>) -> Result<String> {
        let message = Message {
            thread_id: thread,
            ..Message::default()
        };
        let (_, sent) = self
            .hub
            .users()
            .messages_send(message, USER)
            .add_scope(SCOPE)
            .upload(std::io::Cursor::new(raw), "message/rfc822".parse()?)
            .await?;
        sent.id.context("sent message has no id")
    }

    /// Drops cached metadata so the next page() asks Gmail again.
    pub fn forget(&self, ids: &[String]) -> Result<()> {
        self.cache.remove(ids)
    }

    pub async fn apply(&self, action: &Action, ids: &[String]) -> Result<()> {
        for chunk in ids.chunks(CHUNK) {
            let ids = Some(chunk.to_vec());
            let users = self.hub.users();
            let (add, remove) = match action {
                Action::Delete => {
                    users
                        .messages_batch_delete(BatchDeleteMessagesRequest { ids }, USER)
                        .add_scope(SCOPE)
                        .doit()
                        .await?;
                    continue;
                }
                Action::Archive => (vec![], vec!["INBOX".to_string()]),
                Action::Trash => (vec!["TRASH".to_string()], vec!["INBOX".to_string()]),
                Action::Label { id, remove: false } => (vec![id.clone()], vec![]),
                Action::Label { id, remove: true } => (vec![], vec![id.clone()]),
            };
            let req = BatchModifyMessagesRequest {
                ids,
                add_label_ids: Some(add),
                remove_label_ids: Some(remove),
            };
            users
                .messages_batch_modify(req, USER)
                .add_scope(SCOPE)
                .doit()
                .await?;
        }
        self.cache.remove(ids)?;
        Ok(())
    }

    async fn list(
        &self,
        sel: &Selection,
        token: Option<&str>,
        max: u32,
    ) -> Result<(Vec<String>, Option<String>)> {
        let users = self.hub.users();
        let mut call = users
            .messages_list(USER)
            .max_results(max)
            .include_spam_trash(false)
            .add_scope(SCOPE);
        call = match sel {
            Selection::Label(id) => call.add_label_ids(id),
            Selection::Query(q) => call.q(q),
        };
        if let Some(t) = token {
            call = call.page_token(t);
        }
        let (_, res) = call.doit().await?;
        let ids = res
            .messages
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| m.id)
            .collect();
        Ok((ids, res.next_page_token))
    }

    async fn summary(&self, id: String) -> Result<Summary> {
        let (_, msg) = self
            .hub
            .users()
            .messages_get(USER, &id)
            .format("full")
            .param(
                "fields",
                "labelIds,payload(headers(name,value),filename,parts(filename,parts(filename,parts(filename))))",
            )
            .add_scope(SCOPE)
            .doit()
            .await?;
        Ok(Summary {
            from: header(&msg, "From"),
            subject: header(&msg, "Subject"),
            date: header(&msg, "Date"),
            labels: msg.label_ids.unwrap_or_default(),
            attachments: msg.payload.as_ref().map_or_else(Vec::new, attachment_names),
            id,
        })
    }
}

fn attachment_names(part: &MessagePart) -> Vec<String> {
    let mut names = Vec::new();
    collect_names(part, &mut names);
    names
}

fn collect_names(part: &MessagePart, out: &mut Vec<String>) {
    if let Some(name) = part.filename.as_deref().filter(|f| !f.is_empty()) {
        out.push(tidy(name));
    }
    for child in part.parts.iter().flatten() {
        collect_names(child, out);
    }
}

pub fn resolve(labels: &[Label], name: &str) -> Result<String> {
    labels
        .iter()
        .find(|l| l.name == name)
        .map(|l| l.id.clone())
        .ok_or_else(|| anyhow!("unknown label: {name}"))
}

/// Folded MIME header values arrive with embedded line breaks; collapse them.
fn tidy(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse(raw: &[u8]) -> Result<mail_parser::Message<'_>> {
    mail_parser::MessageParser::default()
        .parse(raw)
        .context("cannot parse message")
}

fn header(msg: &Message, name: &str) -> String {
    msg.payload
        .as_ref()
        .and_then(|p| p.headers.as_ref())
        .and_then(|hs| {
            hs.iter().find(|h| {
                h.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
        })
        .and_then(|h| h.value.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> Vec<Label> {
        vec![
            Label {
                id: "Label_1".into(),
                name: "medium".into(),
            },
            Label {
                id: "Label_2".into(),
                name: "work".into(),
            },
        ]
    }

    #[test]
    fn resolve_returns_id_of_exact_name() {
        assert_eq!(resolve(&labels(), "work").unwrap(), "Label_2");
    }

    #[test]
    fn resolve_rejects_unknown_name() {
        assert!(resolve(&labels(), "mediu").is_err());
    }
}
