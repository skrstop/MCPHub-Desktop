//! Smart Routing meta tools (`smart_route_search` / `smart_route_describe` / `smart_route_call`)
//! served on the `$smart` and `$smart/{group}` MCP scopes. Description text is
//! origin-verbatim (`smartRoutingService.ts`) so LLM clients see the same
//! workflow prompts; the search itself is the desktop hybrid retrieval.

use serde_json::{json, Value};

use super::index;
use super::models;
use super::search;

/// True for `$smart` and `$smart/{group}` scopes.
pub fn is_smart_scope(scope_clean: &str) -> bool {
    scope_clean == "$smart" || scope_clean.starts_with("$smart/")
}

fn target_group(scope_clean: &str) -> Option<String> {
    scope_clean
        .strip_prefix("$smart/")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Gate for every `$smart` access point: when Smart Routing is disabled (or
/// the shared model runtime is down), return a bilingual "how to enable"
/// message instead of a bare protocol error. The MCP layer wraps it in a
/// JSON-RPC error; the REST layer returns it as an HTTP 403 JSON body.
pub async fn not_ready_message() -> Option<String> {
    let settings = models::get_settings().await;
    if !settings.enabled {
        return Some(
            "Smart Routing 未开启（Smart Routing is not enabled）。\
             请在 设置 → Smart Routing（智能路由）打开开关后重试 / \
             Enable it in Settings → Smart Routing, then retry."
                .to_string(),
        );
    }
    if !crate::mv::is_running() {
        return Some(
            "Smart Routing 已开启但本地模型未运行（model runtime not running）。\
             请在 设置 → 模型和向量 查看模型状态，或重启应用后重试 / \
             Check the model status in Settings → Model & Vector, or restart the app."
                .to_string(),
        );
    }
    None
}

/// Compute the scope: allowed servers (group filter applied when scoped) +
/// the origin-formatted description/servers list for the meta tool prompts.
pub async fn compute_scope(scope_clean: &str) -> (String, String, Option<Vec<String>>) {
    compute_scope_with(scope_clean, None).await
}

/// Like `compute_scope`, but with an optional bearer-key server allow-list
/// applied BEFORE the servers list is formatted — the meta tool descriptions
/// must not leak server names the bearer key cannot access.
pub async fn compute_scope_with(
    scope_clean: &str,
    bearer_allowed: Option<Vec<String>>,
) -> (String, String, Option<Vec<String>>) {
    let settings = models::get_settings().await;
    let mode = settings.progressive_disclosure; // placeholder, unused below
    let _ = mode;
    let description_mode = crate::services::config_service::get()
        .await
        .ok()
        .and_then(|c| {
            c.get("smartRouting")
                .and_then(|r| r.get("serverDescriptionMode"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "names".to_string());

    let target = target_group(scope_clean);
    let mut allowed: Vec<String> = crate::mcp::pool::get_all_statuses()
        .await
        .into_iter()
        .filter(|s| s.connected)
        .map(|s| s.name)
        .collect();
    // The builtin "mcphub-desktop" server is virtual (no pool entry) — add it
    // when its tools are live (RAG enabled). Without this, a $smart/{builtin}
    // scope retains against a list that doesn't contain the name → empty
    // allowed → zero search results, and global searches would never surface
    // its rag_* tools to resolve_tool either.
    if crate::rag::service::is_enabled() {
        allowed.push(crate::rag::service::BUILTIN_SERVER_NAME.to_string());
    }
    if let Some(g) = &target {
        // Group scope: retain only the group's members (group servers are
        // `{ "name": ... }` entries, same as http_server's extract_server_names).
        let mut matched = false;
        if let Ok(groups) = crate::services::group_service::list_all().await {
            if let Some(grp) = groups.iter().find(|gr| &gr.name == g || &gr.id == g) {
                allowed.retain(|s| {
                    grp.servers.iter().any(|sv| {
                        sv.get("name").and_then(|n| n.as_str()) == Some(s.as_str())
                    })
                });
                matched = true;
            }
        }
        // Single-server scope ($smart/{serverName}): retain just that server.
        // NOT matching a group falls through here — otherwise the allowed set
        // would stay "all connected servers" and the single-server scope would
        // silently degrade to a global search (observed with $smart/mcphub-desktop).
        if !matched {
            allowed.retain(|s| s == g);
        }
    }
    if let Some(keys_allowed) = &bearer_allowed {
        allowed.retain(|s| keys_allowed.contains(s));
    }
    // Bearer access control is applied by the caller (http_server) on the
    // returned list where relevant; here we keep the scope-level view.

    let servers_list = format_servers_list(&allowed, description_mode == "full").await;

    let scope_description = match &target {
        Some(g) => format!("servers in the \"{g}\" group"),
        None => "all available servers".to_string(),
    };
    (scope_description, servers_list, Some(allowed))
}

/// Format the "Available servers" list for the meta tool descriptions.
/// `full` mode = one `- name: description` line per server; otherwise a
/// comma-separated names list. Shared by compute_scope and the bearer-aware
/// tools/list rewrite in http_server.
pub async fn format_servers_list(allowed: &[String], full: bool) -> String {
    if !full {
        return allowed.join(", ");
    }
    let mut lines: Vec<String> = Vec::new();
    for name in allowed {
        let desc = crate::services::server_service::get_by_name(name)
            .await
            .ok()
            .flatten()
            .and_then(|c| c.description)
            .unwrap_or_default();
        let desc = desc.trim().split_whitespace().collect::<Vec<_>>().join(" ");
        if desc.is_empty() {
            lines.push(format!("- {name}"));
        } else {
            lines.push(format!("- {name}: {desc}"));
        }
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("\n{}", lines.join("\n"))
    }
}

/// Build the meta tool definitions (origin `buildSmartRoutingMetaTools`).
pub fn build_meta_tools(scope_description: &str, servers_list: &str, progressive: bool) -> Vec<Value> {
    if progressive {
        vec![
            json!({
                "name": "smart_route_search",
                "description": format!("STEP 1 of 3: Use this tool FIRST to discover and search for relevant tools across {scope_description}. Returns tool names and descriptions only - use smart_route_describe to get full parameter details before calling.\n\nFor optimal results, use specific queries matching your exact needs. Call this tool multiple times with different queries for different parts of complex tasks. Example queries: \"image generation tools\", \"code review tools\", \"data analysis\", \"translation capabilities\", etc. Results are sorted by relevance using vector similarity.\n\nWorkflow: smart_route_search → smart_route_describe (for parameter details) → smart_route_call (to execute)\n\nAvailable servers: {servers_list}"),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "The search query to find relevant tools. Be specific and descriptive about the task you want to accomplish."},
                        "limit": {"type": "integer", "description": "Maximum number of results to return. Use higher values (20-30) for broad searches and lower values (5-10) for specific searches.", "default": 10}
                    },
                    "required": ["query"]
                },
                "annotations": {"title": "Search Tools", "readOnlyHint": true}
            }),
            json!({
                "name": "smart_route_describe",
                "description": "STEP 2 of 3: Use this tool AFTER smart_route_search to get the full parameter schema for a specific tool. This provides the complete inputSchema needed to correctly invoke the tool with smart_route_call.\n\nWorkflow: smart_route_search → smart_route_describe → smart_route_call",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "toolName": {"type": "string", "description": "The exact name of the tool to describe (from smart_route_search results)"}
                    },
                    "required": ["toolName"]
                },
                "annotations": {"title": "Describe Tool", "readOnlyHint": true}
            }),
            json!({
                "name": "smart_route_call",
                "description": "STEP 3 of 3: Use this tool AFTER smart_route_describe to actually execute/invoke any tool you found. This is the execution step.\n\nWorkflow: smart_route_search → smart_route_describe → smart_route_call with the chosen tool name and required arguments.\n\nIMPORTANT: Always use smart_route_describe first to get the tool's inputSchema before invoking to ensure you provide the correct arguments.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "toolName": {"type": "string", "description": "The exact name of the tool to invoke (from smart_route_search results)"},
                        "arguments": {"type": "object", "additionalProperties": true, "description": "The arguments to pass to the tool based on its inputSchema from smart_route_describe (optional if tool requires no arguments)"}
                    },
                    "required": ["toolName"]
                },
                "annotations": {"title": "Call Tool", "openWorldHint": true}
            }),
        ]
    } else {
        vec![
            json!({
                "name": "smart_route_search",
                "description": format!("STEP 1 of 2: Use this tool FIRST to discover and search for relevant tools across {scope_description}. This tool and smart_route_call work together as a two-step process: 1) smart_route_search to find what you need, 2) smart_route_call to execute it.\n\nFor optimal results, use specific queries matching your exact needs. Call this tool multiple times with different queries for different parts of complex tasks. Example queries: \"image generation tools\", \"code review tools\", \"data analysis\", \"translation capabilities\", etc. Results are sorted by relevance using vector similarity.\n\nAfter finding relevant tools, you MUST use the smart_route_call to actually execute them. The smart_route_search only finds tools - it doesn't execute them.\n\nAvailable servers: {servers_list}"),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "The search query to find relevant tools. Be specific and descriptive about the task you want to accomplish."},
                        "limit": {"type": "integer", "description": "Maximum number of results to return. Use higher values (20-30) for broad searches and lower values (5-10) for specific searches.", "default": 10}
                    },
                    "required": ["query"]
                },
                "annotations": {"title": "Search Tools", "readOnlyHint": true}
            }),
            json!({
                "name": "smart_route_call",
                "description": "STEP 2 of 2: Use this tool AFTER smart_route_search to actually execute/invoke any tool you found. This is the execution step - smart_route_search finds tools, smart_route_call runs them.\n\nWorkflow: smart_route_search → examine results → smart_route_call with the chosen tool name and required arguments.\n\nIMPORTANT: Always check the tool's inputSchema from smart_route_search results before invoking to ensure you provide the correct arguments. The search results will show you exactly what parameters each tool expects.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "toolName": {"type": "string", "description": "The exact name of the tool to invoke (from smart_route_search results)"},
                        "arguments": {"type": "object", "additionalProperties": true, "description": "The arguments to pass to the tool based on its inputSchema (optional if tool requires no arguments)"}
                    },
                    "required": ["toolName"]
                },
                "annotations": {"title": "Call Tool", "openWorldHint": true}
            }),
        ]
    }
}

