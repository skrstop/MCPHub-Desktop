//! Hybrid retrieval over the `smart_tool` index (desktop semantics, §4.2):
//! vector channel (cosine via lancedb) + keyword channel (LIKE) merged by
//! weighted score, threshold-filtered, truncated to max_results. This
//! replaces origin's dynamic thresholds / 0.8+0.2 server mixing.
//!
//! The merge is a pure function (`merge_hits`) so the weighting/threshold
//! behavior is unit-testable without a model or lancedb.

use anyhow::{anyhow, Result};
use serde::Serialize;

use super::models;
use super::store::SmartHit;

/// One merged, scored result handed to the meta-tool layer (P4).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartSearchResult {
    pub server_name: String,
    pub tool_name: String,
    pub content_id: String,
    pub content_type: String,
    pub text_content: String,
    pub metadata: Option<serde_json::Value>,
    /// Final weighted score in [0,1].
    pub score: f32,
}

/// Weighted merge of the two channels (PURE — unit tested):
/// - key = (server_name, tool_name) (server rows have an empty tool_name);
/// - vec score = 1 - cosine distance, clamped [0,1];
/// - kw score  = matched query terms / total query terms;
/// - final = vw * vec + kw * kwScore, dropped below `threshold`, sorted desc,
///   truncated to `limit`.
pub fn merge_hits(
    vec_hits: &[SmartHit],
    kw_hits: &[SmartHit],
    query: &str,
    vector_weight: f32,
    keyword_weight: f32,
    threshold: f32,
    limit: usize,
) -> Vec<SmartSearchResult> {
    use std::collections::HashMap;

    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect();
    let term_count = terms.len().max(1);

    // Collect per-channel scores FIRST, then merge — order-independent (a hit
    // in both channels contributes vw*vs + kw*ks regardless of which channel
    // created the entry).
    let mut vs: HashMap<(String, String), (f32, &SmartHit)> = HashMap::new();
    for h in vec_hits {
        vs.insert(
            (h.server_name.clone(), h.tool_name.clone()),
            ((1.0 - h.distance).clamp(0.0, 1.0), h),
        );
    }
    let mut ks: HashMap<(String, String), f32> = HashMap::new();
    for h in kw_hits {
        let lower = h.text_content.to_lowercase();
        let matched = terms.iter().filter(|t| lower.contains(t.as_str())).count();
        ks.insert(
            (h.server_name.clone(), h.tool_name.clone()),
            (matched as f32) / (term_count as f32),
        );
    }

    let mut out: Vec<SmartSearchResult> = Vec::with_capacity(vs.len().max(ks.len()));
    for (key, (score, h)) in vs.iter() {
        let mut r = to_result(h, score * vector_weight);
        if let Some(k) = ks.get(key) {
            r.score += k * keyword_weight;
        }
        out.push(r);
    }
    for (key, k) in ks.iter() {
        if vs.contains_key(key) {
            continue; // already merged above
        }
        if let Some(h) = kw_hits.iter().find(|h| {
            *key == (h.server_name.clone(), h.tool_name.clone())
        }) {
            let mut r = to_result(h, 0.0);
            r.score = k * keyword_weight;
            out.push(r);
        }
    }

    out.retain(|r| r.score >= threshold);
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(limit);
    out
}

fn to_result(h: &SmartHit, score: f32) -> SmartSearchResult {
    SmartSearchResult {
        server_name: h.server_name.clone(),
        tool_name: h.tool_name.clone(),
        content_id: h.content_id.clone(),
        content_type: h.content_type.clone(),
        text_content: h.text_content.clone(),
        metadata: h.metadata.clone(),
        score,
    }
}

