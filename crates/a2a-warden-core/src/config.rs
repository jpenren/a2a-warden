//! Configuration types for A2A Warden

use crate::error::ConfigError;
use crate::policy::PolicyConfig;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use url::Url;

/// Root configuration structure
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Server configuration
    pub server: ServerConfig,
    /// Database configuration
    pub database: DatabaseConfig,
    /// Registered remote agents
    pub agents: Vec<AgentConfig>,
    /// Retention policies
    pub retention: RetentionConfig,
    /// Frontend configuration
    #[serde(default)]
    pub frontend: FrontendConfig,
    /// Administration API configuration
    #[serde(default)]
    pub admin: AdminConfig,
    /// Push notification delivery configuration
    #[serde(default)]
    pub push_notifications: PushNotificationsConfig,
    /// Automatic expiry of stale tasks
    #[serde(default)]
    pub task_expiry: TaskExpiryConfig,
    /// Agent Card verification configuration
    #[serde(default)]
    pub agent_card_verification: AgentCardVerificationConfig,
    #[serde(default)]
    pub policy: PolicyConfig,
}

/// Server configuration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// Listen address (host:port)
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    /// A2A-Version header value
    #[serde(default = "default_a2a_version")]
    pub a2a_version: String,
    /// Request timeout in seconds
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,
    /// Maximum request body size in bytes
    #[serde(default = "default_max_body_size")]
    pub max_body_size: usize,
    /// Enable CORS
    #[serde(default = "default_true")]
    pub cors_enabled: bool,
    /// Allowed CORS origins
    #[serde(default)]
    pub cors_origins: Vec<String>,
    /// Public base URL advertised in the warden Agent Card
    /// (falls back to `http://127.0.0.1:<port>` when unset)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
}

fn default_listen_addr() -> String {
    "0.0.0.0:8080".to_string()
}
fn default_a2a_version() -> String {
    "1.0".to_string()
}
fn default_request_timeout() -> u64 {
    30
}
fn default_max_body_size() -> usize {
    10 * 1024 * 1024
} // 10 MB
fn default_true() -> bool {
    true
}

/// Database backend selected by configuration (decision AD-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseBackend {
    /// Embedded SQLite database (default, no external dependency).
    Sqlite,
    /// PostgreSQL server.
    Postgres,
}

impl DatabaseBackend {
    /// Stable name used in logs, metrics, and error messages.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
        }
    }
}

/// SQLite-specific options.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SqliteConfig {
    /// Path to the database file.
    pub path: PathBuf,
    /// Enable WAL journal mode.
    pub wal_mode: bool,
    /// Busy timeout in milliseconds.
    pub busy_timeout_ms: u32,
}

impl Default for SqliteConfig {
    fn default() -> Self {
        Self {
            path: default_sqlite_path(),
            wal_mode: true,
            busy_timeout_ms: default_busy_timeout(),
        }
    }
}

/// PostgreSQL-specific options.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct PostgresConfig {
    /// Connection URL (may reference environment variables).
    pub url: String,
    /// Maximum pool size.
    pub max_connections: u32,
    /// Minimum pool size.
    pub min_connections: u32,
    /// Connection timeout in seconds.
    pub connect_timeout_secs: u64,
}

impl Default for PostgresConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            max_connections: default_pool_size(),
            min_connections: default_min_connections(),
            connect_timeout_secs: default_connect_timeout(),
        }
    }
}

/// Database configuration: the active backend plus its options.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    /// Active backend.
    pub backend: DatabaseBackend,
    /// SQLite options, used when `backend` is `sqlite`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sqlite: Option<SqliteConfig>,
    /// PostgreSQL options, used when `backend` is `postgres`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postgres: Option<PostgresConfig>,
}

fn default_sqlite_path() -> PathBuf {
    PathBuf::from("./a2a-warden.db")
}
fn default_busy_timeout() -> u32 {
    5000
}
fn default_pool_size() -> u32 {
    10
}
fn default_min_connections() -> u32 {
    1
}
fn default_connect_timeout() -> u64 {
    10
}

