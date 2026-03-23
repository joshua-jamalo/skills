use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;

use super::output::{OutputStream, SessionOutputStore};
use super::store::StateStore;
use super::SessionState;

pub async fn capture_command_output(
    db_path: PathBuf,
    session_id: String,
    mut command: Command,
    output_store: SessionOutputStore,
) -> Result<ExitStatus> {
    let result = async {
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to start process for session {}", session_id))?;

        update_session_state(&db_path, &session_id, SessionState::Running)?;

        let stdout = child.stdout.take().context("Child stdout was not piped")?;
        let stderr = child.stderr.take().context("Child stderr was not piped")?;

        let stdout_task = tokio::spawn(capture_stream(
            db_path.clone(),
            session_id.clone(),
            stdout,
            OutputStream::Stdout,
            output_store.clone(),
        ));
        let stderr_task = tokio::spawn(capture_stream(
            db_path.clone(),
            session_id.clone(),
            stderr,
            OutputStream::Stderr,
            output_store,
        ));

        let status = child.wait().await?;
        stdout_task.await??;
        stderr_task.await??;

        let final_state = if status.success() {
            SessionState::Completed
        } else {
            SessionState::Failed
        };
        update_session_state(&db_path, &session_id, final_state)?;

        Ok(status)
    }
    .await;

    if result.is_err() {
        let _ = update_session_state(&db_path, &session_id, SessionState::Failed);
    }

    result
}

async fn capture_stream<R>(
    db_path: PathBuf,
    session_id: String,
    reader: R,
    stream: OutputStream,
    output_store: SessionOutputStore,
) -> Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        output_store.push_line(&session_id, stream, line.clone());
        append_output_line(&db_path, &session_id, stream, &line)?;
    }

    Ok(())
}

fn append_output_line(
    db_path: &Path,
    session_id: &str,
    stream: OutputStream,
    line: &str,
) -> Result<()> {
    let db = StateStore::open(db_path)?;
    db.append_output_line(session_id, stream, line)?;
    Ok(())
}

fn update_session_state(db_path: &Path, session_id: &str, state: SessionState) -> Result<()> {
    let db = StateStore::open(db_path)?;
    db.update_state(session_id, &state)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::env;

    use anyhow::Result;
    use chrono::Utc;
    use tokio::process::Command;
    use uuid::Uuid;

    use super::capture_command_output;
    use crate::session::output::{SessionOutputStore, OUTPUT_BUFFER_LIMIT};
    use crate::session::store::StateStore;
    use crate::session::{Session, SessionMetrics, SessionState};

    #[tokio::test]
    async fn capture_command_output_persists_lines_and_events() -> Result<()> {
        let db_path = env::temp_dir().join(format!("ecc2-runtime-{}.db", Uuid::new_v4()));
        let db = StateStore::open(&db_path)?;
        let session_id = "session-1".to_string();
        let now = Utc::now();

        db.insert_session(&Session {
            id: session_id.clone(),
            task: "stream output".to_string(),
            agent_type: "test".to_string(),
            state: SessionState::Pending,
            worktree: None,
            created_at: now,
            updated_at: now,
            metrics: SessionMetrics::default(),
        })?;

        let output_store = SessionOutputStore::default();
        let mut rx = output_store.subscribe();
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("printf 'alpha\\n'; printf 'beta\\n' >&2");

        let status =
            capture_command_output(db_path.clone(), session_id.clone(), command, output_store)
                .await?;

        assert!(status.success());

        let db = StateStore::open(&db_path)?;
        let session = db
            .get_session(&session_id)?
            .expect("session should still exist");
        assert_eq!(session.state, SessionState::Completed);

        let lines = db.get_output_lines(&session_id, OUTPUT_BUFFER_LIMIT)?;
        let texts: HashSet<_> = lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(lines.len(), 2);
        assert!(texts.contains("alpha"));
        assert!(texts.contains("beta"));

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event.line.text);
        }

        assert_eq!(events.len(), 2);
        assert!(events.iter().any(|line| line == "alpha"));
        assert!(events.iter().any(|line| line == "beta"));

        let _ = std::fs::remove_file(db_path);

        Ok(())
    }
}
