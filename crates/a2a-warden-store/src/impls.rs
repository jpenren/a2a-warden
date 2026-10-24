//! Trait implementations shared by every supported backend.
//!
//! The SQL is written once in the portable subset understood by both SQLite
//! and PostgreSQL (`$n` placeholders, TEXT columns, INTEGER booleans). The
//! [`impl_store`] macro instantiates it for each backend so that a fix can
//! never diverge between them.

use async_trait::async_trait;
use sqlx::Row;

use a2a_warden_core::error::Error;
use a2a_warden_core::protocol::{Role, TaskState};
use a2a_warden_core::stores::{
    errors, AgentRecord, AgentStore, ApprovalRecord, ApprovalStatus, ApprovalStore, ArtifactRecord,
    AuditRecord, AuditStore, MessageRecord, PushNotificationRecord, PushNotificationStore,
    TaskCursor, TaskFilter, TaskRecord, TaskStore,
};

use crate::{map_db_err, WardenStore};

const SELECT_TASK: &str = "SELECT id, context_id, agent_id, status, created_at, updated_at, \
     metadata_json, remote_task_id FROM tasks";
const SELECT_MESSAGE: &str =
    "SELECT id, message_id, task_id, context_id, role, parts_json, created_at FROM messages";
const SELECT_ARTIFACT: &str = "SELECT id, artifact_id, task_id, name, parts_json, metadata_json, \
     created_at FROM artifacts";
const SELECT_APPROVAL: &str = "SELECT id, task_id, status, decided_by, decided_at, expires_at, \
     reason, created_at FROM approvals";
const SELECT_AUDIT: &str = "SELECT id, ts, actor, action, task_id, context_id, payload_json, \
     previous_hash, hash FROM audit_log";
const SELECT_AGENT: &str = "SELECT id, name, url, skills_json, card_json, verified, verification_error, jwks_kid, \
     verified_at, verification_issuer, verification_alg, quarantined, created_at FROM agents";
const SELECT_PUSH_CONFIG: &str = "SELECT id, task_id, url, token, auth_json, created_at \
     FROM push_notification_configs";

/// Rows always store TEXT columns, so every backend decodes them the same way.
trait TextRow {
    /// Reads a required TEXT column.
    fn text(&self, column: &str) -> Result<String, Error>;
    /// Reads a nullable TEXT column.
    fn opt_text(&self, column: &str) -> Result<Option<String>, Error>;
    /// Reads an INTEGER column.
    fn int(&self, column: &str) -> Result<i32, Error>;
}

impl TextRow for sqlx::sqlite::SqliteRow {
    fn text(&self, column: &str) -> Result<String, Error> {
        self.try_get::<String, _>(column).map_err(errors::query)
    }

    fn opt_text(&self, column: &str) -> Result<Option<String>, Error> {
        self.try_get::<Option<String>, _>(column).map_err(errors::query)
    }

    fn int(&self, column: &str) -> Result<i32, Error> {
        self.try_get::<i32, _>(column).map_err(errors::query)
    }
}

impl TextRow for sqlx::postgres::PgRow {
    fn text(&self, column: &str) -> Result<String, Error> {
        self.try_get::<String, _>(column).map_err(errors::query)
    }

    fn opt_text(&self, column: &str) -> Result<Option<String>, Error> {
        self.try_get::<Option<String>, _>(column).map_err(errors::query)
    }

    fn int(&self, column: &str) -> Result<i32, Error> {
        self.try_get::<i32, _>(column).map_err(errors::query)
    }
}

fn str_col<R: TextRow + ?Sized>(row: &R, column: &str) -> Result<String, Error> {
    row.text(column)
}

fn opt_col<R: TextRow + ?Sized>(row: &R, column: &str) -> Result<Option<String>, Error> {
    row.opt_text(column)
}

fn task_from_row<R: TextRow + ?Sized>(row: &R) -> Result<TaskRecord, Error> {
    let status = str_col(row, "status")?;
    let status = TaskState::parse(&status)
        .ok_or_else(|| errors::query(format!("unknown task status '{status}' in database")))?;
    Ok(TaskRecord {
        id: str_col(row, "id")?,
        context_id: str_col(row, "context_id")?,
        agent_id: str_col(row, "agent_id")?,
        status,
        created_at: str_col(row, "created_at")?,
        updated_at: str_col(row, "updated_at")?,
        metadata_json: opt_col(row, "metadata_json")?,
        remote_task_id: opt_col(row, "remote_task_id")?,
    })
}

