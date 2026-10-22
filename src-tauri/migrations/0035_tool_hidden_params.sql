-- v35 (#1343 parity): per-tool hidden input-schema parameters.
-- JSON array of top-level parameter names; NULL = setting absent.
ALTER TABLE server_tool_config ADD COLUMN hidden_params TEXT;
