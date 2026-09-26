//! Smart Routing settings read from `config_json.smartRouting` (the same keys
//! the Phase 1 settings UI writes). Every numeric field is clamped on read so
//! a hand-edited config can never push the search into a degenerate state
//! (e.g. max_results=0 → empty results, negative weights → inverted scores).

use serde::{Deserialize, Serialize};

/// Retrieval settings (desktop hybrid search) + the flags search/index need.
/// Defaults mirror the Phase 1 UI defaults (SettingsContext).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SmartRoutingSettings {
    /// Whether the Smart Routing consumer is enabled (drives mv preload at
    /// boot and gates indexing).
    pub enabled: bool,
    /// Meta-tool catalog verbosity: true = 3-step (search/describe/call),
    /// false = 2-step (search/call). Origin default `?? false` (flat 2-step).
    pub progressive_disclosure: bool,
    pub vector_weight: f32,
    pub keyword_weight: f32,
    pub max_results: u32,
    pub score_threshold: f32,
}

impl Default for SmartRoutingSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            progressive_disclosure: false,
            vector_weight: 0.5,
            keyword_weight: 0.5,
            max_results: 20,
            score_threshold: 0.5,
        }
    }
}

impl SmartRoutingSettings {
    fn clamp(&mut self) {
        self.vector_weight = self.vector_weight.clamp(0.0, 1.0);
        self.keyword_weight = self.keyword_weight.clamp(0.0, 1.0);
        self.max_results = self.max_results.clamp(1, 200);
        self.score_threshold = self.score_threshold.clamp(0.0, 1.0);
    }
}

/// Read + clamp the settings. Missing keys fall back to the defaults (the
/// config store deep-merges, so partial user configs keep their set values).
pub async fn get_settings() -> SmartRoutingSettings {
    let mut s = crate::services::config_service::get()
        .await
        .ok()
        .and_then(|c| c.get("smartRouting").cloned())
        .and_then(|v| serde_json::from_value::<SmartRoutingSettings>(v).ok())
        .unwrap_or_default();
    s.clamp();
    s
}
