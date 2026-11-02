-- v38 (#1413 parity): bearer-key "all-except" exclusion scope.
-- JSON string arrays (same shape as allowed_groups / allowed_servers);
-- '[]' = nothing excluded. Only meaningful when access_type = 'exclude'.
ALTER TABLE bearer_keys ADD COLUMN excluded_groups TEXT NOT NULL DEFAULT '[]';
ALTER TABLE bearer_keys ADD COLUMN excluded_servers TEXT NOT NULL DEFAULT '[]';