impl DatabaseConfig {
    /// Get the backend type as a string
    #[must_use]
    pub const fn backend_type(&self) -> &'static str {
        self.backend.as_str()
    }

    /// SQLite options, materialized with defaults when unset.
    #[must_use]
    pub fn sqlite(&self) -> SqliteConfig {
        self.sqlite.clone().unwrap_or_default()
    }

    /// Get the SQLite path if using SQLite
    #[must_use]
    pub fn sqlite_path(&self) -> Option<PathBuf> {
        match self.backend {
            DatabaseBackend::Sqlite => Some(self.sqlite().path),
            DatabaseBackend::Postgres => None,
        }
    }

    /// Get the PostgreSQL URL if using PostgreSQL
    #[must_use]
    pub fn postgres_url(&self) -> Option<&str> {
        match self.backend {
            DatabaseBackend::Postgres => {
                self.postgres.as_ref().map(|postgres| postgres.url.as_str())
            }
            DatabaseBackend::Sqlite => None,
        }
    }
}

/// Remote agent configuration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    /// Unique agent identifier
    pub name: String,
    /// Agent's A2A endpoint URL
    #[schemars(with = "String")]
    pub url: Url,
    /// Skills this agent provides
    #[serde(default)]
    pub skills: Vec<String>,
    /// Optional Agent Card JSON (for verification in Phase 3)
    #[serde(default)]
    pub card_json: Option<serde_json::Value>,
    /// Additional metadata
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl AgentConfig {
    #[must_use]
    pub fn gates_skill(&self, _skill: &str) -> bool {
        // Cedar policies handle skill gating; this is kept for backward
        // compatibility with the agent registry but always returns false.
        // Skill interception is now exclusively governed by policy.rules.
        false
    }
}

/// Retention configuration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetentionConfig {
    /// Audit log retention in days
    #[serde(default = "default_audit_days")]
    pub audit_days: u32,
    /// Task data retention in days
    #[serde(default = "default_task_days")]
    pub task_days: u32,
    /// Enable automatic purging
    #[serde(default = "default_true")]
    pub auto_purge: bool,
    /// Purge interval in hours
    #[serde(default = "default_purge_interval")]
    pub purge_interval_hours: u32,
}

fn default_audit_days() -> u32 {
    90
}
fn default_task_days() -> u32 {
    30
}
fn default_purge_interval() -> u32 {
    24
}

/// Frontend configuration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct FrontendConfig {
    /// Enable embedded frontend
    pub enabled: bool,
    /// Path to serve frontend from
    pub path: String,
    /// SPA fallback route
    pub spa_fallback: String,
}

impl Default for FrontendConfig {
    fn default() -> Self {
        Self { enabled: true, path: "/ui".to_string(), spa_fallback: "/ui/index.html".to_string() }
    }
}

/// Administration API configuration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct AdminConfig {
    /// Enable admin API
    pub enabled: bool,
    /// API path prefix
    pub path: String,
    /// Static token for authentication (legacy only - replace with OIDC in Phase 6)
    pub token: Option<String>,
    /// Token header name
    pub token_header: String,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: "/api/v1/admin".to_string(),
            token: None,
            token_header: "X-Admin-Token".to_string(),
        }
    }
}

/// Push notification delivery configuration (SSRF policy).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct PushNotificationsConfig {
    /// Whether clients may register push notification endpoints.
    pub enabled: bool,
    /// Hosts that may receive notifications (`example.com`, `*.example.com`, `*`).
    ///
    /// The list is deny by default: when it is empty every registration is
    /// rejected, even when push notifications are enabled.
    pub allowed_hosts: Vec<String>,
    /// Permit loopback, private, and link-local destinations (development only).
    pub allow_private_addresses: bool,
    /// Delivery timeout in seconds.
    pub timeout_secs: u64,
}

