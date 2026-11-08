use std::future::Future;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap,
};
use ratatui::{DefaultTerminal, Frame};
use tokio::runtime::Runtime;

use crate::cli::{human_size, safe_name};
use crate::gmail::Client;
use crate::mail::{Action, Attachment, Label, Message, Page, Selection};

const HALF_PAGE: usize = 15;
const AUTO_REFRESH: Duration = Duration::from_secs(30);

const ACCENT: Color = Color::Rgb(255, 160, 0);
const KEY: Color = Color::Rgb(90, 200, 250);
const DIM: Color = Color::DarkGray;
const SELECTED: Style = Style::new()
    .bg(Color::Rgb(255, 160, 0))
    .fg(Color::Black)
    .add_modifier(Modifier::BOLD);

struct App {
    email: String,
    labels: Vec<Label>,
    label: usize,
    sel: Selection,
    page: Page,
    cursor: usize,
    view: View,
    status: String,
    error: bool,
    scroll: u16,
    pending_g: bool,
}

enum View {
    List,
    Read(Message),
    Attachments { msg: Message, cursor: usize },
    LabelPicker { cursor: usize },
    Jump { input: String, cursor: usize },
    ConfirmDelete,
    Search(String),
}

struct Ctx<'a> {
    client: &'a Client,
    rt: &'a Runtime,
    terminal: DefaultTerminal,
}

impl Ctx<'_> {
    fn call<T>(&mut self, app: &mut App, fut: impl Future<Output = Result<T>>) -> Result<T> {
        app.status = "working...".to_string();
        app.error = false;
        self.terminal.draw(|f| render(f, app))?;
        let out = self.rt.block_on(fut);
        app.status.clear();
        out
    }
}

pub fn run(client: &Client, rt: &Runtime) -> Result<()> {
    let email = rt.block_on(client.email())?;
    let labels = rt.block_on(client.labels())?;
    let label = labels
        .iter()
        .position(|l| l.id == "INBOX")
        .context("no INBOX label")?;
    let sel = Selection::Label(labels[label].id.clone());
    let page = rt.block_on(client.page(&sel, None))?;
    let mut app = App {
        email,
        labels,
        label,
        sel,
        page,
        cursor: 0,
        view: View::List,
        status: String::new(),
        error: false,
        scroll: 0,
        pending_g: false,
    };
    let mut ctx = Ctx {
        client,
        rt,
        terminal: ratatui::init(),
    };
    let result = run_loop(&mut ctx, &mut app);
    ratatui::restore();
    result
}

fn run_loop(ctx: &mut Ctx, app: &mut App) -> Result<()> {
    let mut last_refresh = Instant::now();
    loop {
        ctx.terminal.draw(|f| render(f, app))?;
        let wait = AUTO_REFRESH.saturating_sub(last_refresh.elapsed());
        if !event::poll(wait)? {
            if matches!(app.view, View::List) {
                let result = reload(ctx, app, false);
                report(app, result);
            }
            last_refresh = Instant::now();
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match handle_key(ctx, app, key) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(e) => report(app, Err(e)),
        }
    }
}

fn report(app: &mut App, result: Result<()>) {
    if let Err(e) = result {
        app.status = format!("{e:#}");
        app.error = true;
    }
}

