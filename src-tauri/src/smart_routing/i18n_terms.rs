//! Cross-lingual (zh ↔ en) query expansion for the Smart Routing keyword
//! channel. Tool descriptions in the wild are mostly single-language
//! (Chinese OR English) and the user's query language is unknown up front,
//! so the keyword channel expands each matched concept to BOTH languages:
//! a Chinese query gains English terms (and vice versa), and a long CJK
//! phrase gains its standalone concept words (whitespace splitting cannot
//! tokenize Chinese). The vector channel keeps the original query — a
//! multilingual embedding model already handles cross-lingual similarity,
//! and mixing translations would dilute the embedding.
//!
//! The dictionary is a curated, domain-targeted list (MCP tool concepts),
//! deliberately NOT a general-purpose translation table: recall beats
//! precision here because merge_hits re-ranks by matched-term ratio.

/// (zh, en) concept pairs, ordered by how likely they are to appear in a
/// tool name/description. Matching is: zh = substring of the raw query;
/// en = whole-word (case-insensitive) for single words, substring for
/// multi-word phrases.
pub const ZH_EN_TERMS: &[(&str, &str)] = &[
    // ── 通用动作 ──
    ("搜索", "search"),
    ("查找", "find"),
    ("查询", "query"),
    ("创建", "create"),
    ("新建", "create"),
    ("删除", "delete"),
    ("更新", "update"),
    ("修改", "modify"),
    ("读取", "read"),
    ("写入", "write"),
    ("下载", "download"),
    ("上传", "upload"),
    ("发送", "send"),
    ("获取", "fetch"),
    ("列出", "list"),
    ("执行", "execute"),
    ("运行", "run"),
    ("转换", "convert"),
    ("翻译", "translate"),
    ("总结", "summarize"),
    ("摘要", "summary"),
    ("生成", "generate"),
    ("分析", "analyze"),
    ("监控", "monitor"),
    ("管理", "manage"),
    ("自动化", "automation"),
    ("调度", "schedule"),
    ("提醒", "reminder"),
    ("通知", "notification"),
    // ── 开发 ──
    ("代码", "code"),
    ("终端", "terminal"),
    ("命令", "command"),
    ("脚本", "script"),
    ("调试", "debug"),
    ("测试", "test"),
    ("编译", "compile"),
    ("构建", "build"),
    ("部署", "deploy"),
    ("版本控制", "version control"),
    ("仓库", "repository"),
    ("提交", "commit"),
    ("分支", "branch"),
    ("合并", "merge"),
    ("拉取请求", "pull request"),
    ("问题", "issue"),
    ("工单", "ticket"),
    ("文档", "documentation"),
    ("接口", "api"),
    ("接口", "endpoint"),
    ("函数", "function"),
    ("类", "class"),
    ("重构", "refactor"),
    ("代码审查", "code review"),
    ("容器", "docker"),
    ("容器编排", "kubernetes"),
    // ── 数据 ──
    ("数据库", "database"),
    ("数据表", "table"),
    ("表格", "spreadsheet"),
    ("电子表格", "excel"),
    ("结构化查询语言", "sql"),
    ("数据", "data"),
    ("数据集", "dataset"),
    ("缓存", "cache"),
    ("索引", "index"),
    ("备份", "backup"),
    ("导入", "import"),
    ("导出", "export"),
    ("同步", "sync"),
    // ── 文件 ──
    ("文件", "file"),
    ("文件夹", "folder"),
    ("目录", "directory"),
    ("路径", "path"),
    ("压缩", "zip"),
    ("解压", "unzip"),
    ("图片", "image"),
    ("截图", "screenshot"),
    ("音频", "audio"),
    ("视频", "video"),
    ("文档处理", "pdf"),
    ("字处理", "word"),
    ("幻灯片", "powerpoint"),
    ("标记语言", "markdown"),
    // ── 网络 ──
    ("网页", "web"),
    ("浏览器", "browser"),
    ("网页抓取", "scrape"),
    ("爬虫", "crawler"),
    ("超文本传输协议请求", "http"),
    ("请求", "request"),
    ("下载链接", "url"),
    ("域名系统", "dns"),
    ("网络", "network"),
    ("服务器", "server"),
    ("客户端", "client"),
    ("代理", "proxy"),
    // ── 通信 ──
    ("邮件", "email"),
    ("消息", "message"),
    ("聊天", "chat"),
    ("日历", "calendar"),
    ("会议", "meeting"),
    ("通讯录", "contacts"),
    // ── AI / 知识 ──
    ("知识库", "knowledge base"),
    ("记忆", "memory"),
    ("向量", "vector"),
    ("嵌入", "embedding"),
    ("模型", "model"),
    ("人工智能", "ai"),
    ("提示词", "prompt"),
    ("大语言模型", "llm"),
    ("语义搜索", "semantic search"),
    ("排序", "rank"),
    ("评分", "score"),
    // ── 生活 / 效率 ──
    ("天气", "weather"),
    ("地图", "map"),
    ("导航", "navigation"),
    ("音乐", "music"),
    ("笔记", "note"),
    ("待办事项", "todo"),
    ("任务", "task"),
    ("项目", "project"),
    ("购物", "shopping"),
    ("支付", "payment"),
    ("订单", "order"),
    ("用户", "user"),
    ("认证", "auth"),
    ("登录", "login"),
    ("密码", "password"),
    ("加密", "encrypt"),
    ("密钥", "token"),
];

