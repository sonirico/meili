use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

use crate::mail::Summary;

const SEP: char = '\x1f';
/// Bump when the table shape changes; the cache is rebuilt from Gmail.
const SCHEMA_VERSION: i64 = 3;

/// Local copy of message metadata so a listing only asks Gmail for ids it has not seen.
pub struct Cache {
    conn: Mutex<Connection>,
}

impl Cache {
    pub fn open(path: &Path) -> Result<Cache> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).context("create cache dir")?;
        }
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < SCHEMA_VERSION {
            conn.execute_batch(&format!(
                "DROP TABLE IF EXISTS messages;
                 CREATE TABLE messages (
                    id TEXT PRIMARY KEY,
                    sender TEXT NOT NULL,
                    subject TEXT NOT NULL,
                    date TEXT NOT NULL,
                    labels TEXT NOT NULL,
                    attachments TEXT NOT NULL
                 );
                 PRAGMA user_version = {SCHEMA_VERSION};"
            ))?;
        }
        Ok(Cache {
            conn: Mutex::new(conn),
        })
    }

    pub fn get(&self, ids: &[String]) -> Result<HashMap<String, Summary>> {
        let conn = self.conn.lock().expect("cache lock");
        let mut stmt = conn.prepare(
            "SELECT sender, subject, date, labels, attachments FROM messages WHERE id = ?1",
        )?;
        let mut found = HashMap::new();
        for id in ids {
            let mut rows = stmt.query(params![id])?;
            if let Some(row) = rows.next()? {
                let labels: String = row.get(3)?;
                let attachments: String = row.get(4)?;
                found.insert(
                    id.clone(),
                    Summary {
                        id: id.clone(),
                        from: row.get(0)?,
                        subject: row.get(1)?,
                        date: row.get(2)?,
                        labels: split(&labels),
                        attachments: split(&attachments),
                    },
                );
            }
        }
        Ok(found)
    }

    pub fn put(&self, items: &[Summary]) -> Result<()> {
        let mut conn = self.conn.lock().expect("cache lock");
        let tx = conn.transaction()?;
        for s in items {
            tx.execute(
                "INSERT OR REPLACE INTO messages (id, sender, subject, date, labels, attachments)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    s.id,
                    s.from,
                    s.subject,
                    s.date,
                    join(&s.labels),
                    join(&s.attachments)
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn remove(&self, ids: &[String]) -> Result<()> {
        let mut conn = self.conn.lock().expect("cache lock");
        let tx = conn.transaction()?;
        for id in ids {
            tx.execute("DELETE FROM messages WHERE id = ?1", params![id])?;
        }
        tx.commit()?;
        Ok(())
    }
}

fn join(items: &[String]) -> String {
    items.join(&SEP.to_string())
}

fn split(joined: &str) -> Vec<String> {
    joined
        .split(SEP)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: &str) -> Summary {
        Summary {
            id: id.to_string(),
            from: "a@b".to_string(),
            subject: "hi".to_string(),
            date: "d".to_string(),
            labels: vec!["INBOX".to_string(), "UNREAD".to_string()],
            attachments: vec!["a.pdf".to_string()],
        }
    }

    #[test]
    fn put_get_remove_roundtrip() {
        let dir = std::env::temp_dir().join(format!("meili-cache-{}", std::process::id()));
        let cache = Cache::open(&dir.join("c.sqlite")).unwrap();
        cache.put(&[summary("1"), summary("2")]).unwrap();

        let got = cache.get(&["1".to_string(), "3".to_string()]).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got["1"].labels, vec!["INBOX", "UNREAD"]);
        assert_eq!(got["1"].attachments, vec!["a.pdf"]);

        cache.remove(&["1".to_string()]).unwrap();
        assert!(cache.get(&["1".to_string()]).unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
