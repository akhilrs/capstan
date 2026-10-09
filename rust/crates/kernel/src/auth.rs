//! Credentials and capabilities: src/controller/auth.ts.

use crate::canonical::sha256;
use crate::env::Env;
use crate::errors::{KernelError, KernelResult};
use crate::helpers::query_all;
use crate::types::ROLES;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use capstan_ledger::Database;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthenticatedActor {
    pub actor_id: String,
    pub project_id: String,
    pub role: String,
    pub seat_id: Option<String>,
    pub capabilities: BTreeSet<String>,
}

pub fn base64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// A new credential: 32 random bytes in base64url.
pub fn issue_credential(env: &dyn Env) -> String {
    base64url(&env.random_bytes(32))
}

pub fn new_actor_id(env: &dyn Env) -> String {
    env.uuid()
}

/// SHA-256 hex of a credential of 32 to 256 UTF-16 code units.
pub fn credential_hash(credential: &str) -> KernelResult<String> {
    let units = credential.encode_utf16().count();
    if !(32..=256).contains(&units) {
        return Err(KernelError::Authentication("invalid credential".into()));
    }
    Ok(sha256(credential))
}

pub fn authenticate_actor(
    database: &Database,
    project_id: &str,
    credential: &str,
) -> KernelResult<AuthenticatedActor> {
    let digest = credential_hash(credential)?;
    let rows = query_all(
        database,
        "SELECT a.actor_id, a.project_id, a.role, a.seat_id, rc.capability
         FROM actors a
         JOIN capability_grants cg
           ON cg.project_id = a.project_id AND cg.actor_id = a.actor_id AND cg.revoked_at IS NULL
         JOIN role_capabilities rc
           ON rc.role = a.role AND rc.capability = cg.capability
         WHERE a.project_id = ? AND a.credential_hash = ? AND a.active = 1 AND a.revoked_at IS NULL
         ORDER BY rc.capability",
        rusqlite::params![project_id, digest],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
            ))
        },
    )?;
    let Some(first) = rows.first() else {
        return Err(KernelError::Authentication(
            "credential is not active for this project".into(),
        ));
    };
    if !ROLES.contains(&first.2.as_str()) {
        return Err(KernelError::Authentication(
            "credential is not active for this project".into(),
        ));
    }
    Ok(AuthenticatedActor {
        actor_id: first.0.clone(),
        project_id: first.1.clone(),
        role: first.2.clone(),
        seat_id: first.3.clone().filter(|s| !s.is_empty()),
        capabilities: rows.iter().map(|r| r.4.clone()).collect(),
    })
}

pub fn require_capability(actor: &AuthenticatedActor, capability: &str) -> KernelResult<()> {
    if actor.capabilities.contains(capability) {
        Ok(())
    } else {
        Err(KernelError::Authorization(format!(
            "actor lacks {capability}"
        )))
    }
}
