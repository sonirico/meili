use std::sync::Mutex;

use anyhow::{Context, Result, anyhow};

use crate::mail::{Action, Attachment, Label, Message, Page, Parent, Selection, Summary};
use crate::provider::Provider;

const EMAIL: &str = "ada@example.com";
const PAGE_SIZE: usize = 50;
const SENT_DATE: &str = "Sat, 21 Nov 2026 09:00:00 +0000";

struct Entry {
    id: String,
    from: String,
    subject: String,
    date: String,
    labels: Vec<String>,
    body: String,
    attachments: Vec<(String, usize)>, // file name and size in bytes
}

struct State {
    labels: Vec<Label>,
    entries: Vec<Entry>,
    created: usize,
}

/// An invented mailbox that lives in memory, for `--demo`. Nothing reaches the network or the disk.
pub struct Demo {
    state: Mutex<State>,
}

impl Demo {
    pub fn new() -> Demo {
        let labels = [
            ("INBOX", "INBOX"),
            ("SENT", "SENT"),
            ("TRASH", "TRASH"),
            ("Label_1", "invoices"),
            ("Label_2", "newsletters"),
            ("Label_3", "team"),
            ("Label_4", "travel"),
        ]
        .into_iter()
        .map(|(id, name)| Label {
            id: id.to_string(),
            name: name.to_string(),
        })
        .collect();
        Demo {
            state: Mutex::new(State {
                labels,
                entries: entries(),
                created: 0,
            }),
        }
    }

    fn state(&self) -> Result<std::sync::MutexGuard<'_, State>> {
        self.state.lock().map_err(|_| anyhow!("demo mailbox lock"))
    }
}

impl Provider for Demo {
    async fn email(&self) -> Result<String> {
        Ok(EMAIL.to_string())
    }

    async fn labels(&self) -> Result<Vec<Label>> {
        let mut labels: Vec<Label> = self
            .state()?
            .labels
            .iter()
            .map(|l| Label {
                id: l.id.clone(),
                name: l.name.clone(),
            })
            .collect();
        labels.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(labels)
    }

    async fn create_label(&self, name: &str) -> Result<Label> {
        let mut state = self.state()?;
        let id = format!("Label_{}", state.labels.len() + 1);
        state.labels.push(Label {
            id: id.clone(),
            name: name.to_string(),
        });
        Ok(Label {
            id,
            name: name.to_string(),
        })
    }

    async fn page(&self, sel: &Selection, token: Option<&str>) -> Result<Page> {
        let state = self.state()?;
        let start: usize = match token {
            Some(t) => t.parse().context("bad page token")?,
            None => 0,
        };
        let selected: Vec<&Entry> = state.entries.iter().filter(|e| selects(sel, e)).collect();
        let end = (start + PAGE_SIZE).min(selected.len());
        Ok(Page {
            items: selected[start.min(end)..end]
                .iter()
                .map(|e| summary(e))
                .collect(),
            next: (end < selected.len()).then(|| end.to_string()),
        })
    }

    async fn all_ids(&self, sel: &Selection) -> Result<Vec<String>> {
        Ok(self
            .state()?
            .entries
            .iter()
            .filter(|e| selects(sel, e))
            .map(|e| e.id.clone())
            .collect())
    }

    async fn message(&self, id: &str) -> Result<Message> {
        let state = self.state()?;
        let entry = find(&state, id)?;
        Ok(Message {
            body: entry.body.clone(),
            attachments: entry
                .attachments
                .iter()
                .map(|(name, size)| Attachment {
                    name: name.clone(),
                    data: vec![b'm'; *size],
                })
                .collect(),
        })
    }

    async fn parent(&self, id: &str) -> Result<Parent> {
        let state = self.state()?;
        let entry = find(&state, id)?;
        let from = entry
            .from
            .rsplit_once('<')
            .map_or(entry.from.as_str(), |(_, addr)| addr.trim_end_matches('>'));
        Ok(Parent {
            thread: entry.id.clone(),
            message_id: format!("{}@example.com", entry.id),
            references: Vec::new(),
            from: from.to_string(),
            subject: entry.subject.clone(),
        })
    }

    async fn send(&self, raw: Vec<u8>, _thread: Option<String>) -> Result<String> {
        let parsed = mail_parser::MessageParser::default()
            .parse(&raw)
            .context("cannot parse message")?;
        let mut state = self.state()?;
        state.created += 1;
        let id = format!("19a6f0c4b2e1{:04x}", state.created);
        let entry = Entry {
            id: id.clone(),
            from: EMAIL.to_string(),
            subject: parsed.subject().unwrap_or_default().to_string(),
            date: SENT_DATE.to_string(),
            labels: vec!["SENT".to_string()],
            body: parsed
                .body_text(0)
                .map(|b| b.into_owned())
                .unwrap_or_default(),
            attachments: parsed
                .attachments()
                .map(|part| {
                    (
                        part.attachment_name().unwrap_or("attachment").to_string(),
                        part.decoded().len(),
                    )
                })
                .collect(),
        };
        state.entries.insert(0, entry);
        Ok(id)
    }

