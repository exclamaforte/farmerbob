//! `fb log <run>` — dump what an arm said and did, from its own session store.
//!
//! Bead farmerbob-x81s.17: both turns of the first KernelBench wave ended
//! SIGTERM while the agent was making progress, and the run log held only
//! the first 400 chars of the prompt. The findings lived in the per-run
//! opencode.db and took sqlite spelunking to recover. This command is that
//! spelunking, bottled: sessions with cost and tokens, assistant turns in
//! full, and tool calls with their titles, status and (truncated) output.
//!
//! Two halves, deliberately. `launch::fit_for_log` keeps both ends of the
//! captured pipe in the run log file; this command reads the session
//! database the launcher kept beside it. Either half alone would have
//! saved that wave's findings.
//!
//! Exit codes: 0 with a transcript; 1 when the run or its session store
//! cannot be found (naming what was looked for); 2 when the database is
//! unreadable.

use rusqlite::{Connection, OpenFlags};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Tool output kept per call: enough for bench numbers and error lines,
/// not megabytes of test logs.
const MAX_TOOL_OUTPUT: usize = 4000;

/// One session row: id, title, model JSON, cost, tokens in/out/reasoning,
/// created/updated millis.
type SessionRow = (String, String, String, f64, i64, i64, i64, i64, i64);

/// Session database for a run, if the opencode launcher kept one.
pub fn session_db(run: &str) -> PathBuf {
    crate::paths::state()
        .join("state")
        .join(run)
        .join("data/opencode/opencode.db")
}

/// Run log and record beside the session store.
fn run_files(run: &str) -> (PathBuf, PathBuf) {
    let logs = crate::paths::logs();
    (
        logs.join(format!("{run}.log")),
        logs.join(format!("{run}.json")),
    )
}

fn elapsed_ms(since_ms: i64, at_ms: i64) -> String {
    let elapsed = (at_ms - since_ms).max(0) / 1000;
    format!("{:02}:{:02}", elapsed / 60, elapsed % 60)
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}[...{} bytes elided...]", &text[..end], text.len() - end)
}

fn model_name(model: &str) -> String {
    serde_json::from_str::<serde_json::Value>(model)
        .ok()
        .and_then(|v| v.get("id").and_then(|id| id.as_str()).map(str::to_string))
        .unwrap_or_else(|| model.to_string())
}

