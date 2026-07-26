//! Tool infrastructure for gbuild-shell.
//!
//! All tool execution goes through `gbuild-tools` via the `ToolBridge`.
//! Types (ToolOutput, ToolInput, TodoState, etc.) come from `gbuild-tools` directly.

pub mod bridge;
pub mod config;
pub mod notification_bridge;
pub mod retry;
pub mod todo;
pub mod tool_context;

pub use self::{
    config::{BashToolConfig, FileToolset, ShellToolsetConfig},
    retry::{RetryConfig, execute_with_retry},
    tool_context::ToolContext,
};

// Re-export key types from gbuild-tools for convenience
pub use self::todo::{TodoId, TodoItem, TodoPriority, TodoStatus};
pub use gbuild_tools::types::output::ToolOutput;
pub use gbuild_tools::types::{MCPToolInput, ToolInput};