fn message_from_row<R: TextRow + ?Sized>(row: &R) -> Result<MessageRecord, Error> {
    let role = str_col(row, "role")?;
    let role = Role::parse(&role)
        .ok_or_else(|| errors::query(format!("unknown message role '{role}' in database")))?;
    Ok(MessageRecord {
        id: str_col(row, "id")?,
        message_id: str_col(row, "message_id")?,
        task_id: str_col(row, "task_id")?,
        context_id: str_col(row, "context_id")?,
        role,
        parts_json: str_col(row, "parts_json")?,
        created_at: str_col(row, "created_at")?,
    })
}

fn artifact_from_row<R: TextRow + ?Sized>(row: &R) -> Result<ArtifactRecord, Error> {
    Ok(ArtifactRecord {
        id: str_col(row, "id")?,
        artifact_id: str_col(row, "artifact_id")?,
        task_id: str_col(row, "task_id")?,
        name: opt_col(row, "name")?,
        parts_json: str_col(row, "parts_json")?,
        metadata_json: opt_col(row, "metadata_json")?,
        created_at: str_col(row, "created_at")?,
    })
}

fn approval_from_row<R: TextRow + ?Sized>(row: &R) -> Result<ApprovalRecord, Error> {
    let status = str_col(row, "status")?;
    let status = ApprovalStatus::parse(&status)
        .ok_or_else(|| errors::query(format!("unknown approval status '{status}' in database")))?;
    Ok(ApprovalRecord {
        id: str_col(row, "id")?,
        task_id: str_col(row, "task_id")?,
        status,
        decided_by: opt_col(row, "decided_by")?,
        decided_at: opt_col(row, "decided_at")?,
        expires_at: opt_col(row, "expires_at")?,
        reason: opt_col(row, "reason")?,
        created_at: str_col(row, "created_at")?,
    })
}

fn audit_from_row<R: TextRow + ?Sized>(row: &R) -> Result<AuditRecord, Error> {
    Ok(AuditRecord {
        id: str_col(row, "id")?,
        ts: str_col(row, "ts")?,
        actor: str_col(row, "actor")?,
        action: str_col(row, "action")?,
        task_id: opt_col(row, "task_id")?,
        context_id: opt_col(row, "context_id")?,
        payload_json: opt_col(row, "payload_json")?,
        previous_hash: opt_col(row, "previous_hash")?,
        hash: opt_col(row, "hash")?,
    })
}

fn agent_from_row<R: TextRow + ?Sized>(row: &R) -> Result<AgentRecord, Error> {
    let verified = row.int("verified").unwrap_or(0);
    let quarantined = row.int("quarantined").unwrap_or(0);
    Ok(AgentRecord {
        id: str_col(row, "id")?,
        name: str_col(row, "name")?,
        url: str_col(row, "url")?,
        skills_json: str_col(row, "skills_json")?,
        card_json: opt_col(row, "card_json")?,
        verified: verified != 0,
        verification_error: opt_col(row, "verification_error")?,
        jwks_kid: opt_col(row, "jwks_kid")?,
        verified_at: opt_col(row, "verified_at")?,
        verification_issuer: opt_col(row, "verification_issuer")?,
        verification_alg: opt_col(row, "verification_alg")?,
        quarantined: quarantined != 0,
        created_at: str_col(row, "created_at")?,
    })
}

fn push_config_from_row<R: TextRow + ?Sized>(row: &R) -> Result<PushNotificationRecord, Error> {
    Ok(PushNotificationRecord {
        id: str_col(row, "id")?,
        task_id: str_col(row, "task_id")?,
        url: str_col(row, "url")?,
        token: opt_col(row, "token")?,
        auth_json: opt_col(row, "auth_json")?,
        created_at: str_col(row, "created_at")?,
    })
}

