# migrations/ (historical snapshot)

**此目录仅是历史快照，不是运行时数据源。**

- 实际迁移逻辑以 `src-tauri/src/db/migration.rs`（`migrate_v1`–`migrate_v34` 内联 SQL，`TARGET_VERSION` 为准）为**唯一权威**。
- 早期版本仅部分有对应 `.sql` 快照（v1–v8、v13、v20、v24–v26 内联实现；v27 起新迁移均补了 `.sql` 快照）——**没有 .sql 文件不代表迁移缺失**，以目录实际文件与 `migration.rs` 为准。
- 运行时不读取本目录（无 `include_str!`/`sqlx::migrate!`），新增迁移只需改 `migration.rs`；是否补一份 .sql 快照纯属可选。