/// Full hybrid search. `allowed_servers` = None → all servers (global `$smart`
/// scope); Some(list) → group/server scope filter. `limit_override` = the
/// meta-tool's `limit` argument (clamped to [1, max_results]).
pub async fn search(
    query: &str,
    limit_override: Option<u32>,
    allowed_servers: Option<Vec<String>>,
) -> Result<Vec<SmartSearchResult>> {
    let settings = models::get_settings().await;
    if !settings.enabled {
        return Err(anyhow!("smart routing is not enabled"));
    }
    if !crate::mv::is_running() {
        return Err(anyhow!("mv runtime not running"));
    }
    let limit = match limit_override {
        Some(n) => n.clamp(1, settings.max_results) as usize,
        None => settings.max_results as usize,
    };
    // Fetch more candidates than the limit so the Rust-side merge + threshold
    // filtering still yields enough hits after pruning (same rationale as RAG).
    let fetch = limit * 4;

    // Phase the locks like rag::search: mv (embed) first, released before the
    // store queries (each opens its own table handle off the shared connection).
    let qvec = crate::mv::with_model(|m| m.embed(query))
        .await
        .map_err(|e| anyhow!("embed query: {}", e))?;

    let embed_dim = crate::mv::embed_dim().unwrap_or(0);
    let conn = crate::mv::connection_async().await?;
    let store = super::store::SmartToolStore::open_with_conn(conn, embed_dim).await?;
    let allowed = allowed_servers.as_deref();

    let vec_hits = if settings.vector_weight > 0.0 {
        store
            .vector_search(&qvec, fetch, allowed)
            .await
            .unwrap_or_else(|e| {
                log::warn!("[smart] vector search failed: {}", e);
                Vec::new()
            })
    } else {
        Vec::new()
    };
    let kw_hits = if settings.keyword_weight > 0.0 {
        store
            .keyword_search(query, fetch, allowed)
            .await
            .unwrap_or_else(|e| {
                log::warn!("[smart] keyword search failed: {}", e);
                Vec::new()
            })
    } else {
        Vec::new()
    };

    // Server-level rows exist only for the performance panel; they must not
    // consume `limit` slots in the merge truncation (meta layer would filter
    // them out AFTER truncate, wasting result slots for real tools).
    let keep_tool = |h: &super::store::SmartHit| h.content_type == "tool";
    let vec_hits: Vec<_> = vec_hits.into_iter().filter(keep_tool).collect();
    let kw_hits: Vec<_> = kw_hits.into_iter().filter(keep_tool).collect();

    // Enabled-state drift filter: a tool disabled AFTER indexing must not be
    // served. One config read per distinct server (tiny table).
    let hits = filter_disabled(vec_hits, kw_hits).await;

    Ok(merge_hits(
        &hits.0,
        &hits.1,
        query,
        settings.vector_weight,
        settings.keyword_weight,
        settings.score_threshold,
        limit,
    ))
}