fn render(f: &mut Frame, app: &App) {
    let [header, body, hints, status] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(f.area());

    render_header(f, app, header);
    match &app.view {
        View::Read(msg) => render_read(f, app, msg, body),
        _ => render_table(f, app, body),
    }
    match &app.view {
        View::Attachments { msg, cursor } => render_attachments(f, msg, *cursor, body),
        View::LabelPicker { cursor } => render_picker(f, app, *cursor, body),
        View::Jump { input, cursor } => render_jump(f, app, input, *cursor, body),
        _ => {}
    }
    render_hints(f, app, hints);
    render_status(f, app, status);
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let where_ = match &app.sel {
        Selection::Label(_) => app.labels[app.label].name.clone(),
        Selection::Query(q) => format!("/{q}"),
    };
    let more = if app.page.next.is_some() { "+" } else { "" };
    let lines = vec![
        Line::from(vec![
            Span::styled(
                " meili ",
                Style::default()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(&app.email, Style::default().fg(KEY)),
        ]),
        Line::from(vec![
            Span::styled(" view   ", Style::default().fg(DIM)),
            Span::styled(
                where_,
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("   {}{more} messages", app.page.items.len()),
                Style::default().fg(DIM),
            ),
        ]),
        Line::from(""),
    ];
    f.render_widget(Paragraph::new(lines), area);
}

fn render_table(f: &mut Frame, app: &App, area: Rect) {
    let rows = app.page.items.iter().map(|s| {
        let unread = s.labels.iter().any(|l| l == "UNREAD");
        let text = if unread {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let icons: String = s.attachments.iter().take(2).map(|n| icon(n)).collect();
        Row::new(vec![
            Cell::from(icons),
            Cell::from(short_date(&s.date)).style(Style::default().fg(DIM)),
            Cell::from(short_from(&s.from)).style(Style::default().fg(KEY)),
            Cell::from(s.subject.as_str()).style(text),
        ])
    });
    let header = Row::new(vec!["", "DATE", "FROM", "SUBJECT"]).style(
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
    );
    let widths = [
        Constraint::Length(4),
        Constraint::Length(17),
        Constraint::Length(26),
        Constraint::Min(0),
    ];
    let mut state = TableState::default().with_selected(Some(app.cursor));
    f.render_stateful_widget(
        Table::new(rows, widths)
            .header(header)
            .block(pane(""))
            .column_spacing(2)
            .row_highlight_style(SELECTED),
        area,
        &mut state,
    );
}

fn render_read(f: &mut Frame, app: &App, msg: &Message, area: Rect) {
    let item = &app.page.items[app.cursor];
    let block = pane(&format!("{}  <{}>", item.subject, item.from));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [head, text] = Layout::vertical([
        Constraint::Length(if msg.attachments.is_empty() { 0 } else { 2 }),
        Constraint::Min(0),
    ])
    .areas(inner);
    if !msg.attachments.is_empty() {
        let mut spans = vec![Span::styled("attachments: ", Style::default().fg(DIM))];
        for a in &msg.attachments {
            spans.push(Span::styled(
                format!(
                    "{} {} ({})  ",
                    icon(&a.name),
                    a.name,
                    human_size(a.data.len())
                ),
                Style::default().fg(ACCENT),
            ));
        }
        f.render_widget(
            Paragraph::new(Line::from(spans)).wrap(Wrap { trim: false }),
            head,
        );
    }
    f.render_widget(
        Paragraph::new(msg.body.as_str())
            .wrap(Wrap { trim: false })
            .scroll((app.scroll, 0)),
        text,
    );
}

fn render_attachments(f: &mut Frame, msg: &Message, cursor: usize, area: Rect) {
    let rows = msg.attachments.iter().map(|a| {
        ListItem::new(format!(
            "{} {}  {}",
            icon(&a.name),
            a.name,
            human_size(a.data.len())
        ))
    });
    let mut state = ListState::default().with_selected(Some(cursor));
    let area = popup(area, 70, msg.attachments.len() as u16 + 2);
    f.render_widget(Clear, area);
    f.render_stateful_widget(
        List::new(rows)
            .block(pane("attachments: <enter> open  <s> save"))
            .highlight_style(SELECTED),
        area,
        &mut state,
    );
}

fn render_picker(f: &mut Frame, app: &App, cursor: usize, area: Rect) {
    let item = &app.page.items[app.cursor];
    let rows = app.labels.iter().map(|l| {
        let mark = if item.labels.contains(&l.id) {
            "[x]"
        } else {
            "[ ]"
        };
        ListItem::new(format!("{mark} {}", l.name))
    });
    let mut state = ListState::default().with_selected(Some(cursor));
    let area = popup(area, 40, app.labels.len() as u16 + 2);
    f.render_widget(Clear, area);
    f.render_stateful_widget(
        List::new(rows)
            .block(pane("toggle label"))
            .highlight_style(SELECTED),
        area,
        &mut state,
    );
}

fn render_jump(f: &mut Frame, app: &App, input: &str, cursor: usize, area: Rect) {
    let matches = jump_matches(&app.labels, input);
    let rows = matches
        .iter()
        .map(|&i| ListItem::new(app.labels[i].name.as_str()));
    let mut state = ListState::default().with_selected(Some(cursor));
    let area = popup(area, 40, matches.len() as u16 + 2);
    f.render_widget(Clear, area);
    f.render_stateful_widget(
        List::new(rows)
            .block(pane(&format!(":{input}")))
            .highlight_style(SELECTED),
        area,
        &mut state,
    );
}

fn render_hints(f: &mut Frame, app: &App, area: Rect) {
    let keys: &[(&str, &str)] = match app.view {
        View::Read(_) => &[
            ("j/k", "scroll"),
            ("o", "attachments"),
            ("a", "archive"),
            ("d", "trash"),
            ("D", "delete"),
            ("l", "labels"),
            ("q", "back"),
        ],
        View::List => &[
            ("j/k", "move"),
            ("g/G", "top/end"),
            ("C-d/u", "page"),
            ("enter", "read"),
            ("o", "attachments"),
            ("a", "archive"),
            ("d", "trash"),
            ("D", "delete"),
            ("l", "labels"),
            ("r/R", "refresh/force"),
            ("/", "search"),
            (":", "go to label"),
            ("q", "quit"),
        ],
        _ => &[("enter", "select"), ("esc", "cancel")],
    };
    let mut spans = Vec::new();
    for (k, what) in keys {
        spans.push(Span::styled(
            format!(" <{k}>"),
            Style::default().fg(KEY).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {what} "), Style::default().fg(DIM)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_status(f: &mut Frame, app: &App, area: Rect) {
    let line = match &app.view {
        View::Search(q) => Span::styled(format!("/{q}"), Style::default().fg(Color::Yellow)),
        View::ConfirmDelete => Span::styled(
            " delete permanently? y/n ",
            Style::default()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ),
        _ if app.error => Span::styled(
            format!(" {} ", app.status),
            Style::default().fg(Color::White).bg(Color::Red),
        ),
        _ => Span::styled(&app.status, Style::default().fg(Color::Yellow)),
    };
    f.render_widget(Paragraph::new(line), area);
}

fn pane(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(KEY))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
}

/// One glyph per attachment kind, by file extension.
fn icon(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map_or("", |(_, e)| e)
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "bmp" | "svg" => "\u{1f5bc}\u{fe0f}",
        "pdf" => "\u{1f4d5}",
        "doc" | "docx" | "odt" | "rtf" | "txt" | "md" => "\u{1f4dd}",
        "xls" | "xlsx" | "ods" | "csv" => "\u{1f4ca}",
        "ppt" | "pptx" | "odp" => "\u{1f4fd}\u{fe0f}",
        "zip" | "7z" | "rar" | "gz" | "tar" | "tgz" => "\u{1f4e6}",
        "mp3" | "wav" | "m4a" | "ogg" | "flac" => "\u{1f3b5}",
        "mp4" | "mov" | "mkv" | "avi" | "webm" => "\u{1f3ac}",
        "ics" => "\u{1f4c5}",
        _ => "\u{1f4ce}",
    }
}

fn popup(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// "Sat, 07 Nov 2026 15:50:37 +0000" -> "07 Nov 2026 15:50"
fn short_date(date: &str) -> String {
    let date = date.split_once(", ").map_or(date, |(_, d)| d);
    let mut parts = date.split_whitespace();
    let day = parts.next().unwrap_or("");
    let mon = parts.next().unwrap_or("");
    let year = parts.next().unwrap_or("");
    let time = parts.next().unwrap_or("");
    let time = time.rsplit_once(':').map_or(time, |(hm, _)| hm);
    format!("{day:>2} {mon} {year} {time}")
}

/// "Name <addr>" -> "Name"; bare address stays.
fn short_from(from: &str) -> String {
    let name = from.split_once(" <").map_or(from, |(n, _)| n);
    name.trim_matches('"').chars().take(26).collect()
}

fn jump_matches(labels: &[Label], input: &str) -> Vec<usize> {
    let needle = input.to_lowercase();
    labels
        .iter()
        .enumerate()
        .filter(|(_, l)| l.name.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect()
}

fn handle_key(ctx: &mut Ctx, app: &mut App, key: KeyEvent) -> Result<bool> {
    if key.code != KeyCode::Char('g') {
        app.pending_g = false;
    }
    match &mut app.view {
        View::Search(q) => {
            match key.code {
                KeyCode::Char(c) => q.push(c),
                KeyCode::Backspace => {
                    q.pop();
                }
                KeyCode::Esc => app.view = View::List,
                KeyCode::Enter => {
                    let q = q.trim().to_string();
                    if q.is_empty() {
                        app.view = View::List;
                    } else {
                        load(ctx, app, Selection::Query(q))?;
                    }
                }
                _ => {}
            }
            Ok(false)
        }
        View::Jump { input, cursor } => {
            match key.code {
                KeyCode::Char(c) => {
                    input.push(c);
                    *cursor = 0;
                }
                KeyCode::Backspace => {
                    input.pop();
                    *cursor = 0;
                }
                KeyCode::Down => *cursor += 1,
                KeyCode::Up => *cursor = cursor.saturating_sub(1),
                KeyCode::Esc => app.view = View::List,
                KeyCode::Enter => {
                    let matches = jump_matches(&app.labels, input);
                    if let Some(&i) = matches.get((*cursor).min(matches.len().saturating_sub(1))) {
                        app.label = i;
                        load_label(ctx, app)?;
                    }
                }
                _ => {}
            }
            if let View::Jump { input, cursor } = &mut app.view {
                let n = jump_matches(&app.labels, input).len();
                *cursor = (*cursor).min(n.saturating_sub(1));
            }
            Ok(false)
        }
        View::Attachments { msg, cursor } => {
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    *cursor = (*cursor + 1).min(msg.attachments.len() - 1)
                }
                KeyCode::Up | KeyCode::Char('k') => *cursor = cursor.saturating_sub(1),
                KeyCode::Enter | KeyCode::Char('o') => {
                    let path = save_attachment(&msg.attachments[*cursor])?;
                    open::that_detached(&path)
                        .with_context(|| format!("open {}", path.display()))?;
                    app.status = format!("opened {}", path.display());
                }
                KeyCode::Char('s') => {
                    let path = save_attachment(&msg.attachments[*cursor])?;
                    app.status = format!("saved {}", path.display());
                }
                KeyCode::Char('q') | KeyCode::Esc => {
                    let View::Attachments { msg, .. } =
                        std::mem::replace(&mut app.view, View::List)
                    else {
                        unreachable!()
                    };
                    app.view = View::Read(msg);
                }
                _ => {}
            }
            Ok(false)
        }
        View::ConfirmDelete => {
            if key.code == KeyCode::Char('y') {
                act(ctx, app, Action::Delete)?;
            } else {
                app.view = View::List;
            }
            Ok(false)
        }
        View::LabelPicker { cursor } => {
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    *cursor = (*cursor + 1).min(app.labels.len() - 1)
                }
                KeyCode::Up | KeyCode::Char('k') => *cursor = cursor.saturating_sub(1),
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let id = app.labels[*cursor].id.clone();
                    let remove = app.page.items[app.cursor].labels.contains(&id);
                    act(ctx, app, Action::Label { id, remove })?;
                }
                KeyCode::Char('q') | KeyCode::Esc => app.view = View::List,
                _ => {}
            }
            Ok(false)
        }
        View::Read(_) => handle_read_key(ctx, app, key),
        View::List => handle_list_key(ctx, app, key),
    }
}

