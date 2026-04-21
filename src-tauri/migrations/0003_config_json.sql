-- Add flexible JSON config storage to replace rigid column-based config
ALTER TABLE system_config ADD COLUMN config_json TEXT NOT NULL DEFAULT '{}';
