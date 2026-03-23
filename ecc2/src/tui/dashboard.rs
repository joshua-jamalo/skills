use std::collections::HashMap;

use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, Paragraph, Tabs},
};
use tokio::sync::broadcast;

use crate::config::Config;
use crate::session::output::{OutputEvent, OutputLine, SessionOutputStore, OUTPUT_BUFFER_LIMIT};
use crate::session::store::StateStore;
use crate::session::{Session, SessionState};

pub struct Dashboard {
    db: StateStore,
    output_store: SessionOutputStore,
    output_rx: broadcast::Receiver<OutputEvent>,
    sessions: Vec<Session>,
    session_output_cache: HashMap<String, Vec<OutputLine>>,
    selected_pane: Pane,
    selected_session: usize,
    show_help: bool,
    output_follow: bool,
    output_scroll_offset: usize,
    last_output_height: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Pane {
    Sessions,
    Output,
    Metrics,
}

impl Dashboard {
    pub fn new(db: StateStore, cfg: Config) -> Self {
        Self::with_output_store(db, cfg, SessionOutputStore::default())
    }

    pub fn with_output_store(
        db: StateStore,
        _cfg: Config,
        output_store: SessionOutputStore,
    ) -> Self {
        let sessions = db.list_sessions().unwrap_or_default();
        let output_rx = output_store.subscribe();

        let mut dashboard = Self {
            db,
            output_store,
            output_rx,
            sessions,
            session_output_cache: HashMap::new(),
            selected_pane: Pane::Sessions,
            selected_session: 0,
            show_help: false,
            output_follow: true,
            output_scroll_offset: 0,
            last_output_height: 0,
        };
        dashboard.sync_selected_output();
        dashboard
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Header
                Constraint::Min(10),   // Main content
                Constraint::Length(3), // Status bar
            ])
            .split(frame.area());

        self.render_header(frame, chunks[0]);

