-- Add source_ip column to activity_log for tracking client IP
ALTER TABLE activity_log ADD COLUMN source_ip TEXT;