fn text_response(payload: Value) -> Value {
    // Pretty-print: the text lands in UI test dialogs and MCP client logs —
    // a single escaped line is unreadable; 2-space JSON is the norm.
    json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(&payload).unwrap_or_default()}]})
}

/// Handle `smart_route_search` (meta). `limit` defaults to 10 (origin), clamped
/// [1, settings.max_results] on top of the settings-driven scoring.
pub async fn handle_search_tools(
    query: &str,
    limit: Value,
    allowed: Option<Vec<String>>,
) -> Result<Value, String> {
    if query.is_empty() {
        return Err("Query parameter is required and must be a string".to_string());
    }
    let settings = models::get_settings().await;
    let limit_n = limit
        .as_u64()
        .unwrap_or(10)
        .clamp(1, settings.max_results.max(1) as u64) as u32;

    let results = search::search(query, Some(limit_n), allowed)
        .await
        .map_err(|e| e.to_string())?;
    let progressive = settings.progressive_disclosure;
    let name_sep = tool_name_separator().await;

    // Tools-only view (server rows have an empty tool_name — skip them).
    let tools: Vec<Value> = results
        .iter()
        .filter(|r| !r.tool_name.is_empty())
        .map(|r| {
            if progressive {
                json!({
                    "name": display_tool_name(&name_sep, &r.server_name, &r.tool_name),
                    "description": r.metadata.as_ref()
                        .and_then(|m| m.get("description"))
                        .and_then(|d| d.as_str())
                        .unwrap_or(""),
                    "serverName": r.server_name,
                    // Relevance score in [0,1] — lets external clients rank /
                    // threshold results themselves (parity with RAG search).
                    "score": (r.score * 1000.0).round() / 1000.0,
                })
            } else {
                json!({
                    "name": display_tool_name(&name_sep, &r.server_name, &r.tool_name),
                    "description": r.metadata.as_ref()
                        .and_then(|m| m.get("description"))
                        .and_then(|d| d.as_str())
                        .unwrap_or(""),
                    "inputSchema": r.metadata.as_ref()
                        .and_then(|m| m.get("inputSchema"))
                        .cloned()
                        .unwrap_or(json!({})),
                    "serverName": r.server_name,
                    "score": (r.score * 1000.0).round() / 1000.0,
                })
            }
        })
        .collect();

    let (guideline, next_steps) = if progressive {
        if !tools.is_empty() {
            ("Found relevant tools. Use smart_route_describe to get the full parameter schema before calling. If these tools don't match exactly what you need, try another search with more specific keywords.",
             "Use smart_route_describe with the toolName to get the full inputSchema, then use smart_route_call to execute.")
        } else {
            ("No tools found. Try broadening your search or using different keywords.",
             "Consider searching for related capabilities or more general terms.")
        }
    } else if !tools.is_empty() {
        ("Found relevant tools. If these tools don't match exactly what you need, try another search with more specific keywords.",
         "To use a tool, call smart_route_call with the toolName and required arguments.")
    } else {
        ("No tools found. Try broadening your search or using different keywords.",
         "Consider searching for related capabilities or more general terms.")
    };

    Ok(text_response(json!({
        "tools": tools,
        "metadata": {
            "query": query,
            "totalResults": tools.len(),
            "progressiveDisclosure": progressive,
            "guideline": guideline,
            "nextSteps": next_steps,
        }
    })))
}