fn handle_read_key(ctx: &mut Ctx, app: &mut App, key: KeyEvent) -> Result<bool> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => app.scroll = app.scroll.saturating_add(1),
        KeyCode::Up | KeyCode::Char('k') => app.scroll = app.scroll.saturating_sub(1),
        KeyCode::Char('d') | KeyCode::Char('f') if ctrl => {
            app.scroll = app.scroll.saturating_add(HALF_PAGE as u16)
        }
        KeyCode::Char('u') | KeyCode::Char('b') if ctrl => {
            app.scroll = app.scroll.saturating_sub(HALF_PAGE as u16)
        }
        KeyCode::Char('g') => app.scroll = 0,
        KeyCode::Char('G') => app.scroll = u16::MAX / 2,
        KeyCode::Char('q') | KeyCode::Esc => app.view = View::List,
        KeyCode::Char('o') => {
            let View::Read(msg) = std::mem::replace(&mut app.view, View::List) else {
                unreachable!()
            };
            if msg.attachments.is_empty() {
                app.status = "no attachments".to_string();
                app.view = View::Read(msg);
            } else {
                app.view = View::Attachments { msg, cursor: 0 };
            }
        }
        _ => handle_action_key(ctx, app, key.code)?,
    }
    Ok(false)
}

fn handle_list_key(ctx: &mut Ctx, app: &mut App, key: KeyEvent) -> Result<bool> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char('c') if ctrl => return Ok(true),
        KeyCode::Esc => {
            if let Selection::Query(_) = app.sel {
                load_label(ctx, app)?;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => next_row(ctx, app, 1)?,
        KeyCode::Up | KeyCode::Char('k') => app.cursor = app.cursor.saturating_sub(1),
        KeyCode::Char('d') | KeyCode::Char('f') | KeyCode::PageDown
            if ctrl || key.code == KeyCode::PageDown =>
        {
            next_row(ctx, app, HALF_PAGE)?
        }
        KeyCode::Char('u') | KeyCode::Char('b') | KeyCode::PageUp
            if ctrl || key.code == KeyCode::PageUp =>
        {
            app.cursor = app.cursor.saturating_sub(HALF_PAGE)
        }
        KeyCode::Char('g') => {
            if app.pending_g {
                app.cursor = 0;
                app.pending_g = false;
            } else {
                app.pending_g = true;
            }
        }
        KeyCode::Char('G') => {
            while app.page.next.is_some() {
                next_row(ctx, app, usize::MAX)?;
            }
            app.cursor = app.page.items.len().saturating_sub(1);
        }
        KeyCode::Right | KeyCode::Char(']') | KeyCode::Tab => {
            app.label = (app.label + 1) % app.labels.len();
            load_label(ctx, app)?;
        }
        KeyCode::Left | KeyCode::Char('[') | KeyCode::BackTab => {
            app.label = (app.label + app.labels.len() - 1) % app.labels.len();
            load_label(ctx, app)?;
        }
        KeyCode::Char('r') => {
            reload(ctx, app, false)?;
            app.status = "refreshed".to_string();
        }
        KeyCode::Char('R') => {
            reload(ctx, app, true)?;
            app.status = "refreshed (cache dropped)".to_string();
        }
        KeyCode::Char('/') => app.view = View::Search(String::new()),
        KeyCode::Char(':') => {
            app.view = View::Jump {
                input: String::new(),
                cursor: 0,
            }
        }
        KeyCode::Enter => read(ctx, app)?,
        KeyCode::Char('o') => {
            if app
                .page
                .items
                .get(app.cursor)
                .is_some_and(|s| !s.attachments.is_empty())
            {
                read(ctx, app)?;
                if let View::Read(msg) = std::mem::replace(&mut app.view, View::List) {
                    app.view = View::Attachments { msg, cursor: 0 };
                }
            } else {
                app.status = "no attachments".to_string();
            }
        }
        _ => handle_action_key(ctx, app, key.code)?,
    }
    Ok(false)
}

