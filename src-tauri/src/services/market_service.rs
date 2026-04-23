use crate::models::market::MarketServer;
use std::collections::HashMap;
use std::sync::OnceLock;

// Embed the community servers.json catalog at compile time.
const SERVERS_JSON: &str = include_str!("../../../servers.json");

static SERVERS: OnceLock<Vec<MarketServer>> = OnceLock::new();

fn all_servers() -> &'static Vec<MarketServer> {
    SERVERS.get_or_init(|| {
        let map: HashMap<String, MarketServer> =
            serde_json::from_str(SERVERS_JSON).unwrap_or_default();
        map.into_values().collect()
    })
}

pub fn list(q: Option<&str>, category: Option<&str>, tag: Option<&str>) -> Vec<MarketServer> {
    let servers = all_servers();
    let q_lower = q.unwrap_or("").to_lowercase();
    let cat = category.unwrap_or("");
    let tag_filter = tag.unwrap_or("");

    servers
        .iter()
        .filter(|s| {
            if !q_lower.is_empty() {
                let name_match = s.name.to_lowercase().contains(&q_lower);
                let display_match = s
                    .display_name
                    .as_deref()
                    .map(|d| d.to_lowercase().contains(&q_lower))
                    .unwrap_or(false);
                let desc_match = s
                    .description
                    .as_deref()
                    .map(|d| d.to_lowercase().contains(&q_lower))
                    .unwrap_or(false);
                if !name_match && !display_match && !desc_match {
                    return false;
                }
            }
            if !cat.is_empty() && !s.categories.iter().any(|c| c == cat) {
                return false;
            }
            if !tag_filter.is_empty() && !s.tags.iter().any(|t| t == tag_filter) {
                return false;
            }
            true
        })
        .cloned()
        .collect()
}

pub fn get(name: &str) -> Option<MarketServer> {
    all_servers().iter().find(|s| s.name == name).cloned()
}

pub fn categories() -> Vec<String> {
    let mut cats: Vec<String> = all_servers()
        .iter()
        .flat_map(|s| s.categories.iter().cloned())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    cats.sort();
    cats
}

pub fn tags() -> Vec<String> {
    let mut tags: Vec<String> = all_servers()
        .iter()
        .flat_map(|s| s.tags.iter().cloned())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    tags.sort();
    tags
}