    fn forget(&self, _ids: &[String]) -> Result<()> {
        Ok(())
    }

    async fn apply(&self, action: &Action, ids: &[String]) -> Result<()> {
        let mut state = self.state()?;
        if let Action::Delete = action {
            state.entries.retain(|e| !ids.contains(&e.id));
            return Ok(());
        }
        for entry in state.entries.iter_mut().filter(|e| ids.contains(&e.id)) {
            let (add, remove) = match action {
                Action::Archive => (None, Some("INBOX")),
                Action::Trash => (Some("TRASH"), Some("INBOX")),
                Action::Label { id, remove: false } => (Some(id.as_str()), None),
                Action::Label { id, remove: true } => (None, Some(id.as_str())),
                Action::Delete => unreachable!("handled above"),
            };
            if let Some(label) = remove {
                entry.labels.retain(|l| l != label);
            }
            if let Some(label) = add
                && !entry.labels.iter().any(|l| l == label)
            {
                entry.labels.push(label.to_string());
            }
        }
        Ok(())
    }
}

fn find<'a>(state: &'a State, id: &str) -> Result<&'a Entry> {
    state
        .entries
        .iter()
        .find(|e| e.id == id)
        .ok_or_else(|| anyhow!("unknown message: {id}"))
}

fn summary(entry: &Entry) -> Summary {
    Summary {
        id: entry.id.clone(),
        from: entry.from.clone(),
        subject: entry.subject.clone(),
        date: entry.date.clone(),
        labels: entry.labels.clone(),
        attachments: entry.attachments.iter().map(|(n, _)| n.clone()).collect(),
    }
}

/// A small subset of Gmail search: `from:`, `has:attachment`, `in:trash` and plain words.
/// Like Gmail, Trash stays out of a query unless the query names it.
fn selects(sel: &Selection, entry: &Entry) -> bool {
    let query = match sel {
        Selection::Label(id) => return entry.labels.contains(id),
        Selection::Query(q) => q.to_lowercase(),
    };
    let trashed = entry.labels.iter().any(|l| l == "TRASH");
    let wants_trash = query.split_whitespace().any(|t| t == "in:trash");
    trashed == wants_trash
        && query.split_whitespace().all(|term| {
            if let Some(sender) = term.strip_prefix("from:") {
                entry.from.to_lowercase().contains(sender)
            } else if term == "has:attachment" {
                !entry.attachments.is_empty()
            } else if term == "in:trash" {
                true
            } else {
                [&entry.from, &entry.subject, &entry.body]
                    .iter()
                    .any(|field| field.to_lowercase().contains(term))
            }
        })
}

fn entry(
    n: usize,
    date: &str,
    from: &str,
    subject: &str,
    labels: &[&str],
    body: &str,
    attachments: &[(&str, usize)],
) -> Entry {
    Entry {
        id: format!("19a6f0c4b2e0{n:04x}"),
        from: from.to_string(),
        subject: subject.to_string(),
        date: date.to_string(),
        labels: labels.iter().map(|l| l.to_string()).collect(),
        body: body.to_string(),
        attachments: attachments
            .iter()
            .map(|(name, size)| (name.to_string(), *size))
            .collect(),
    }
}

