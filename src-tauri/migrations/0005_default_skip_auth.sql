-- 桌面版默认开启免登录开关：对历史/全新数据库回填默认值。
-- 仅在 routing.skipAuth 字段尚未显式设置时设置为 true，避免覆盖用户的显式选择。
UPDATE system_config
SET config_json = json_set(
        COALESCE(config_json, '{}'),
        '$.routing',
        json_set(
            COALESCE(json_extract(config_json, '$.routing'), json('{}')),
            '$.skipAuth',
            json('true')
        )
    )
WHERE id = 1
  AND json_extract(COALESCE(config_json, '{}'), '$.routing.skipAuth') IS NULL;

