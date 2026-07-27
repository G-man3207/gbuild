//! API-agnostic conversation representation.
//!
//! The canonical types now live in `gbuild_sampling_types::conversation`.
//! This module re-exports them and adds gbuild-shell-specific types
//! (`ConversationRequestTrace`) that depend on internal crate types.

// Re-export everything from the standalone crate.
pub use gbuild_sampling_types::conversation::*;

// ============================================================================
// gbuild-shell-specific types (depend on internal crate types)
// ============================================================================

// Tests for conversation types now live in gbuild-sampling-types crate.