/// Group-level tool whitelist, keyed by server name (`None` entry value =
/// server not group-restricted at tool level). Sourced from
/// `mcp_scope_server_filters` by the HTTP layer for `$smart/{group}` scopes;
/// `None` (the whole map) = no group tool restriction in this scope.
pub type GroupToolGate = std::collections::HashMap<String, Option<Vec<String>>>;

/// Check the group tool whitelist for (server, tool). `Some(map)` gates;
/// missing server entry / `None` whitelist / containing the tool all pass.
fn group_tool_allowed(gate: Option<&GroupToolGate>, server: &str, tool: &str) -> bool {
    match gate {
        None => true,
        Some(map) => match map.get(server) {
            None => true,
            Some(None) => true,
            Some(Some(list)) => list.iter().any(|t| t == tool),
        },
    }
}

/// Public display name for a tool: `{server}{nameSeparator}{tool}` — the same
/// naming the /mcp endpoint uses for grouped tools, so external clients see one
/// consistent identifier across search results, describe output and direct
/// MCP calls.
pub(crate) fn display_tool_name(sep: &str, server: &str, tool: &str) -> String {
    format!("{server}{sep}{tool}")
}

/// Read the configured tool-name separator (`nameSeparator`, default `-`).
pub(crate) async fn tool_name_separator() -> String {
    crate::services::config_service::get()
        .await
        .ok()
        .and_then(|c| c.get("nameSeparator").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_else(|| "-".to_string())
}

/// Resolve a (possibly `{server}{sep}tool` prefixed) meta smart_route_call name to
/// (server, original tool) within `allowed`.
async fn resolve_tool(
    tool_name: &str,
    allowed: Option<&[String]>,
) -> Option<(String, String)> {
    let servers: Vec<String> = match allowed {
        Some(list) => list.to_vec(),
        None => {
            let mut names: Vec<String> = crate::mcp::pool::get_all_statuses()
                .await
                .into_iter()
                .filter(|s| s.connected)
                .map(|s| s.name)
                .collect();
            // resolve_tool only looks at tools of servers in this list; the
            // builtin server lives outside the pool, so add it when live —
            // otherwise smart_route_describe/call can't resolve rag_* tools.
            if crate::rag::service::is_enabled() {
                names.push(crate::rag::service::BUILTIN_SERVER_NAME.to_string());
            }
            names
        }
    };
    let sep = crate::services::config_service::get()
        .await
        .ok()
        .and_then(|c| c.get("nameSeparator").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_else(|| "-".to_string());
    for sn in &servers {
        let prefix = format!("{sn}{sep}");
        let orig = tool_name.strip_prefix(&prefix).unwrap_or(tool_name);
        if let Ok(ts) = crate::mcp::pool::list_tools_for(sn).await {
            if ts.iter().any(|t| t.name == orig) {
                return Some((sn.clone(), orig.to_string()));
            }
        }
    }
    None
}

/// Handle `smart_route_describe` (meta, progressive mode). Returns the full tool
/// info (origin response shape) or an isError response.
pub async fn handle_describe_tool(
    tool_name: &str,
    allowed: Option<Vec<String>>,
    group_tools: Option<&GroupToolGate>,
) -> Result<Value, String> {
    if tool_name.is_empty() {
        return Err("toolName parameter is required and must be a string".to_string());
    }
    if let Some((server, orig)) = resolve_tool(tool_name, allowed.as_deref()).await {
        if !group_tool_allowed(group_tools, &server, &orig) {
            return Ok(text_response(json!({
                "error": format!("Tool '{tool_name}' is not available in this group scope"),
                "metadata": {"message": "The tool is excluded by the group's tool filter. Use smart_route_search to discover available tools."}
            })));
        }
        if let Ok(ts) = crate::mcp::pool::list_tools_for(&server).await {
            let filtered = crate::services::server_tool_config_service::apply_tool_filters(&server, ts)
                .await
                .unwrap_or_default();
            if let Some(t) = filtered.iter().find(|t| t.name == orig) {
                if !t.enabled {
                    return Ok(text_response(json!({
                        "error": format!("Tool '{tool_name}' is disabled"),
                        "metadata": {"message": "The specified tool is disabled. Use smart_route_search to discover available tools."}
                    })));
                }
                let tool_info = json!({
                    "name": display_tool_name(&tool_name_separator().await, &server, &t.name),
                    "description": t.description,
                    "inputSchema": t.input_schema,
                    "serverName": server,
                });
                return Ok(text_response(json!({
                    "tool": tool_info,
                    "metadata": {"message": format!("Full schema for tool '{tool_name}'. Use smart_route_call with the toolName and arguments based on the inputSchema.")}
                })));
            }
        }
    }
    Ok(text_response(json!({
        "error": format!("Tool '{tool_name}' not found or not available"),
        "metadata": {"message": "The specified tool was not found. Use smart_route_search to discover available tools."}
    })))
}

/// Handle `smart_route_call` (meta): resolve the target and execute through the
/// shared pool path (on-demand wake included). Returns the raw result JSON
/// (content/isError) or an Err string for JSON-RPC error wrapping.
pub async fn handle_call_tool(
    tool_name: &str,
    args: Value,
    allowed: Option<Vec<String>>,
    group_tools: Option<&GroupToolGate>,
) -> Result<Value, String> {
    if tool_name.is_empty() {
        return Err("toolName parameter is required".to_string());
    }
    let Some((server, orig)) = resolve_tool(tool_name, allowed.as_deref()).await else {
        return Err(format!("Tool not available: {tool_name}"));
    };
    if !group_tool_allowed(group_tools, &server, &orig) {
        return Err(format!(
            "Tool '{orig}' is excluded by the group's tool filter"
        ));
    }
    // Enabled check (index may be stale after a post-index disable).
    if let Ok(ts) = crate::mcp::pool::list_tools_for(&server).await {
        let filtered =
            crate::services::server_tool_config_service::apply_tool_filters(&server, ts)
                .await
                .unwrap_or_default();
        if let Some(t) = filtered.iter().find(|t| t.name == orig) {
            if !t.enabled {
                return Err(format!("Tool '{}' is disabled", orig));
            }
        }
    }
    log::info!("[smart] meta smart_route_call '{}' -> {}:{}", tool_name, server, orig);
    let result = crate::mcp::pool::call_tool(&server, &orig, args)
        .await
        .map_err(|e| e.to_string())?;
    let mut resp = json!({"content": result.content, "isError": result.is_error});
    if let Some(sc) = &result.structured_content {
        resp["structuredContent"] = sc.clone();
    }
    Ok(resp)
}

/// Public for the Tauri reindex command + boot restore logging.
pub async fn reindex_all_servers() -> anyhow::Result<usize> {
    index::reindex_all().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_parsing_matrix() {
        assert!(is_smart_scope("$smart"));
        assert!(is_smart_scope("$smart/"));
        assert!(is_smart_scope("$smart/mygroup"));
        assert!(!is_smart_scope("mygroup"));
        assert!(!is_smart_scope("$smartx"));
        assert!(!is_smart_scope(""));
        assert_eq!(target_group("$smart"), None);
        assert_eq!(target_group("$smart/"), None, "empty group = global scope");
        assert_eq!(target_group("$smart/g1").as_deref(), Some("g1"));
    }

    #[test]
    fn meta_tools_shape_progressive_vs_flat() {
        let p = build_meta_tools("all servers", "a, b", true);
        assert_eq!(p.len(), 3, "progressive = search/describe/call");
        let names: Vec<&str> = p.iter().filter_map(|t| t.get("name").and_then(|n| n.as_str())).collect();
        assert_eq!(names, vec!["smart_route_search", "smart_route_describe", "smart_route_call"]);
        // Progressive search description should NOT inline the schemas hint.
        assert!(names.iter().all(|n| !n.is_empty()));
        let f = build_meta_tools("all servers", "a, b", false);
        assert_eq!(f.len(), 2, "flat = search/call");
        // Only smart_route_search carries the scope + servers list (origin parity:
        // describe/call descriptions are scope-independent workflow text).
        for tools in [&f, &p] {
            let d = tools[0].get("description").and_then(|v| v.as_str()).unwrap();
            assert!(d.contains("all servers"), "scope description injected");
            assert!(d.contains("a, b"), "servers list injected");
            assert!(!tools[1].get("description").and_then(|v| v.as_str()).unwrap().contains("a, b"),
                "smart_route_describe must NOT embed the servers list");
        }
        // Schemas: required fields present.
        let search = &p[0];
        let req = search["inputSchema"]["required"].as_array().unwrap();
        assert_eq!(req.len(), 1);
        assert_eq!(req[0].as_str(), Some("query"));
    }
}

// ── Builtin "mcphub-desktop" server integration ─────────────────────────────

/// Names of the smart-routing meta tools as exposed by the builtin
/// "mcphub-desktop" server (Tauri smart_route_call path + tools/list).
pub const META_TOOL_NAMES: [&str; 3] = ["smart_route_search", "smart_route_describe", "smart_route_call"];

pub fn is_meta_tool(name: &str) -> bool {
    META_TOOL_NAMES.contains(&name)
}

/// The meta tools as `Tool` entries for the builtin "mcphub-desktop" server.
/// Empty when Smart Routing is disabled — the builtin server then lists only
/// its RAG tools / prompts / resources. Static descriptions (global scope,
/// no per-request servers list): this listing drives UI display and schema
/// hints; the HTTP MCP `$smart` path keeps its dynamic descriptions.
pub async fn builtin_meta_tools() -> Vec<crate::models::server::Tool> {
    if !models::get_settings().await.enabled {
        return Vec::new();
    }
    let progressive = models::get_settings().await.progressive_disclosure;
    let scope_desc = "all MCP servers managed by MCPHub Desktop";
    build_meta_tools(scope_desc, "", progressive)
        .into_iter()
        .map(|v| crate::models::server::Tool {
            name: v.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
            description: v.get("description").and_then(|d| d.as_str()).map(String::from),
            input_schema: v.get("inputSchema").cloned().unwrap_or(json!({})),
            server_name: crate::rag::service::BUILTIN_SERVER_NAME.to_string(),
            enabled: true,
            annotations: None,
            output_schema: None,
        })
        .collect()
}

/// Execute a meta tool from the builtin server (Tauri `smart_route_call` command /
/// tools panel test-invocation). Global scope (allowed=None, no group gate) —
/// same semantics as the unscoped `/mcp` path. Returns a text-content result;
/// `is_error` mirrors handler errors so the UI test dialog shows failure.
pub async fn call_meta_tool_builtin(
    tool_name: &str,
    args: &serde_json::Value,
) -> anyhow::Result<crate::models::server::ToolCallResult> {
    use crate::models::server::ToolCallResult;
    if let Some(msg) = not_ready_message().await {
        return Ok(ToolCallResult {
            content: vec![json!({"type": "text", "text": msg})],
            is_error: true,
            structured_content: None,
        });
    }
    let result: Result<Value, String> = match tool_name {
        "smart_route_search" => {
            let q = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let limit = args.get("limit").cloned().unwrap_or(json!(10));
            handle_search_tools(q, limit, None).await
        }
        "smart_route_describe" => {
            let tn = args.get("toolName").and_then(|v| v.as_str()).unwrap_or("");
            handle_describe_tool(tn, None, None).await
        }
        "smart_route_call" => {
            let tn = args.get("toolName").and_then(|v| v.as_str()).unwrap_or("");
            let a = args.get("arguments").cloned().unwrap_or(json!({}));
            handle_call_tool(tn, a, None, None).await
        }
        _ => Err(format!("Unknown meta tool: {tool_name}")),
    };
    Ok(match result {
        Ok(payload) => ToolCallResult {
            content: vec![json!({"type": "text", "text": serde_json::to_string_pretty(&payload).unwrap_or_default()})],
            is_error: false,
            structured_content: None,
        },
        Err(e) => ToolCallResult {
            content: vec![json!({"type": "text", "text": e})],
            is_error: true,
            structured_content: None,
        },
    })
}