fn handle_action_key(ctx: &mut Ctx, app: &mut App, code: KeyCode) -> Result<()> {
    if app.page.items.is_empty() {
        return Ok(());
    }
    match code {
        KeyCode::Char('a') => act(ctx, app, Action::Archive),
        KeyCode::Char('d') => act(ctx, app, Action::Trash),
        KeyCode::Char('D') => {
            app.view = View::ConfirmDelete;
            Ok(())
        }
        KeyCode::Char('l') => {
            app.view = View::LabelPicker { cursor: 0 };
            Ok(())
        }
        _ => Ok(()),
    }
}

fn load(ctx: &mut Ctx, app: &mut App, sel: Selection) -> Result<()> {
    let client = ctx.client;
    let page = ctx.call(app, client.page(&sel, None))?;
    app.sel = sel;
    app.page = page;
    app.cursor = 0;
    app.view = View::List;
    Ok(())
}

/// Re-reads the first page of the current selection and keeps the cursor on the same message.
/// `force` also drops the cached metadata of the loaded rows and re-reads the labels.
fn reload(ctx: &mut Ctx, app: &mut App, force: bool) -> Result<()> {
    let client = ctx.client;
    if force {
        let ids: Vec<String> = app.page.items.iter().map(|s| s.id.clone()).collect();
        client.forget(&ids)?;
        let labels = ctx.call(app, client.labels())?;
        let current = app.labels[app.label].id.clone();
        app.label = labels.iter().position(|l| l.id == current).unwrap_or(0);
        app.labels = labels;
    }
    let current = app.page.items.get(app.cursor).map(|s| s.id.clone());
    let sel = app.sel.clone();
    let page = ctx.call(app, client.page(&sel, None))?;
    app.page = page;
    app.cursor = current
        .and_then(|id| app.page.items.iter().position(|s| s.id == id))
        .unwrap_or(0)
        .min(app.page.items.len().saturating_sub(1));
    Ok(())
}

