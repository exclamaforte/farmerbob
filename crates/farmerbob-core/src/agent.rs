//! Agents: the coding backends farmerbob can dispatch work to.

use serde::{Deserialize, Serialize};

use crate::ids::AgentId;

/// A supported agent backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentKind {
    Claude,
    Codex,
    Gemini,
    Glm,
    Ifm,
    OpenRouter,
}

/// One configured agent: a backend plus the model and provider to drive.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Agent {
    pub id: AgentId,
    pub kind: AgentKind,
    /// Model identifier as the backend expects it, e.g. `"glm-5.3-flash"`.
    pub model: String,
    /// Provider or source the agent is served through, e.g. `"openrouter"`.
    pub provider: String,
}
