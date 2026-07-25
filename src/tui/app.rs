use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    DefaultTerminal,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
};

use crate::agents::coordinator::Coordinator;
use crate::tui::event::{Event, EventHandler};
use rig_core::completion::CompletionModel;

const MAX_VISIBLE_MESSAGES: usize = 1000;

struct Message {
    role: String,
    content: String,
}

pub struct App {
    messages: VecDeque<Message>,
    input: String,
    scroll_offset: usize,
    loading: bool,
}

impl App {
    fn new() -> Self {
        Self {
            messages: VecDeque::new(),
            input: String::new(),
            scroll_offset: 0,
            loading: false,
        }
    }

    fn add_message(&mut self, role: &str, content: &str) {
        self.messages.push_back(Message {
            role: role.to_string(),
            content: content.to_string(),
        });
        if self.messages.len() > MAX_VISIBLE_MESSAGES {
            self.messages.pop_front();
        }
        self.scroll_offset = 0;
    }

    fn render(&self, frame: &mut ratatui::Frame) {
        let area = frame.area();

        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(area);
        let header_area = chunks[0];
        let messages_area = chunks[1];
        let input_area = chunks[2];

        let header = Span::styled(
            " kubedoc — Interactive Session  |  /help commands  |  Ctrl+D quit  |  Ctrl+L clear",
            Style::default()
                .fg(Color::White)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_widget(Paragraph::new(header), header_area);

        let message_lines: Vec<Line> = self
            .messages
            .iter()
            .flat_map(|msg| {
                let style = match msg.role.as_str() {
                    "user" => Style::default().fg(Color::Green).bold(),
                    "assistant" => Style::default().fg(Color::Cyan),
                    "error" => Style::default().fg(Color::Red),
                    _ => Style::default().fg(Color::DarkGray),
                };
                let prefix = match msg.role.as_str() {
                    "user" => "You",
                    "assistant" => "Agent",
                    _ => &msg.role,
                };
                let header_line = Line::from(Span::styled(
                    format!(" {prefix}>"),
                    style.add_modifier(Modifier::BOLD),
                ));
                let content_lines: Vec<Line> = msg
                    .content
                    .lines()
                    .map(|line| Line::from(Span::styled(format!("   {line}"), style)))
                    .collect();

                let mut lines = Vec::with_capacity(content_lines.len() + 1);
                lines.push(header_line);
                lines.extend(content_lines);
                lines.push(Line::from(""));
                lines
            })
            .collect();

        let max_scroll = message_lines
            .len()
            .saturating_sub(messages_area.height as usize);
        let scroll = self.scroll_offset.min(max_scroll);

        let messages_widget = Paragraph::new(message_lines)
            .block(Block::bordered().title(" Conversation "))
            .wrap(Wrap { trim: false })
            .scroll((scroll as u16, 0));
        frame.render_widget(messages_widget, messages_area);

        let input_style = if self.loading {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        };
        let input_display = if self.loading {
            " Waiting for agent response...".to_string()
        } else if self.input.is_empty() {
            " Type your Kubernetes question here...".to_string()
        } else {
            format!(" {}", self.input)
        };
        let input_widget = Paragraph::new(input_display)
            .block(Block::bordered().title(" Input "))
            .style(input_style);
        frame.render_widget(input_widget, input_area);

        if !self.loading {
            let cursor_x = input_area.x + 2 + self.input.len() as u16;
            let cursor_y = input_area.y + 1;
            if cursor_x < input_area.x + input_area.width.saturating_sub(1) {
                frame.set_cursor_position((cursor_x, cursor_y));
            }
        }
    }

    fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(3);
    }

    fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(3);
    }
}

type AgentFuture = Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>>>>;

pub async fn run<M: CompletionModel + Clone + 'static>(
    coordinator: Coordinator<M>,
    session_id: &str,
    session_manager: Option<&crate::session::SessionManager>,
    mut session_data: Option<crate::session::SessionData>,
    audit_log: Option<Arc<crate::audit::AuditLog>>,
) -> anyhow::Result<()> {
    let mut terminal = ratatui::try_init()?;

    if let Some(ref log) = audit_log {
        let _ = log.session_start();
    }

    let mut app = App::new();
    app.add_message(
        "system",
        &format!("Session: {session_id} — type your Kubernetes questions below."),
    );

    let result = run_loop(
        &mut terminal,
        &mut app,
        &coordinator,
        session_manager,
        &mut session_data,
        audit_log.as_deref(),
    )
    .await;

    if let Some(ref log) = audit_log {
        let _ = log.session_end();
    }

    if let (Some(sm), Some(sd)) = (session_manager, &session_data) {
        let _ = sm.save(sd);
    }

    ratatui::try_restore()?;
    result
}