impl Default for PushNotificationsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_hosts: Vec::new(),
            allow_private_addresses: false,
            timeout_secs: 10,
        }
    }
}

/// Automatic expiry of tasks that never reached a terminal state.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct TaskExpiryConfig {
    /// Whether the background sweeper runs.
    pub enabled: bool,
    /// Seconds between two sweeps.
    pub sweep_interval_secs: u64,
    /// Seconds of inactivity after which a non-terminal task expires.
    pub ttl_secs: u64,
}

impl Default for TaskExpiryConfig {
    fn default() -> Self {
        Self { enabled: false, sweep_interval_secs: 60, ttl_secs: 86_400 }
    }
}

/// JWKS source configuration for Agent Card verification.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct JwksConfig {
    /// Issuer identifier (used for validation).
    #[serde(default)]
    pub issuer: Option<String>,
    /// Static JWKS JSON to use (inline).
    #[serde(default)]
    pub jwks_json: Option<serde_json::Value>,
    /// JWKS URL to fetch (if not inline).
    #[serde(default)]
    pub url: Option<String>,
    /// Allowed algorithms (must be pinned; defaults to secure set).
    #[serde(default = "default_verification_algorithms")]
    pub allowed_algorithms: Vec<String>,
}

/// Allowed algorithms by default for Agent Card verification.
fn default_verification_algorithms() -> Vec<String> {
    vec![
        "ES256".to_string(),
        "ES384".to_string(),
        "ES512".to_string(),
        "EdDSA".to_string(),
        "RS256".to_string(),
    ]
}

impl Default for JwksConfig {
    fn default() -> Self {
        Self {
            issuer: None,
            jwks_json: None,
            url: None,
            allowed_algorithms: default_verification_algorithms(),
        }
    }
}

/// Agent Card verification configuration.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct AgentCardVerificationConfig {
    /// Enable mandatory verification when registering agents.
    #[serde(default)]
    pub enabled: bool,
    /// Quarantine agents with unsigned or unverifiable Agent Cards.
    #[serde(default = "default_true")]
    pub quarantine_unverified: bool,
    /// JWKS sources for verification.
    #[serde(default)]
    pub jwks: Vec<JwksConfig>,
}

impl Default for AgentCardVerificationConfig {
    fn default() -> Self {
        Self { enabled: false, quarantine_unverified: true, jwks: Vec::new() }
    }
}

