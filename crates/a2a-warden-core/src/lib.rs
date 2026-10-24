//! A2A Warden Core
//!
//! Core domain types, traits, and logic for the A2A Warden.
//!
//! This crate defines the fundamental building blocks:
//! - A2A protocol types (minimal subset for legacy)
//! - Storage traits (TaskStore, ApprovalStore, AuditStore, AgentStore)
//! - Domain logic for task interception and approval workflows
//! - Configuration types
//! - Cedar policy engine (Phase 4)

pub mod config;
pub mod error;
pub mod pii;
pub mod policy;
pub mod protocol;
pub mod push;
pub mod service;
pub mod stores;
pub mod verification;
pub mod wire;
pub mod workflow;

/// Generates a new time-ordered identifier (UUID v7).
#[must_use]
pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Current UTC timestamp in RFC 3339 with millisecond precision.
#[must_use]
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests;