async fn run_loop<M: CompletionModel + Clone + 'static>(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    coordinator: &Coordinator<M>,
    session_manager: Option<&crate::session::SessionManager>,
    session_data: &mut Option<crate::session::SessionData>,
    audit_log: Option<&crate::audit::AuditLog>,
) -> anyhow::Result<()> {
    let mut events = EventHandler::new(Duration::from_millis(100));
    let mut agent_fut: Option<AgentFuture> = None;

    loop {
        terminal.draw(|frame| app.render(frame))?;

        if app.loading {
            // While waiting for agent, race the agent future against terminal events.
            if let Some(fut) = agent_fut.as_mut() {
                tokio::select! {
                    result = fut.as_mut() => {
                        match result {
                            Ok(response) => {
                                app.add_message("assistant", &response);
                                if let (Some(sm), Some(sd)) = (session_manager, session_data.as_mut()) {
                                    let _ = sm.add_entry(sd, "assistant", &response);
                                }
                            }
                            Err(e) => {
                                app.add_message("error", &format!("Agent error: {e}"));
                            }
                        }
                        app.loading = false;
                        agent_fut = None;
                    }
                    event = events.next() => {
                        match event {
                            Some(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                                match key.code {
                                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                                    KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                        app.messages.clear();
                                    }
                                    _ => {}
                                }
                            }
                            Some(_) | None => {}
                        }
                    }
                }
            }
            continue;
        }

        // Not loading — wait for next event.
        let event = match events.next().await {
            Some(e) => e,
            None => break,
        };

        let Event::Key(key) = event;

        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
            KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.messages.clear();
            }
            KeyCode::Enter if !app.input.is_empty() && !app.loading => {
                let input = std::mem::take(&mut app.input);

                if input.starts_with('/') {
                    match handle_slash_command(
                        &input,
                        app,
                        session_manager,
                        session_data.as_mut(),
                    ) {
                        CommandResult::Continue => {}
                        CommandResult::Exit => break,
                    }
                    continue;
                }

                app.add_message("user", &input);

                if let Some(log) = audit_log {
                    let _ = log.user_prompt(&input);
                }

                if let (Some(sm), Some(sd)) = (session_manager, session_data.as_mut()) {
                    let _ = sm.add_entry(sd, "user", &input);
                }

                app.loading = true;
                let prompt = input;
                let coord = coordinator.clone();
                agent_fut = Some(Box::pin(async move { coord.run(&prompt).await }));
            }
            KeyCode::Char(c) => {
                app.input.push(c);
            }
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Up => {
                app.scroll_up();
            }
            KeyCode::Down => {
                app.scroll_down();
            }
            KeyCode::PageUp => {
                app.scroll_offset = app.scroll_offset.saturating_add(20);
            }
            KeyCode::PageDown => {
                app.scroll_offset = app.scroll_offset.saturating_sub(20);
            }
            _ => {}
        }
    }

    Ok(())
}

enum CommandResult {
    Continue,
    Exit,
}