impl Config {
    /// Load configuration from file with environment variable overrides
    pub fn load(path: Option<&PathBuf>) -> Result<Self, ConfigError> {
        let mut builder = config::Config::builder();

        // Default config file
        if let Some(p) = path {
            builder = builder.add_source(config::File::from(p.clone()).required(true));
        } else {
            // Try default locations
            for default_path in
                ["config/default.yaml", "./config/default.yaml", "/etc/a2a-warden/config.yaml"]
            {
                if std::path::Path::new(default_path).exists() {
                    builder = builder.add_source(
                        config::File::from(std::path::PathBuf::from(default_path)).required(true),
                    );
                    break;
                }
            }
        }

        // Environment variable overrides use the A2AWARDEN_ prefix with `__`
        // separating nested keys (documented in docs/configuration.md).
        builder = builder.add_source(
            config::Environment::with_prefix("A2AWARDEN")
                .prefix_separator("_")
                .separator("__")
                .try_parsing(true),
        );

        let config = builder.build()?;
        let result: Config = config.try_deserialize()?;
        result.validate()?;
        Ok(result)
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Validate server config
        if self.server.listen_addr.is_empty() {
            return Err(ConfigError::Validation("server.listen_addr cannot be empty".to_string()));
        }

        // Validate database config
        match self.database.backend {
            DatabaseBackend::Sqlite => {
                let path = self.database.sqlite().path;
                if let Some(parent) = path.parent() {
                    if !parent.as_os_str().is_empty() && !parent.exists() {
                        return Err(ConfigError::Validation(format!(
                            "SQLite database directory does not exist: {}",
                            parent.display()
                        )));
                    }
                }
            }
            DatabaseBackend::Postgres => {
                let url = self
                    .database
                    .postgres
                    .as_ref()
                    .map(|postgres| postgres.url.as_str())
                    .unwrap_or_default();
                if url.is_empty() {
                    return Err(ConfigError::Validation(
                        "database.postgres.url cannot be empty".to_string(),
                    ));
                }
                // Basic URL validation
                Url::parse(url)
                    .map_err(|e| ConfigError::Validation(format!("Invalid PostgreSQL URL: {e}")))?;
            }
        }

        // Validate agents
        if self.agents.is_empty() {
            return Err(ConfigError::Validation(
                "At least one agent must be configured".to_string(),
            ));
        }

        let mut agent_names = std::collections::HashSet::new();
        for agent in &self.agents {
            if agent.name.is_empty() {
                return Err(ConfigError::Validation("Agent name cannot be empty".to_string()));
            }
            if !agent_names.insert(&agent.name) {
                return Err(ConfigError::Validation(format!(
                    "Duplicate agent name: {}",
                    agent.name
                )));
            }
            // Validate URL
            if agent.url.scheme() != "http" && agent.url.scheme() != "https" {
                return Err(ConfigError::Validation(format!(
                    "Agent '{}' URL must use http or https scheme",
                    agent.name
                )));
            }
        }

        // Validate the advertised public URL when present
        if let Some(public_url) = &self.server.public_url {
            Url::parse(public_url)
                .map_err(|e| ConfigError::Validation(format!("Invalid server.public_url: {e}")))?;
        }

        // Validate retention
        if self.retention.audit_days == 0 {
            return Err(ConfigError::Validation("retention.audit_days must be > 0".to_string()));
        }
        if self.retention.task_days == 0 {
            return Err(ConfigError::Validation("retention.task_days must be > 0".to_string()));
        }

        // Validate admin config
        if self.admin.enabled && self.admin.token.is_none() {
            // Warning: no token configured
            // In production, this should be an error
        }

        // Validate push notification policy. An enabled endpoint with an empty
        // allow list is a valid (deny everything) configuration, but the shape
        // of every advertised host must be usable.
        if self.push_notifications.timeout_secs == 0 {
            return Err(ConfigError::Validation(
                "push_notifications.timeout_secs must be > 0".to_string(),
            ));
        }
        for host in &self.push_notifications.allowed_hosts {
            let host = host.trim();
            if host.is_empty() || host.contains('/') || host.contains(' ') {
                return Err(ConfigError::Validation(format!(
                    "push_notifications.allowed_hosts contains an invalid host: '{host}'"
                )));
            }
        }

        // Validate task expiry.
        if self.task_expiry.enabled {
            if self.task_expiry.ttl_secs == 0 {
                return Err(ConfigError::Validation(
                    "task_expiry.ttl_secs must be > 0".to_string(),
                ));
            }
            if self.task_expiry.sweep_interval_secs == 0 {
                return Err(ConfigError::Validation(
                    "task_expiry.sweep_interval_secs must be > 0".to_string(),
                ));
            }
        }

        // Validate Agent Card verification. The algorithms are a pinned
        // allowlist: anything outside the documented set is rejected so a
        // configuration can never downgrade verification silently.
        const PINNED_ALGORITHMS: [&str; 5] = ["ES256", "ES384", "ES512", "EdDSA", "RS256"];
        for (index, source) in self.agent_card_verification.jwks.iter().enumerate() {
            if source.jwks_json.is_none() && source.url.is_none() {
                return Err(ConfigError::Validation(format!(
                    "agent_card_verification.jwks[{index}] must define jwks_json or url"
                )));
            }
            if let Some(url) = &source.url {
                Url::parse(url).map_err(|e| {
                    ConfigError::Validation(format!(
                        "agent_card_verification.jwks[{index}].url is invalid: {e}"
                    ))
                })?;
            }
            for algorithm in &source.allowed_algorithms {
                if !PINNED_ALGORITHMS.contains(&algorithm.as_str()) {
                    return Err(ConfigError::Validation(format!(
                        "agent_card_verification.jwks[{index}] allows unpinned algorithm '{algorithm}' (allowed: {PINNED_ALGORITHMS:?})"
                    )));
                }
            }
        }
        if self.agent_card_verification.enabled && self.agent_card_verification.jwks.is_empty() {
            tracing::warn!(
                "agent_card_verification is enabled without a JWKS source; every signed card will be quarantined"
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Writes the YAML to a temporary file with a `.yaml` suffix so the
    /// `config` crate can detect the format. The file is removed on drop.
    fn temp_yaml(yaml: &str) -> tempfile::TempPath {
        let mut file = tempfile::Builder::new().suffix(".yaml").tempfile().expect("temporary file");
        file.write_all(yaml.as_bytes()).expect("write");
        file.flush().expect("flush");
        file.into_temp_path()
    }

    const VALID_YAML: &str = r#"
server:
  listen_addr: "0.0.0.0:8080"
database:
  backend: sqlite
  sqlite:
    path: "./test.db"
agents:
  - name: "echo-agent"
    url: "http://127.0.0.1:9001"
    skills: ["echo"]

# Cedar policy engine configuration (Phase 4)
policy:
  rules:
    - id: "escalate-all-skills"
      description: "Escalate all tasks for human approval by default"
      active: true
      effect: escalate
      condition: "targetAgent == 'echo-agent' && skill != ''"
      version: 1
      created_at: "2026-01-01T00:00:00Z"
retention:
  audit_days: 90
"#;

    #[test]
    fn test_load_valid_config() {
        let path = temp_yaml(VALID_YAML);

        let config = Config::load(Some(&path.to_path_buf())).expect("valid config");
        assert_eq!(config.server.listen_addr, "0.0.0.0:8080");
        assert_eq!(config.agents.len(), 1);
        assert_eq!(config.agents[0].name, "echo-agent");
        assert_eq!(config.database.backend_type(), "sqlite");
    }

    #[test]
    fn test_env_override() {
        let path = temp_yaml(VALID_YAML);

        // A key that no other test asserts on, because process environment
        // variables are shared between concurrently running tests.
        std::env::set_var("A2AWARDEN_SERVER__MAX_BODY_SIZE", "4096");
        let config = Config::load(Some(&path.to_path_buf())).expect("valid config");
        assert_eq!(config.server.max_body_size, 4096);
        std::env::remove_var("A2AWARDEN_SERVER__MAX_BODY_SIZE");
    }

    #[test]
    fn test_invalid_config_missing_agents() {
        let path = temp_yaml(
            r#"
server:
  listen_addr: "0.0.0.0:8080"
database:
  backend: sqlite
  sqlite:
    path: "./test.db"
agents: []
retention:
  audit_days: 90
"#,
        );

        let result = Config::load(Some(&path.to_path_buf()));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("At least one agent"));
    }

    #[test]
    fn test_invalid_config_rejects_unknown_fields() {
        let path = temp_yaml(
            r#"
server:
  listen_addr: "0.0.0.0:8080"
  typo_field: true
database:
  backend: sqlite
  sqlite:
    path: "./test.db"
agents:
  - name: "echo-agent"
    url: "http://127.0.0.1:9001"
retention:
  audit_days: 90
"#,
        );
        assert!(Config::load(Some(&path.to_path_buf())).is_err());
    }

    #[test]
    fn test_invalid_config_rejects_bad_agent_url() {
        let path = temp_yaml(
            r#"
server:
  listen_addr: "0.0.0.0:8080"
database:
  backend: sqlite
  sqlite:
    path: "./test.db"
agents:
  - name: "echo-agent"
    url: "ftp://127.0.0.1:9001"
retention:
  audit_days: 90
"#,
        );
        let error = Config::load(Some(&path.to_path_buf())).expect_err("scheme must be rejected");
        assert!(error.to_string().contains("http or https"));
    }
}
