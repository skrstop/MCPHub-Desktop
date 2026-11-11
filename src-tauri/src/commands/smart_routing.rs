//! Smart Routing Tauri commands (Phase 4): status / reindex / performance / search.

use serde::{Deserialize, Serialize};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartRoutingStatus {
    /// The smartRouting config intent.
    pub enabled: bool,
    /// Progressive disclosure flag (drives 3-step vs 2-step meta tools).
    pub progressive_disclosure: bool,
    /// Whether the shared mv runtime is up (model loaded).
    pub mv_running: bool,
    /// Real embedding dimension of the loaded model.
    pub embed_dim: Option<u32>,
    /// Per-server row counts in the smart_tool index.
    pub indexed_servers: Vec<ServerIndexInfo>,
    /// Estimated context footprint (tokens, ~4 chars = 1 token) of the $smart
    /// meta tools as a client would see them on the root scope. None when the
    /// config cannot be read.
    pub meta_tools_context_tokens: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerIndexInfo {
    pub server: String,
    pub tools: usize,
}

#[tauri::command]
pub async fn smart_routing_status() -> Result<SmartRoutingStatus, String> {
    let settings = crate::smart_routing::models::get_settings().await;
    let mv_running = crate::mv::is_running();
    let indexed = if mv_running {
        crate::smart_routing::store_index_summary()
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    // Context footprint of the meta tools on the root scope: mirror what a
    // client connecting to /mcp/$smart receives in tools/list.
    let meta_tools_context_tokens = if settings.enabled {
        let (scope_description, servers_list, _) =
            crate::smart_routing::meta::compute_scope("").await;
        let tools = crate::smart_routing::meta::build_meta_tools(
            &scope_description,
            &servers_list,
            settings.progressive_disclosure,
            None,
        );
        let chars: usize = tools
            .iter()
            .map(|t| serde_json::to_string(t).map(|s| s.len()).unwrap_or(0))
            .sum();
        Some((chars / 4) as u64)
    } else {
        None
    };
    Ok(SmartRoutingStatus {
        enabled: settings.enabled,
        progressive_disclosure: settings.progressive_disclosure,
        mv_running,
        embed_dim: crate::mv::embed_dim().map(|d| d as u32),
        meta_tools_context_tokens,
        indexed_servers: indexed
            .into_iter()
            .map(|(server, tools)| ServerIndexInfo { server, tools })
            .collect(),
    })
}

#[tauri::command]
/// Participant count for the model-swap dialog's confirm-phase meta
/// ("N 个服务器"). Read-only; still gated for consistency with reindex.
pub async fn smart_routing_reindex_participants(
    session: tauri::State<'_, crate::commands::auth::SessionState>,
) -> Result<(usize, usize), String> {
    crate::commands::config::require_admin(&session).await?;
    crate::smart_routing::index::reindex_participants()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn smart_routing_reindex(session: tauri::State<'_, crate::commands::auth::SessionState>) -> Result<usize, String> {
    // Triggers a full embedding re-index and can spin up the shared mv model
    // runtime — admin-only (review round 8, 2026-10-04).
    crate::commands::config::require_admin(&session).await?;
    crate::smart_routing::index::reindex_all()
        .await
        .map_err(|e| e.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartRoutingPerformance {
    /// Server rows in the index (server-level embeddings).
    pub server_rows: usize,
    /// Tool rows in the index.
    pub tool_rows: usize,
    /// Total rows.
    pub total_rows: usize,
}

#[tauri::command]
pub async fn smart_routing_performance() -> Result<SmartRoutingPerformance, String> {
    let (server_rows, tool_rows, total_rows) = crate::smart_routing::store_performance()
        .await
        .map_err(|e| e.to_string())?;
    Ok(SmartRoutingPerformance {
        server_rows,
        tool_rows,
        total_rows,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartSearchPayload {
    pub query: String,
    /// "all" (default) | `group:{id-or-name}` | `server:{name}` | bare
    /// group name/id or server name (legacy). The frontend sends the
    /// prefixed forms; see `resolve_search_scope` for full semantics.
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Pure scope resolution for the relevance search (unit tested): `None` =
/// all servers (global); `Some(list)` = candidate servers.
///
/// Scope forms (frontend sends the raw option value, prefix included):
/// - `all` / empty / None → global.
/// - `group:{id-or-name}` → that group's member server names; empty value
///   after the prefix yields an EMPTY candidate set (deny-by-default).
/// - `server:{name}` → single-server scope; empty value → empty set.
/// - bare string (legacy/direct callers) → group by name OR id, else
///   single-server scope. An unknown bare scope yields a SINGLE-SERVER
///   scope (possibly a sleeping on-demand server) — never a silent degrade
///   to a global search.
///
/// Prefix precedence note: the `group:`/`server:` prefix branches run BEFORE
/// the bare-name lookup, so a group literally named `group:web` is only
/// reachable via the doubled prefix `group:group:web` (bare `group:web`
/// resolves to the prefix branch and denies). Deny-safe by construction;
/// pinned by tests.
pub fn resolve_search_scope(scope: &str, groups: &[crate::models::group::Group]) -> Option<Vec<String>> {
    let s = scope.trim();
    if s.is_empty() || s == "all" {
        return None;
    }
    if let Some(rest) = s.strip_prefix("group:") {
        // Explicit group kind: the value is normally the group id, but a
        // name also matches (robustness for hand-built scopes). Empty value
        // → empty candidate set, never global.
        let rest = rest.trim();
        if rest.is_empty() {
            return Some(Vec::new());
        }
        return match groups.iter().find(|g| g.id == rest || g.name == rest) {
            Some(grp) => Some(
                grp.servers
                    .iter()
                    .filter_map(|sv| sv.get("name").and_then(|n| n.as_str()).map(|x| x.to_string()))
                    .collect::<Vec<String>>(),
            ),
            None => Some(Vec::new()),
        };
    }
    if let Some(rest) = s.strip_prefix("server:") {
        // Explicit server kind: the value is taken verbatim (a server may
        // legitimately be named "all" or contain colons). Empty → empty set.
        let rest = rest.trim();
        if rest.is_empty() {
            return Some(Vec::new());
        }
        return Some(vec![rest.to_string()]);
    }
    match groups.iter().find(|g| g.name == s || g.id == s) {
        Some(grp) => Some(
            grp.servers
                .iter()
                .filter_map(|sv| sv.get("name").and_then(|n| n.as_str()).map(|x| x.to_string()))
                .collect::<Vec<String>>(),
        ),
        None => Some(vec![s.to_string()]),
    }
}

/// Pure query normalization for the search command (unit tested): trim
/// whitespace; empty/whitespace-only queries are rejected.
pub fn normalize_search_query(query: &str) -> Result<String, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("Query is required".to_string());
    }
    Ok(q.to_string())
}

/// Relevance search over the Smart Routing tool index (ServersPage dialog).
/// Same hybrid search the $smart meta tool uses; the scope selector narrows
/// the candidate set to one group's members or a single server.
#[tauri::command]
pub async fn smart_routing_search(
    session: tauri::State<'_, crate::commands::auth::SessionState>,
    payload: SmartSearchPayload,
) -> Result<Vec<crate::smart_routing::search::SmartSearchResult>, String> {
    crate::commands::config::require_admin(&session).await?;
    let query = normalize_search_query(&payload.query)?;
    let allowed_servers = match payload.scope.as_deref() {
        None => None,
        Some(s) => {
            let groups = crate::services::group_service::list_all()
                .await
                .map_err(|e| e.to_string())?;
            resolve_search_scope(s, &groups)
        }
    };
    crate::smart_routing::search::search(&query, payload.limit, allowed_servers, None, Some(0.0))
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod smart_search_tests {
    use super::*;
    use crate::models::group::Group;
    use serde_json::json;

    fn group(id: &str, name: &str, servers: serde_json::Value) -> Group {
        Group {
            id: id.to_string(),
            name: name.to_string(),
            description: None,
            instructions: None,
            servers: serde_json::from_value(servers).unwrap(),
            created_at: String::new(),
        }
    }

    fn fixture() -> Vec<Group> {
        vec![
            group(
                "11111111-1111-1111-1111-111111111111",
                "web",
                json!([{"name": "playwright"}, {"name": "codegraph"}]),
            ),
            group(
                "22222222-2222-2222-2222-222222222222",
                "data",
                json!(["legacy-string-member"]),
            ),
        ]
    }

    #[test]
    fn all_and_empty_and_none_are_global() {
        assert!(resolve_search_scope("all", &fixture()).is_none());
        assert!(resolve_search_scope("", &fixture()).is_none());
        assert!(resolve_search_scope("  ", &fixture()).is_none());
    }

    #[test]
    fn group_by_name_expands_members() {
        let r = resolve_search_scope("web", &fixture()).unwrap();
        assert_eq!(r, vec!["playwright".to_string(), "codegraph".to_string()]);
    }

    #[test]
    fn group_by_id_expands_members() {
        let r =
            resolve_search_scope("11111111-1111-1111-1111-111111111111", &fixture()).unwrap();
        assert_eq!(r, vec!["playwright".to_string(), "codegraph".to_string()]);
    }

    #[test]
    fn group_with_legacy_string_members_yields_empty_set() {
        // Old-format members are plain strings (no "name" key): expansion
        // yields an EMPTY set — deny-all, never a silent global fallback.
        let r = resolve_search_scope("data", &fixture()).unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn unknown_scope_is_single_server_not_global() {
        // Not a group → single-server scope (may be a sleeping on-demand
        // server not in any group). Must NOT degrade to None (global).
        let r = resolve_search_scope("solo-server", &fixture()).unwrap();
        assert_eq!(r, vec!["solo-server".to_string()]);
    }

    #[test]
    fn scope_is_trimmed() {
        let r = resolve_search_scope("  web  ", &fixture()).unwrap();
        assert_eq!(r, vec!["playwright".to_string(), "codegraph".to_string()]);
    }

    #[test]
    fn bare_scope_resolves_to_group_members() {
        // A bare name resolves via the group list (by name or id); there is
        // no competing server lookup on this branch.
        let r = resolve_search_scope("web", &fixture()).unwrap();
        assert_eq!(r, vec!["playwright".to_string(), "codegraph".to_string()]);
    }

    #[test]
    fn payload_rejects_wrong_query_type() {
        let r = serde_json::from_str::<SmartSearchPayload>(r#"{"query":123}"#);
        assert!(r.is_err());
    }

    #[test]
    fn payload_ignores_unknown_fields() {
        let p: SmartSearchPayload =
            serde_json::from_str(r#"{"query":"q","bogus":true}"#).unwrap();
        assert_eq!(p.query, "q");
        assert_eq!(p.scope, None);
    }

    #[test]
    fn payload_deserializes_camel_case() {
        let p: SmartSearchPayload =
            serde_json::from_str(r#"{"query":"deploy","scope":"web","limit":5}"#).unwrap();
        assert_eq!(p.query, "deploy");
        assert_eq!(p.scope.as_deref(), Some("web"));
        assert_eq!(p.limit, Some(5));
    }

    #[test]
    fn payload_defaults() {
        let p: SmartSearchPayload = serde_json::from_str(r#"{"query":"x"}"#).unwrap();
        assert!(p.scope.is_none());
        assert!(p.limit.is_none());
    }

    // ── Prefixed scope forms (frontend sends the raw option value) ──

    #[test]
    fn prefixed_group_by_id_expands_members() {
        let r = resolve_search_scope(
            "group:11111111-1111-1111-1111-111111111111",
            &fixture(),
        )
        .unwrap();
        assert_eq!(r, vec!["playwright".to_string(), "codegraph".to_string()]);
    }

    #[test]
    fn prefixed_group_by_name_also_matches() {
        let r = resolve_search_scope("group:web", &fixture()).unwrap();
        assert_eq!(r, vec!["playwright".to_string(), "codegraph".to_string()]);
    }

    #[test]
    fn prefixed_group_unknown_id_yields_empty_set_not_global() {
        let r = resolve_search_scope("group:no-such-group", &fixture()).unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn prefixed_group_empty_value_yields_empty_set_not_global() {
        // 'group:' with an empty id must DENY (empty set), never degrade to
        // a global search — the deny-by-default contract.
        assert_eq!(resolve_search_scope("group:", &fixture()).unwrap(), Vec::<String>::new());
        assert_eq!(
            resolve_search_scope("group:   ", &fixture()).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn prefixed_server_takes_value_verbatim() {
        // A server literally named "all" must scope to THAT server, not
        // degrade to a global search.
        let r = resolve_search_scope("server:all", &fixture()).unwrap();
        assert_eq!(r, vec!["all".to_string()]);
    }

    #[test]
    fn prefixed_server_with_colons_kept_verbatim() {
        let r = resolve_search_scope("server:server:foo", &fixture()).unwrap();
        assert_eq!(r, vec!["server:foo".to_string()]);
    }

    #[test]
    fn prefixed_server_empty_value_yields_empty_set_not_global() {
        assert_eq!(resolve_search_scope("server:", &fixture()).unwrap(), Vec::<String>::new());
    }

    // ── Query normalization (command-path logic, extracted for testability) ──

    #[test]
    fn normalize_query_trims_whitespace() {
        assert_eq!(normalize_search_query("  deploy web  ").unwrap(), "deploy web");
    }

    #[test]
    fn normalize_query_rejects_empty_and_whitespace_only() {
        assert!(normalize_search_query("").is_err());
        assert!(normalize_search_query("   \t\n ").is_err());
        assert_eq!(
            normalize_search_query("   ").unwrap_err(),
            "Query is required".to_string()
        );
    }

    #[test]
    fn normalize_query_keeps_inner_whitespace() {
        assert_eq!(normalize_search_query("a  b").unwrap(), "a  b");
    }

    // ── Prefix-precedence pinning: a group literally named "group:x" ──

    #[test]
    fn group_named_with_prefix_unreachable_via_bare_scope() {
        // A group literally named "group:web": the bare scope "group:web"
        // enters the prefix branch (looks for id/name "web"), misses, and
        // denies — it can never resolve to this group by bare name.
        let groups = vec![group("g-1", "group:web", json!([{"name": "playwright"}]))];
        let r = resolve_search_scope("group:web", &groups).unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn group_named_with_prefix_reachable_via_doubled_prefix() {
        let groups = vec![group("g-1", "group:web", json!([{"name": "playwright"}]))];
        let r = resolve_search_scope("group:group:web", &groups).unwrap();
        assert_eq!(r, vec!["playwright".to_string()]);
    }
}
