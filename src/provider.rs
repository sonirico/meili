use anyhow::Result;

use crate::mail::{Action, Label, Message, Page, Parent, Selection};

/// What both front ends need from a mailbox.
pub trait Provider {
    async fn email(&self) -> Result<String>;
    async fn labels(&self) -> Result<Vec<Label>>;
    async fn create_label(&self, name: &str) -> Result<Label>;
    async fn page(&self, sel: &Selection, token: Option<&str>) -> Result<Page>;
    async fn all_ids(&self, sel: &Selection) -> Result<Vec<String>>;
    async fn message(&self, id: &str) -> Result<Message>;
    async fn parent(&self, id: &str) -> Result<Parent>;
    /// Sends an RFC 2822 message as the logged-in account, into `thread` when given.
    async fn send(&self, raw: Vec<u8>, thread: Option<String>) -> Result<String>;
    /// Drops cached metadata so the next page() asks the provider again.
    fn forget(&self, ids: &[String]) -> Result<()>;
    async fn apply(&self, action: &Action, ids: &[String]) -> Result<()>;
}
