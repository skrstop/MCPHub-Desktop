-- v23 → v24: FTS5 全文索引表（中/英/拼音分词，写入侧 Rust 分词——方案 B）
-- 本文件仅供 sqlx::migrate! 兼容；实际迁移由 src/db/migration.rs::migrate_v24 执行。
-- 列语义：ref_id = 源表定位键（servers→name / groups→id / rag_docs→id /
--         skills→dir_name / prompts/resources→name / app_log→id）；
--         zh = charabia 词元（kvariants 繁体规范形，lowercase）；
--         py = CJK 词元全拼连写 token（英文词原样并入）；
--         ini = CJK 词元首字母连写 token。

CREATE VIRTUAL TABLE IF NOT EXISTS fts_servers   USING fts5(ref_id UNINDEXED, zh, py, ini);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_groups    USING fts5(ref_id UNINDEXED, zh, py, ini);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_rag_docs  USING fts5(ref_id UNINDEXED, zh, py, ini);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_skills    USING fts5(ref_id UNINDEXED, zh, py, ini);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_prompts   USING fts5(ref_id UNINDEXED, zh, py, ini);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_resources USING fts5(ref_id UNINDEXED, zh, py, ini);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_app_log   USING fts5(ref_id UNINDEXED, zh, py, ini);
