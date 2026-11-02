use crate::{db, models::bearer_key::{BearerKey, BearerKeyPayload}};
use anyhow::Result;
use sqlx::Row;
use uuid::Uuid;

pub async fn list_all() -> Result<Vec<BearerKey>> {
    let rows = sqlx::query(
        "SELECT id, name, token, enabled, access_type, allowed_groups, allowed_servers, \
         excluded_groups, excluded_servers, created_at \
         FROM bearer_keys ORDER BY created_at DESC",
    )
    .fetch_all(db::pool())
    .await?;

    rows.into_iter()
        .map(|r| {
            let enabled: i64 = r.try_get("enabled")?;
            let groups_json: String = r.try_get("allowed_groups")?;
            let servers_json: String = r.try_get("allowed_servers")?;
            let ex_groups_json: String = r.try_get("excluded_groups")?;
            let ex_servers_json: String = r.try_get("excluded_servers")?;
            Ok(BearerKey {
                id: r.try_get("id")?,
                name: r.try_get("name")?,
                token: r.try_get("token")?,
                enabled: enabled != 0,
                access_type: r.try_get("access_type")?,
                allowed_groups: serde_json::from_str(&groups_json).unwrap_or_default(),
                allowed_servers: serde_json::from_str(&servers_json).unwrap_or_default(),
                excluded_groups: serde_json::from_str(&ex_groups_json).unwrap_or_default(),
                excluded_servers: serde_json::from_str(&ex_servers_json).unwrap_or_default(),
                created_at: r.try_get("created_at")?,
            })
        })
        .collect()
}

pub async fn find_by_token(token: &str) -> Result<Option<BearerKey>> {
    // Constant-time token comparison: an indexed `WHERE token = ?` lookup
    // leaks timing information about matching prefixes. The key count is
    // small (desktop scale), so load ALL keys and compare in constant time —
    // the CALLER filters disabled keys (a disabled key must fail 401, not
    // pass auth).
    let candidates = list_all().await?;
    let token_bytes = token.as_bytes();
    let mut best: Option<BearerKey> = None;
    let mut best_found = 0u8;
    for key in candidates {
        let k = key.token.as_bytes();
        // Constant-time equality over min length + length difference folded
        // in, so early-exit timing on length is eliminated.
        let n = k.len().max(token_bytes.len());
        let mut diff = (k.len() ^ token_bytes.len()) as u8;
        for i in 0..n {
            let a = k.get(i).copied().unwrap_or(0);
            let b = token_bytes.get(i).copied().unwrap_or(0);
            diff |= a ^ b;
        }
        let matched = (diff == 0) as u8;
        // Branchless-ish selection: `best_found` accumulates a match bit so
        // later keys can never overwrite an earlier match; the `if` only
        // guards the assignment, not the comparison outcome.
        if best_found == 0 && matched == 1 {
            best = Some(key);
        }
        best_found |= matched;
    }
    Ok(best)
}

/// Origin bearerKeyController parity: an `exclude` key must exclude at least
/// one group or server, otherwise it would be an unrestricted "all" key
/// wearing an exclusion badge.
fn validate_excluded_scope(
    access_type: &str,
    excluded_groups: &[String],
    excluded_servers: &[String],
) -> Result<()> {
    if access_type != "exclude" {
        return Ok(());
    }
    if excluded_groups.is_empty() && excluded_servers.is_empty() {
        return Err(anyhow::anyhow!(
            "Exclude scope requires at least one excluded group or server"
        ));
    }
    Ok(())
}

