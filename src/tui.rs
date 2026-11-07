use std::future::Future;

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

use crate::gmail::Client;
use crate::mail::{Action, Label, Page, Selection};

const HALF_PAGE: usize = 15;

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
    Read(String),
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
    loop {
        ctx.terminal.draw(|f| render(f, app))?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match handle_key(ctx, app, key) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(e) => {
                app.status = format!("{e:#}");
                app.error = true;
            }
        }
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
        View::Read(text) => render_read(f, app, text, body),
        _ => render_table(f, app, body),
    }
    match &app.view {
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
        let clip = if s.has_attachment { "@" } else { "" };
        Row::new(vec![
            Cell::from(clip).style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Cell::from(short_date(&s.date)).style(Style::default().fg(DIM)),
            Cell::from(short_from(&s.from)).style(Style::default().fg(KEY)),
            Cell::from(s.subject.as_str()).style(text),
        ])
    });
    let header = Row::new(vec![" ", "DATE", "FROM", "SUBJECT"]).style(
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
    );
    let widths = [
        Constraint::Length(1),
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

fn render_read(f: &mut Frame, app: &App, text: &str, area: Rect) {
    let item = &app.page.items[app.cursor];
    f.render_widget(
        Paragraph::new(text)
            .block(pane(&format!("{}  <{}>", item.subject, item.from)))
            .wrap(Wrap { trim: false })
            .scroll((app.scroll, 0)),
        area,
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
            ("a", "archive"),
            ("d", "trash"),
            ("D", "delete"),
            ("l", "labels"),
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
        KeyCode::Char('/') => app.view = View::Search(String::new()),
        KeyCode::Char(':') => {
            app.view = View::Jump {
                input: String::new(),
                cursor: 0,
            }
        }
        KeyCode::Enter => read(ctx, app)?,
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
    let body = ctx.call(app, client.message(&id))?;
    app.view = View::Read(body);
    app.scroll = 0;
    Ok(())
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
