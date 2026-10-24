//! A2A Warden Store
//!
//! sqlx implementations of the core store traits.
//! Provides SQLite and PostgreSQL backends for task, approval, audit, and
//! agent storage (decision AD-2: one implementation, two interchangeable
//! backends selected by configuration).

mod impls;

use std::sync::Arc;
use std::time::Duration;

use sqlx::migrate::{Migrate, Migrator};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Database, Pool};

use a2a_warden_core::config::{DatabaseBackend, DatabaseConfig, PostgresConfig, SqliteConfig};
use a2a_warden_core::error::{DatabaseError, Error};
use a2a_warden_core::stores::Store;

/// Embedded migrations, applied identically on SQLite and PostgreSQL.
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Storage engine over a sqlx pool. The backend is a compile-time parameter so
/// that no domain code ever depends on which one is active.
pub struct WardenStore<DB: Database> {
    pool: Pool<DB>,
}

impl<DB: Database> Clone for WardenStore<DB> {
    fn clone(&self) -> Self {
        Self { pool: self.pool.clone() }
    }
}

impl<DB: Database> WardenStore<DB> {
    /// Wraps an already connected pool.
    #[must_use]
    pub const fn new(pool: Pool<DB>) -> Self {
        Self { pool }
    }

    /// Returns the underlying pool.
    #[must_use]
    pub fn pool(&self) -> &Pool<DB> {
        &self.pool
    }
}

impl<DB> WardenStore<DB>
where
    DB: Database + Send + Sync,
    DB::Connection: Migrate,
{
    /// Applies every pending migration. Called by the CLI at startup so a cold
    /// start needs no manual step .
    pub async fn migrate(&self) -> Result<(), Error> {
        MIGRATOR
            .run(&self.pool)
            .await
            .map_err(|err| DatabaseError::Migration(err.to_string()).into())
    }
}

impl WardenStore<sqlx::Sqlite> {
    /// Connects to an embedded SQLite database, creating the file if needed.
    pub async fn connect_sqlite(config: &SqliteConfig) -> Result<Self, Error> {
        let mut options =
            SqliteConnectOptions::new().filename(&config.path).create_if_missing(true);

        if config.wal_mode {
            options = options.journal_mode(SqliteJournalMode::Wal);
        }
        options = options.busy_timeout(Duration::from_millis(config.busy_timeout_ms.into()));

        // An in-memory database must be shared by a single connection or every
        // connection would see its own empty database.
        let in_memory = config.path.to_string_lossy().contains(":memory:");
        let max_connections = if in_memory { 1 } else { 5 };

        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
            .connect_with(options)
            .await
            .map_err(|err| DatabaseError::Connection(err.to_string()))?;
        Ok(Self::new(pool))
    }
}

impl WardenStore<sqlx::Postgres> {
    /// Connects to a PostgreSQL server.
    pub async fn connect_postgres(config: &PostgresConfig) -> Result<Self, Error> {
        let options = config
            .url
            .parse::<PgConnectOptions>()
            .map_err(|err| DatabaseError::Connection(err.to_string()))?;
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(Duration::from_secs(config.connect_timeout_secs))
            .connect_with(options)
            .await
            .map_err(|err| DatabaseError::Connection(err.to_string()))?;
        Ok(Self::new(pool))
    }
}

/// Connects the backend selected by configuration, applies migrations, and
/// returns the trait object the rest of the application is programmed against.
pub async fn connect(config: &DatabaseConfig) -> Result<Arc<dyn Store>, Error> {
    match config.backend {
        DatabaseBackend::Sqlite => {
            let store = WardenStore::<sqlx::Sqlite>::connect_sqlite(&config.sqlite()).await?;
            store.migrate().await?;
            Ok(Arc::new(store))
        }
        DatabaseBackend::Postgres => {
            let postgres = config.postgres.clone().unwrap_or_default();
            let store = WardenStore::<sqlx::Postgres>::connect_postgres(&postgres).await?;
            store.migrate().await?;
            Ok(Arc::new(store))
        }
    }
}

/// Maps a backend error to the domain error space, preserving unique
/// constraint violations as [`a2a_warden_core::stores::StorageError::Duplicate`].
pub(crate) fn map_db_err(err: sqlx::Error, entity: &str) -> Error {
    match &err {
        sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
            a2a_warden_core::stores::errors::duplicate(entity)
        }
        _ => a2a_warden_core::stores::errors::query(err),
    }
}
