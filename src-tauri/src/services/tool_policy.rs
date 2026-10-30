//! Unified MCP tool access policy (origin #1399 parity).
//!
//! Rules live in `system_config.toolPolicies` (JSON, deep-merged by
//! `config_service` — no DB migration). Every tool access face evaluates the
//! same rule set: deny rules win, allow rules form a union, and when any
//! allow rule applies to the subject, targets without a matching allow are
//! denied (`no-matching-allow`).
//!
//! Desktop deviations from origin (documented):
//! - `username` selector: kept for config round-trip parity, but the desktop
//!   subject username is always `None` (single user, no UserContextService) —
//!   a rule that sets `username` never applies (origin semantics: a set
//!   selector must equal the subject value).
//! - Config read failure (`get_strict` Err) or invalid stored rules: origin
//!   throws out of `getToolPolicy` (failClosed DAO read); the desktop returns
//!   a fail-closed policy whose `authorize` always denies, with a loud warn.
//! - Activity log: the desktop `activity_log` table has no `policy` column
//!   (no new migrations) — the decision is recorded in `error_message` as a
//!   compact JSON audit object.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// One access rule. Unset selectors match everything; set selectors must
/// equal the subject/target value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolPolicyRule {
    pub id: String,
    /// "allow" | "deny"
    pub effect: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

/// Result of a policy evaluation (origin `PolicyDecision`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PolicyDecision {
    /// "allow" | "deny"
    pub decision: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
}

/// Who is calling. Desktop: `username` is always `None` (single user).
#[derive(Debug, Clone, Copy, Default)]
pub struct SubjectRef<'a> {
    pub username: Option<&'a str>,
    pub key_id: Option<&'a str>,
}

/// What is being accessed. `tool: None` + `potential=true` models the
/// "could any tool on this target be allowed" listing probe.
#[derive(Debug, Clone, Copy, Default)]
pub struct TargetRef<'a> {
    pub group_id: Option<&'a str>,
    pub server_id: Option<&'a str>,
    pub tool: Option<&'a str>,
}

const INVALID_RULE_MSG: &str =
    "Invalid toolPolicies rule: use unique IDs, allow/deny and non-empty selectors";

const SELECTOR_KEYS: &[&str] = &["username", "keyId", "groupId", "serverId", "tool"];
const RULE_KEYS: &[&str] = &[
    "id",
    "effect",
    "username",
    "keyId",
    "groupId",
    "serverId",
    "tool",
];