/// One rendered tool call, from a part's data object.
fn render_tool(data: &serde_json::Value) -> String {
    let name = data
        .get("tool")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?");
    let state = data.get("state");
    let status = state
        .and_then(|s| s.get("status"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?");
    let title = data
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            state
                .and_then(|s| s.get("input"))
                .and_then(|i| i.get("command"))
                .and_then(serde_json::Value::as_str)
                .map(|c| c.lines().next().unwrap_or("").chars().take(160).collect())
        })
        .unwrap_or_default();
    let exit = state
        .and_then(|s| s.get("metadata"))
        .and_then(|m| m.get("exit"))
        .and_then(serde_json::Value::as_i64)
        .map(|e| format!(" exit {e}"))
        .unwrap_or_default();
    let output = state
        .and_then(|s| s.get("output").and_then(serde_json::Value::as_str))
        .or_else(|| {
            state
                .and_then(|s| s.get("metadata"))
                .and_then(|m| m.get("output"))
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or("");
    let mut out = format!("tool {name}: {title} [{status}{exit}]");
    if !output.is_empty() {
        out.push_str(&format!(
            "\n    {}",
            truncate(output, MAX_TOOL_OUTPUT).replace('\n', "\n    ")
        ));
    }
    out
}

/// Dump one session database. Returns the exit code.
pub fn dump_db(db: &Path, out: &mut dyn Write) -> i32 {
    let conn = match Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(conn) => conn,
        Err(e) => {
            let _ = writeln!(out, "error: cannot open {}: {e}", db.display());
            return 2;
        }
    };
    // A running agent may hold the write lock; wait briefly, never hang.
    let _ = conn.pragma_update(None, "busy_timeout", 2000);
    let mut session_stmt = match conn.prepare(
        "SELECT id, title, model, cost, tokens_input, tokens_output, tokens_reasoning, time_created, time_updated FROM session ORDER BY time_created",
    ) {
        Ok(stmt) => stmt,
        Err(e) => {
            let _ = writeln!(out, "error: {} is not an opencode session db: {e}", db.display());
            return 2;
        }
    };
    let sessions: Vec<SessionRow> = match session_stmt
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
            ))
        })
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
    {
        Ok(sessions) => sessions,
        Err(e) => {
            let _ = writeln!(out, "error: cannot read sessions: {e}");
            return 2;
        }
    };
    if sessions.is_empty() {
        let _ = writeln!(out, "no sessions in {}", db.display());
        return 1;
    }
    for (id, title, model, cost, tok_in, tok_out, tok_reason, created, _updated) in &sessions {
        let _ = writeln!(
            out,
            "session {} \"{}\" {} cost {cost:.4} tokens {tok_in}/{tok_out}/{tok_reason}",
            &id[..id.len().min(16)],
            title,
            model_name(model),
        );
        let mut message_stmt = match conn.prepare(
            "SELECT id, time_created, data FROM message WHERE session_id = ?1 ORDER BY time_created, id",
        ) {
            Ok(stmt) => stmt,
            Err(e) => {
                let _ = writeln!(out, "error: cannot read messages: {e}");
                return 2;
            }
        };
        let messages: Vec<(String, i64, String)> = match message_stmt
            .query_map([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        {
            Ok(messages) => messages,
            Err(e) => {
                let _ = writeln!(out, "error: cannot read messages: {e}");
                return 2;
            }
        };
        for (message_id, _, message_data) in &messages {
            let mut part_stmt = match conn.prepare(
                "SELECT time_created, data FROM part WHERE message_id = ?1 ORDER BY time_created, id",
            ) {
                Ok(stmt) => stmt,
                Err(e) => {
                    let _ = writeln!(out, "error: cannot read parts: {e}");
                    return 2;
                }
            };
            let parts: Vec<(i64, String)> = match part_stmt
                .query_map([message_id], |row| Ok((row.get(0)?, row.get(1)?)))
                .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            {
                Ok(parts) => parts,
                Err(e) => {
                    let _ = writeln!(out, "error: cannot read parts: {e}");
                    return 2;
                }
            };
            let role: String = serde_json::from_str::<serde_json::Value>(message_data)
                .ok()
                .and_then(|v| v.get("role").and_then(|r| r.as_str()).map(str::to_string))
                .unwrap_or_else(|| "?".to_string());
            for (at, data) in &parts {
                let value: serde_json::Value = match serde_json::from_str(data) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                let kind = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match kind {
                    "text" => {
                        if let Some(text) = value.get("text").and_then(|t| t.as_str()) {
                            let _ =
                                writeln!(out, "  [{}] {role}: {text}", elapsed_ms(*created, *at));
                        }
                    }
                    "tool" => {
                        let _ = writeln!(
                            out,
                            "  [{}] {}",
                            elapsed_ms(*created, *at),
                            render_tool(&value).replace('\n', "\n  ")
                        );
                    }
                    _ => {}
                }
            }
        }
    }
    0
}

/// Dump a run: session transcript when kept, run files always named.
pub fn run(run: &str, out: &mut dyn Write) -> i32 {
    let db = session_db(run);
    let (log, record) = run_files(run);
    if !db.is_file() {
        let _ = writeln!(out, "no session db for run {run} at {}", db.display());
        for path in [&log, &record] {
            if path.is_file() {
                let _ = writeln!(out, "kept: {}", path.display());
            }
        }
        return 1;
    }
    let code = dump_db(&db, out);
    for path in [&log, &record] {
        if path.is_file() {
            let _ = writeln!(out, "kept: {}", path.display());
        }
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a session database the way opencode does (the tables this
    /// command reads), with one text turn and one tool call.
    fn fixture_db(path: &Path) {
        let conn = Connection::open(path).expect("fixture db");
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, workspace_id TEXT, parent_id TEXT, slug TEXT NOT NULL, directory TEXT NOT NULL, path TEXT, title TEXT NOT NULL, version TEXT NOT NULL, share_url TEXT, summary_additions INTEGER, summary_deletions INTEGER, summary_files INTEGER, summary_diffs TEXT, metadata TEXT, cost REAL DEFAULT 0 NOT NULL, tokens_input INTEGER DEFAULT 0 NOT NULL, tokens_output INTEGER DEFAULT 0 NOT NULL, tokens_reasoning INTEGER DEFAULT 0 NOT NULL, tokens_cache_read INTEGER DEFAULT 0 NOT NULL, tokens_cache_write INTEGER DEFAULT 0 NOT NULL, revert TEXT, permission TEXT, agent TEXT, model TEXT, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, time_compacting INTEGER, time_archived INTEGER);
            CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);
            CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);",
        )
        .expect("schema");
        conn.execute(
            "INSERT INTO session (id, project_id, slug, directory, title, version, model, cost, tokens_input, tokens_output, tokens_reasoning, time_created, time_updated) VALUES ('ses_1', 'p', 's', '/w', 'Beat it', 'v', '{\"id\":\"m\",\"providerID\":\"n\"}', 0.0, 100, 20, 0, 1000000, 2000000)",
            [],
        )
        .expect("session");
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES ('m1', 'ses_1', 1001000, 1001000, '{\"role\":\"assistant\"}')",
            [],
        )
        .expect("message");
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('p1', 'm1', 'ses_1', 1001000, 1001000, '{\"type\":\"text\",\"text\":\"found 19.5ms\"}')",
            [],
        )
        .expect("text part");
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('p2', 'm1', 'ses_1', 1002000, 1002000, '{\"type\":\"tool\",\"tool\":\"bash\",\"title\":\"bench\",\"state\":{\"status\":\"completed\",\"input\":{\"command\":\"bench.sh\"},\"metadata\":{\"exit\":0,\"output\":\"19.5\"}}}')",
            [],
        )
        .expect("tool part");
    }

    /// The transcript names the session, the text turn, and the tool
    /// call with its output -- the recovery this command exists for.
    #[test]
    fn transcript_dumps_sessions_turns_and_tools() {
        let dir = std::env::temp_dir().join(format!("fb-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let db = dir.join("opencode.db");
        fixture_db(&db);
        let mut out = Vec::new();
        assert_eq!(dump_db(&db, &mut out), 0);
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("Beat it"), "{text}");
        assert!(text.contains("found 19.5ms"), "{text}");
        assert!(text.contains("tool bash"), "{text}");
        assert!(text.contains("19.5"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A missing database reads as missing, naming the path; garbage is
    /// refused, not misread.
    #[test]
    fn missing_and_foreign_databases_are_named() {
        let dir = std::env::temp_dir().join(format!("fb-log-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let mut out = Vec::new();
        assert_eq!(dump_db(&dir.join("nope.db"), &mut out), 2);
        let foreign = dir.join("foreign.db");
        Connection::open(&foreign)
            .expect("open")
            .execute_batch("CREATE TABLE t (x TEXT);")
            .expect("table");
        let mut out = Vec::new();
        assert_eq!(dump_db(&foreign, &mut out), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Tool rendering degrades gracefully: unknown shapes still name
    /// the tool rather than vanishing.
    #[test]
    fn tool_rendering_is_total() {
        let bare: serde_json::Value = serde_json::json!({"type": "tool"});
        assert!(render_tool(&bare).contains("tool ?"));
        let cmd: serde_json::Value = serde_json::json!({"type": "tool", "tool": "bash", "state": {"status": "x", "input": {"command": "echo hi\necho lo"}}});
        assert!(render_tool(&cmd).contains("echo hi"));
    }
}