/// Split hits back into the two channels after removing rows whose tool is
/// now disabled via server_tool_config (server rows always pass).
async fn filter_disabled(
    vec_hits: Vec<super::store::SmartHit>,
    kw_hits: Vec<super::store::SmartHit>,
) -> (Vec<super::store::SmartHit>, Vec<super::store::SmartHit>) {
    let mut servers: Vec<String> = Vec::new();
    for h in vec_hits.iter().chain(kw_hits.iter()) {
        if h.content_type == "tool" && !servers.iter().any(|s| s == &h.server_name) {
            servers.push(h.server_name.clone());
        }
    }
    // Set of disabled (server, tool) pairs.
    let mut disabled: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();
    for s in &servers {
        if let Ok(cfgs) =
            crate::services::server_tool_config_service::list_for_server(s, Some("tool")).await
        {
            for c in cfgs {
                if !c.enabled {
                    disabled.insert((s.clone(), c.item_name.clone()));
                }
            }
        }
    }
    if disabled.is_empty() {
        return (vec_hits, kw_hits);
    }
    let keep = |h: &super::store::SmartHit| {
        h.content_type != "tool" || !disabled.contains(&(h.server_name.clone(), h.tool_name.clone()))
    };
    (
        vec_hits.into_iter().filter(keep).collect(),
        kw_hits.into_iter().filter(keep).collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::server::Tool;

    fn hit(server: &str, tool: &str, text: &str, distance: f32) -> SmartHit {
        SmartHit {
            content_type: if tool.is_empty() { "server" } else { "tool" }.to_string(),
            content_id: format!("{}:{}", server, tool),
            server_name: server.to_string(),
            tool_name: tool.to_string(),
            text_content: text.to_string(),
            metadata: None,
            distance,
        }
    }

    #[test]
    fn merge_weights_and_threshold() {
        let vec_hits = vec![hit("s1", "alpha", "alpha text", 0.2), hit("s1", "beta", "beta text", 0.9)];
        let kw_hits = vec![hit("s1", "alpha", "alpha text", 0.0)];
        // alpha: vec 0.8, kw 1.0 (single term matched) → 0.5*0.8 + 0.5*1.0 = 0.9
        // beta:  vec 0.1, kw 0.0 → 0.05
        let out = merge_hits(&vec_hits, &kw_hits, "alpha", 0.5, 0.5, 0.4, 50);
        assert_eq!(out.len(), 1, "beta (0.05) must be dropped by threshold 0.4");
        assert_eq!(out[0].tool_name, "alpha");
        assert!((out[0].score - 0.9).abs() < 1e-4);
    }

    #[test]
    fn merge_order_independent() {
        // A hit present in BOTH channels gets vw*vs + kw*ks; a kw-only hit is
        // still emitted. Combined case exercises both merge paths in one call.
        let v = vec![hit("s1", "alpha", "alpha text", 0.2)]; // vs=0.8
        let k = vec![
            hit("s1", "alpha", "alpha text", 0.0),   // ks=1.0 → 0.5*0.8+0.5*1.0=0.9
            hit("s1", "gamma", "alpha gamma", 0.0),  // kw-only, ks=1.0 → 0.5
        ];
        let out = merge_hits(&v, &k, "alpha", 0.5, 0.5, 0.4, 50);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].tool_name, "alpha");
        assert!((out[0].score - 0.9).abs() < 1e-4);
        assert_eq!(out[1].tool_name, "gamma");
        assert!((out[1].score - 0.5).abs() < 1e-4);
        // A kw-only hit whose text matches nothing scores 0 → dropped at
        // threshold 0.4 (0 >= 0 would pass, so use a no-match text).
        let k2 = vec![hit("s1", "delta", "unrelated words", 0.0)];
        let out2 = merge_hits(&v, &k2, "alpha", 0.5, 0.5, 0.0, 50);
        assert_eq!(out2.len(), 2, "delta scores 0 but threshold 0 keeps it");
        assert_eq!(out2[1].tool_name, "delta");
    }

    #[test]
    fn merge_keyword_only_and_ordering() {
        let kw_hits = vec![
            hit("s1", "a", "deploy stuff", 0.0),
            hit("s1", "b", "deploy deploy things", 0.0),
        ];
        // Two-term query: b matches both (1.0), a matches one (0.5) — b first.
        let out = merge_hits(&[], &kw_hits, "deploy things", 0.0, 1.0, 0.0, 50);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].tool_name, "b");
        assert!((out[0].score - 1.0).abs() < 1e-4);
        assert!((out[1].score - 0.5).abs() < 1e-4);
    }

    #[test]
    fn merge_respects_limit() {
        let kw_hits: Vec<SmartHit> = (0..10)
            .map(|i| hit("s", &format!("t{}", i), "deploy", 0.0))
            .collect();
        let out = merge_hits(&[], &kw_hits, "deploy", 0.0, 1.0, 0.0, 3);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn merge_server_rows_participate() {
        // Server rows (empty tool_name) get scored like tools; the meta layer
        // decides what to do with them.
        let vec_hits = vec![hit("s1", "", "server s1 description", 0.1)];
        let out = merge_hits(&vec_hits, &[], "server", 1.0, 0.0, 0.0, 50);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content_type, "server");
        assert!((out[0].score - 0.9).abs() < 1e-4);
    }

    // ── hash tests ──────────────────────────────────────────────────────────

    fn tool(name: &str, schema: serde_json::Value) -> Tool {
        Tool {
            name: name.to_string(),
            description: Some("upstream desc".to_string()),
            input_schema: schema,
            server_name: "s".to_string(),
            enabled: true,
            annotations: None,
            output_schema: None,
        }
    }

    #[test]
    fn toolset_hash_stable_and_change_sensitive() {
        let a = tool("t1", serde_json::json!({"type":"object","properties":{"x":{"type":"string"}}}));
        let b = tool("t2", serde_json::json!({"type":"object"}));
        let h1 = super::super::index::build_toolset_hash(&[a.clone(), b.clone()]);
        let h2 = super::super::index::build_toolset_hash(&[b, a.clone()]); // order-insensitive
        assert_eq!(h1, h2, "tool order must not change the hash");
        // Different description (upstream churn) must NOT change the hash.
        let mut a2 = a.clone();
        a2.description = Some("totally different dynamic content".into());
        let h3 = super::super::index::build_toolset_hash(&[a2, tool("t2", serde_json::json!({"type":"object"}))]);
        assert_eq!(h1, h3, "upstream description churn must not invalidate the cache");
        // A schema change MUST change the hash.
        let h4 = super::super::index::build_toolset_hash(&[
            a,
            tool("t2", serde_json::json!({"type":"object","properties":{"y":{"type":"number"}}})),
        ]);
        assert_ne!(h1, h4);
        // Determinism across calls (scrypt is keyed + deterministic).
        assert_eq!(h3, super::super::index::build_toolset_hash(&[
            tool("t1", serde_json::json!({"type":"object","properties":{"x":{"type":"string"}}})),
            tool("t2", serde_json::json!({"type":"object"})),
        ]));
    }

    #[test]
    fn stable_hash_serialize_sorts_keys() {
        let v = serde_json::json!({"b": 1, "a": [2, {"z": 1, "y": 2}]});
        assert_eq!(
            super::super::index::stable_hash_serialize(&v),
            r#"{"a":[2,{"y":2,"z":1}],"b":1}"#
        );
    }

    #[test]
    fn tool_searchable_text_composition() {
        let t = tool(
            "get_user",
            serde_json::json!({
                "type": "object",
                "required": ["id"],
                "properties": {"id": {"type": "string"}, "name": {"type": "string"}}
            }),
        );
        t_desc_and_keys(&t);
    }

    fn t_desc_and_keys(t: &Tool) {
        let text = super::super::index::tool_searchable_text(t);
        assert!(text.contains("get_user"), "name must be included: {text}");
        assert!(text.contains("upstream desc"), "description must be included");
        assert!(text.contains("required"), "top-level schema keys except type/properties");
        assert!(!text.split(' ').any(|p| p == "object"), "'type' key value excluded");
        assert!(text.contains("id") && text.contains("name"), "property names included");
    }
}