/// Origin-verbatim validation over the raw JSON (`validateToolPolicies`):
/// array of objects with unique non-empty `id`, `effect` ∈ allow/deny, no
/// unknown keys, and every present selector a non-empty string (present-but-
/// null selectors are rejected, matching JS `!== undefined` semantics).
/// Returns the parsed rules on success.
pub fn validate_tool_policies(value: &Value) -> Result<Vec<ToolPolicyRule>, String> {
    let arr = value
        .as_array()
        .ok_or_else(|| "toolPolicies must be an array".to_string())?;
    let mut ids = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(arr.len());
    for rule in arr {
        let obj = rule
            .as_object()
            .ok_or_else(|| INVALID_RULE_MSG.to_string())?;
        let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if id.trim().is_empty() || !ids.insert(id.to_string()) {
            return Err(INVALID_RULE_MSG.to_string());
        }
        let effect = obj.get("effect").and_then(|v| v.as_str()).unwrap_or("");
        if effect != "allow" && effect != "deny" {
            return Err(INVALID_RULE_MSG.to_string());
        }
        if obj.keys().any(|k| !RULE_KEYS.contains(&k.as_str())) {
            return Err(INVALID_RULE_MSG.to_string());
        }
        for key in SELECTOR_KEYS {
            match obj.get(*key) {
                None => {}
                Some(Value::String(s)) if !s.trim().is_empty() => {}
                _ => return Err(INVALID_RULE_MSG.to_string()),
            }
        }
        out.push(serde_json::from_value(rule.clone()).map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Origin `evaluateToolPolicy`, verbatim semantics:
/// - subject-inapplicable rules are skipped before anything else;
/// - the first matching deny wins (`matching-deny`); in `potential` mode a
///   deny rule with a `tool` selector never fires;
/// - allow rules form a union — any applicable allow but no matching one is
///   `no-matching-allow` (deny);
/// - no applicable allow rules at all → allow (`no-applicable-allow`).
pub fn evaluate_tool_policy(
    rules: &[ToolPolicyRule],
    subject: SubjectRef<'_>,
    target: TargetRef<'_>,
    potential: bool,
) -> PolicyDecision {
    let applicable: Vec<&ToolPolicyRule> = rules
        .iter()
        .filter(|rule| {
            rule.username
                .as_deref()
                .map_or(true, |u| Some(u) == subject.username)
                && rule
                    .key_id
                    .as_deref()
                    .map_or(true, |k| Some(k) == subject.key_id)
        })
        .collect();
    let matches = |rule: &ToolPolicyRule| {
        rule.group_id
            .as_deref()
            .map_or(true, |g| Some(g) == target.group_id)
            && rule
                .server_id
                .as_deref()
                .map_or(true, |s| Some(s) == target.server_id)
            && rule.tool.as_deref().map_or(true, |t| {
                (potential && target.tool.is_none()) || Some(t) == target.tool
            })
    };
    if let Some(deny) = applicable.iter().find(|r| {
        r.effect == "deny" && matches(r) && !(potential && r.tool.is_some())
    }) {
        return PolicyDecision {
            decision: "deny".to_string(),
            reason: "matching-deny".to_string(),
            policy_id: Some(deny.id.clone()),
        };
    }
    let allows: Vec<&&ToolPolicyRule> =
        applicable.iter().filter(|r| r.effect == "allow").collect();
    let allow = allows.iter().find(|r| matches(r));
    if !allows.is_empty() && allow.is_none() {
        return PolicyDecision {
            decision: "deny".to_string(),
            reason: "no-matching-allow".to_string(),
            policy_id: None,
        };
    }
    PolicyDecision {
        decision: "allow".to_string(),
        reason: if allow.is_some() {
            "matching-allow"
        } else {
            "no-applicable-allow"
        }
        .to_string(),
        policy_id: allow.map(|r| r.id.clone()),
    }
}

/// A reference from a rule to an entity that no longer exists (origin
/// `findMissingToolPolicyReferences`).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MissingPolicyReference {
    pub policy_id: String,
    /// "serverId" | "keyId" | "groupId"
    pub selector: String,
    pub value: String,
}

pub fn find_missing_tool_policy_references(
    rules: &[ToolPolicyRule],
    server_ids: &[String],
    key_ids: &[String],
    group_ids: &[String],
) -> Vec<MissingPolicyReference> {
    let mut out = Vec::new();
    for rule in rules {
        for (selector, value, known) in [
            ("serverId", &rule.server_id, server_ids),
            ("keyId", &rule.key_id, key_ids),
            ("groupId", &rule.group_id, group_ids),
        ] {
            if let Some(v) = value {
                if !known.contains(v) {
                    out.push(MissingPolicyReference {
                        policy_id: rule.id.clone(),
                        selector: selector.to_string(),
                        value: v.clone(),
                    });
                }
            }
        }
    }
    out
}

/// Reason recorded when the policy store itself is unreadable — every
/// authorize denies (fail-closed).
pub const FAIL_REASON: &str = "policy-config-unavailable";

/// Resolved policy for one request: rules + subject + pre-resolved group and
/// server-id map. Cheap to clone-free reuse across a single call's faces.
pub struct ToolPolicy {
    active: bool,
    /// Config read/parse failure: `authorize` always denies.
    failed: bool,
    rules: Vec<ToolPolicyRule>,
    subject_key_id: Option<String>,
    group_id: Option<String>,
    /// server name → id (origin resolves ids per authorize; the desktop
    /// snapshots the map once per request).
    server_ids: std::collections::HashMap<String, String>,
}

impl ToolPolicy {
    /// True when at least one rule applies to this subject — callers gate
    /// their filtering work on this (origin `policy.active`).
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn group_id(&self) -> Option<&str> {
        self.group_id.as_deref()
    }

    pub fn server_id(&self, name: &str) -> Option<String> {
        self.server_ids.get(name).cloned()
    }

    pub fn authorize(&self, server_name: &str, tool: Option<&str>) -> PolicyDecision {
        self.authorize_potential(server_name, tool, false)
    }

    /// `potential=true` models the listing probe: "is anything on this server
    /// callable" (origin `authorize(name, undefined, true)`).
    pub fn authorize_potential(
        &self,
        server_name: &str,
        tool: Option<&str>,
        potential: bool,
    ) -> PolicyDecision {
        if self.failed {
            return PolicyDecision {
                decision: "deny".to_string(),
                reason: FAIL_REASON.to_string(),
                policy_id: None,
            };
        }
        evaluate_tool_policy(
            &self.rules,
            SubjectRef {
                username: None,
                key_id: self.subject_key_id.as_deref(),
            },
            TargetRef {
                group_id: self.group_id.as_deref(),
                server_id: self.server_id(server_name).as_deref(),
                tool,
            },
            potential,
        )
    }

    /// Origin `filterTools`: keep only allowed tools. `{server}{sep}`-prefixed
    /// display names are stripped before evaluation so the helper is safe for
    /// both bare (pool) and prefixed (exposed) name lists.
    pub fn filter_tools(
        &self,
        server_name: &str,
        tools: Vec<crate::models::server::Tool>,
        sep: &str,
    ) -> Vec<crate::models::server::Tool> {
        let prefix = format!("{server_name}{sep}");
        tools
            .into_iter()
            .filter(|t| {
                let bare = t.name.strip_prefix(&prefix).unwrap_or(&t.name);
                self.authorize(server_name, Some(bare)).decision == "allow"
            })
            .collect()
    }

    fn failed_policy() -> Self {
        Self {
            active: true,
            failed: true,
            rules: Vec::new(),
            subject_key_id: None,
            group_id: None,
            server_ids: Default::default(),
        }
    }
}

/// Origin route→group mapping: `$smart` itself has no group; `$smart/{group}`
/// and bare group/server routes yield the segment; empty → None.
fn policy_route_group(route: Option<&str>) -> Option<String> {
    match route {
        None => None,
        Some("$smart") => None,
        Some(r) => {
            let seg = r.strip_prefix("$smart/").unwrap_or(r);
            if seg.is_empty() {
                None
            } else {
                Some(seg.to_string())
            }
        }
    }
}

/// Build the request-scoped policy (origin `getToolPolicy`). `route` is the
/// MCP scope segment (`""` root / `$smart` / `$smart/{group}` / group or
/// server name); `key_id` is the caller's bearer-key id (None on faces
/// without a key context — key-scoped rules then never apply, matching
/// origin's undefined-keyId subject).
pub async fn get_tool_policy(route: Option<&str>, key_id: Option<&str>) -> ToolPolicy {
    let configured = match crate::services::config_service::get_strict().await {
        Ok(v) => v,
        Err(e) => {
            log::warn!(
                "[tool-policy] system config read failed — denying tool access (fail-closed): {e:#}"
            );
            return ToolPolicy::failed_policy();
        }
    };
    let rules = match configured.get("toolPolicies") {
        None => Vec::new(),
        Some(v) => match validate_tool_policies(v) {
            Ok(r) => r,
            Err(e) => {
                log::warn!(
                    "[tool-policy] stored toolPolicies invalid — denying tool access (fail-closed): {e}"
                );
                return ToolPolicy::failed_policy();
            }
        },
    };
    let subject_key_id = key_id.map(str::to_string);
    if rules.is_empty() {
        return ToolPolicy {
            active: false,
            failed: false,
            rules,
            subject_key_id,
            group_id: None,
            server_ids: Default::default(),
        };
    }
    let active = rules.iter().any(|rule| {
        // Desktop subject username is always None: a rule that sets
        // `username` never applies (origin set-selector equality semantics).
        rule.username.is_none()
            && rule
                .key_id
                .as_deref()
                .map_or(true, |k| Some(k) == subject_key_id.as_deref())
    });
    let group_id = if active {
        match policy_route_group(route) {
            Some(name) => {
                // find_by_name_or_id covers origin's findByName ?? uuid-findById
                // (ids are uuids, so the OR-id arm only ever matches uuid input).
                crate::services::group_service::find_by_name_or_id(&name)
                    .await
                    .ok()
                    .flatten()
                    .map(|g| g.id)
            }
            None => None,
        }
    } else {
        None
    };
    let server_ids: std::collections::HashMap<String, String> = if active {
        crate::services::server_service::list_all()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|s| !s.id.is_empty())
            .map(|s| (s.name, s.id))
            .collect()
    } else {
        Default::default()
    };
    ToolPolicy {
        active,
        failed: false,
        rules,
        subject_key_id,
        group_id,
        server_ids,
    }
}

/// Compact audit JSON for the activity log's `error_message` field (the
/// desktop activity_log has no dedicated policy column).
pub fn audit_text(
    decision: &PolicyDecision,
    server_id: Option<String>,
    group_id: Option<&str>,
) -> String {
    serde_json::to_string(&json!({
        "policy": {
            "decision": decision.decision,
            "reason": decision.reason,
            "policyId": decision.policy_id,
            "serverId": server_id,
            "groupId": group_id,
        }
    }))
    .unwrap_or_else(|_| decision.reason.clone())
}

/// Startup diagnostics (origin `toolPolicyDiagnostics`): warn for every rule
/// reference to a missing server/key/group id. Never prevents startup.
pub async fn warn_missing_tool_policy_references() {
    match collect_missing_references().await {
        Ok(refs) => {
            for r in refs {
                log::warn!(
                    "[tool-policy] rule '{}' references missing {} '{}'; review this rule before relying on it",
                    r.policy_id,
                    r.selector,
                    r.value
                );
            }
        }
        Err(e) => log::warn!("[tool-policy] could not check tool policy references: {e:#}"),
    }
}

async fn collect_missing_references() -> anyhow::Result<Vec<MissingPolicyReference>> {
    let configured = crate::services::config_service::get_strict().await?;
    let rules = match configured.get("toolPolicies") {
        None => Vec::new(),
        Some(v) => validate_tool_policies(v).map_err(anyhow::Error::msg)?,
    };
    if rules.is_empty() {
        return Ok(Vec::new());
    }
    let (servers, keys, groups) = tokio::join!(
        crate::services::server_service::list_all(),
        crate::services::bearer_key_service::list_all(),
        crate::services::group_service::list_all(),
    );
    let server_ids: Vec<String> = servers?
        .into_iter()
        .filter(|s| !s.id.is_empty())
        .map(|s| s.id)
        .collect();
    let key_ids: Vec<String> = keys?.into_iter().map(|k| k.id).collect();
    let group_ids: Vec<String> = groups?.into_iter().map(|g| g.id).collect();
    Ok(find_missing_tool_policy_references(
        &rules,
        &server_ids,
        &key_ids,
        &group_ids,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rules(v: Value) -> Vec<ToolPolicyRule> {
        validate_tool_policies(&v).expect("valid rules")
    }

    fn eval(
        rules: &[ToolPolicyRule],
        key_id: Option<&str>,
        group_id: Option<&str>,
        server_id: Option<&str>,
        tool: Option<&str>,
    ) -> PolicyDecision {
        evaluate_tool_policy(
            rules,
            SubjectRef {
                username: None,
                key_id,
            },
            TargetRef {
                group_id,
                server_id,
                tool,
            },
            false,
        )
    }

    fn eval_potential(
        rules: &[ToolPolicyRule],
        key_id: Option<&str>,
        server_id: Option<&str>,
        tool: Option<&str>,
    ) -> PolicyDecision {
        evaluate_tool_policy(
            rules,
            SubjectRef {
                username: None,
                key_id,
            },
            TargetRef {
                group_id: None,
                server_id,
                tool,
            },
            true,
        )
    }

    // ── evaluate truth table (origin toolPolicyService.test.ts) ──────────

    #[test]
    fn scopes_the_allow_list_by_subject_before_matching_the_target() {
        let allow = rules(json!([{
            "id": "read", "effect": "allow", "keyId": "agent",
            "serverId": "a", "tool": "read"
        }]));
        assert_eq!(
            eval(&allow, Some("agent"), None, Some("a"), Some("read")).decision,
            "allow"
        );
        for (server, tool) in [("b", "read"), ("a", "new_tool")] {
            let d = eval(&allow, Some("agent"), None, Some(server), Some(tool));
            assert_eq!(d.decision, "deny");
            assert_eq!(d.reason, "no-matching-allow");
        }
        // Subject without the key: rule not applicable → allow.
        assert_eq!(
            eval(&allow, None, None, Some("b"), None).decision,
            "allow"
        );
    }

    #[test]
    fn deny_wins_and_allows_form_a_union() {
        let mut allow_a = json!({
            "id": "read", "effect": "allow", "keyId": "agent",
            "serverId": "a", "tool": "read"
        });
        allow_a["serverId"] = json!("a");
        let mut allow_b = allow_a.clone();
        allow_b["id"] = json!("other");
        allow_b["serverId"] = json!("b");
        let deny = json!({
            "id": "deny", "effect": "deny", "keyId": "agent", "tool": "read"
        });
        let all = rules(json!([allow_a, allow_b, deny]));
        // Deny wins over the matching allow on server b.
        let d = eval(&all, Some("agent"), None, Some("b"), Some("read"));
        assert_eq!(d.decision, "deny");
        assert_eq!(d.policy_id.as_deref(), Some("deny"));
        // Multiple allows form a union.
        let two = rules(json!([allow_a.clone(), allow_b.clone()]));
        assert_eq!(
            eval(&two, Some("agent"), None, Some("b"), Some("read")).decision,
            "allow"
        );
        // A username-scoped rule never applies on the desktop (subject
        // username is always None).
        let username_deny = rules(json!([{"id": "u", "effect": "deny", "username": "admin"}]));
        assert_eq!(
            eval(&username_deny, Some("agent"), None, Some("a"), Some("read")).decision,
            "allow"
        );
    }

    #[test]
    fn global_subject_rules_apply_and_group_only_grants_cannot_cross_routes() {
        let group_rule = rules(json!([{ "id": "group", "effect": "allow", "groupId": "g" }]));
        assert_eq!(
            eval(&group_rule, Some("k"), Some("g"), Some("a"), None).decision,
            "allow"
        );
        let d = eval(&group_rule, Some("k"), None, Some("a"), None);
        assert_eq!(d.decision, "deny");
        assert_eq!(d.reason, "no-matching-allow");
        // A deny scoped to another server does not affect this target.
        let deny_other = rules(json!([{ "id": "deny", "effect": "deny", "serverId": "b" }]));
        assert_eq!(
            eval(&deny_other, Some("k"), None, Some("a"), None).decision,
            "allow"
        );
    }

    #[test]
    fn potential_mode_ignores_tool_scoped_denies_and_matches_tool_scoped_allows() {
        // Deny with a tool selector must not fire on the potential probe.
        let deny_tool = rules(json!([{ "id": "d", "effect": "deny", "serverId": "a", "tool": "x" }]));
        assert_eq!(
            eval_potential(&deny_tool, Some("k"), Some("a"), None).decision,
            "allow"
        );
        // …but fires on the concrete call.
        assert_eq!(
            eval(&deny_tool, Some("k"), None, Some("a"), Some("x")).decision,
            "deny"
        );
        // Allow with a tool selector matches the potential probe.
        let allow_tool = rules(json!([{ "id": "a", "effect": "allow", "serverId": "a", "tool": "x" }]));
        assert_eq!(
            eval_potential(&allow_tool, Some("k"), Some("a"), None).decision,
            "allow"
        );
        // Server-level deny still fires in potential mode.
        let deny_server = rules(json!([{ "id": "d", "effect": "deny", "serverId": "a" }]));
        assert_eq!(
            eval_potential(&deny_server, Some("k"), Some("a"), None).decision,
            "deny"
        );
    }

    // ── validate (origin malformed-form table) ───────────────────────────

    #[test]
    fn rejects_malformed_rules() {
        for v in [
            json!(null),
            json!({}),
            json!([{ "id": "x", "effect": "typo" }]),
            json!([{ "id": "x", "effect": "allow", "tool": "" }]),
            json!([{ "id": "x", "effect": "allow", "server": "typo" }]),
            json!([{ "id": "x", "effect": "allow", "tool": null }]),
            json!([
                { "id": "x", "effect": "allow" },
                { "id": "x", "effect": "deny" }
            ]),
            json!([{ "effect": "allow" }]),
            json!([{ "id": "  ", "effect": "allow" }]),
        ] {
            assert!(
                validate_tool_policies(&v).is_err(),
                "must reject: {v}"
            );
        }
    }

    #[test]
    fn accepts_valid_rules_and_preserves_selectors() {
        let parsed = rules(json!([
            { "id": "r1", "effect": "deny", "keyId": "k1", "serverId": "s1", "groupId": "g1", "tool": "t" },
            { "id": "r2", "effect": "allow" },
        ]));
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].key_id.as_deref(), Some("k1"));
        assert_eq!(parsed[0].server_id.as_deref(), Some("s1"));
        assert_eq!(parsed[0].group_id.as_deref(), Some("g1"));
        assert_eq!(parsed[0].tool.as_deref(), Some("t"));
        assert!(parsed[1].key_id.is_none());
    }

    // ── find_missing_tool_policy_references ──────────────────────────────

    #[test]
    fn reports_missing_references_in_selector_order() {
        let parsed = rules(json!([
            { "id": "valid", "effect": "deny", "serverId": "server", "keyId": "key", "groupId": "group" },
            { "id": "stale", "effect": "deny", "serverId": "deleted", "keyId": "key", "groupId": "gone" },
            { "id": "future-key", "effect": "allow", "keyId": "future" },
        ]));
        let missing = find_missing_tool_policy_references(
            &parsed,
            &["server".to_string()],
            &["key".to_string()],
            &["group".to_string()],
        );
        assert_eq!(
            missing,
            vec![
                MissingPolicyReference {
                    policy_id: "stale".into(),
                    selector: "serverId".into(),
                    value: "deleted".into()
                },
                MissingPolicyReference {
                    policy_id: "stale".into(),
                    selector: "groupId".into(),
                    value: "gone".into()
                },
                MissingPolicyReference {
                    policy_id: "future-key".into(),
                    selector: "keyId".into(),
                    value: "future".into()
                },
            ]
        );
    }

    #[test]
    fn no_rules_means_no_references() {
        assert!(find_missing_tool_policy_references(
            &[],
            &[],
            &[],
            &[]
        )
        .is_empty());
    }

    // ── route → group mapping ────────────────────────────────────────────

    #[test]
    fn policy_route_group_mapping() {
        assert_eq!(policy_route_group(None), None);
        assert_eq!(policy_route_group(Some("")), None);
        assert_eq!(policy_route_group(Some("$smart")), None);
        assert_eq!(policy_route_group(Some("$smart/GroupA")), Some("GroupA".into()));
        assert_eq!(policy_route_group(Some("GroupA")), Some("GroupA".into()));
        assert_eq!(policy_route_group(Some("$smart/")), None);
    }

    // ── get_tool_policy against the shared test pool ─────────────────────

    async fn snapshot_config() -> Option<String> {
        sqlx::query_scalar("SELECT config_json FROM system_config WHERE id=1")
            .fetch_one(crate::db::pool())
            .await
            .expect("snapshot config")
    }

    async fn write_config_json(v: Option<&str>) {
        match v {
            Some(s) => {
                sqlx::query("UPDATE system_config SET config_json=? WHERE id=1")
                    .bind(s)
                    .execute(crate::db::pool())
                    .await
                    .expect("write config_json");
            }
            None => {
                sqlx::query("UPDATE system_config SET config_json=NULL WHERE id=1")
                    .execute(crate::db::pool())
                    .await
                    .expect("clear config_json");
            }
        }
    }

    #[tokio::test]
    async fn get_tool_policy_inactive_without_rules() {
        let _g = crate::services::config_service::CONFIG_TEST_LOCK.lock().unwrap();
        crate::db::ensure_test_pool().await;
        let snap = snapshot_config().await;
        write_config_json(Some(r#"{"routing":{}}"#)).await;
        let p = get_tool_policy(None, Some("agent")).await;
        assert!(!p.is_active());
        assert_eq!(
            p.authorize("any", Some("tool")).decision,
            "allow"
        );
        write_config_json(snap.as_deref()).await;
    }

    #[tokio::test]
    async fn get_tool_policy_global_deny_rule_is_active_for_every_subject() {
        let _g = crate::services::config_service::CONFIG_TEST_LOCK.lock().unwrap();
        crate::db::ensure_test_pool().await;
        let snap = snapshot_config().await;
        write_config_json(Some(
            r#"{"toolPolicies":[{"id":"d","effect":"deny"}]}"#,
        ))
        .await;
        let p = get_tool_policy(None, None).await;
        assert!(p.is_active());
        let d = p.authorize("srv-x", Some("tool"));
        assert_eq!(d.decision, "deny");
        assert_eq!(d.reason, "matching-deny");
        assert_eq!(d.policy_id.as_deref(), Some("d"));
        write_config_json(snap.as_deref()).await;
    }

    #[tokio::test]
    async fn get_tool_policy_key_scoped_rule_ignores_keyless_subject() {
        let _g = crate::services::config_service::CONFIG_TEST_LOCK.lock().unwrap();
        crate::db::ensure_test_pool().await;
        let snap = snapshot_config().await;
        write_config_json(Some(
            r#"{"toolPolicies":[{"id":"k","effect":"deny","keyId":"agent"}]}"#,
        ))
        .await;
        // No key context (Tauri face): rule not applicable → inactive.
        let p = get_tool_policy(None, None).await;
        assert!(!p.is_active());
        assert_eq!(p.authorize("srv", Some("t")).decision, "allow");
        // Matching key: active and denying.
        let p2 = get_tool_policy(None, Some("agent")).await;
        assert!(p2.is_active());
        assert_eq!(p2.authorize("srv", Some("t")).decision, "deny");
        write_config_json(snap.as_deref()).await;
    }

    #[tokio::test]
    async fn get_tool_policy_corrupt_config_fails_closed() {
        let _g = crate::services::config_service::CONFIG_TEST_LOCK.lock().unwrap();
        crate::db::ensure_test_pool().await;
        let snap = snapshot_config().await;
        write_config_json(Some("{not json")).await;
        let p = get_tool_policy(None, None).await;
        assert!(p.is_active(), "failed read must engage the policy gate");
        let d = p.authorize("srv", Some("t"));
        assert_eq!(d.decision, "deny");
        assert_eq!(d.reason, FAIL_REASON);
        write_config_json(snap.as_deref()).await;
    }

    #[tokio::test]
    async fn get_tool_policy_invalid_stored_rules_fail_closed() {
        let _g = crate::services::config_service::CONFIG_TEST_LOCK.lock().unwrap();
        crate::db::ensure_test_pool().await;
        let snap = snapshot_config().await;
        write_config_json(Some(r#"{"toolPolicies":[{"id":"x","effect":"typo"}]}"#)).await;
        let p = get_tool_policy(None, None).await;
        assert!(p.is_active());
        assert_eq!(p.authorize("srv", Some("t")).reason, FAIL_REASON);
        write_config_json(snap.as_deref()).await;
    }

    #[test]
    fn filter_tools_strips_display_prefix_before_evaluation() {
        let parsed = rules(json!([
            { "id": "a", "effect": "allow", "serverId": "srv-id", "tool": "ok" },
        ]));
        let policy = ToolPolicy {
            active: true,
            failed: false,
            rules: parsed,
            subject_key_id: None,
            group_id: None,
            server_ids: [("srv".to_string(), "srv-id".to_string())].into_iter().collect(),
        };
        let mk = |name: &str| crate::models::server::Tool {
            name: name.to_string(),
            description: None,
            input_schema: json!({}),
            server_name: "srv".to_string(),
            enabled: true,
            annotations: None,
            output_schema: None,
            title: None,
            execution: None,
            icons: None,
            meta: None,
            hidden_params: None,
            description_overridden: false,
        };
        let kept = policy.filter_tools("srv", vec![mk("ok"), mk("no")], "-");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "ok");
        // Prefixed display name is stripped before evaluation.
        let kept2 = policy.filter_tools("srv", vec![mk("srv-ok")], "-");
        assert_eq!(kept2.len(), 1);
    }
}
