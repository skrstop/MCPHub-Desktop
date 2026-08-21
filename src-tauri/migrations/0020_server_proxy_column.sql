-- v20: servers 表添加 proxy 列（Proxychains4 配置 JSON）。
-- 上游 #1055 proxy 配置 round-trip 的持久化落点。可为空。
ALTER TABLE servers ADD COLUMN proxy TEXT;
