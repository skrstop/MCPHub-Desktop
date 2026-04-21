/// Full system configuration stored as flexible JSON.
/// Mirrors the TypeScript SystemConfig shape (routing, install, smartRouting, etc.).
/// Using serde_json::Value allows storing any nested sub-config without rigid column definitions.
pub type SystemConfig = serde_json::Value;
