//! Tasks: units of work an agent is asked to perform.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ids::TaskId;

/// A unit of work: which repository to operate on and where the task's
/// instructions and artifacts live.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub name: String,
    /// Path to the repository the task is performed against.
    pub repo_path: PathBuf,
    /// Directory holding the task's instructions, fixtures, and outputs.
    pub task_dir: PathBuf,
}
