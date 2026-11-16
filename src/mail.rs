pub struct Label {
    pub id: String,
    pub name: String,
}

pub struct Summary {
    pub id: String,
    pub from: String,
    pub subject: String,
    pub date: String,
    pub labels: Vec<String>,
    pub attachments: Vec<String>, // file names, from the MIME parts
}

pub struct Page {
    pub items: Vec<Summary>,
    pub next: Option<String>,
}

#[derive(Clone)]
pub enum Selection {
    Label(String),
    Query(String),
}

pub enum Action {
    Archive,
    Trash,
    Delete,
    Label { id: String, remove: bool },
}

pub struct Attachment {
    pub name: String,
    pub data: Vec<u8>,
}

pub struct Message {
    pub body: String,
    pub attachments: Vec<Attachment>,
}

/// What a reply needs from the message it answers.
pub struct Parent {
    pub thread: String,
    pub message_id: String,
    pub references: Vec<String>,
    pub from: String,
    pub subject: String,
}