pub async fn create(payload: &BearerKeyPayload) -> Result<BearerKey> {
    validate_excluded_scope(&payload.access_type, &payload.excluded_groups, &payload.excluded_servers)?;
    let id = Uuid::new_v4().to_string();
    // Generate a random 32-byte bearer token
    let token = format!("mcphub_{}", Uuid::new_v4().simple());
    let groups_json = serde_json::to_string(&payload.allowed_groups)?;
    let servers_json = serde_json::to_string(&payload.allowed_servers)?;
    let ex_groups_json = serde_json::to_string(&payload.excluded_groups)?;
    let ex_servers_json = serde_json::to_string(&payload.excluded_servers)?;

    sqlx::query(
        "INSERT INTO bearer_keys (id, name, token, enabled, access_type, allowed_groups, allowed_servers, excluded_groups, excluded_servers) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&payload.name)
    .bind(&token)
    .bind(payload.enabled as i64)
    .bind(&payload.access_type)
    .bind(&groups_json)
    .bind(&servers_json)
    .bind(&ex_groups_json)
    .bind(&ex_servers_json)
    .execute(db::pool())
    .await?;

    let row = sqlx::query("SELECT created_at FROM bearer_keys WHERE id = ?")
        .bind(&id)
        .fetch_one(db::pool())
        .await?;

    Ok(BearerKey {
        id,
        name: payload.name.clone(),
        token,
        enabled: payload.enabled,
        access_type: payload.access_type.clone(),
        allowed_groups: payload.allowed_groups.clone(),
        allowed_servers: payload.allowed_servers.clone(),
        excluded_groups: payload.excluded_groups.clone(),
        excluded_servers: payload.excluded_servers.clone(),
        created_at: row.try_get("created_at")?,
    })
}

pub async fn update(id: &str, payload: &BearerKeyPayload) -> Result<Option<BearerKey>> {
    // Desktop update is a full-row replace (the payload always carries every
    // field), so the payload values ARE the resulting values — origin's
    // "updates.accessType ?? existing.accessType" resolution collapses to
    // payload.access_type here.
    let excluded_groups = if payload.access_type == "exclude" {
        validate_excluded_scope(
            &payload.access_type,
            &payload.excluded_groups,
            &payload.excluded_servers,
        )?;
        payload.excluded_groups.clone()
    } else {
        // Drop exclusions when the key no longer uses the exclude scope.
        Vec::new()
    };
    let excluded_servers = if payload.access_type == "exclude" {
        payload.excluded_servers.clone()
    } else {
        Vec::new()
    };

    let groups_json = serde_json::to_string(&payload.allowed_groups)?;
    let servers_json = serde_json::to_string(&payload.allowed_servers)?;
    let ex_groups_json = serde_json::to_string(&excluded_groups)?;
    let ex_servers_json = serde_json::to_string(&excluded_servers)?;

    let affected = sqlx::query(
        "UPDATE bearer_keys SET name = ?, enabled = ?, access_type = ?, allowed_groups = ?, allowed_servers = ?, \
         excluded_groups = ?, excluded_servers = ? \
         WHERE id = ?",
    )
    .bind(&payload.name)
    .bind(payload.enabled as i64)
    .bind(&payload.access_type)
    .bind(&groups_json)
    .bind(&servers_json)
    .bind(&ex_groups_json)
    .bind(&ex_servers_json)
    .bind(id)
    .execute(db::pool())
    .await?
    .rows_affected();

    if affected == 0 {
        return Ok(None);
    }

    let row = sqlx::query(
        "SELECT id, name, token, enabled, access_type, allowed_groups, allowed_servers, \
         excluded_groups, excluded_servers, created_at \
         FROM bearer_keys WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db::pool())
    .await?;

    match row {
        None => Ok(None),
        Some(r) => {
            let enabled: i64 = r.try_get("enabled")?;
            let groups_json: String = r.try_get("allowed_groups")?;
            let servers_json: String = r.try_get("allowed_servers")?;
            let ex_groups_json: String = r.try_get("excluded_groups")?;
            let ex_servers_json: String = r.try_get("excluded_servers")?;
            Ok(Some(BearerKey {
                id: r.try_get("id")?,
                name: r.try_get("name")?,
                token: r.try_get("token")?,
                enabled: enabled != 0,
                access_type: r.try_get("access_type")?,
                allowed_groups: serde_json::from_str(&groups_json).unwrap_or_default(),
                allowed_servers: serde_json::from_str(&servers_json).unwrap_or_default(),
                excluded_groups: serde_json::from_str(&ex_groups_json).unwrap_or_default(),
                excluded_servers: serde_json::from_str(&ex_servers_json).unwrap_or_default(),
                created_at: r.try_get("created_at")?,
            }))
        }
    }
}

pub async fn delete(id: &str) -> Result<bool> {
    let affected = sqlx::query("DELETE FROM bearer_keys WHERE id = ?")
        .bind(id)
        .execute(db::pool())
        .await?
        .rows_affected();
    Ok(affected > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(name: &str, access_type: &str) -> BearerKeyPayload {
        BearerKeyPayload {
            name: name.to_string(),
            enabled: true,
            access_type: access_type.to_string(),
            allowed_groups: Vec::new(),
            allowed_servers: Vec::new(),
            excluded_groups: Vec::new(),
            excluded_servers: Vec::new(),
        }
    }

    /// #1413: an `exclude` key with no exclusions is an unrestricted "all"
    /// key wearing an exclusion badge — reject with the origin message.
    #[tokio::test]
    async fn create_exclude_without_exclusions_is_rejected() {
        crate::db::ensure_test_pool().await;
        let err = create(&payload("x", "exclude")).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "Exclude scope requires at least one excluded group or server"
        );
    }

    /// #1413: a valid `exclude` key stores both exclusion lists.
    #[tokio::test]
    async fn create_exclude_with_exclusions_round_trips() {
        crate::db::ensure_test_pool().await;
        let mut p = payload("x", "exclude");
        p.excluded_groups = vec!["g1".to_string()];
        p.excluded_servers = vec!["srv-a".to_string()];
        let created = create(&p).await.unwrap();
        assert_eq!(created.excluded_groups, vec!["g1"]);
        assert_eq!(created.excluded_servers, vec!["srv-a"]);
        let listed = list_all().await.unwrap();
        let row = listed.iter().find(|k| k.id == created.id).unwrap();
        assert_eq!(row.excluded_groups, vec!["g1"]);
        assert_eq!(row.excluded_servers, vec!["srv-a"]);
        delete(&created.id).await.unwrap();
    }

    /// Origin create parity: exclusions are stored (dormant) even for
    /// non-exclude access types — only update clears them.
    #[tokio::test]
    async fn create_non_exclude_keeps_exclusions_dormant() {
        crate::db::ensure_test_pool().await;
        let mut p = payload("x", "all");
        p.excluded_servers = vec!["srv-a".to_string()];
        let created = create(&p).await.unwrap();
        assert_eq!(created.excluded_servers, vec!["srv-a"]);
        delete(&created.id).await.unwrap();
    }

    /// Origin update parity: switching away from `exclude` clears both
    /// exclusion lists.
    #[tokio::test]
    async fn update_exclude_to_all_clears_exclusions() {
        crate::db::ensure_test_pool().await;
        let mut p = payload("x", "exclude");
        p.excluded_groups = vec!["g1".to_string()];
        p.excluded_servers = vec!["srv-a".to_string()];
        let created = create(&p).await.unwrap();

        let mut upd = payload("x", "all");
        upd.excluded_groups = vec!["g1".to_string()];
        upd.excluded_servers = vec!["srv-a".to_string()];
        let updated = update(&created.id, &upd).await.unwrap().unwrap();
        assert!(updated.excluded_groups.is_empty());
        assert!(updated.excluded_servers.is_empty());
        delete(&created.id).await.unwrap();
    }

    /// Origin update parity: switching TO `exclude` without exclusions is
    /// rejected (validated against the resulting values).
    #[tokio::test]
    async fn update_all_to_exclude_without_exclusions_is_rejected() {
        crate::db::ensure_test_pool().await;
        let created = create(&payload("x", "all")).await.unwrap();
        let err = update(&created.id, &payload("x", "exclude"))
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Exclude scope requires at least one excluded group or server"
        );
        delete(&created.id).await.unwrap();
    }

    /// Origin update parity: switching TO `exclude` with exclusions succeeds.
    #[tokio::test]
    async fn update_all_to_exclude_with_exclusions_succeeds() {
        crate::db::ensure_test_pool().await;
        let created = create(&payload("x", "all")).await.unwrap();
        let mut upd = payload("x", "exclude");
        upd.excluded_servers = vec!["srv-a".to_string()];
        let updated = update(&created.id, &upd).await.unwrap().unwrap();
        assert_eq!(updated.access_type, "exclude");
        assert_eq!(updated.excluded_servers, vec!["srv-a"]);
        delete(&created.id).await.unwrap();
    }
}
