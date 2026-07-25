use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
    DefaultTerminal,
};

use crate::agents::coordinator::Coordinator;
use rig_core::completion::CompletionModel;

const MAX_VISIBLE_MESSAGES: usize = 1000;

struct Message {
    role: String,
    content: String,
}

pub struct App {
    messages: Vec<Message>,
    input: String,
    scroll_offset: usize,
    loading: bool,
}

impl App {
    fn new() -> Self {
        Self {
            messages: Vec::new(),
            input: String::new(),
            scroll_offset: 0,
            loading: false,
        }
    }

    fn add_message(&mut self, role: &str, content: &str) {
        self.messages.push(Message {
            role: role.to_string(),
            content: content.to_string(),
        });
        if self.messages.len() > MAX_VISIBLE_MESSAGES {
            self.messages.remove(0);
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
            " kubedoc — Interactive Session  |  Ctrl+D quit  |  Ctrl+L clear  |  ↑↓ scroll",
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

        let max_scroll = message_lines.len().saturating_sub(messages_area.height as usize);
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
                frame.set_cursor(cursor_x, cursor_y);
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

pub async fn run<M: CompletionModel + 'static>(
    coordinator: Coordinator<M>,
    session_id: &str,
    session_manager: Option<&crate::session::SessionManager>,
    mut session_data: Option<crate::session::SessionData>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = ratatui::try_init()?;

    let mut app = App::new();
    app.add_message(
        "system",
        &format!("Session: {session_id} — type your Kubernetes questions below."),
    );

    let result = run_loop(&mut terminal, &mut app, &coordinator, session_manager, &mut session_data).await;

    if let (Some(sm), Some(sd)) = (session_manager, &session_data) {
        let _ = sm.save(sd);
    }

    ratatui::try_restore()?;
    result
}

async fn run_loop<M: CompletionModel + 'static>(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    coordinator: &Coordinator<M>,
    session_manager: Option<&crate::session::SessionManager>,
    session_data: &mut Option<crate::session::SessionData>,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        terminal.draw(|frame| app.render(frame))?;

        if !crossterm::event::poll(Duration::from_millis(100))? {
            continue;
        }

        let event = event::read()?;
        if let Event::Key(key) = event {
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
                    let prompt = std::mem::take(&mut app.input);
                    app.add_message("user", &prompt);

                    if let (Some(sm), Some(sd)) = (session_manager, session_data.as_mut()) {
                        let _ = sm.add_entry(sd, "user", &prompt);
                    }

                    app.loading = true;
                    terminal.draw(|frame| app.render(frame))?;

                    match coordinator.run(&prompt).await {
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
    }

    Ok(())
}