        if self.show_help {
            self.render_help(frame, chunks[1]);
        } else {
            let main_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(35), // Session list
                    Constraint::Percentage(65), // Output/details
                ])
                .split(chunks[1]);

            self.render_sessions(frame, main_chunks[0]);

            let right_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(70), // Output
                    Constraint::Percentage(30), // Metrics
                ])
                .split(main_chunks[1]);

            self.render_output(frame, right_chunks[0]);
            self.render_metrics(frame, right_chunks[1]);
        }

        self.render_status_bar(frame, chunks[2]);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let running = self
            .sessions
            .iter()
            .filter(|s| s.state == SessionState::Running)
            .count();
        let total = self.sessions.len();

        let title = format!(" ECC 2.0 | {running} running / {total} total ");
        let tabs = Tabs::new(vec!["Sessions", "Output", "Metrics"])
            .block(Block::default().borders(Borders::ALL).title(title))
            .select(match self.selected_pane {
                Pane::Sessions => 0,
                Pane::Output => 1,
                Pane::Metrics => 2,
            })
            .highlight_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            );

        frame.render_widget(tabs, area);
    }

    fn render_sessions(&self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .sessions
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let state_icon = match s.state {
                    SessionState::Running => "●",
                    SessionState::Idle => "○",
                    SessionState::Completed => "✓",
                    SessionState::Failed => "✗",
                    SessionState::Stopped => "■",
                    SessionState::Pending => "◌",
                };
                let style = if i == self.selected_session {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let text = format!(
                    "{state_icon} {} [{}] {}",
                    &s.id[..8.min(s.id.len())],
                    s.agent_type,
                    s.task
                );
                ListItem::new(text).style(style)
            })
            .collect();

        let border_style = if self.selected_pane == Pane::Sessions {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        };

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Sessions ")
                .border_style(border_style),
        );
        frame.render_widget(list, area);
    }

    fn render_output(&mut self, frame: &mut Frame, area: Rect) {
        self.sync_output_scroll(area.height.saturating_sub(2) as usize);

        let content = if self.sessions.get(self.selected_session).is_some() {
            let lines = self.selected_output_lines();

            if lines.is_empty() {
                "Waiting for session output...".to_string()
            } else {
                lines
                    .iter()
                    .map(|line| line.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        } else {
            "No sessions. Press 'n' to start one.".to_string()
        };

        let border_style = if self.selected_pane == Pane::Output {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        };

        let paragraph = Paragraph::new(content)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Output ")
                    .border_style(border_style),
            )
            .scroll((self.output_scroll_offset as u16, 0));
        frame.render_widget(paragraph, area);
    }

    fn render_metrics(&self, frame: &mut Frame, area: Rect) {
        let content = if let Some(session) = self.sessions.get(self.selected_session) {
            let m = &session.metrics;
            format!(
                "Tokens: {} | Tools: {} | Files: {} | Cost: ${:.4} | Duration: {}s",
                m.tokens_used, m.tool_calls, m.files_changed, m.cost_usd, m.duration_secs
            )
        } else {
            "No metrics available".to_string()
        };

        let border_style = if self.selected_pane == Pane::Metrics {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        };

        let paragraph = Paragraph::new(content).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Metrics ")
                .border_style(border_style),
        );
        frame.render_widget(paragraph, area);
    }

    fn render_status_bar(&self, frame: &mut Frame, area: Rect) {
        let text = " [n]ew session  [s]top  [Tab] switch pane  [j/k] scroll  [?] help  [q]uit ";
        let paragraph = Paragraph::new(text)
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(paragraph, area);
    }

    fn render_help(&self, frame: &mut Frame, area: Rect) {
        let help = vec![
            "Keyboard Shortcuts:",
            "",
            "  n       New session",
            "  s       Stop selected session",
            "  Tab     Next pane",
            "  S-Tab   Previous pane",
            "  j/↓     Scroll down",
            "  k/↑     Scroll up",
            "  r       Refresh",
            "  ?       Toggle help",
            "  q/C-c   Quit",
        ];

        let paragraph = Paragraph::new(help.join("\n")).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help ")
                .border_style(Style::default().fg(Color::Yellow)),
        );
        frame.render_widget(paragraph, area);
    }

    pub fn next_pane(&mut self) {
        self.selected_pane = match self.selected_pane {
            Pane::Sessions => Pane::Output,
            Pane::Output => Pane::Metrics,
            Pane::Metrics => Pane::Sessions,
        };
    }

    pub fn prev_pane(&mut self) {
        self.selected_pane = match self.selected_pane {
            Pane::Sessions => Pane::Metrics,
            Pane::Output => Pane::Sessions,
            Pane::Metrics => Pane::Output,
        };
    }

    pub fn scroll_down(&mut self) {
        if self.selected_pane == Pane::Sessions && !self.sessions.is_empty() {
            self.selected_session = (self.selected_session + 1).min(self.sessions.len() - 1);
            self.reset_output_view();
            self.sync_selected_output();
        } else if self.selected_pane == Pane::Output {
            let max_scroll = self.max_output_scroll();

            if self.output_follow {
                return;
            }

            if self.output_scroll_offset >= max_scroll.saturating_sub(1) {
                self.output_follow = true;
                self.output_scroll_offset = max_scroll;
            } else {
                self.output_scroll_offset = self.output_scroll_offset.saturating_add(1);
            }
        }
    }

    pub fn scroll_up(&mut self) {
        if self.selected_pane == Pane::Sessions {
            self.selected_session = self.selected_session.saturating_sub(1);
            self.reset_output_view();
            self.sync_selected_output();
        } else if self.selected_pane == Pane::Output {
            if self.output_follow {
                self.output_follow = false;
                self.output_scroll_offset = self.max_output_scroll();
            }

            self.output_scroll_offset = self.output_scroll_offset.saturating_sub(1);
        } else {
            self.output_scroll_offset = self.output_scroll_offset.saturating_sub(1);
        }
    }

    pub fn new_session(&mut self) {
        // TODO: Open a dialog to create a new session
        tracing::info!("New session dialog requested");
    }

    pub fn stop_selected(&mut self) {
        if let Some(session) = self.sessions.get(self.selected_session) {
            let _ = self.db.update_state(&session.id, &SessionState::Stopped);
            self.refresh();
        }
    }

    pub fn refresh(&mut self) {
        self.sessions = self.db.list_sessions().unwrap_or_default();
        self.clamp_selected_session();
        self.sync_selected_output();
    }

    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    pub async fn tick(&mut self) {
        loop {
            match self.output_rx.try_recv() {
                Ok(_event) => {}
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Closed) => break,
            }
        }

        self.sessions = self.db.list_sessions().unwrap_or_default();
        self.clamp_selected_session();
        self.sync_selected_output();
    }

    fn clamp_selected_session(&mut self) {
        if self.sessions.is_empty() {
            self.selected_session = 0;
        } else {
            self.selected_session = self.selected_session.min(self.sessions.len() - 1);
        }
    }

    fn sync_selected_output(&mut self) {
        let Some(session_id) = self.selected_session_id().map(ToOwned::to_owned) else {
            return;
        };

        match self.db.get_output_lines(&session_id, OUTPUT_BUFFER_LIMIT) {
            Ok(lines) => {
                self.output_store.replace_lines(&session_id, lines.clone());
                self.session_output_cache.insert(session_id, lines);
            }
            Err(error) => {
                tracing::warn!("Failed to load session output: {error}");
            }
        }
    }

    fn selected_session_id(&self) -> Option<&str> {
        self.sessions
            .get(self.selected_session)
            .map(|session| session.id.as_str())
    }

    fn selected_output_lines(&self) -> &[OutputLine] {
        self.selected_session_id()
            .and_then(|session_id| self.session_output_cache.get(session_id))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn sync_output_scroll(&mut self, viewport_height: usize) {
        self.last_output_height = viewport_height.max(1);
        let max_scroll = self.max_output_scroll();

        if self.output_follow {
            self.output_scroll_offset = max_scroll;
        } else {
            self.output_scroll_offset = self.output_scroll_offset.min(max_scroll);
        }
    }

    fn max_output_scroll(&self) -> usize {
        self.selected_output_lines()
            .len()
            .saturating_sub(self.last_output_height.max(1))
    }

    fn reset_output_view(&mut self) {
        self.output_follow = true;
        self.output_scroll_offset = 0;
    }

    #[cfg(test)]
    fn selected_output_text(&self) -> String {
        self.selected_output_lines()
            .iter()
            .map(|line| line.text.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use std::env;

    use anyhow::Result;
    use chrono::Utc;
    use uuid::Uuid;

    use super::{Dashboard, Pane};
    use crate::config::Config;
    use crate::session::output::OutputStream;
    use crate::session::store::StateStore;
    use crate::session::{Session, SessionMetrics, SessionState};

    #[test]
    fn refresh_loads_selected_session_output_and_follows_tail() -> Result<()> {
        let db_path = env::temp_dir().join(format!("ecc2-dashboard-{}.db", Uuid::new_v4()));
        let db = StateStore::open(&db_path)?;
        let now = Utc::now();

        db.insert_session(&Session {
            id: "session-1".to_string(),
            task: "tail output".to_string(),
            agent_type: "claude".to_string(),
            state: SessionState::Running,
            worktree: None,
            created_at: now,
            updated_at: now,
            metrics: SessionMetrics::default(),
        })?;

        for index in 0..12 {
            db.append_output_line("session-1", OutputStream::Stdout, &format!("line {index}"))?;
        }

        let mut dashboard = Dashboard::new(db, Config::default());
        dashboard.selected_pane = Pane::Output;
        dashboard.refresh();
        dashboard.sync_output_scroll(4);

        assert_eq!(dashboard.output_scroll_offset, 8);
        assert!(dashboard.selected_output_text().contains("line 11"));

        let _ = std::fs::remove_file(db_path);

        Ok(())
    }

    #[test]
    fn scrolling_up_disables_follow_mode() -> Result<()> {
        let db_path = env::temp_dir().join(format!("ecc2-dashboard-{}.db", Uuid::new_v4()));
        let db = StateStore::open(&db_path)?;
        let now = Utc::now();

        db.insert_session(&Session {
            id: "session-1".to_string(),
            task: "inspect output".to_string(),
            agent_type: "claude".to_string(),
            state: SessionState::Running,
            worktree: None,
            created_at: now,
            updated_at: now,
            metrics: SessionMetrics::default(),
        })?;

        for index in 0..6 {
            db.append_output_line("session-1", OutputStream::Stdout, &format!("line {index}"))?;
        }

        let mut dashboard = Dashboard::new(db, Config::default());
        dashboard.selected_pane = Pane::Output;
        dashboard.refresh();
        dashboard.sync_output_scroll(3);
        dashboard.scroll_up();

        assert!(!dashboard.output_follow);
        assert_eq!(dashboard.output_scroll_offset, 2);

        let _ = std::fs::remove_file(db_path);

        Ok(())
    }
}