/// Maximum number of expansion terms added to the keyword query (guards
/// against a pathological query matching dozens of concepts and diluting
/// the matched/total term ratio in merge_hits).
pub const MAX_EXPANSION_TERMS: usize = 12;

/// Expand `query` with cross-lingual terms for the keyword channel.
/// Returns the additional terms (deduped, never duplicating a whole
/// whitespace token already present in the query, capped at
/// [`MAX_EXPANSION_TERMS`]). The original query is NOT included — callers
/// append these to it.
pub fn expand_query_terms(query: &str) -> Vec<String> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let lower = query.to_lowercase();
    // Whole ASCII word tokens of the query (for en whole-word matching and
    // for the "already present" dedup).
    let tokens: std::collections::HashSet<String> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect();

    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = tokens.clone();
    for (zh, en) in ZH_EN_TERMS {
        if out.len() >= MAX_EXPANSION_TERMS {
            break;
        }
        let en_lower = en.to_lowercase();
        let matched = if zh.chars().any(|c| c.is_ascii_alphanumeric()) {
            // Non-CJK zh entry (defensive): substring match.
            lower.contains(&zh.to_lowercase())
        } else {
            query.contains(zh)
        } || (if en_lower.contains(' ') {
            // Multi-word en phrase: substring on the lowercased query.
            lower.contains(&en_lower)
        } else {
            // Single en word: whole-token match only ("cat" must not fire
            // inside "category").
            tokens.contains(&en_lower)
        });
        if !matched {
            continue;
        }
        for term in [zh.to_string(), en.to_string()] {
            // Skip terms the query already contains as a whole token —
            // they would only dilute the matched/total ratio.
            let term_lower = term.to_lowercase();
            if term_lower.contains(' ') || !seen.contains(&term_lower) {
                if seen.insert(term_lower) && out.len() < MAX_EXPANSION_TERMS {
                    out.push(term);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_query_gains_english_terms() {
        let out = expand_query_terms("部署数据库工具");
        assert!(out.contains(&"deploy".to_string()), "{:?}", out);
        assert!(out.contains(&"database".to_string()), "{:?}", out);
        // Standalone zh concept words are added too (CJK has no whitespace
        // tokenization — the original phrase alone would under-match).
        assert!(out.contains(&"部署".to_string()), "{:?}", out);
        assert!(out.contains(&"数据库".to_string()), "{:?}", out);
    }

    #[test]
    fn english_query_gains_chinese_terms() {
        let out = expand_query_terms("search the web");
        assert!(out.contains(&"搜索".to_string()), "{:?}", out);
        assert!(out.contains(&"网页".to_string()), "{:?}", out);
    }

    #[test]
    fn english_word_match_is_whole_word() {
        // "cat" must not fire inside "category".
        let out = expand_query_terms("categorize my files");
        assert!(!out.contains(&"猫".to_string()), "{:?}", out);
        // But a real whole word does.
        let out2 = expand_query_terms("send an email");
        assert!(out2.contains(&"邮件".to_string()), "{:?}", out2);
    }

    #[test]
    fn terms_already_in_query_are_not_duplicated() {
        let out = expand_query_terms("search database");
        assert!(!out.contains(&"search".to_string()), "{:?}", out);
        assert!(!out.contains(&"database".to_string()), "{:?}", out);
        assert!(out.contains(&"搜索".to_string()), "{:?}", out);
    }

    #[test]
    fn expansion_is_capped() {
        // A query touching many concepts must not blow up the term list.
        let q = "搜索 创建 删除 更新 下载 上传 发送 获取 执行 运行 转换 翻译 总结 生成 分析 监控";
        let out = expand_query_terms(q);
        assert!(out.len() <= MAX_EXPANSION_TERMS, "{:?}", out.len());
    }

    #[test]
    fn empty_query_yields_no_expansions() {
        assert!(expand_query_terms("").is_empty());
        assert!(expand_query_terms("   ").is_empty());
    }

    #[test]
    fn no_concept_match_yields_no_expansions() {
        assert!(expand_query_terms("zzzqqq").is_empty());
    }

    #[test]
    fn multi_word_english_phrase_matches_by_substring() {
        let out = expand_query_terms("manage my pull requests");
        assert!(out.contains(&"拉取请求".to_string()), "{:?}", out);
    }
}