fn entries() -> Vec<Entry> {
    vec![
        entry(
            1,
            "Fri, 20 Nov 2026 17:42:10 +0000",
            "Northwind Hosting <billing@northwind.example>",
            "Your November invoice is attached",
            &["INBOX", "UNREAD", "Label_1"],
            "Hi Ada,\n\nYour invoice for November is attached. The total is 42.00 EUR and it \
             will be charged to the card on file on 1 December.\n\nPlan: Standard, 2 servers\n\
             Period: 1 Nov 2026 to 30 Nov 2026\n\nThanks for hosting with us.\nNorthwind Hosting",
            &[("invoice-2026-11.pdf", 254_270)],
        ),
        entry(
            2,
            "Fri, 20 Nov 2026 15:06:31 +0000",
            "Priya Nair <priya@harbor.example>",
            "Re: Q4 planning notes",
            &["INBOX", "UNREAD", "Label_3"],
            "Ada,\n\nI went through the notes. Two things before Monday:\n\n1. The migration \
             cannot start until the read replicas are in place.\n2. Support wants a freeze \
             during the last week of December.\n\nI put both in the sheet, with owners. Have a \
             look when you can.\n\nPriya",
            &[("q4-plan.xlsx", 48_112), ("timeline.png", 310_554)],
        ),
        entry(
            3,
            "Fri, 20 Nov 2026 11:20:05 +0000",
            "Kestrel Air <no-reply@kestrel.example>",
            "Your trip to Lisbon: boarding passes",
            &["INBOX", "Label_4"],
            "Booking reference: KX4T9P\n\nLIS  Lisbon   Thu 26 Nov  07:35\nMAD  Madrid   Sun 29 \
             Nov  21:10\n\nYour boarding passes and a calendar entry are attached. Bag drop \
             closes 45 minutes before departure.",
            &[("boarding-passes.pdf", 181_904), ("trip.ics", 2_310)],
        ),
        entry(
            4,
            "Fri, 20 Nov 2026 08:01:44 +0000",
            "Lumen Weekly <hello@lumen.example>",
            "Issue 212: the terminal is not dead",
            &["INBOX", "UNREAD", "Label_2"],
            "This week:\n\n- Why text interfaces keep winning for repetitive work\n- A field \
             guide to vim motions outside vim\n- Reader mail: your worst inbox stories\n\nRead \
             on the web or just keep scrolling.",
            &[],
        ),
        entry(
            5,
            "Thu, 19 Nov 2026 19:33:27 +0000",
            "Tomas Vidal <tomas@harbor.example>",
            "Design review recording",
            &["INBOX", "Label_3"],
            "The recording from today's review is attached, with the slides. The open question \
             is still whether the cache belongs in the client or behind the API.\n\nTomas",
            &[
                ("design-review.mp4", 18_734_002),
                ("slides.pptx", 2_204_771),
            ],
        ),
        entry(
            6,
            "Thu, 19 Nov 2026 16:12:09 +0000",
            "Chen Wei <chen@harbor.example>",
            "\u{5de5}\u{4f5c}\u{62a5}\u{544a} Weekly report",
            &["INBOX", "UNREAD", "Label_3"],
            "Hi Ada,\n\nThe weekly report is attached, in both languages. Highlights:\n\n- Latency \
             on the search path is down 18 percent.\n- Two incidents, both closed within the \
             hour.\n- Hiring: one offer out, one onsite next week.\n\nChen",
            &[
                ("weekly-report.pdf", 96_480),
                ("weekly-report.docx", 61_203),
            ],
        ),
        entry(
            7,
            "Thu, 19 Nov 2026 10:48:52 +0000",
            "Orchard Bank <statements@orchard.example>",
            "Your October statement is ready",
            &["INBOX", "Label_1"],
            "Your statement for October is attached as a spreadsheet and as a PDF. No action \
             is needed.",
            &[
                ("statement-2026-10.csv", 12_044),
                ("statement-2026-10.pdf", 143_871),
            ],
        ),
        entry(
            8,
            "Wed, 18 Nov 2026 22:15:40 +0000",
            "Lumen Weekly <hello@lumen.example>",
            "Issue 211: small tools, sharp edges",
            &["INBOX", "Label_2"],
            "This week:\n\n- Building command line tools people keep\n- SQLite as a cache, and \
             when not to\n- An interview with a maintainer of thirty years",
            &[],
        ),
        entry(
            9,
            "Wed, 18 Nov 2026 14:02:18 +0000",
            "Mara Lindqvist <mara@fjord.example>",
            "Photos from the offsite",
            &["INBOX"],
            "Finally sorted them. The best ones are in the archive, and I attached my favourite \
             so you do not have to unzip anything.\n\nMara",
            &[("offsite.zip", 48_220_113), ("group.jpg", 2_871_340)],
        ),
        entry(
            10,
            "Wed, 18 Nov 2026 09:27:03 +0000",
            "Harbor CI <ci@harbor.example>",
            "[harbor/api] Build failed on main",
            &["INBOX", "UNREAD"],
            "Build 4812 failed.\n\nStage: test\nFailed: 3 of 1204\nDuration: 6m 12s\n\nThe full \
             log is attached.",
            &[("build-4812.log", 88_412)],
        ),
        entry(
            11,
            "Tue, 17 Nov 2026 18:40:29 +0000",
            "Kestrel Air <no-reply@kestrel.example>",
            "Receipt for booking KX4T9P",
            &["INBOX", "Label_1", "Label_4"],
            "Thank you for flying with Kestrel Air. Your receipt is attached.\n\nTotal paid: \
             186.40 EUR",
            &[("receipt-KX4T9P.pdf", 74_655)],
        ),
        entry(
            12,
            "Tue, 17 Nov 2026 12:11:57 +0000",
            "Priya Nair <priya@harbor.example>",
            "Lunch on Thursday?",
            &["INBOX"],
            "The new place on the corner finally opened. Thursday at one?\n\nPriya",
            &[],
        ),
        entry(
            13,
            "Mon, 16 Nov 2026 20:05:12 +0000",
            "Pine Street Podcast <show@pinestreet.example>",
            "Episode 88 is out",
            &["INBOX", "Label_2"],
            "Episode 88: forty minutes on mail, of all things. The audio is attached for the \
             offline listeners.",
            &[("episode-88.mp3", 31_402_880)],
        ),
        entry(
            14,
            "Mon, 16 Nov 2026 09:14:38 +0000",
            "Northwind Hosting <status@northwind.example>",
            "Scheduled maintenance on 28 November",
            &["INBOX"],
            "We will replace network hardware in the Frankfurt region on 28 November between \
             02:00 and 04:00 UTC. Expect two short interruptions of under a minute each.",
            &[],
        ),
        entry(
            15,
            "Sun, 15 Nov 2026 17:30:00 +0000",
            "Lumen Weekly <hello@lumen.example>",
            "Issue 210: the cost of a notification",
            &["INBOX", "Label_2"],
            "This week:\n\n- What an interruption really costs\n- Batching mail: three routines \
             that survive a busy week",
            &[],
        ),
        entry(
            16,
            "Sat, 14 Nov 2026 11:02:46 +0000",
            "Tomas Vidal <tomas@harbor.example>",
            "Contract draft, second pass",
            &["INBOX", "Label_3"],
            "Changes from legal are in. Section 4 is the one to read closely.\n\nTomas",
            &[("contract-v2.docx", 132_990)],
        ),
        entry(
            17,
            "Thu, 19 Nov 2026 16:40:02 +0000",
            "ada@example.com",
            "Re: Design review recording",
            &["SENT"],
            "Thanks, Tomas. I vote for behind the API. I will write it up tomorrow.\n\nAda",
            &[],
        ),
        entry(
            18,
            "Fri, 13 Nov 2026 07:55:21 +0000",
            "Bright Deals <offers@brightdeals.example>",
            "Last chance: 70% off everything",
            &["TRASH"],
            "Ends tonight. Everything must go.",
            &[],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVOICE: &str = "19a6f0c4b2e00001";

    fn inbox() -> Selection {
        Selection::Label("INBOX".to_string())
    }

    fn query(q: &str) -> Selection {
        Selection::Query(q.to_string())
    }

    #[tokio::test]
    async fn trash_leaves_the_inbox_and_queries() {
        let demo = Demo::new();
        let before = demo.all_ids(&inbox()).await.unwrap().len();

        demo.apply(&Action::Trash, &[INVOICE.to_string()])
            .await
            .unwrap();

        assert_eq!(demo.all_ids(&inbox()).await.unwrap().len(), before - 1);
        assert!(demo.all_ids(&query("invoice")).await.unwrap().is_empty());
        assert_eq!(
            demo.all_ids(&query("in:trash invoice")).await.unwrap(),
            [INVOICE]
        );
    }

    #[tokio::test]
    async fn query_terms_all_have_to_match() {
        let demo = Demo::new();

        let ids = demo
            .all_ids(&query("from:kestrel has:attachment receipt"))
            .await
            .unwrap();

        assert_eq!(ids, ["19a6f0c4b2e0000b"]);
    }

    #[tokio::test]
    async fn label_toggles_on_one_message() {
        let demo = Demo::new();
        let travel = Selection::Label("Label_4".to_string());
        let before = demo.all_ids(&travel).await.unwrap().len();
        let add = Action::Label {
            id: "Label_4".to_string(),
            remove: false,
        };

        demo.apply(&add, &[INVOICE.to_string()]).await.unwrap();
        demo.apply(&add, &[INVOICE.to_string()]).await.unwrap();

        assert_eq!(demo.all_ids(&travel).await.unwrap().len(), before + 1);
    }

    #[tokio::test]
    async fn sent_mail_lands_in_sent() {
        let demo = Demo::new();
        let raw =
            b"From: ada@example.com\r\nTo: priya@harbor.example\r\nSubject: Hello\r\n\r\nHi.\r\n";

        let id = demo.send(raw.to_vec(), None).await.unwrap();

        let sent = demo
            .page(&Selection::Label("SENT".to_string()), None)
            .await
            .unwrap();
        assert_eq!(sent.items[0].id, id);
        assert_eq!(sent.items[0].subject, "Hello");
    }
}