fn handle_slash_command(
    input: &str,
    app: &mut App,
    session_manager: Option<&crate::session::SessionManager>,
    session_data: Option<&mut crate::session::SessionData>,
) -> CommandResult {
    let parts: Vec<&str> = input.trim().splitn(2, ' ').collect();
    let cmd = parts[0].to_lowercase();
    let arg = parts.get(1).map(|s| s.trim());

    match cmd.as_str() {
        "/exit" | "/quit" | "/q" => {
            return CommandResult::Exit;
        }
        "/help" | "/h" => {
            app.add_message(
                "system",
                "\
Available commands:
  /sessions, /ls          List saved sessions
  /show <session-id>      Show entries from a session
  /load <session-id>      Load a previous session
  /delete <session-id>    Delete a session
  /exit, /quit, /q        Exit kubedoc
  /help, /h               Show this help",
            );
        }
        "/sessions" | "/ls" => {
            let Some(sm) = session_manager else {
                app.add_message("error", "Session manager not available.");
                return CommandResult::Continue;
            };
            match sm.list() {
                Ok(sessions) => {
                    if sessions.is_empty() {
                        app.add_message("system", "No saved sessions.");
                    } else {
                        let mut out = String::from("Saved sessions:\n");
                        for s in &sessions {
                            let id = &s.session_id;
                            let entries = s.entries.len();
                            let updated = s.updated_at.get(..19).unwrap_or(&s.updated_at);
                            out.push_str(&format!("  {id:<32}  {entries} entries  {updated}\n"));
                        }
                        app.add_message("system", &out);
                    }
                }
                Err(e) => {
                    app.add_message("error", &format!("Failed to list sessions: {e}"));
                }
            }
        }
        "/show" => {
            let Some(session_id) = arg else {
                app.add_message("error", "Usage: /show <session-id>");
                return CommandResult::Continue;
            };
            let Some(sm) = session_manager else {
                app.add_message("error", "Session manager not available.");
                return CommandResult::Continue;
            };
            match sm.load(session_id) {
                Ok(Some(data)) => {
                    let mut out = format!(
                        "Session: {}  (created {})\n---\n",
                        data.session_id,
                        data.created_at.get(..19).unwrap_or(&data.created_at)
                    );
                    for entry in &data.entries {
                        let prefix = match entry.role.as_str() {
                            "user" => "You",
                            "assistant" => "Agent",
                            other => other,
                        };
                        out.push_str(&format!("[{}]\n{}\n\n", prefix, entry.content));
                    }
                    app.add_message("system", &out);
                }
                Ok(None) => {
                    app.add_message("error", &format!("Session not found: {session_id}"));
                }
                Err(e) => {
                    app.add_message("error", &format!("Failed to load session: {e}"));
                }
            }
        }
        "/load" => {
            let Some(session_id) = arg else {
                app.add_message("error", "Usage: /load <session-id>");
                return CommandResult::Continue;
            };
            let Some(sm) = session_manager else {
                app.add_message("error", "Session manager not available.");
                return CommandResult::Continue;
            };
            // Save current session first
            if let Some(ref sd) = session_data {
                let _ = sm.save(sd);
            }
            match sm.load(session_id) {
                Ok(Some(data)) => {
                    // Clear current messages and replay loaded session
                    app.messages.clear();
                    app.add_message(
                        "system",
                        &format!(
                            "Loaded session: {} ({} entries)",
                            data.session_id,
                            data.entries.len()
                        ),
                    );
                    for entry in &data.entries {
                        let role = entry.role.as_str();
                        app.add_message(role, &entry.content);
                    }
                    // Update session data in place
                    if let Some(sd) = session_data {
                        *sd = data;
                    }
                }
                Ok(None) => {
                    app.add_message("error", &format!("Session not found: {session_id}"));
                }
                Err(e) => {
                    app.add_message("error", &format!("Failed to load session: {e}"));
                }
            }
        }
        "/delete" => {
            let Some(session_id) = arg else {
                app.add_message("error", "Usage: /delete <session-id>");
                return CommandResult::Continue;
            };
            let Some(sm) = session_manager else {
                app.add_message("error", "Session manager not available.");
                return CommandResult::Continue;
            };
            // Don't allow deleting the current session
            if let Some(sd) = session_data
                && sd.session_id == session_id
            {
                app.add_message("error", "Cannot delete the current session.");
                return CommandResult::Continue;
            }
            match sm.load(session_id) {
                Ok(Some(_)) => {
                    let _ = sm.delete(session_id);
                    app.add_message("system", &format!("Deleted session: {session_id}"));
                }
                Ok(None) => {
                    app.add_message("error", &format!("Session not found: {session_id}"));
                }
                Err(e) => {
                    app.add_message("error", &format!("Failed to delete session: {e}"));
                }
            }
        }
        _ => {
            app.add_message(
                "error",
                &format!("Unknown command: {cmd}\nType /help for available commands."),
            );
        }
    }

    CommandResult::Continue
}