macro_rules! impl_store {
    ($db:ty) => {
        #[async_trait]
        impl TaskStore for WardenStore<$db> {
            async fn create_task(&self, task: &TaskRecord) -> Result<(), Error> {
                sqlx::query(
                    "INSERT INTO tasks (id, context_id, agent_id, status, created_at, updated_at, \
                     metadata_json, remote_task_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                )
                .bind(task.id.as_str())
                .bind(task.context_id.as_str())
                .bind(task.agent_id.as_str())
                .bind(task.status.as_str())
                .bind(task.created_at.as_str())
                .bind(task.updated_at.as_str())
                .bind(task.metadata_json.as_deref())
                .bind(task.remote_task_id.as_deref())
                .execute(&self.pool)
                .await
                .map_err(|err| map_db_err(err, "task"))?;
                Ok(())
            }

            async fn get_task(&self, id: &str) -> Result<TaskRecord, Error> {
                let row = sqlx::query(&format!("{SELECT_TASK} WHERE id = $1"))
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(errors::query)?
                    .ok_or_else(|| errors::task_not_found(id))?;
                task_from_row(&row)
            }

            async fn update_task_status(
                &self,
                id: &str,
                status: TaskState,
                updated_at: &str,
            ) -> Result<(), Error> {
                let result = sqlx::query(
                    "UPDATE tasks SET status = $1, updated_at = $2 WHERE id = $3",
                )
                .bind(status.as_str())
                .bind(updated_at)
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(errors::query)?;
                if result.rows_affected() == 0 {
                    return Err(errors::task_not_found(id));
                }
                Ok(())
            }

            async fn set_remote_task_id(&self, id: &str, remote_task_id: &str) -> Result<(), Error> {
                let result = sqlx::query(
                    "UPDATE tasks SET remote_task_id = $1 WHERE id = $2",
                )
                .bind(remote_task_id)
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(errors::query)?;
                if result.rows_affected() == 0 {
                    return Err(errors::task_not_found(id));
                }
                Ok(())
            }

            async fn list_tasks(&self, filter: &TaskFilter) -> Result<Vec<TaskRecord>, Error> {
                let mut sql = String::from(SELECT_TASK);
                sql.push_str(
                    " WHERE ($1 IS NULL OR status = $1)
                          AND ($2 IS NULL OR agent_id = $2)
                          AND ($3 IS NULL OR context_id = $3)
                          AND ($4 IS NULL OR updated_at > $4)
                      ORDER BY created_at DESC, id DESC",
                );
                if let Some(limit) = filter.limit {
                    sql.push_str(&format!(" LIMIT {limit}"));
                }
                if let Some(offset) = filter.offset {
                    sql.push_str(&format!(" OFFSET {offset}"));
                }
                let rows = sqlx::query(&sql)
                    .bind(filter.status.map(TaskState::as_str))
                    .bind(filter.agent.as_deref())
                    .bind(filter.context_id.as_deref())
                    .bind(filter.updated_after.as_deref())
                    .fetch_all(&self.pool)
                    .await
                    .map_err(errors::query)?;
                rows.iter().map(task_from_row).collect()
            }

            async fn list_tasks_page(
                &self,
                filter: &TaskFilter,
                cursor: Option<&TaskCursor>,
                limit: u32,
            ) -> Result<Vec<TaskRecord>, Error> {
                let mut sql = String::from(SELECT_TASK);
                sql.push_str(
                    " WHERE ($1 IS NULL OR status = $1)
                          AND ($2 IS NULL OR agent_id = $2)
                          AND ($3 IS NULL OR context_id = $3)
                          AND ($4 IS NULL OR updated_at > $4)
                          AND ($5 IS NULL OR (created_at < $5 OR (created_at = $5 AND id < $6)))
                      ORDER BY created_at DESC, id DESC
                      LIMIT $7",
                );
                let rows = sqlx::query(&sql)
                    .bind(filter.status.map(TaskState::as_str))
                    .bind(filter.agent.as_deref())
                    .bind(filter.context_id.as_deref())
                    .bind(filter.updated_after.as_deref())
                    .bind(cursor.map(|value| value.created_at.as_str()))
                    .bind(cursor.map(|value| value.id.as_str()))
                    .bind(i64::from(limit))
                    .fetch_all(&self.pool)
                    .await
                    .map_err(errors::query)?;
                rows.iter().map(task_from_row).collect()
            }

            async fn count_tasks(&self, filter: &TaskFilter) -> Result<u64, Error> {
                let count = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM tasks
                     WHERE ($1 IS NULL OR status = $1)
                       AND ($2 IS NULL OR agent_id = $2)
                       AND ($3 IS NULL OR context_id = $3)
                       AND ($4 IS NULL OR updated_at > $4)",
                )
                .bind(filter.status.map(TaskState::as_str))
                .bind(filter.agent.as_deref())
                .bind(filter.context_id.as_deref())
                .bind(filter.updated_after.as_deref())
                .fetch_one(&self.pool)
                .await
                .map_err(errors::query)?;
                Ok(u64::try_from(count).unwrap_or(0))
            }

            async fn list_expired_tasks(&self, cutoff: &str) -> Result<Vec<TaskRecord>, Error> {
                let rows = sqlx::query(&format!(
                    "{SELECT_TASK} WHERE updated_at < $1 \
                     AND status NOT IN ('completed', 'canceled', 'failed', 'rejected') \
                     ORDER BY updated_at ASC"
                ))
                .bind(cutoff)
                .fetch_all(&self.pool)
                .await
                .map_err(errors::query)?;
                rows.iter().map(task_from_row).collect()
            }

            async fn append_message(&self, message: &MessageRecord) -> Result<(), Error> {
                sqlx::query(
                    "INSERT INTO messages (id, message_id, task_id, context_id, role, parts_json, \
                     created_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
                )
                .bind(message.id.as_str())
                .bind(message.message_id.as_str())
                .bind(message.task_id.as_str())
                .bind(message.context_id.as_str())
                .bind(message.role.as_str())
                .bind(message.parts_json.as_str())
                .bind(message.created_at.as_str())
                .execute(&self.pool)
                .await
                .map_err(|err| map_db_err(err, "message"))?;
                Ok(())
            }

            async fn list_messages(&self, task_id: &str) -> Result<Vec<MessageRecord>, Error> {
                let rows = sqlx::query(&format!(
                    "{SELECT_MESSAGE} WHERE task_id = $1 ORDER BY created_at ASC, id ASC"
                ))
                .bind(task_id)
                .fetch_all(&self.pool)
                .await
                .map_err(errors::query)?;
                rows.iter().map(message_from_row).collect()
            }

            async fn append_artifact(&self, artifact: &ArtifactRecord) -> Result<(), Error> {
                sqlx::query(
                    "INSERT INTO artifacts (id, artifact_id, task_id, name, parts_json, \
                     metadata_json, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
                )
                .bind(artifact.id.as_str())
                .bind(artifact.artifact_id.as_str())
                .bind(artifact.task_id.as_str())
                .bind(artifact.name.as_deref())
                .bind(artifact.parts_json.as_str())
                .bind(artifact.metadata_json.as_deref())
                .bind(artifact.created_at.as_str())
                .execute(&self.pool)
                .await
                .map_err(|err| map_db_err(err, "artifact"))?;
                Ok(())
            }

            async fn list_artifacts(&self, task_id: &str) -> Result<Vec<ArtifactRecord>, Error> {
                let rows = sqlx::query(&format!(
                    "{SELECT_ARTIFACT} WHERE task_id = $1 ORDER BY created_at ASC, id ASC"
                ))
                .bind(task_id)
                .fetch_all(&self.pool)
                .await
                .map_err(errors::query)?;
                rows.iter().map(artifact_from_row).collect()
            }
        }

        #[async_trait]
        impl ApprovalStore for WardenStore<$db> {
            async fn create_approval(&self, approval: &ApprovalRecord) -> Result<(), Error> {
                sqlx::query(
                    "INSERT INTO approvals (id, task_id, status, decided_by, decided_at, \
                     expires_at, reason, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                )
                .bind(approval.id.as_str())
                .bind(approval.task_id.as_str())
                .bind(approval.status.as_str())
                .bind(approval.decided_by.as_deref())
                .bind(approval.decided_at.as_deref())
                .bind(approval.expires_at.as_deref())
                .bind(approval.reason.as_deref())
                .bind(approval.created_at.as_str())
                .execute(&self.pool)
                .await
                .map_err(|err| map_db_err(err, "approval"))?;
                Ok(())
            }

            async fn get_approval(&self, id: &str) -> Result<ApprovalRecord, Error> {
                let row = sqlx::query(&format!("{SELECT_APPROVAL} WHERE id = $1"))
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(errors::query)?
                    .ok_or_else(|| errors::approval_not_found(id))?;
                approval_from_row(&row)
            }

            async fn pending_approval_for_task(
                &self,
                task_id: &str,
            ) -> Result<Option<ApprovalRecord>, Error> {
                let row = sqlx::query(&format!(
                    "{SELECT_APPROVAL} WHERE task_id = $1 AND status = 'pending' LIMIT 1"
                ))
                .bind(task_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(errors::query)?;
                row.as_ref().map(approval_from_row).transpose()
            }

            async fn list_approvals(
                &self,
                status: Option<ApprovalStatus>,
                limit: Option<u32>,
                offset: Option<u32>,
            ) -> Result<Vec<ApprovalRecord>, Error> {
                let mut sql = format!("{SELECT_APPROVAL} WHERE ($1 IS NULL OR status = $1) ORDER BY created_at DESC");
                if let Some(l) = limit {
                    sql.push_str(&format!(" LIMIT {l}"));
                }
                if let Some(o) = offset {
                    sql.push_str(&format!(" OFFSET {o}"));
                }
                let rows = sqlx::query(&sql)
                    .bind(status.map(ApprovalStatus::as_str))
                    .fetch_all(&self.pool)
                    .await
                    .map_err(errors::query)?;
                rows.iter().map(approval_from_row).collect()
            }

            async fn decide_approval(
                &self,
                id: &str,
                status: ApprovalStatus,
                decided_by: &str,
                decided_at: &str,
                reason: Option<&str>,
            ) -> Result<ApprovalRecord, Error> {
                let result = sqlx::query(
                    "UPDATE approvals SET status = $1, decided_by = $2, decided_at = $3, \
                     reason = $4 WHERE id = $5 AND status = 'pending'",
                )
                .bind(status.as_str())
                .bind(decided_by)
                .bind(decided_at)
                .bind(reason)
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(errors::query)?;

                if result.rows_affected() == 0 {
                    // Distinguish "never existed" from "already decided".
                    let existing = self.get_approval(id).await?;
                    return Err(Error::Conflict(format!(
                        "approval {id} is already {}",
                        existing.status.as_str()
                    )));
                }
                self.get_approval(id).await
            }
        }

        #[async_trait]
        impl AuditStore for WardenStore<$db> {
            async fn append_audit(&self, entry: &AuditRecord) -> Result<(), Error> {
                sqlx::query(
                    "INSERT INTO audit_log (id, ts, actor, action, task_id, context_id, \
                     payload_json, previous_hash, hash) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                )
                .bind(entry.id.as_str())
                .bind(entry.ts.as_str())
                .bind(entry.actor.as_str())
                .bind(entry.action.as_str())
                .bind(entry.task_id.as_deref())
                .bind(entry.context_id.as_deref())
                .bind(entry.payload_json.as_deref())
                .bind(entry.previous_hash.as_deref())
                .bind(entry.hash.as_deref())
                .execute(&self.pool)
                .await
                .map_err(|err| map_db_err(err, "audit entry"))?;
                Ok(())
            }

            async fn last_audit_hash(&self) -> Result<Option<String>, Error> {
                let row = sqlx::query_scalar::<_, Option<String>>(
                    "SELECT hash FROM audit_log ORDER BY ts DESC, id DESC LIMIT 1",
                )
                .fetch_optional(&self.pool)
                .await
                .map_err(errors::query)?;
                Ok(row.flatten())
            }

            async fn list_audit_for_task(&self, task_id: &str) -> Result<Vec<AuditRecord>, Error> {
                let rows = sqlx::query(&format!(
                    "{SELECT_AUDIT} WHERE task_id = $1 ORDER BY ts ASC, id ASC"
                ))
                .bind(task_id)
                .fetch_all(&self.pool)
                .await
                .map_err(errors::query)?;
                rows.iter().map(audit_from_row).collect()
            }

            async fn list_all_audit(&self) -> Result<Vec<AuditRecord>, Error> {
                let rows = sqlx::query(&format!("{SELECT_AUDIT} ORDER BY ts ASC, id ASC"))
                    .fetch_all(&self.pool)
                    .await
                    .map_err(errors::query)?;
                rows.iter().map(audit_from_row).collect()
            }
        }

        #[async_trait]
        impl AgentStore for WardenStore<$db> {
            async fn upsert_agent(&self, agent: &AgentRecord) -> Result<(), Error> {
                sqlx::query(
                    "INSERT INTO agents (id, name, url, skills_json, card_json, verified, \
                     verification_error, jwks_kid, verified_at, verification_issuer, \
                     verification_alg, quarantined, created_at) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
                     ON CONFLICT (name) DO UPDATE SET url = excluded.url, \
                     skills_json = excluded.skills_json, \
                     card_json = excluded.card_json, \
                     verified = excluded.verified, \
                     verification_error = excluded.verification_error, \
                     jwks_kid = excluded.jwks_kid, \
                     verified_at = excluded.verified_at, \
                     verification_issuer = excluded.verification_issuer, \
                     verification_alg = excluded.verification_alg, \
                     quarantined = excluded.quarantined",
                )
                .bind(agent.id.as_str())
                .bind(agent.name.as_str())
                .bind(agent.url.as_str())
                .bind(agent.skills_json.as_str())
                .bind(agent.card_json.as_deref())
                .bind(i32::from(agent.verified))
                .bind(agent.verification_error.as_deref())
                .bind(agent.jwks_kid.as_deref())
                .bind(agent.verified_at.as_deref())
                .bind(agent.verification_issuer.as_deref())
                .bind(agent.verification_alg.as_deref())
                .bind(i32::from(agent.quarantined))
                .bind(agent.created_at.as_str())
                .execute(&self.pool)
                .await
                .map_err(|err| map_db_err(err, "agent"))?;
                Ok(())
            }

            async fn get_agent(&self, name: &str) -> Result<AgentRecord, Error> {
                let row = sqlx::query(&format!("{SELECT_AGENT} WHERE name = $1"))
                    .bind(name)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(errors::query)?
                    .ok_or_else(|| errors::agent_not_found(name))?;
                agent_from_row(&row)
            }

            async fn list_agents(&self) -> Result<Vec<AgentRecord>, Error> {
                let rows = sqlx::query(&format!("{SELECT_AGENT} ORDER BY name ASC"))
                    .fetch_all(&self.pool)
                    .await
                    .map_err(errors::query)?;
                rows.iter().map(agent_from_row).collect()
            }
        }

        #[async_trait]
        impl PushNotificationStore for WardenStore<$db> {
            async fn upsert_push_config(
                &self,
                record: &PushNotificationRecord,
            ) -> Result<(), Error> {
                sqlx::query(
                    "INSERT INTO push_notification_configs \
                     (id, task_id, url, token, auth_json, created_at) \
                     VALUES ($1, $2, $3, $4, $5, $6) \
                     ON CONFLICT (id) DO UPDATE SET url = excluded.url, \
                     token = excluded.token, auth_json = excluded.auth_json",
                )
                .bind(record.id.as_str())
                .bind(record.task_id.as_str())
                .bind(record.url.as_str())
                .bind(record.token.as_deref())
                .bind(record.auth_json.as_deref())
                .bind(record.created_at.as_str())
                .execute(&self.pool)
                .await
                .map_err(|err| map_db_err(err, "push notification configuration"))?;
                Ok(())
            }

            async fn get_push_config(
                &self,
                task_id: &str,
                id: &str,
            ) -> Result<PushNotificationRecord, Error> {
                let row = sqlx::query(&format!(
                    "{SELECT_PUSH_CONFIG} WHERE task_id = $1 AND id = $2"
                ))
                .bind(task_id)
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(errors::query)?
                .ok_or_else(|| errors::push_config_not_found(&format!("{task_id}/{id}")))?;
                push_config_from_row(&row)
            }

            async fn list_push_configs(
                &self,
                task_id: &str,
            ) -> Result<Vec<PushNotificationRecord>, Error> {
                let rows = sqlx::query(&format!(
                    "{SELECT_PUSH_CONFIG} WHERE task_id = $1 ORDER BY created_at ASC, id ASC"
                ))
                .bind(task_id)
                .fetch_all(&self.pool)
                .await
                .map_err(errors::query)?;
                rows.iter().map(push_config_from_row).collect()
            }

            async fn delete_push_config(&self, task_id: &str, id: &str) -> Result<(), Error> {
                let result = sqlx::query(
                    "DELETE FROM push_notification_configs WHERE task_id = $1 AND id = $2",
                )
                .bind(task_id)
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(errors::query)?;
                if result.rows_affected() == 0 {
                    return Err(errors::push_config_not_found(&format!("{task_id}/{id}")));
                }
                Ok(())
            }
        }
    };
}

impl_store!(sqlx::Sqlite);
impl_store!(sqlx::Postgres);
