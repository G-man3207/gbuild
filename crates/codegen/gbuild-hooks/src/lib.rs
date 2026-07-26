//! # gbuild-hooks
//!
//! Runtime hook system for gBuild — file-based discovery, command execution,
//! and policy enforcement.
//!
//! ## Overview
//!
//! This crate provides a minimal hooks system for gBuild. Hooks are discovered
//! from dedicated directories (`~/.gbuild/hooks/` and `<git-worktree-root>/.gbuild/hooks/`),
//! defined in JSON files (compatible settings format), and executed as child processes.
//!
//! ## v0 scope
//!
//! - Four event types: `session_start`, `pre_tool_use`, `post_tool_use`, `session_end`
//! - Command-backed hooks only
//! - `pre_tool_use` hooks can deny/allow (blocking); all others are non-blocking
//! - Fail-open by default: hook failures do not block normal operation
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use std::path::Path;
//! use gbuild_hooks::discovery::load_hooks;
//! use gbuild_hooks::event::HookEventName;
//!
//! let (registry, errors) = load_hooks(
//!     Some(Path::new("/home/user/.gbuild/hooks")),
//!     Some(Path::new("/project/.gbuild/hooks")),
//! );
//!
//! for err in &errors {
//!     eprintln!("hook load warning: {err}");
//! }
//!
//! let pre_hooks = registry.hooks_for(HookEventName::PreToolUse);
//! println!("loaded {} pre_tool_use hooks", pre_hooks.len());
//! ```

pub mod config;
pub mod discovery;
pub mod dispatcher;
mod env_expand;
pub mod error;
pub mod event;
pub mod matcher;
pub mod result;
pub mod runner;
#[cfg(test)]
mod test_support;
pub mod trust;
