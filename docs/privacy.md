# A2A Warden data privacy statement

This document states what the warden records, why, and how long it is kept. It
covers the current implementation (phase 1) and the commitments of the later
phases.

## 1. What the warden records

The warden is a supervisory intermediary: to supervise traffic it must
capture it. The following is stored:

| Data                | Contents                                                        | Why                                              |
| ------------------- | --------------------------------------------------------------- | ------------------------------------------------ |
| Tasks               | Identifiers, state, timestamps, and the metadata of the request | Needed to resume, cancel, and audit a task       |
| Messages            | The full content of every message, in both directions           | This is the supervised traffic itself             |
| Approvals           | Who decided, when, and with which reason                        | The decision log required by |
| Audit log           | Every state transition with its actor                            | The forensic trail required by |
| Registered agents   | Name, URL, skills, and the interception rule                    | The agent registry                               |
| Agent Cards         | The card published by the registered agent                       | Identity of the peers (phase 3 verifies the signature) |

Message content is stored verbatim, including any personal data the agents
exchange. There is no masking in phase 1.

## 2. What the warden does not do

* It does not send traffic to any third party. Delegation goes only to the
  agents registered in the configuration.
* It does not read the content of a message to decide anything: the
  interception rule of the Cedar policy engine is declarative and content agnostic.
* It does not log to stdout the content of the messages. The logs contain
  identifiers, states, and actions.

## 3. Who can read the data

| Surface                 | Access                                                        |
| ----------------------- | ------------------------------------------------------------- |
| Web console             | Anyone who can reach the console prefix                        |
| Administration API      | Anyone who can reach the API; a static token protects it when `admin.token` is set |
| A2A JSON-RPC binding    | Anyone who can reach the listener                              |
| Database file           | Anyone with access to the host and the file permissions        |

The warden ships without authentication on the console by default. The
administration API accepts a static token (`admin.token`) and the warden logs
a warning on every request while no token is configured. Phase 6 replaces the
static token with OIDC and adds multi-tenancy with per-tenant isolation.

**Deployments that expose the warden beyond localhost must set
`admin.token` and put the listener behind a reverse proxy or a private
network.**

## 4. Retention

Retention is configured under `retention`:

```yaml
retention:
  audit_days: 90
  task_days: 30
  auto_purge: false
  purge_interval_hours: 24
```

The values are the *target* retention windows. In phase 1 the scheduled
purging job is not implemented yet (`auto_purge` is accepted and stored but
does not run), so the tables grow until the operator removes the old rows.
Phase 5 implements the scheduled purge, the hash-chained audit log, the
selective masking of sensitive content for reviewers, and a certifiable
export.

Until phase 5 lands, retention is enforced operationally, for example:

```sql
DELETE FROM messages WHERE created_at < '2026-01-01';
DELETE FROM tasks    WHERE created_at < '2026-01-01';
DELETE FROM audit_log WHERE ts < '2026-01-01';
```

## 5. Identifiers

The warden generates its own identifiers (UUID v7) for tasks, approvals, audit
entries, and message rows. The identifiers of the peers are stored as they
arrive (`messages.message_id`) and are never used as a primary key, so a
malicious peer cannot overwrite an existing row by replaying an identifier.

## 6. Transport security

The warden speaks plain HTTP. Terminate TLS in front of it (reverse proxy or
service mesh) for any deployment that leaves the host. The `https` scheme is
required for the URLs of the remote agents, and the outbound client uses
rustls with the platform trust store.

## 7. Reporting

Security issues are handled as described in [SECURITY.md](../SECURITY.md).
Privacy questions and requests about the handling of recorded data can be
raised as a GitHub issue in the same repository.
