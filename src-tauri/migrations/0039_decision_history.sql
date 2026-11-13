-- v38 -> v39: decision-support request history (决策支持 page).
CREATE TABLE IF NOT EXISTS decision_history (
    id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    decision_type TEXT NOT NULL,
    state TEXT,
    question TEXT NOT NULL,
    options_json TEXT NOT NULL,
    scores_json TEXT NOT NULL,
    probabilities_json TEXT NOT NULL,
    selected_index INTEGER NOT NULL,
    selected_option TEXT NOT NULL,
    duration_ms INTEGER,
    source TEXT
);
CREATE INDEX IF NOT EXISTS idx_decision_history_created ON decision_history(created_at DESC);