fn load_label(ctx: &mut Ctx, app: &mut App) -> Result<()> {
    let sel = Selection::Label(app.labels[app.label].id.clone());
    load(ctx, app, sel)
}

/// Moves the cursor down `n` rows, loading the next page when it runs past the end.
fn next_row(ctx: &mut Ctx, app: &mut App, n: usize) -> Result<()> {
    let target = app.cursor.saturating_add(n);
    if target >= app.page.items.len()
        && let Some(token) = app.page.next.clone()
    {
        let sel = app.sel.clone();
        let client = ctx.client;
        let next = ctx.call(app, client.page(&sel, Some(&token)))?;
        app.page.items.extend(next.items);
        app.page.next = next.next;
    }
    app.cursor = target.min(app.page.items.len().saturating_sub(1));
    Ok(())
}

fn read(ctx: &mut Ctx, app: &mut App) -> Result<()> {
    let Some(item) = app.page.items.get(app.cursor) else {
        return Ok(());
    };
    let id = item.id.clone();
    let client = ctx.client;
    let msg = ctx.call(app, client.message(&id))?;
    app.view = View::Read(msg);
    app.scroll = 0;
    Ok(())
}

/// Writes the attachment under ~/Downloads/meili and returns its path.
fn save_attachment(a: &Attachment) -> Result<std::path::PathBuf> {
    let dir = dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .context("no download dir")?
        .join("meili");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = dir.join(safe_name(&a.name));
    std::fs::write(&path, &a.data).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

fn act(ctx: &mut Ctx, app: &mut App, action: Action) -> Result<()> {
    let id = app.page.items[app.cursor].id.clone();
    let client = ctx.client;
    ctx.call(app, client.apply(&action, std::slice::from_ref(&id)))?;
    match action {
        Action::Label { id, remove } => {
            let labels = &mut app.page.items[app.cursor].labels;
            if remove {
                labels.retain(|l| *l != id);
            } else {
                labels.push(id);
            }
        }
        _ => {
            app.page.items.remove(app.cursor);
            app.cursor = app.cursor.min(app.page.items.len().saturating_sub(1));
            app.view = View::List;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_follows_extension_case_insensitively() {
        assert_eq!(icon("Informe.PDF"), icon("x.pdf"));
        assert_eq!(icon("a.jpeg"), icon("b.png"));
        assert_ne!(icon("a.pdf"), icon("a.xlsx"));
        assert_eq!(icon("noext"), icon("weird.bin"));
    }

    #[test]
    fn short_date_drops_weekday_and_seconds() {
        assert_eq!(
            short_date("Sat, 07 Nov 2026 15:50:37 +0000"),
            "07 Nov 2026 15:50"
        );
        assert_eq!(
            short_date("Sat, 7 Nov 2026 13:45:49 +0100"),
            " 7 Nov 2026 13:45"
        );
    }

    #[test]
    fn short_from_keeps_display_name() {
        assert_eq!(
            short_from("Northwind Hosting <billing@northwind.example>"),
            "Northwind Hosting"
        );
        assert_eq!(short_from("\"CI\" <ci@harbor.example>"), "CI");
        assert_eq!(short_from("plain@addr.com"), "plain@addr.com");
    }
}