#[cfg(test)]
mod edge_tests {
    use super::tests::*;
    use super::*;

    #[test]
    fn empty_query_yields_kw_zero_but_vec_only() {
        // No terms → kw score 0 for every hit; only vector signal survives
        // (above threshold).
        let vec_hits = vec![hit("s", "a", "text", 0.1)];
        let out = merge_hits(&vec_hits, &[], "", 0.5, 0.5, 0.3, 10);
        assert_eq!(out.len(), 1);
        assert!((out[0].score - 0.45).abs() < 1e-4, "0.5*0.9=0.45");
    }

    #[test]
    fn zero_weights_yield_zero_scores_dropped_by_threshold() {
        let vec_hits = vec![hit("s", "a", "text", 0.1)];
        let kw_hits = vec![hit("s", "a", "text", 0.0)];
        let out = merge_hits(&vec_hits, &kw_hits, "text", 0.0, 0.0, 0.01, 10);
        assert!(out.is_empty(), "0+0 < 0.01 threshold");
        // threshold 0 keeps everything.
        let out2 = merge_hits(&vec_hits, &kw_hits, "text", 0.0, 0.0, 0.0, 10);
        assert_eq!(out2.len(), 1);
    }

    #[test]
    fn limit_truncates_sorted_desc() {
        let vec_hits: Vec<SmartHit> = (0..10)
            .map(|i| hit("s", &format!("t{i}"), "text", 0.5 - i as f32 * 0.05))
            .collect();
        let out = merge_hits(&vec_hits, &[], "text", 1.0, 0.0, 0.0, 3);
        assert_eq!(out.len(), 3);
        assert!(out[0].score >= out[1].score && out[1].score >= out[2].score);
    }

    #[test]
    fn negative_distance_clamped_not_negative_score() {
        // distance slightly > 1 (bad upstream) → vec score clamps to 0, never
        // negative (would invert the sort).
        let vec_hits = vec![hit("s", "a", "text", 1.5)];
        let out = merge_hits(&vec_hits, &[], "x", 1.0, 0.0, 0.0, 10);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].score, 0.0);
    }

    // Re-export helper visibility shim: tests module items are accessible via
    // super::tests::* only if pub; keep a local duplicate instead.
    fn hit(server: &str, tool: &str, text: &str, distance: f32) -> SmartHit {
        SmartHit {
            content_type: if tool.is_empty() { "server" } else { "tool" }.to_string(),
            content_id: format!("{}:{}", server, tool),
            server_name: server.to_string(),
            tool_name: tool.to_string(),
            text_content: text.to_string(),
            metadata: None,
            distance,
        }
    }
    use hit as _hit_unused; // silence unused warning when tests::hit is importable
}
