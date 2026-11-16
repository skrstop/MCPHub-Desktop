//! Language selection for the hub's builtin MCP tool descriptions
//! (rag_* / decision_decide / smart_route_*). The tool `name` is an API
//! identifier and never changes; only the tool-level `description` (and the
//! smart meta tools' annotation titles) switch language. Schema/property
//! text stays English — it is technical parameter documentation.
//!
//! Config keys (`system_config.mcp`):
//! - `toolLanguage`: "en" (default) | "auto" | "zh" | "fr" | "tr" | "vi" | "es"
//! - `uiLanguage`: the frontend UI language, synced by the webview so that
//!   `toolLanguage: "auto"` can follow it.
//!
//! The resolved language is cached in a process-global (tools/list is a sync
//! hot path); `refresh_from_config` re-resolves it at startup and whenever
//! either config key changes.

use std::sync::RwLock;

/// Languages with builtin tool description translations.
pub const TOOL_LANGS: [&str; 6] = ["en", "zh", "fr", "tr", "vi", "es"];

/// All `prop_description` keys — the mechanical source of truth for tests
/// (every key must resolve in every `TOOL_LANGS` entry).
pub const PROP_KEYS: [&str; 31] = [
    "query_simple", "tags_filter", "doc_id_plain", "search_key",
    "doc_name_create", "doc_type_create", "doc_content_create", "tags_attach",
    "doc_id_create_or_search", "doc_name_new", "doc_type_new",
    "doc_content_new", "doc_content_append", "update_external_file",
    "tags_add", "tags_remove", "history_version_read", "from_version",
    "to_version", "doc_id_a", "doc_id_b", "decision_type", "decision_state",
    "decision_question", "decision_options", "smart_query", "smart_limit",
    "smart_tool_name_describe", "smart_tool_name_call", "smart_arguments_full",
    "smart_arguments",
];

/// All `rag_tool_description` names.
pub const RAG_TOOL_KEYS: [&str; 10] = [
    "rag_search", "rag_get", "rag_tag_search", "rag_file_create",
    "rag_file_update", "rag_file_delete", "rag_file_history_versions",
    "rag_file_history_get", "rag_file_history_diff", "rag_file_diff",
];

static TOOL_LANG: RwLock<String> = RwLock::new(String::new()); // "" → "en"

/// Normalize a language tag to one of `TOOL_LANGS`: lowercase primary
/// subtag only ("zh-CN" → "zh", "EN" → "en"). Unknown → "".
fn normalize_lang(tag: &str) -> &str {
    let primary = tag
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    // to_ascii_lowercase allocates; map back to a static via the whitelist.
    match primary.as_str() {
        "en" => "en",
        "zh" => "zh",
        "fr" => "fr",
        "tr" => "tr",
        "vi" => "vi",
        "es" => "es",
        _ => "",
    }
}

/// Pure resolution: explicit `tool_language` wins; `"auto"` follows
/// `ui_language`; anything unknown/empty falls back to English.
pub fn resolve_tool_language(tool_language: &str, ui_language: &str) -> String {
    let candidate = if tool_language.trim().eq_ignore_ascii_case("auto") {
        normalize_lang(ui_language)
    } else {
        normalize_lang(tool_language)
    };
    if candidate.is_empty() {
        "en".to_string()
    } else {
        candidate.to_string()
    }
}

/// Store the resolved language (from `refresh_from_config` / tests).
pub fn set_tool_language(lang: &str) {
    let resolved = if normalize_lang(lang).is_empty() {
        "en".to_string()
    } else {
        normalize_lang(lang).to_string()
    };
    match TOOL_LANG.write() {
        Ok(mut g) => *g = resolved,
        Err(p) => *p.into_inner() = resolved,
    }
}

/// The cached resolved language (""/unset reads as "en").
pub fn current_tool_language() -> String {
    let g = match TOOL_LANG.read() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if g.is_empty() {
        "en".to_string()
    } else {
        g.clone()
    }
}

/// Serializes cache refresh + change detection (the detection window is
/// prev-read → config get → set → now-compare; without the lock two
/// concurrent config writes can interleave a stale snapshot into the other's
/// window, losing both the cache update and the rebuild trigger).
static REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Re-resolve from persisted config and report whether the RESOLVED language
/// changed. Called at startup and after any write that touches
/// `mcp.toolLanguage` / `mcp.uiLanguage`.
pub async fn refresh_from_config_and_report_change() -> bool {
    let _g = REFRESH_LOCK.lock().await;
    let prev = current_tool_language();
    let cfg = match crate::services::config_service::get().await {
        Ok(cfg) => cfg,
        Err(e) => {
            // Keep the previous cache on a transient read failure —
            // resolving to "en" here would flip the served language.
            log::warn!("[tool_i18n] config refresh failed, keeping cached '{prev}': {e:#}");
            return false;
        }
    };
    let mcp = cfg.get("mcp");
    let tool = mcp
        .and_then(|m| m.get("toolLanguage"))
        .and_then(|v| v.as_str())
        .unwrap_or("en");
    let ui = mcp
        .and_then(|m| m.get("uiLanguage"))
        .and_then(|v| v.as_str())
        .unwrap_or("en");
    set_tool_language(&resolve_tool_language(tool, ui));
    current_tool_language() != prev
}

/// Re-resolve without needing the change report (startup path).
pub async fn refresh_from_config() {
    let _ = refresh_from_config_and_report_change().await;
}

// ── rag_* tool descriptions ─────────────────────────────────────────────────

/// Tool-level description for one rag_* tool in `lang` (falls back to the
/// English text for unknown pairs). The exact English strings are the
/// pre-i18n values — default-`en` output must stay byte-identical.
pub fn rag_tool_description(name: &str, lang: &str) -> &'static str {
    let en = match name {
        "rag_search" => "Search RAG documents by semantic similarity. Returns matching text fragments with their document ids, titles, and similarity scores. Optionally filter to documents that have any of the given tags.",
        "rag_get" => "Get the full text content of a RAG document by its id (as returned by rag_search).",
        "rag_tag_search" => "List distinct tags in the RAG library. Pass `search_key` (an array of strings) to filter tags by case-insensitive substring (returns tags matching any key); omit/empty to return all tags.",
        "rag_file_create" => "Create a new RAG document from UTF-8 text content and index it for semantic search. The file is stored as {docName}.{docType} (e.g. readme.md). Returns the created docId. Documents with duplicate names coexist (a new docId is created); use rag_file_update to modify an existing document.",
        "rag_file_update" => "Update an existing RAG document by docId. Any of docName/docType/docContent can be omitted (only supplied fields are updated). When docContent is provided, docContentAppend decides whether to append to the existing content (true) or replace it (false, default); the document is re-indexed so semantic search reflects the new text. docId stays the same. By default only the copy stored inside rag/files is modified; pass updateExternalFile=true to also overwrite the external source file (see that parameter for scope and risks).",
        "rag_file_delete" => "Delete a RAG document by docId. Removes its content file, metadata, and all its vector chunks from the index. Irreversible.",
        "rag_file_history_versions" => "List the historical versions of a RAG document, newest first. A snapshot is taken each time a document's content is overwritten (via rag_file_update or a source refresh). Returns version, savedAt, size, name, md5 and source per entry.",
        "rag_file_history_get" => "Read the full text content of one historical version of a RAG document (use rag_file_history_versions to list available versions).",
        "rag_file_history_diff" => "Compare two versions of a RAG document and return a unified diff. Omit toVersion to diff a historical version against the CURRENT document content.",
        "rag_file_diff" => "Compare two DIFFERENT RAG documents and return a unified diff (docIdA is the old side, docIdB the new side). Independent of version history.",
        _ => "",
    };
    match (name, normalize_lang(lang)) {
        ("rag_search", "zh") => "按语义相似度搜索 RAG 文档。返回匹配的文本片段及其文档 id、标题和相似度得分。可选按给定标签过滤：只返回带有其中任意标签的文档。",
        ("rag_get", "zh") => "按文档 id（rag_search 返回）读取 RAG 文档的完整文本内容。",
        ("rag_tag_search", "zh") => "列出 RAG 库中去重后的全部标签。传入 `search_key`（字符串数组）按不区分大小写的子串过滤标签（返回命中任意关键词的标签）；省略或留空则返回全部标签。",
        ("rag_file_create", "zh") => "用 UTF-8 文本内容创建新的 RAG 文档并建立语义检索索引。文件以 {docName}.{docType} 形式存储（如 readme.md），返回创建的 docId。同名文档可以共存（每次创建新的 docId）；修改已有文档请用 rag_file_update。",
        ("rag_file_update", "zh") => "按 docId 更新已有的 RAG 文档。docName/docType/docContent 均可省略（只更新传入的字段）。提供 docContent 时，docContentAppend 决定是在原内容上追加（true）还是替换（false，默认）；文档会重建索引，语义搜索反映新内容，docId 保持不变。默认只修改 rag/files 内的副本；传 updateExternalFile=true 可同时覆盖外部源文件（范围与风险见该参数说明）。",
        ("rag_file_delete", "zh") => "按 docId 删除 RAG 文档。删除其内容文件、元数据以及索引中的全部向量分片。不可恢复。",
        ("rag_file_history_versions", "zh") => "列出 RAG 文档的历史版本，最新在前。每次文档内容被覆盖（rag_file_update 或数据源刷新）都会保存快照。每条返回 version、savedAt、size、name、md5 与 source。",
        ("rag_file_history_get", "zh") => "读取 RAG 文档某个历史版本的完整文本内容（版本列表见 rag_file_history_versions）。",
        ("rag_file_history_diff", "zh") => "对比 RAG 文档的两个版本并返回统一 diff。省略 toVersion 时，将历史版本与当前文档内容对比。",
        ("rag_file_diff", "zh") => "对比两个不同的 RAG 文档并返回统一 diff（docIdA 为旧方，docIdB 为新方）。与版本历史无关。",
        ("rag_search", "fr") => "Recherche des documents RAG par similarité sémantique. Renvoie les extraits de texte correspondants avec leurs identifiants de document, titres et scores de similarité. Filtrage facultatif aux documents portant l'un des tags indiqués.",
        ("rag_get", "fr") => "Récupère le contenu textuel complet d'un document RAG par son id (tel que renvoyé par rag_search).",
        ("rag_tag_search", "fr") => "Liste les tags distincts de la bibliothèque RAG. Passez `search_key` (un tableau de chaînes) pour filtrer les tags par sous-chaîne insensible à la casse (renvoie les tags correspondant à l'une des clés) ; omittez/videz pour renvoyer tous les tags.",
        ("rag_file_create", "fr") => "Crée un nouveau document RAG à partir de contenu textuel UTF-8 et l'indexe pour la recherche sémantique. Le fichier est stocké sous la forme {docName}.{docType} (p. ex. readme.md). Renvoie le docId créé. Les documents homonymes coexistent (un nouveau docId est créé) ; utilisez rag_file_update pour modifier un document existant.",
        ("rag_file_update", "fr") => "Met à jour un document RAG existant par docId. docName/docType/docContent peuvent être omis (seuls les champs fournis sont mis à jour). Quand docContent est fourni, docContentAppend décide d'ajouter au contenu existant (true) ou de le remplacer (false, défaut) ; le document est réindexé pour que la recherche sémantique reflète le nouveau texte. Le docId reste identique. Par défaut seule la copie stockée dans rag/files est modifiée ; passez updateExternalFile=true pour aussi écraser le fichier source externe (voir la portée et les risques de ce paramètre).",
        ("rag_file_delete", "fr") => "Supprime un document RAG par docId. Retire son fichier de contenu, ses métadonnées et tous ses fragments vectoriels de l'index. Irréversible.",
        ("rag_file_history_versions", "fr") => "Liste les versions historiques d'un document RAG, la plus récente en premier. Un instantané est pris à chaque écrasement du contenu (via rag_file_update ou un rafraîchissement de source). Renvoie version, savedAt, size, name, md5 et source par entrée.",
        ("rag_file_history_get", "fr") => "Lit le contenu textuel complet d'une version historique d'un document RAG (utilisez rag_file_history_versions pour lister les versions disponibles).",
        ("rag_file_history_diff", "fr") => "Compare deux versions d'un document RAG et renvoie un diff unifié. Omettez toVersion pour comparer une version historique au contenu ACTUEL du document.",
        ("rag_file_diff", "fr") => "Compare deux documents RAG DIFFÉRENTS et renvoie un diff unifié (docIdA est l'ancien côté, docIdB le nouveau). Indépendant de l'historique des versions.",
        ("rag_search", "tr") => "RAG belgelerini anlamsal benzerliğe göre arar. Eşleşen metin parçalarını belge kimlikleri, başlıkları ve benzerlik puanlarıyla döndürür. İsteğe bağlı olarak verilen etiketlerden herhangi birine sahip belgelere filtre uygulayın.",
        ("rag_get", "tr") => "Bir RAG belgesinin tam metin içeriğini kimliğiyle (rag_search'in döndürdüğü) alır.",
        ("rag_tag_search", "tr") => "RAG kitaplığındaki farklı etiketleri listeler. Etiketleri büyük/küçük harf duyarsız alt dizeye göre filtrelemek için `search_key` (dizeler dizisi) verin (herhangi bir anahtarla eşleşen etiketleri döndürür); tüm etiketler için boş bırakın/atlayın.",
        ("rag_file_create", "tr") => "UTF-8 metin içeriğinden yeni bir RAG belgesi oluşturur ve anlamsal arama için indeksler. Dosya {docName}.{docType} biçiminde saklanır (ör. readme.md). Oluşturulan docId'yi döndürür. Aynı adlı belgeler birlikte bulunabilir (yeni bir docId oluşturulur); mevcut bir belgeyi değiştirmek için rag_file_update kullanın.",
        ("rag_file_update", "tr") => "Mevcut bir RAG belgesini docId ile günceller. docName/docType/docContent alanları atlanabilir (yalnız verilen alanlar güncellenir). docContent verildiğinde docContentAppend, mevcut içeriğe eklemeyi (true) veya değiştirmeyi (false, varsayılan) belirler; belge yeniden indekslenir, anlamsal arama yeni metni yansıtır. docId aynı kalır. Varsayılan olarak yalnız rag/files içindeki kopya değişir; dış kaynak dosyasının üzerine de yazmak için updateExternalFile=true verin (kapsam ve riskler için o parametreye bakın).",
        ("rag_file_delete", "tr") => "Bir RAG belgesini docId ile siler. İçerik dosyasını, meta verilerini ve indeksteki tüm vektör parçalarını kaldırır. Geri alınamaz.",
        ("rag_file_history_versions", "tr") => "Bir RAG belgesinin geçmiş sürümlerini listeler, en yenisi önce. İçerik her değiştirildiğinde (rag_file_update veya kaynak yenileme) bir anlık görüntü alınır. Her kayıt için version, savedAt, size, name, md5 ve source döndürür.",
        ("rag_file_history_get", "tr") => "Bir RAG belgesinin bir geçmiş sürümünün tam metin içeriğini okur (mevcut sürümleri listelemek için rag_file_history_versions kullanın).",
        ("rag_file_history_diff", "tr") => "Bir RAG belgesinin iki sürümünü karşılaştırır ve birleşik bir diff döndürür. Geçmiş bir sürümü belgenin GÜNCEL içeriğiyle karşılaştırmak için toVersion'ı atlayın.",
        ("rag_file_diff", "tr") => "İki FARKLI RAG belgesini karşılaştırır ve birleşik bir diff döndürür (docIdA eski taraf, docIdB yeni taraf). Sürüm geçmişinden bağımsızdır.",
        ("rag_search", "vi") => "Tìm kiếm tài liệu RAG theo độ tương đồng ngữ nghĩa. Trả về các đoạn văn bản khớp kèm id tài liệu, tiêu đề và điểm tương đồng. Tùy chọn lọc theo thẻ: chỉ trả về tài liệu có bất kỳ thẻ nào trong danh sách.",
        ("rag_get", "vi") => "Đọc toàn bộ nội dung văn bản của một tài liệu RAG theo id (do rag_search trả về).",
        ("rag_tag_search", "vi") => "Liệt kê các thẻ khác nhau trong thư viện RAG. Truyền `search_key` (mảng chuỗi) để lọc thẻ theo chuỗi con không phân biệt hoa thường (trả về thẻ khớp bất kỳ khóa nào); bỏ qua hoặc bỏ trống để trả về tất cả thẻ.",
        ("rag_file_create", "vi") => "Tạo tài liệu RAG mới từ nội dung văn bản UTF-8 và lập chỉ mục tìm kiếm ngữ nghĩa. Tệp được lưu dạng {docName}.{docType} (vd. readme.md). Trả về docId vừa tạo. Tài liệu trùng tên vẫn cùng tồn tại (tạo docId mới); dùng rag_file_update để sửa tài liệu hiện có.",
        ("rag_file_update", "vi") => "Cập nhật tài liệu RAG hiện có theo docId. Có thể bỏ qua docName/docType/docContent (chỉ cập nhật trường được cung cấp). Khi có docContent, docContentAppend quyết định nối thêm vào nội dung (true) hay thay thế (false, mặc định); tài liệu được lập chỉ mục lại để tìm kiếm ngữ nghĩa phản ánh văn bản mới. docId giữ nguyên. Mặc định chỉ sửa bản sao trong rag/files; truyền updateExternalFile=true để ghi đè cả tệp nguồn bên ngoài (xem tham số đó để biết phạm vi và rủi ro).",
        ("rag_file_delete", "vi") => "Xóa tài liệu RAG theo docId. Loại bỏ tệp nội dung, siêu dữ liệu và toàn bộ phân đoạn vectơ khỏi chỉ mục. Không thể hoàn tác.",
        ("rag_file_history_versions", "vi") => "Liệt kê các phiên bản lịch sử của tài liệu RAG, mới nhất trước. Mỗi lần nội dung bị ghi đè (rag_file_update hoặc làm mới nguồn) sẽ lưu một ảnh chụp. Mỗi mục trả về version, savedAt, size, name, md5 và source.",
        ("rag_file_history_get", "vi") => "Đọc toàn bộ nội dung văn bản của một phiên bản lịch sử của tài liệu RAG (dùng rag_file_history_versions để liệt kê các phiên bản).",
        ("rag_file_history_diff", "vi") => "So sánh hai phiên bản của tài liệu RAG và trả về diff hợp nhất. Bỏ toVersion để so phiên bản lịch sử với nội dung HIỆN TẠI của tài liệu.",
        ("rag_file_diff", "vi") => "So sánh hai tài liệu RAG KHÁC NHAU và trả về diff hợp nhất (docIdA là bên cũ, docIdB là bên mới). Độc lập với lịch sử phiên bản.",
        ("rag_search", "es") => "Busca documentos RAG por similitud semántica. Devuelve fragmentos de texto coincidentes con sus ids de documento, títulos y puntuaciones de similitud. Filtrado opcional a documentos que tengan cualquiera de las etiquetas indicadas.",
        ("rag_get", "es") => "Obtiene el contenido textual completo de un documento RAG por su id (el devuelto por rag_search).",
        ("rag_tag_search", "es") => "Lista las etiquetas distintas de la biblioteca RAG. Pase `search_key` (un array de cadenas) para filtrar etiquetas por subcadena sin distinción de mayúsculas (devuelve las que coincidan con cualquier clave); omítalo/vacíelo para devolver todas las etiquetas.",
        ("rag_file_create", "es") => "Crea un documento RAG nuevo a partir de contenido textual UTF-8 y lo indexa para búsqueda semántica. El archivo se guarda como {docName}.{docType} (p. ej. readme.md). Devuelve el docId creado. Los documentos con nombres duplicados coexisten (se crea un docId nuevo); use rag_file_update para modificar un documento existente.",
        ("rag_file_update", "es") => "Actualiza un documento RAG existente por docId. docName/docType/docContent pueden omitirse (solo se actualizan los campos proporcionados). Cuando se proporciona docContent, docContentAppend decide si añadir al contenido existente (true) o reemplazarlo (false, por defecto); el documento se reindexa para que la búsqueda semántica refleje el nuevo texto. docId no cambia. Por defecto solo se modifica la copia guardada en rag/files; pase updateExternalFile=true para sobrescribir también el archivo fuente externo (ver ese parámetro para alcance y riesgos).",
        ("rag_file_delete", "es") => "Elimina un documento RAG por docId. Retira su archivo de contenido, sus metadatos y todos sus fragmentos vectoriales del índice. Irreversible.",
        ("rag_file_history_versions", "es") => "Lista las versiones históricas de un documento RAG, la más reciente primero. Se toma una instantánea cada vez que se sobrescribe el contenido (vía rag_file_update o una actualización de fuente). Devuelve version, savedAt, size, name, md5 y source por entrada.",
        ("rag_file_history_get", "es") => "Lee el contenido textual completo de una versión histórica de un documento RAG (use rag_file_history_versions para listar las versiones disponibles).",
        ("rag_file_history_diff", "es") => "Compara dos versiones de un documento RAG y devuelve un diff unificado. Omita toVersion para comparar una versión histórica con el contenido ACTUAL del documento.",
        ("rag_file_diff", "es") => "Compara dos documentos RAG DIFERENTES y devuelve un diff unificado (docIdA es el lado antiguo, docIdB el nuevo). Independiente del historial de versiones.",
        _ => en,
    }
}

// ── decision_decide description ─────────────────────────────────────────────

/// Tool-level description for `decision_decide` in `lang`.
pub fn decision_tool_description(lang: &str) -> &'static str {
    match normalize_lang(lang) {
        "zh" => "用本地决策模型运行一次结构化决策：给定决策类型、上下文 state、问题和 2-20 个候选选项，返回每个选项的分数、概率与选中项。模型是一个小型本地路由模型，适合在选项间挑选、判断是否需要调用工具、评估相关度；不适合事实性、计算或知识类问题。",
        "fr" => "Exécute une décision structurée avec le modèle de décision local : étant donné un type de décision, un contexte state, une question et 2 à 20 options candidates, renvoie les scores, probabilités et l'option retenue pour chaque option. Le modèle est un petit modèle de routage local, adapté pour choisir entre des options, décider d'appeler un outil ou évaluer une pertinence ; il ne convient pas aux questions factuelles, de calcul ou de connaissance.",
        "tr" => "Yerel karar modeliyle yapılandırılmış bir karar çalıştırır: bir karar türü, bağlam state, bir soru ve 2-20 aday seçenek verildiğinde, seçenek başına puanları, olasılıkları ve seçilen seçeneği döndürür. Model küçük bir yerel yönlendirme modelidir; seçenekler arasında seçim yapmak, araç çağırma kararı vermek ve alaka düzeyini puanlamak için uygundur; olgusal, hesaplama veya bilgi soruları için uygun değildir.",
        "vi" => "Chạy một quyết định có cấu trúc với mô hình quyết định cục bộ: với loại quyết định, ngữ cảnh state, câu hỏi và 2-20 tùy chọn ứng viên, trả về điểm, xác suất và tùy chọn được chọn cho từng lựa chọn. Mô hình là một mô hình định tuyến cục bộ nhỏ, phù hợp để chọn giữa các tùy chọn, quyết định có gọi công cụ hay không và chấm điểm mức độ liên quan; không phù hợp cho câu hỏi sự thật, tính toán hay kiến thức.",
        "es" => "Ejecuta una decisión estructurada con el modelo de decisión local: dado un tipo de decisión, un contexto state, una pregunta y de 2 a 20 opciones candidatas, devuelve las puntuaciones, probabilidades y la opción seleccionada de cada una. El modelo es un pequeño modelo de enrutamiento local, adecuado para elegir entre opciones, decidir si llamar a una herramienta y puntuar relevancia; no es adecuado para preguntas fácticas, de cálculo o de conocimiento.",
        _ => "Run a structured decision with the local decision model: given a question type, context state, question and 2-20 candidate options, returns per-option scores, probabilities and the selected option. The model is a small local routing model — suited to picking between options, tool-call routing and relevance scoring; do not use it for factual, calculation or knowledge questions.",
    }
}

// ── smart_route_* meta tool descriptions ────────────────────────────────────
// Templates carry the same placeholders the builders interpolate:
// {scope_description}, {servers_list}, {schema_note}.

fn fill(template: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (k, v) in pairs {
        out = out.replace(k, v);
    }
    out
}

/// `smart_route_search` description (progressive = 3-step mode).
pub fn smart_route_search_desc(
    lang: &str,
    progressive: bool,
    scope_description: &str,
    servers_list: &str,
    schema_note: &str,
) -> String {
    let tmpl = match (progressive, normalize_lang(lang)) {
        (true, "zh") => "第 3 步流程的第 1 步：最先使用本工具在 {scope_description} 中发现并搜索相关工具。只返回工具名称与描述——调用前请用 smart_route_describe 获取完整参数详情。\n\n为获得最佳结果，请使用与需求精确匹配的具体查询；复杂任务的不同部分请用不同查询多次调用。示例查询：\"image generation tools\"、\"code review tools\"、\"data analysis\"、\"translation capabilities\" 等。结果按向量相似度排序。\n\n工作流：smart_route_search → smart_route_describe（参数详情）→ smart_route_call（执行）\n\n可用服务器：{servers_list}",
        (true, "fr") => "ÉTAPE 1 sur 3 : utilisez CET outil EN PREMIER pour découvrir et rechercher des outils pertinents dans {scope_description}. Renvoie uniquement les noms et descriptions d'outils — utilisez smart_route_describe pour obtenir les détails de paramètres complets avant l'appel.\n\nPour de meilleurs résultats, utilisez des requêtes spécifiques correspondant exactement à vos besoins. Appelez cet outil plusieurs fois avec des requêtes différentes pour différentes parties de tâches complexes. Exemples de requêtes : « image generation tools », « code review tools », « data analysis », « translation capabilities », etc. Les résultats sont triés par pertinence via la similarité vectorielle.\n\nFlux : smart_route_search → smart_route_describe (détails des paramètres) → smart_route_call (exécution)\n\nServeurs disponibles : {servers_list}",
        (true, "tr") => "3 ADIMLI akışın 1. ADIMI: {scope_description} içinde ilgili araçları keşfetmek ve aramak için ÖNCE BU ARACI kullanın. Yalnızca araç adlarını ve açıklamalarını döndürür — çağırmadan önce tam parametre ayrıntıları için smart_route_describe kullanın.\n\nEn iyi sonuçlar için ihtiyaçlarınıza tam olarak uyan özel sorgular kullanın. Karmaşık görevlerin farklı bölümleri için farklı sorgularla birden çok kez çağırın. Örnek sorgular: \"image generation tools\", \"code review tools\", \"data analysis\", \"translation capabilities\" vb. Sonuçlar vektör benzerliğiyle ilgi düzeyine göre sıralanır.\n\nİş akışı: smart_route_search → smart_route_describe (parametre ayrıntıları) → smart_route_call (yürütme)\n\nKullanılabilir sunucular: {servers_list}",
        (true, "vi") => "BƯỚC 1 trong 3 bước: dùng công cụ NÀY TRƯỚC để khám phá và tìm kiếm công cụ liên quan trong {scope_description}. Chỉ trả về tên và mô tả công cụ — dùng smart_route_describe để lấy chi tiết tham số đầy đủ trước khi gọi.\n\nĐể có kết quả tốt nhất, hãy dùng truy vấn cụ thể khớp chính xác nhu cầu của bạn. Gọi công cụ này nhiều lần với các truy vấn khác nhau cho các phần khác nhau của tác vụ phức tạp. Ví dụ: \"image generation tools\", \"code review tools\", \"data analysis\", \"translation capabilities\"... Kết quả được sắp xếp theo độ liên quan bằng tương đồng vectơ.\n\nQuy trình: smart_route_search → smart_route_describe (chi tiết tham số) → smart_route_call (thực thi)\n\nMáy chủ khả dụng: {servers_list}",
        (true, "es") => "PASO 1 de 3: use ESTA herramienta PRIMERO para descubrir y buscar herramientas relevantes en {scope_description}. Devuelve solo nombres y descripciones de herramientas: use smart_route_describe para obtener los detalles completos de parámetros antes de invocar.\n\nPara mejores resultados, use consultas específicas que coincidan exactamente con sus necesidades. Llame a esta herramienta varias veces con consultas diferentes para distintas partes de tareas complejas. Ejemplos: \"image generation tools\", \"code review tools\", \"data analysis\", \"translation capabilities\", etc. Los resultados se ordenan por relevancia mediante similitud vectorial.\n\nFlujo: smart_route_search → smart_route_describe (detalles de parámetros) → smart_route_call (ejecución)\n\nServidores disponibles: {servers_list}",
        (false, "zh") => "第 2 步流程的第 1 步：最先使用本工具在 {scope_description} 中发现并搜索相关工具。本工具与 smart_route_call 协同构成两步流程：1）smart_route_search 找到所需工具，2）smart_route_call 执行它。\n\n为获得最佳结果，请使用与需求精确匹配的具体查询；复杂任务的不同部分请用不同查询多次调用。示例查询：\"image generation tools\"、\"code review tools\"、\"data analysis\"、\"translation capabilities\" 等。结果按向量相似度排序。\n\n找到相关工具后，必须用 smart_route_call 实际执行——smart_route_search 只负责查找，不执行。{schema_note}\n\n可用服务器：{servers_list}",
        (false, "fr") => "ÉTAPE 1 sur 2 : utilisez CET outil EN PREMIER pour découvrir et rechercher des outils pertinents dans {scope_description}. Cet outil et smart_route_call fonctionnent ensemble en deux étapes : 1) smart_route_search pour trouver ce qu'il vous faut, 2) smart_route_call pour l'exécuter.\n\nPour de meilleurs résultats, utilisez des requêtes spécifiques correspondant exactement à vos besoins. Appelez cet outil plusieurs fois avec des requêtes différentes pour différentes parties de tâches complexes. Exemples : « image generation tools », « code review tools », « data analysis », « translation capabilities », etc. Les résultats sont triés par pertinence via la similarité vectorielle.\n\nAprès avoir trouvé les outils pertinents, vous DEVEZ utiliser smart_route_call pour les exécuter. smart_route_search ne fait que chercher — il n'exécute pas.{schema_note}\n\nServeurs disponibles : {servers_list}",
        (false, "tr") => "2 ADIMLI akışın 1. ADIMI: {scope_description} içinde ilgili araçları keşfetmek ve aramak için ÖNCE BU ARACI kullanın. Bu araç ve smart_route_call iki adımlık bir süreçte birlikte çalışır: 1) ihtiyacınızı bulmak için smart_route_search, 2) yürütmek için smart_route_call.\n\nEn iyi sonuçlar için ihtiyaçlarınıza tam olarak uyan özel sorgular kullanın. Karmaşık görevlerin farklı bölümleri için farklı sorgularla birden çok kez çağırın. Örnek sorgular: \"image generation tools\", \"code review tools\", \"data analysis\", \"translation capabilities\" vb. Sonuçlar vektör benzerliğiyle ilgi düzeyine göre sıralanır.\n\nİlgili araçları bulduktan sonra bunları gerçekten yürütmek için smart_route_call KULLANMALISINIZ. smart_route_search yalnızca arar — yürütmez.{schema_note}\n\nKullanılabilir sunucular: {servers_list}",
        (false, "vi") => "BƯỚC 1 trong 2 bước: dùng công cụ NÀY TRƯỚC để khám phá và tìm kiếm công cụ liên quan trong {scope_description}. Công cụ này và smart_route_call phối hợp thành quy trình hai bước: 1) smart_route_search để tìm thứ bạn cần, 2) smart_route_call để thực thi.\n\nĐể có kết quả tốt nhất, hãy dùng truy vấn cụ thể khớp chính xác nhu cầu của bạn. Gọi công cụ này nhiều lần với các truy vấn khác nhau cho các phần khác nhau của tác vụ phức tạp. Ví dụ: \"image generation tools\", \"code review tools\", \"data analysis\", \"translation capabilities\"... Kết quả được sắp xếp theo độ liên quan bằng tương đồng vectơ.\n\nSau khi tìm được công cụ liên quan, BẠN PHẢI dùng smart_route_call để thực thi chúng. smart_route_search chỉ tìm — không thực thi.{schema_note}\n\nMáy chủ khả dụng: {servers_list}",
        (false, "es") => "PASO 1 de 2: use ESTA herramienta PRIMERO para descubrir y buscar herramientas relevantes en {scope_description}. Esta herramienta y smart_route_call trabajan juntas en un proceso de dos pasos: 1) smart_route_search para encontrar lo que necesita, 2) smart_route_call para ejecutarlo.\n\nPara mejores resultados, use consultas específicas que coincidan exactamente con sus necesidades. Llame a esta herramienta varias veces con consultas diferentes para distintas partes de tareas complejas. Ejemplos: \"image generation tools\", \"code review tools\", \"data analysis\", \"translation capabilities\", etc. Los resultados se ordenan por relevancia mediante similitud vectorial.\n\nTras encontrar las herramientas relevantes, DEBE usar smart_route_call para ejecutarlas. smart_route_search solo busca — no ejecuta.{schema_note}\n\nServidores disponibles: {servers_list}",
        _ => "",
    };
    let tmpl = if tmpl.is_empty() {
        // English (default): the exact pre-i18n strings.
        if progressive {
            "STEP 1 of 3: Use this tool FIRST to discover and search for relevant tools across {scope_description}. Returns tool names and descriptions only - use smart_route_describe to get full parameter details before calling.\n\nFor optimal results, use specific queries matching your exact needs. Call this tool multiple times with different queries for different parts of complex tasks. Example queries: \"image generation tools\", \"code review tools\", \"data analysis\", \"translation capabilities\", etc. Results are sorted by relevance using vector similarity.\n\nWorkflow: smart_route_search → smart_route_describe (for parameter details) → smart_route_call (to execute)\n\nAvailable servers: {servers_list}"
        } else {
            "STEP 1 of 2: Use this tool FIRST to discover and search for relevant tools across {scope_description}. This tool and smart_route_call work together as a two-step process: 1) smart_route_search to find what you need, 2) smart_route_call to execute it.\n\nFor optimal results, use specific queries matching your exact needs. Call this tool multiple times with different queries for different parts of complex tasks. Example queries: \"image generation tools\", \"code review tools\", \"data analysis\", \"translation capabilities\", etc. Results are sorted by relevance using vector similarity.\n\nAfter finding relevant tools, you MUST use the smart_route_call to actually execute them. The smart_route_search only finds tools - it doesn't execute them.{schema_note}\n\nAvailable servers: {servers_list}"
        }
    } else {
        tmpl
    };
    fill(
        tmpl,
        &[
            ("{scope_description}", scope_description),
            ("{servers_list}", servers_list),
            ("{schema_note}", schema_note),
        ],
    )
}

/// `smart_route_describe` description (progressive = 3-step mode).
pub fn smart_route_describe_desc(lang: &str, progressive: bool) -> &'static str {
    match (progressive, normalize_lang(lang)) {
        (true, "zh") => "第 3 步流程的第 2 步：在 smart_route_search 之后使用本工具获取某工具的完整参数 schema，以便用 smart_route_call 正确调用。\n\n工作流：smart_route_search → smart_route_describe → smart_route_call",
        (true, "fr") => "ÉTAPE 2 sur 3 : utilisez cet outil APRÈS smart_route_search pour obtenir le schéma de paramètres complet d'un outil. Fournit l'inputSchema complet nécessaire pour invoquer correctement l'outil avec smart_route_call.\n\nFlux : smart_route_search → smart_route_describe → smart_route_call",
        (true, "tr") => "3 ADIMLI akışın 2. ADIMI: belirli bir aracın tam parametre şemasını almak için smart_route_search SONRASI bu aracı kullanın. smart_route_call ile aracı doğru çağırmak için gereken eksiksiz inputSchema'yı sağlar.\n\nİş akışı: smart_route_search → smart_route_describe → smart_route_call",
        (true, "vi") => "BƯỚC 2 trong 3 bước: dùng công cụ này SAU smart_route_search để lấy schema tham số đầy đủ của một công cụ cụ thể. Cung cấp inputSchema đầy đủ cần thiết để gọi đúng công cụ bằng smart_route_call.\n\nQuy trình: smart_route_search → smart_route_describe → smart_route_call",
        (true, "es") => "PASO 2 de 3: use esta herramienta DESPUÉS de smart_route_search para obtener el esquema completo de parámetros de una herramienta concreta. Proporciona el inputSchema completo necesario para invocarla correctamente con smart_route_call.\n\nFlujo: smart_route_search → smart_route_describe → smart_route_call",
        (false, "zh") => "用于获取 smart_route_search 返回的某条结果的完整参数 schema（该结果未附 inputSchema 时），在用 smart_route_call 调用前使用。",
        (false, "fr") => "Pour obtenir le schéma de paramètres complet d'un résultat de smart_route_search renvoyé sans inputSchema, avant de l'invoquer avec smart_route_call.",
        (false, "tr") => "smart_route_search sonucu inputSchema olmadan döndüyse, smart_route_call ile çağırmadan önce o sonucun tam parametre şemasını almak için kullanın.",
        (false, "vi") => "Dùng để lấy schema tham số đầy đủ của một kết quả smart_route_search được trả về mà không kèm inputSchema, trước khi gọi bằng smart_route_call.",
        (false, "es") => "Para obtener el esquema completo de parámetros de un resultado de smart_route_search devuelto sin inputSchema, antes de invocarlo con smart_route_call.",
        _ => {
            if progressive {
                "STEP 2 of 3: Use this tool AFTER smart_route_search to get the full parameter schema for a specific tool. This provides the complete inputSchema needed to correctly invoke the tool with smart_route_call.\n\nWorkflow: smart_route_search → smart_route_describe → smart_route_call"
            } else {
                "Use this tool to get the full parameter schema of a smart_route_search result that was returned without an inputSchema, before invoking it with smart_route_call."
            }
        }
    }
}

/// `smart_route_call` description (progressive = 3-step mode).
pub fn smart_route_call_desc(lang: &str, progressive: bool) -> &'static str {
    match (progressive, normalize_lang(lang)) {
        (true, "zh") => "第 3 步流程的第 3 步：在 smart_route_describe 之后使用本工具实际执行找到的工具。这是执行步骤。\n\n工作流：smart_route_search → smart_route_describe → smart_route_call（带上所选工具名与必填参数）。\n\n重要：调用前务必先用 smart_route_describe 获取工具的 inputSchema，确保提供正确参数。",
        (true, "fr") => "ÉTAPE 3 sur 3 : utilisez cet outil APRÈS smart_route_describe pour réellement exécuter/invoquer l'outil trouvé. C'est l'étape d'exécution.\n\nFlux : smart_route_search → smart_route_describe → smart_route_call avec le nom d'outil choisi et les paramètres requis.\n\nIMPORTANT : utilisez toujours smart_route_describe d'abord pour obtenir l'inputSchema de l'outil avant l'invocation, afin de fournir les bons paramètres.",
        (true, "tr") => "3 ADIMLI akışın 3. ADIMI: bulduğunuz aracı gerçekten çalıştırmak/çağırmak için smart_route_search SONRASI değil, smart_route_describe SONRASI bu aracı kullanın. Bu yürütme adımıdır.\n\nİş akışı: smart_route_search → smart_route_describe → seçilen araç adı ve gerekli parametrelerle smart_route_call.\n\nÖNEMLİ: doğru parametreleri sağlamak için çağırmadan önce her zaman smart_route_describe ile aracın inputSchema'sını alın.",
        (true, "vi") => "BƯỚC 3 trong 3 bước: dùng công cụ này SAU smart_route_describe để thực thi/gọi công cụ đã tìm được. Đây là bước thực thi.\n\nQuy trình: smart_route_search → smart_route_describe → smart_route_call với tên công cụ đã chọn và các tham số bắt buộc.\n\nQUAN TRỌNG: luôn dùng smart_route_describe trước để lấy inputSchema của công cụ trước khi gọi, đảm bảo cung cấp đúng tham số.",
        (true, "es") => "PASO 3 de 3: use esta herramienta DESPUÉS de smart_route_describe para ejecutar/invocar realmente la herramienta encontrada. Este es el paso de ejecución.\n\nFlujo: smart_route_search → smart_route_describe → smart_route_call con el nombre de herramienta elegido y los parámetros requeridos.\n\nIMPORTANTE: use siempre smart_route_describe antes para obtener el inputSchema de la herramienta y asegurarse de proporcionar los argumentos correctos.",
        (false, "zh") => "第 2 步流程的第 2 步：在 smart_route_search 之后使用本工具实际执行找到的工具——smart_route_search 只负责查找，smart_route_call 负责运行。\n\n工作流：smart_route_search → 查看结果 → smart_route_call（带上所选工具名与必填参数）。\n\n重要：调用前务必核对 smart_route_search 结果中的工具 inputSchema，确保提供正确参数；搜索结果会明确给出每个工具需要的参数。",
        (false, "fr") => "ÉTAPE 2 sur 2 : utilisez cet outil APRÈS smart_route_search pour réellement exécuter/invoquer l'outil trouvé. smart_route_search trouve les outils, smart_route_call les exécute.\n\nFlux : smart_route_search → examiner les résultats → smart_route_call avec le nom d'outil choisi et les paramètres requis.\n\nIMPORTANT : vérifiez toujours l'inputSchema de l'outil dans les résultats de smart_route_search avant l'invocation pour fournir les bons paramètres. Les résultats de recherche montrent exactement quels paramètres chaque outil attend.",
        (false, "tr") => "2 ADIMLI akışın 2. ADIMI: bulduğunuz aracı gerçekten çalıştırmak/çağırmak için smart_route_search SONRASI bu aracı kullanın - smart_route_search araçları bulur, smart_route_call çalıştırır.\n\nİş akışı: smart_route_search → sonuçları inceleyin → seçilen araç adı ve gerekli parametrelerle smart_route_call.\n\nÖNEMLİ: doğru parametreleri sağlamak için çağırmadan önce smart_route_search sonuçlarındaki aracın inputSchema'sını her zaman kontrol edin. Arama sonuçları her aracın hangi parametreleri istediğini tam olarak gösterir.",
        (false, "vi") => "BƯỚC 2 trong 2 bước: dùng công cụ này SAU smart_route_search để thực thi/gọi công cụ đã tìm được — smart_route_search chỉ tìm công cụ, smart_route_call chạy chúng.\n\nQuy trình: smart_route_search → xem kết quả → smart_route_call với tên công cụ đã chọn và các tham số bắt buộc.\n\nQUAN TRỌNG: luôn kiểm tra inputSchema của công cụ trong kết quả smart_route_search trước khi gọi để cung cấp đúng tham số. Kết quả tìm kiếm cho biết chính xác mỗi công cụ cần tham số nào.",
        (false, "es") => "PASO 2 de 2: use esta herramienta DESPUÉS de smart_route_search para ejecutar/invocar realmente la herramienta encontrada: smart_route_search encuentra herramientas, smart_route_call las ejecuta.\n\nFlujo: smart_route_search → examinar resultados → smart_route_call con el nombre de herramienta elegido y los parámetros requeridos.\n\nIMPORTANTE: compruebe siempre el inputSchema de la herramienta en los resultados de smart_route_search antes de invocar para proporcionar los argumentos correctos. Los resultados de búsqueda muestran exactamente qué parámetros espera cada herramienta.",
        _ => {
            if progressive {
                "STEP 3 of 3: Use this tool AFTER smart_route_describe to actually execute/invoke any tool you found. This is the execution step.\n\nWorkflow: smart_route_search → smart_route_describe → smart_route_call with the chosen tool name and required arguments.\n\nIMPORTANT: Always use smart_route_describe first to get the tool's inputSchema before invoking to ensure you provide the correct arguments."
            } else {
                "STEP 2 of 2: Use this tool AFTER smart_route_search to actually execute/invoke any tool you found. This is the execution step - smart_route_search finds tools, smart_route_call runs them.\n\nWorkflow: smart_route_search → examine results → smart_route_call with the chosen tool name and required arguments.\n\nIMPORTANT: Always check the tool's inputSchema from smart_route_search results before invoking to ensure you provide the correct arguments. The search results will show you exactly what parameters each tool expects."
            }
        }
    }
}

/// Annotation titles for the smart meta tools.
pub fn smart_tool_title(kind: &str, lang: &str) -> &'static str {
    match (kind, normalize_lang(lang)) {
        ("search", "zh") => "搜索工具",
        ("describe", "zh") => "描述工具",
        ("call", "zh") => "调用工具",
        ("search", "fr") => "Rechercher des outils",
        ("describe", "fr") => "Décrire l'outil",
        ("call", "fr") => "Appeler l'outil",
        ("search", "tr") => "Araçları Ara",
        ("describe", "tr") => "Aracı Tanımla",
        ("call", "tr") => "Aracı Çağır",
        ("search", "vi") => "Tìm công cụ",
        ("describe", "vi") => "Mô tả công cụ",
        ("call", "vi") => "Gọi công cụ",
        ("search", "es") => "Buscar herramientas",
        ("describe", "es") => "Describir herramienta",
        ("call", "es") => "Invocar herramienta",
        ("search", _) => "Search Tools",
        ("describe", _) => "Describe Tool",
        ("call", _) => "Call Tool",
        _ => "",
    }
}


// ── inputSchema property descriptions ────────────────────────────────────────

/// Property (inputSchema) description, localized like the tool-level one.
/// Keyed by the exact English text's stable id; unknown keys return "".
pub fn prop_description(key: &str, lang: &str) -> &'static str {
    let en = match key {
        "query_simple" => "The search query.",
        "tags_filter" => "Optional tag filter: only return documents that have at least one of these tags.",
        "doc_id_plain" => "The document id.",
        "search_key" => "Optional substring filters; returns tags matching any of them.",
        "doc_name_create" => "Document name (without extension, e.g. \"readme\"). Path separators are sanitized to dashes.",
        "doc_type_create" => "File type as a bare extension without dot, e.g. \"md\", \"java\", \"py\", \"txt\".",
        "doc_content_create" => "Full text content of the document (UTF-8). Encoding detection is skipped - the input must already be valid UTF-8.",
        "tags_attach" => "Optional tags to attach to the document (used for tag-filtered searches).",
        "doc_id_create_or_search" => "The document id (as returned by rag_file_create or rag_search).",
        "doc_name_new" => "New document name (optional).",
        "doc_type_new" => "New file type as a bare extension, e.g. \"md\" (optional).",
        "doc_content_new" => "New text content (optional). When set, docContentAppend controls append vs replace.",
        "doc_content_append" => "Only meaningful when docContent is set. true = append to existing content; false (default) = replace. Ignored if docContent is omitted.",
        "update_external_file" => "Write-through to the EXTERNAL source file. Default false. FUNCTION: when true, the new text content is additionally written to the file the document was imported from (original_path), keeping the source in sync. SCOPE: (1) symlink-imported docs — read-only via this tool by default; this flag unlocks overwriting the linked original. (2) copy-imported docs — the external source file is overwritten too, so a later source refresh will not revert MCP-written content. Docs without an external source (MCP-created, lost original) are unaffected — only the local copy changes. IMPACT/RISKS: text sources (.txt/.md/code) are replaced with the new text as-is; binary sources (PDF/Office/image) are REPLACED by plain text — the binary content is destroyed irreversibly. A history snapshot of the pre-update content is always taken first and is recoverable via rag_file_history_versions/rag_file_history_get. When false (default): symlink docs reject content updates; copy docs only modify the local copy inside rag/files.",
        "tags_add" => "Optional tags to add to the document (case-insensitive dedup; preserves existing order).",
        "tags_remove" => "Optional tags to remove from the document (case-insensitive match).",
        "history_version_read" => "The history version number to read.",
        "from_version" => "The older history version number (old side of the diff).",
        "to_version" => "The newer history version number (optional; omit to compare against the current content).",
        "doc_id_a" => "First document id (old side).",
        "doc_id_b" => "Second document id (new side).",
        "decision_type" => "Question type: choice (pick one), score (rate each), noul (choice with a no-answer option pair [false, true]).",
        "decision_state" => "Context the decision is grounded in (text or JSON string).",
        "decision_question" => "The question to decide.",
        "decision_options" => "2-20 non-empty candidate descriptions.",
        "smart_query" => "The search query to find relevant tools. Be specific and descriptive about the task you want to accomplish.",
        "smart_limit" => "Maximum number of results to return. Use higher values (20-30) for broad searches and lower values (5-10) for specific searches.",
        "smart_tool_name_describe" => "The exact name of the tool to describe (from smart_route_search results)",
        "smart_tool_name_call" => "The exact name of the tool to invoke (from smart_route_search results)",
        "smart_arguments_full" => "The arguments to pass to the tool based on its inputSchema from smart_route_describe (optional if tool requires no arguments)",
        "smart_arguments" => "The arguments to pass to the tool based on its inputSchema (optional if tool requires no arguments)",
        _ => "",
    };
    match (key, normalize_lang(lang)) {
        ("query_simple", "zh") => "搜索查询。",
        ("query_simple", "fr") => "La requête de recherche.",
        ("query_simple", "tr") => "Arama sorgusu.",
        ("query_simple", "vi") => "Truy vấn tìm kiếm.",
        ("query_simple", "es") => "La consulta de búsqueda.",
        ("tags_filter", "zh") => "可选标签过滤：只返回带有其中任意标签的文档。",
        ("tags_filter", "fr") => "Filtre de tags facultatif : ne renvoie que les documents portant au moins l'un de ces tags.",
        ("tags_filter", "tr") => "İsteğe bağlı etiket filtresi: yalnızca bu etiketlerden en az birine sahip belgeleri döndürür.",
        ("tags_filter", "vi") => "Lọc thẻ tùy chọn: chỉ trả về tài liệu có ít nhất một trong các thẻ này.",
        ("tags_filter", "es") => "Filtro de etiquetas opcional: solo devuelve documentos con al menos una de estas etiquetas.",
        ("doc_id_plain", "zh") => "文档 id。",
        ("doc_id_plain", "fr") => "L'identifiant du document.",
        ("doc_id_plain", "tr") => "Belge kimliği.",
        ("doc_id_plain", "vi") => "Id tài liệu.",
        ("doc_id_plain", "es") => "El id del documento.",
        ("search_key", "zh") => "可选的子串过滤器；返回命中任意一个的标签。",
        ("search_key", "fr") => "Filtres de sous-chaîne facultatifs ; renvoie les tags correspondant à l'un d'eux.",
        ("search_key", "tr") => "İsteğe bağlı alt dize filtreleri; bunlardan herhangi biriyle eşleşen etiketleri döndürür.",
        ("search_key", "vi") => "Bộ lọc chuỗi con tùy chọn; trả về thẻ khớp bất kỳ khóa nào.",
        ("search_key", "es") => "Filtros de subcadena opcionales; devuelve las etiquetas que coincidan con cualquiera de ellos.",
        ("doc_name_create", "zh") => "文档名称（不含扩展名，如 \"readme\"）。路径分隔符会被替换为短横线。",
        ("doc_name_create", "fr") => "Nom du document (sans extension, p. ex. \"readme\"). Les séparateurs de chemin sont remplacés par des tirets.",
        ("doc_name_create", "tr") => "Belge adı (uzantı hariç, örn. \"readme\"). Yol ayraçları tire ile değiştirilir.",
        ("doc_name_create", "vi") => "Tên tài liệu (không gồm phần mở rộng, vd. \"readme\"). Ký tự phân cách đường dẫn được thay bằng gạch ngang.",
        ("doc_name_create", "es") => "Nombre del documento (sin extensión, p. ej. \"readme\"). Los separadores de ruta se sustituyen por guiones.",
        ("doc_type_create", "zh") => "文件类型：不带点的裸扩展名，如 \"md\"、\"java\"、\"py\"、\"txt\"。",
        ("doc_type_create", "fr") => "Type de fichier : extension brute sans point, p. ex. \"md\", \"java\", \"py\", \"txt\".",
        ("doc_type_create", "tr") => "Dosya türü: noktasız uzantı, örn. \"md\", \"java\", \"py\", \"txt\".",
        ("doc_type_create", "vi") => "Loại tệp: phần mở rộng không dấu chấm, vd. \"md\", \"java\", \"py\", \"txt\".",
        ("doc_type_create", "es") => "Tipo de archivo: extensión sin punto, p. ej. \"md\", \"java\", \"py\", \"txt\".",
        ("doc_content_create", "zh") => "文档的完整文本内容（UTF-8）。跳过编码检测——输入必须已是合法 UTF-8。",
        ("doc_content_create", "fr") => "Contenu textuel complet du document (UTF-8). La détection d'encodage est ignorée — l'entrée doit déjà être un UTF-8 valide.",
        ("doc_content_create", "tr") => "Belgenin tam metin içeriği (UTF-8). Kodlama algılama atlanır — girdi zaten geçerli UTF-8 olmalıdır.",
        ("doc_content_create", "vi") => "Toàn bộ nội dung văn bản của tài liệu (UTF-8). Bỏ qua phát hiện mã hóa — dữ liệu đầu vào phải đã là UTF-8 hợp lệ.",
        ("doc_content_create", "es") => "Contenido textual completo del documento (UTF-8). Se omite la detección de codificación: la entrada ya debe ser UTF-8 válido.",
        ("tags_attach", "zh") => "可选标签：附加到文档（用于标签过滤搜索）。",
        ("tags_attach", "fr") => "Tags facultatifs à attacher au document (utilisés pour les recherches filtrées par tag).",
        ("tags_attach", "tr") => "Belgeye eklenecek isteğe bağlı etiketler (etiket filtreli aramalar için kullanılır).",
        ("tags_attach", "vi") => "Thẻ tùy chọn gắn vào tài liệu (dùng cho tìm kiếm lọc theo thẻ).",
        ("tags_attach", "es") => "Etiquetas opcionales para adjuntar al documento (usadas en búsquedas filtradas por etiqueta).",
        ("doc_id_create_or_search", "zh") => "文档 id（rag_file_create 或 rag_search 返回）。",
        ("doc_id_create_or_search", "fr") => "L'identifiant du document (renvoyé par rag_file_create ou rag_search).",
        ("doc_id_create_or_search", "tr") => "Belge kimliği (rag_file_create veya rag_search'in döndürdüğü).",
        ("doc_id_create_or_search", "vi") => "Id tài liệu (do rag_file_create hoặc rag_search trả về).",
        ("doc_id_create_or_search", "es") => "El id del documento (devuelto por rag_file_create o rag_search).",
        ("doc_name_new", "zh") => "新文档名称（可选）。",
        ("doc_name_new", "fr") => "Nouveau nom de document (facultatif).",
        ("doc_name_new", "tr") => "Yeni belge adı (isteğe bağlı).",
        ("doc_name_new", "vi") => "Tên tài liệu mới (tùy chọn).",
        ("doc_name_new", "es") => "Nuevo nombre del documento (opcional).",
        ("doc_type_new", "zh") => "新文件类型：不带点的裸扩展名，如 \"md\"（可选）。",
        ("doc_type_new", "fr") => "Nouveau type de fichier : extension brute sans point, p. ex. \"md\" (facultatif).",
        ("doc_type_new", "tr") => "Yeni dosya türü: noktasız uzantı, örn. \"md\" (isteğe bağlı).",
        ("doc_type_new", "vi") => "Loại tệp mới: phần mở rộng không dấu chấm, vd. \"md\" (tùy chọn).",
        ("doc_type_new", "es") => "Nuevo tipo de archivo: extensión sin punto, p. ej. \"md\" (opcional).",
        ("doc_content_new", "zh") => "新文本内容（可选）。设置时由 docContentAppend 决定追加还是替换。",
        ("doc_content_new", "fr") => "Nouveau contenu textuel (facultatif). Quand il est fourni, docContentAppend décide d'ajouter ou de remplacer.",
        ("doc_content_new", "tr") => "Yeni metin içeriği (isteğe bağlı). Verildiğinde docContentAppend ekleme mi değiştirme mi olduğunu belirler.",
        ("doc_content_new", "vi") => "Nội dung văn bản mới (tùy chọn). Khi cung cấp, docContentAppend quyết định nối thêm hay thay thế.",
        ("doc_content_new", "es") => "Nuevo contenido textual (opcional). Cuando se proporciona, docContentAppend decide añadir o reemplazar.",
        ("doc_content_append", "zh") => "仅在设置了 docContent 时有意义。true = 追加到原内容；false（默认）= 替换。未传 docContent 时忽略。",
        ("doc_content_append", "fr") => "Pertinent uniquement quand docContent est défini. true = ajouter au contenu existant ; false (défaut) = remplacer. Ignoré si docContent est omis.",
        ("doc_content_append", "tr") => "Yalnız docContent ayarlandığında anlamlıdır. true = mevcut içeriğe ekle; false (varsayılan) = değiştir. docContent verilmezse yok sayılır.",
        ("doc_content_append", "vi") => "Chỉ có ý nghĩa khi có docContent. true = nối thêm vào nội dung; false (mặc định) = thay thế. Bỏ qua nếu thiếu docContent.",
        ("doc_content_append", "es") => "Solo tiene sentido cuando se define docContent. true = añadir al contenido existente; false (por defecto) = reemplazar. Se ignora si se omite docContent.",
        ("update_external_file", "zh") => "写穿到外部源文件。默认 false。作用：为 true 时，新文本内容会额外写入文档导入时的源文件（original_path），保持源同步。范围：（1）symlink 导入的文档——本工具默认只读，此开关解锁对所链接原文件的覆盖；（2）copy 导入的文档——外部源文件也会被覆盖，之后的数据源刷新不会回退 MCP 写入的内容。无外部源的文档（MCP 创建、原文件丢失）不受影响——只改本地副本。影响/风险：文本源（.txt/.md/代码）按新文本原样替换；二进制源（PDF/Office/图片）会被替换为纯文本——二进制内容不可逆销毁。覆盖前始终先保存历史快照，可通过 rag_file_history_versions/rag_file_history_get 找回。为 false（默认）时：symlink 文档拒绝内容更新；copy 文档只修改 rag/files 内的本地副本。",
        ("update_external_file", "fr") => "Écriture directe dans le fichier source EXTERNE. Défaut false. FONCTION : quand true, le nouveau contenu textuel est aussi écrit dans le fichier d'importation du document (original_path), gardant la source synchronisée. PORTÉE : (1) documents importés en symlink — lecture seule par défaut ; ce drapeau débloque l'écrasement de l'original lié. (2) documents importés en copie — le fichier source externe est aussi écrasé, un rafraîchissement ultérieur de la source n'écrasera pas le contenu écrit par MCP. Les documents sans source externe (créés par MCP, original perdu) ne sont pas affectés — seule la copie locale change. IMPACTS/RISQUES : les sources texte (.txt/.md/code) sont remplacées par le nouveau texte tel quel ; les sources binaires (PDF/Office/image) sont REMPLACÉES par du texte brut — le contenu binaire est détruit irréversiblement. Un instantané historique du contenu avant mise à jour est toujours pris d'abord, récupérable via rag_file_history_versions/rag_file_history_get. Quand false (défaut) : les documents symlink refusent les mises à jour de contenu ; les documents en copie ne modifient que la copie locale dans rag/files.",
        ("update_external_file", "tr") => "DIŞ kaynak dosyasına doğrudan yazma. Varsayılan false. İŞLEV: true olduğunda yeni metin içeriği ayrıca belgenin içe aktarıldığı dosyaya (original_path) yazılır ve kaynak senkron kalır. KAPSAM: (1) symlink ile içe aktarılan belgeler — varsayılan olarak salt okunur; bu bayrak bağlantılı orijinalin üzerine yazılmasını açar. (2) kopya ile içe aktarılan belgeler — dış kaynak dosyası da üzerine yazılır, sonraki bir kaynak yenileme MCP'nin yazdığı içeriği geri almaz. Dış kaynağı olmayan belgeler (MCP oluşturulmuş, orijinal kaybolmuş) etkilenmez — yalnız yerel kopya değişir. ETKİ/RİSKLER: metin kaynakları (.txt/.md/kod) yeni metinle olduğu gibi değiştirilir; ikili kaynaklar (PDF/Ofis/görsel) düz metinle DEĞİŞTİRİLİR — ikili içerik geri alınamaz şekilde yok olur. Güncellemeden önceki içeriğin anlık görüntüsü her zaman önce alınır ve rag_file_history_versions/rag_file_history_get ile kurtarılabilir. false (varsayılan) olduğunda: symlink belgeler içerik güncellemelerini reddeder; kopya belgeler yalnız rag/files içindeki yerel kopyayı değiştirir.",
        ("update_external_file", "vi") => "Ghi thẳng vào tệp nguồn BÊN NGOÀI. Mặc định false. CHỨC NĂNG: khi true, nội dung văn bản mới cũng được ghi vào tệp mà tài liệu được nhập từ đó (original_path), giữ nguồn đồng bộ. PHẠM VI: (1) tài liệu nhập bằng symlink — mặc định chỉ đọc; cờ này mở khả năng ghi đè tệp gốc được liên kết. (2) tài liệu nhập bằng bản sao — tệp nguồn bên ngoài cũng bị ghi đè, lần làm mới nguồn sau sẽ không hoàn tác nội dung MCP đã ghi. Tài liệu không có nguồn bên ngoài (do MCP tạo, mất gốc) không bị ảnh hưởng — chỉ bản cục bộ thay đổi. TÁC ĐỘNG/RỦI RO: nguồn văn bản (.txt/.md/mã) được thay bằng văn bản mới nguyên trạng; nguồn nhị phân (PDF/Office/hình ảnh) bị THAY BẰNG văn bản thuần — nội dung nhị phân bị hủy không thể khôi phục. Ảnh chụp lịch sử của nội dung trước cập nhật luôn được lưu trước, khôi phục qua rag_file_history_versions/rag_file_history_get. Khi false (mặc định): tài liệu symlink từ chối cập nhật nội dung; tài liệu bản sao chỉ sửa bản cục bộ trong rag/files.",
        ("update_external_file", "es") => "Escritura directa en el archivo fuente EXTERNO. Por defecto false. FUNCIÓN: cuando es true, el nuevo contenido textual también se escribe en el archivo del que se importó el documento (original_path), manteniendo la fuente sincronizada. ALCANCE: (1) documentos importados por symlink — de solo lectura por defecto; esta marca desbloquea sobrescribir el original enlazado. (2) documentos importados por copia — el archivo fuente externo también se sobrescribe, por lo que una actualización posterior no revertirá el contenido escrito por MCP. Los documentos sin fuente externa (creados por MCP, original perdido) no se ven afectados: solo cambia la copia local. IMPACTO/RIESGOS: las fuentes de texto (.txt/.md/código) se reemplazan con el nuevo texto tal cual; las fuentes binarias (PDF/Office/imagen) se REEMPLAZAN por texto plano: el contenido binario se destruye de forma irreversible. Siempre se toma primero una instantánea histórica del contenido previo, recuperable vía rag_file_history_versions/rag_file_history_get. Con false (por defecto): los documentos symlink rechazan actualizaciones de contenido; los documentos por copia solo modifican la copia local en rag/files.",
        ("tags_add", "zh") => "可选：为文档添加标签（不区分大小写去重；保留原有顺序）。",
        ("tags_add", "fr") => "Tags facultatifs à ajouter au document (dédup insensible à la casse ; préserve l'ordre existant).",
        ("tags_add", "tr") => "Belgeye eklenecek isteğe bağlı etiketler (büyük/küçük harf duyarsız tekilleştirme; mevcut sırayı korur).",
        ("tags_add", "vi") => "Thẻ tùy chọn thêm vào tài liệu (loại bỏ trùng lặp không phân biệt hoa thường; giữ thứ tự hiện có).",
        ("tags_add", "es") => "Etiquetas opcionales para añadir al documento (deduplicación sin distinción de mayúsculas; conserva el orden existente).",
        ("tags_remove", "zh") => "可选：移除文档的标签（不区分大小写匹配）。",
        ("tags_remove", "fr") => "Tags facultatifs à retirer du document (correspondance insensible à la casse).",
        ("tags_remove", "tr") => "Belgeden kaldırılacak isteğe bağlı etiketler (büyük/küçük harf duyarsız eşleşme).",
        ("tags_remove", "vi") => "Thẻ tùy chọn cần xóa khỏi tài liệu (khớp không phân biệt hoa thường).",
        ("tags_remove", "es") => "Etiquetas opcionales para quitar del documento (coincidencia sin distinción de mayúsculas).",
        ("history_version_read", "zh") => "要读取的历史版本号。",
        ("history_version_read", "fr") => "Le numéro de version historique à lire.",
        ("history_version_read", "tr") => "Okunacak geçmiş sürüm numarası.",
        ("history_version_read", "vi") => "Số phiên bản lịch sử cần đọc.",
        ("history_version_read", "es") => "El número de versión histórica a leer.",
        ("from_version", "zh") => "较旧的历史版本号（diff 的旧方）。",
        ("from_version", "fr") => "Le numéro de version historique le plus ancien (côté ancien du diff).",
        ("from_version", "tr") => "Daha eski geçmiş sürüm numarası (diff'in eski tarafı).",
        ("from_version", "vi") => "Số phiên bản lịch sử cũ hơn (bên cũ của diff).",
        ("from_version", "es") => "El número de versión histórica más antiguo (lado antiguo del diff).",
        ("to_version", "zh") => "较新的历史版本号（可选；省略则与当前内容对比）。",
        ("to_version", "fr") => "Le numéro de version historique le plus récent (facultatif ; omis pour comparer au contenu actuel).",
        ("to_version", "tr") => "Daha yeni geçmiş sürüm numarası (isteğe bağlı; atlanırsa güncel içerikle karşılaştırılır).",
        ("to_version", "vi") => "Số phiên bản lịch sử mới hơn (tùy chọn; bỏ qua để so với nội dung hiện tại).",
        ("to_version", "es") => "El número de versión histórica más reciente (opcional; omítalo para comparar con el contenido actual).",
        ("doc_id_a", "zh") => "第一个文档 id（旧方）。",
        ("doc_id_a", "fr") => "Identifiant du premier document (ancien côté).",
        ("doc_id_a", "tr") => "Birinci belge kimliği (eski taraf).",
        ("doc_id_a", "vi") => "Id tài liệu thứ nhất (bên cũ).",
        ("doc_id_a", "es") => "Id del primer documento (lado antiguo).",
        ("doc_id_b", "zh") => "第二个文档 id（新方）。",
        ("doc_id_b", "fr") => "Identifiant du second document (nouveau côté).",
        ("doc_id_b", "tr") => "İkinci belge kimliği (yeni taraf).",
        ("doc_id_b", "vi") => "Id tài liệu thứ hai (bên mới).",
        ("doc_id_b", "es") => "Id del segundo documento (lado nuevo).",
        ("decision_type", "zh") => "决策类型：choice（选一个）、score（逐项评分）、noul（带无答案选项对 [false, true] 的选择）。",
        ("decision_type", "fr") => "Type de décision : choice (choisir une option), score (évaluer chaque option), noul (choix avec une paire d'options sans réponse [false, true]).",
        ("decision_type", "tr") => "Karar türü: choice (birini seç), score (her biri puanla), noul (yanıtsız seçenek çifti [false, true] içeren seçim).",
        ("decision_type", "vi") => "Loại quyết định: choice (chọn một), score (chấm điểm từng lựa chọn), noul (lựa chọn với cặp tùy chọn không-trả-lời [false, true]).",
        ("decision_type", "es") => "Tipo de decisión: choice (elegir una), score (puntuar cada una), noul (elección con un par de opciones de sin-respuesta [false, true]).",
        ("decision_state", "zh") => "决策所依据的上下文（文本或 JSON 字符串）。",
        ("decision_state", "fr") => "Le contexte sur lequel la décision s'appuie (texte ou chaîne JSON).",
        ("decision_state", "tr") => "Kararın dayandığı bağlam (metin veya JSON dizesi).",
        ("decision_state", "vi") => "Ngữ cảnh mà quyết định dựa vào (chuỗi văn bản hoặc JSON).",
        ("decision_state", "es") => "El contexto en el que se basa la decisión (texto o cadena JSON).",
        ("decision_question", "zh") => "要决策的问题。",
        ("decision_question", "fr") => "La question à trancher.",
        ("decision_question", "tr") => "Karar verilecek soru.",
        ("decision_question", "vi") => "Câu hỏi cần quyết định.",
        ("decision_question", "es") => "La pregunta a decidir.",
        ("decision_options", "zh") => "2-20 个非空候选描述。",
        ("decision_options", "fr") => "2 à 20 descriptions candidates non vides.",
        ("decision_options", "tr") => "2-20 adet boş olmayan aday açıklama.",
        ("decision_options", "vi") => "2-20 mô tả ứng viên không rỗng.",
        ("decision_options", "es") => "De 2 a 20 descripciones candidatas no vacías.",
        ("smart_query", "zh") => "用于查找相关工具的搜索查询。请具体、清楚地描述你想完成的任务。",
        ("smart_query", "fr") => "La requête de recherche pour trouver les outils pertinents. Soyez spécifique et descriptif sur la tâche à accomplir.",
        ("smart_query", "tr") => "İlgili araçları bulmak için arama sorgusu. Yapmak istediğiniz görevi açık ve ayrıntılı biçimde tanımlayın.",
        ("smart_query", "vi") => "Truy vấn tìm kiếm để tìm công cụ liên quan. Hãy cụ thể và mô tả rõ nhiệm vụ bạn muốn hoàn thành.",
        ("smart_query", "es") => "La consulta de búsqueda para encontrar herramientas relevantes. Sea específico y descriptivo sobre la tarea que desea realizar.",
        ("smart_limit", "zh") => "返回的最大结果数。广搜用较大值（20-30），精确搜用较小值（5-10）。",
        ("smart_limit", "fr") => "Nombre maximal de résultats. Valeurs élevées (20-30) pour les recherches larges, faibles (5-10) pour les recherches précises.",
        ("smart_limit", "tr") => "Döndürülecek en fazla sonuç sayısı. Geniş aramalar için yüksek (20-30), özel aramalar için düşük (5-10) değerler kullanın.",
        ("smart_limit", "vi") => "Số kết quả tối đa trả về. Dùng giá trị lớn (20-30) cho tìm kiếm rộng, nhỏ (5-10) cho tìm kiếm cụ thể.",
        ("smart_limit", "es") => "Número máximo de resultados. Use valores altos (20-30) para búsquedas amplias y bajos (5-10) para búsquedas específicas.",
        ("smart_tool_name_describe", "zh") => "要查看详情的工具的确切名称（来自 smart_route_search 结果）",
        ("smart_tool_name_describe", "fr") => "Le nom exact de l'outil à décrire (issu des résultats de smart_route_search)",
        ("smart_tool_name_describe", "tr") => "Tanımlanacak aracın tam adı (smart_route_search sonuçlarından)",
        ("smart_tool_name_describe", "vi") => "Tên chính xác của công cụ cần mô tả (từ kết quả smart_route_search)",
        ("smart_tool_name_describe", "es") => "El nombre exacto de la herramienta a describir (de los resultados de smart_route_search)",
        ("smart_tool_name_call", "zh") => "要调用的工具的确切名称（来自 smart_route_search 结果）",
        ("smart_tool_name_call", "fr") => "Le nom exact de l'outil à invoquer (issu des résultats de smart_route_search)",
        ("smart_tool_name_call", "tr") => "Çağrılacak aracın tam adı (smart_route_search sonuçlarından)",
        ("smart_tool_name_call", "vi") => "Tên chính xác của công cụ cần gọi (từ kết quả smart_route_search)",
        ("smart_tool_name_call", "es") => "El nombre exacto de la herramienta a invocar (de los resultados de smart_route_search)",
        ("smart_arguments_full", "zh") => "根据 smart_route_describe 给出的 inputSchema 传给工具的参数（工具不需要参数时可省略）",
        ("smart_arguments_full", "fr") => "Les paramètres à passer à l'outil selon son inputSchema de smart_route_describe (facultatif si l'outil n'exige aucun paramètre)",
        ("smart_arguments_full", "tr") => "smart_route_describe'in verdiği inputSchema'ya göre araca geçirilecek argümanlar (aracın zorunlu parametresi yoksa isteğe bağlı)",
        ("smart_arguments_full", "vi") => "Tham số truyền cho công cụ dựa trên inputSchema từ smart_route_describe (tùy chọn nếu công cụ không yêu cầu tham số)",
        ("smart_arguments_full", "es") => "Los argumentos que se pasan a la herramienta según su inputSchema de smart_route_describe (opcional si la herramienta no requiere argumentos)",
        ("smart_arguments", "zh") => "根据其 inputSchema 传给工具的参数（工具不需要参数时可省略）",
        ("smart_arguments", "fr") => "Les paramètres à passer à l'outil selon son inputSchema (facultatif si l'outil n'exige aucun paramètre)",
        ("smart_arguments", "tr") => "Aracın inputSchema'sına göre araca geçirilecek argümanlar (aracın zorunlu parametresi yoksa isteğe bağlı)",
        ("smart_arguments", "vi") => "Tham số truyền cho công cụ dựa trên inputSchema của nó (tùy chọn nếu công cụ không yêu cầu tham số)",
        ("smart_arguments", "es") => "Los argumentos que se pasan a la herramienta según su inputSchema (opcional si la herramienta no requiere argumentos)",
        _ => en,
    }
}

/// Serializes tests that mutate OR cross-compare across calls the
/// process-global TOOL_LANG cache (the walk test flips it per language; the
/// decision-service catalog-identity tests read localized output twice).
/// `pub(crate)` so foreign test modules reading localized output across two
/// calls can hold it against the mutators (observed ~2% flake otherwise).
#[cfg(test)]
static LANG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn lang_test_lock() -> std::sync::MutexGuard<'static, ()> {
    match LANG_LOCK.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}


// ── smart result guideline (schema_note) ─────────────────────────────────────

/// `describe_full_schema_top_n` sentence (origin #1234), localized. Shared by
/// the meta-tool description and the search-result guideline. `n > 1`
/// templates carry a `{n}` placeholder.
pub fn schema_note_full_schema_top_n(n: u32, lang: &str) -> String {
    let (en, per_lang): (&str, [(&str, &str); 5]) = match n {
        0 => (
            "Results do not include the inputSchema; use smart_route_describe to get it before calling a tool.",
            [
                ("zh", "结果不包含 inputSchema；调用工具前请先用 smart_route_describe 获取。"),
                ("fr", "Les résultats n'incluent pas l'inputSchema ; utilisez smart_route_describe pour l'obtenir avant d'appeler un outil."),
                ("tr", "Sonuçlar inputSchema içermez; bir aracı çağırmadan önce smart_route_describe ile alın."),
                ("vi", "Kết quả không kèm inputSchema; dùng smart_route_describe để lấy trước khi gọi công cụ."),
                ("es", "Los resultados no incluyen el inputSchema; use smart_route_describe para obtenerlo antes de invocar una herramienta."),
            ],
        ),
        1 => (
            "Only the first result includes the full inputSchema; use smart_route_describe to get it for any other result before calling it.",
            [
                ("zh", "仅第一条结果包含完整 inputSchema；其他结果请先用 smart_route_describe 获取后再调用。"),
                ("fr", "Seul le premier résultat inclut l'inputSchema complet ; utilisez smart_route_describe pour l'obtenir pour tout autre résultat avant de l'appeler."),
                ("tr", "Yalnız ilk sonuç tam inputSchema içerir; diğer sonuçlar için çağırmadan önce smart_route_describe ile alın."),
                ("vi", "Chỉ kết quả đầu tiên kèm inputSchema đầy đủ; với các kết quả khác, dùng smart_route_describe để lấy trước khi gọi."),
                ("es", "Solo el primer resultado incluye el inputSchema completo; use smart_route_describe para obtenerlo para cualquier otro resultado antes de invocarlo."),
            ],
        ),
        _ => (
            "Only the first {n} results include the full inputSchema; use smart_route_describe to get it for any other result before calling it.",
            [
                ("zh", "仅前 {n} 条结果包含完整 inputSchema；其他结果请先用 smart_route_describe 获取后再调用。"),
                ("fr", "Seuls les {n} premiers résultats incluent l'inputSchema complet ; utilisez smart_route_describe pour l'obtenir pour tout autre résultat avant de l'appeler."),
                ("tr", "Yalnız ilk {n} sonuç tam inputSchema içerir; diğer sonuçlar için çağırmadan önce smart_route_describe ile alın."),
                ("vi", "Chỉ {n} kết quả đầu tiên kèm inputSchema đầy đủ; với các kết quả khác, dùng smart_route_describe để lấy trước khi gọi."),
                ("es", "Solo los {n} primeros resultados incluyen el inputSchema completo; use smart_route_describe para obtenerlo para cualquier otro resultado antes de invocarlo."),
            ],
        ),
    };
    let norm = normalize_lang(lang);
    let mut text = en.to_string();
    for (l, t) in per_lang {
        if l == norm {
            text = t.to_string();
            break;
        }
    }
    text.replace("{n}", &n.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_tool_language_matrix() {
        // Explicit wins over ui; auto follows ui; unknown falls back to en.
        assert_eq!(resolve_tool_language("en", "zh"), "en");
        assert_eq!(resolve_tool_language("zh", "en"), "zh");
        assert_eq!(resolve_tool_language("auto", "zh-CN"), "zh");
        assert_eq!(resolve_tool_language("AUTO", "fr"), "fr");
        assert_eq!(resolve_tool_language("auto", "klingon"), "en");
        assert_eq!(resolve_tool_language("klingon", "zh"), "en");
        assert_eq!(resolve_tool_language("", "zh"), "en");
        assert_eq!(resolve_tool_language("auto", ""), "en");
        // Region/case normalization.
        assert_eq!(resolve_tool_language("ZH-cn", "en"), "zh");
        assert_eq!(resolve_tool_language("tr_TR", "en"), "tr");
    }

    #[test]
    fn rag_descriptions_localized_and_en_byte_identical() {
        let names = [
            "rag_search",
            "rag_get",
            "rag_tag_search",
            "rag_file_create",
            "rag_file_update",
            "rag_file_delete",
            "rag_file_history_versions",
            "rag_file_history_get",
            "rag_file_history_diff",
            "rag_file_diff",
        ];
        for name in names {
            let en = rag_tool_description(name, "en");
            assert!(!en.is_empty(), "{name} en description missing");
            for lang in ["zh", "fr", "tr", "vi", "es"] {
                let d = rag_tool_description(name, lang);
                assert!(!d.is_empty(), "{name} {lang} description missing");
                assert_ne!(d, en, "{name} {lang} equals English (not translated)");
            }
            // Unknown language falls back to the English text.
            assert_eq!(rag_tool_description(name, "xx"), en);
        }
        // Guard the exact pre-i18n English string (byte-identical default).
        assert_eq!(
            rag_tool_description("rag_get", "en"),
            "Get the full text content of a RAG document by its id (as returned by rag_search)."
        );
    }

    #[test]
    fn decision_description_localized() {
        let en = decision_tool_description("en");
        for lang in ["zh", "fr", "tr", "vi", "es"] {
            let d = decision_tool_description(lang);
            assert!(!d.is_empty());
            assert_ne!(d, en, "decision {lang} equals English");
        }
        assert_eq!(decision_tool_description("xx"), en);
    }

    #[test]
    fn smart_templates_keep_placeholders_and_localize() {
        for progressive in [true, false] {
            let en = smart_route_search_desc("en", progressive, "{S}", "{L}", "{N}");
            assert!(en.contains("smart_route_search"));
            for lang in ["zh", "fr", "tr", "vi", "es"] {
                let d = smart_route_search_desc(lang, progressive, "{S}", "{L}", "{N}");
                assert_ne!(d, en, "search {lang} prog={progressive} equals English");
                assert!(d.contains("{S}") && d.contains("{L}"), "placeholder lost: {lang}");
                if !progressive {
                    assert!(d.contains("{N}"), "schema_note placeholder lost: {lang}");
                }
            }
            for f in [
                smart_route_describe_desc("en", progressive),
                smart_route_call_desc("en", progressive),
            ] {
                assert!(!f.is_empty());
            }
            for lang in ["zh", "fr", "tr", "vi", "es"] {
                assert_ne!(smart_route_describe_desc(lang, progressive), smart_route_describe_desc("en", progressive));
                assert_ne!(smart_route_call_desc(lang, progressive), smart_route_call_desc("en", progressive));
            }
        }
        // Titles
        assert_eq!(smart_tool_title("search", "en"), "Search Tools");
        assert_eq!(smart_tool_title("search", "zh"), "搜索工具");
    }

    #[test]
    fn set_and_current_roundtrip() {
        let _g = lang_test_lock();
        set_tool_language("zh-CN");
        assert_eq!(current_tool_language(), "zh");
        set_tool_language("xx");
        assert_eq!(current_tool_language(), "en");
        set_tool_language("en");
    }

    #[test]
    fn prop_descriptions_en_pins() {
        // Byte-identity guards on the pre-i18n English texts.
        assert_eq!(prop_description("query_simple", "en"), "The search query.");
        assert!(
            prop_description("update_external_file", "en")
                .starts_with("Write-through to the EXTERNAL source file. Default false.")
        );
        assert_eq!(
            prop_description("doc_id_plain", "fr"),
            "L'identifiant du document."
        );
        assert_eq!(prop_description("no_such_key", "zh"), "");
    }

    /// Build-level invariant: every `description` in every built builtin tool
    /// definition (tool-level AND inputSchema property-level, all surfaces)
    /// must be localized — a non-en language must differ from English
    /// everywhere. An unwired prop left hardcoded in a schema fails here.
    #[test]
    fn every_description_localized_in_all_langs() {
        let _g = lang_test_lock();
        fn walk(v: &serde_json::Value, out: &mut Vec<String>) {
            match v {
                serde_json::Value::Object(m) => {
                    for (k, val) in m {
                        if k == "description" || k == "title" {
                            if let Some(s) = val.as_str() {
                                out.push(s.to_string());
                            }
                        } else {
                            walk(val, out);
                        }
                    }
                }
                serde_json::Value::Array(a) => {
                    for x in a {
                        walk(x, out);
                    }
                }
                _ => {}
            }
        }
        let collect = |lang: &str| -> Vec<String> {
            set_tool_language(lang);
            let mut defs: Vec<serde_json::Value> = crate::rag::service::tool_definitions();
            defs.extend(crate::services::decision_service::decision_tool_definitions());
            defs.extend(crate::smart_routing::meta::build_meta_tools(
                "all servers",
                "a, b",
                true,
                Some(3),
            ));
            defs.extend(crate::smart_routing::meta::build_meta_tools(
                "all servers",
                "a, b",
                false,
                None,
            ));
            // standard + fullSchemaTopN: the ONLY shape that builds the
            // standard-mode smart_route_describe tool (its desc/prop/title
            // call sites would otherwise be unwalked).
            defs.extend(crate::smart_routing::meta::build_meta_tools(
                "all servers",
                "a, b",
                false,
                Some(3),
            ));
            let mut out = Vec::new();
            for d in &defs {
                walk(d, &mut out);
            }
            out
        };
        let en = collect("en");
        assert!(en.len() >= 40, "expected tool+prop descriptions, got {}", en.len());
        for lang in ["zh", "fr", "tr", "vi", "es"] {
            let got = collect(lang);
            assert_eq!(got.len(), en.len(), "{lang}: description count differs");
            for (i, (a, b)) in en.iter().zip(got.iter()).enumerate() {
                assert!(!a.is_empty(), "en: description[{i}] empty (en-arm regression)");
                assert!(!b.is_empty(), "{lang}: description[{i}] empty");
                assert_ne!(a, b, "{lang}: description[{i}] still English (unwired)");
            }
        }
        set_tool_language("en");
    }

    /// FNV-1a 64 — deterministic across platforms/rustc versions, unlike
    /// `DefaultHasher`. Used to pin the exact English bytes of every builtin
    /// description without duplicating the strings in the test.
    fn fnv1a64(bytes: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }

    /// Mechanical alignment: every key in `PROP_KEYS` resolves to a non-empty
    /// text in every `TOOL_LANGS` entry, each non-en text differs from en, and
    /// unknown keys still return "". A key added to the match arms but not to
    /// `PROP_KEYS` shows up as an unmatched arm in the digest walk below; a key
    /// added to `PROP_KEYS` but missing a language arm goes red here.
    #[test]
    fn prop_keys_cover_all_languages() {
        for key in PROP_KEYS {
            let en = prop_description(key, "en");
            assert!(!en.is_empty(), "{key}: en missing");
            for lang in TOOL_LANGS {
                if lang == "en" {
                    continue;
                }
                let d = prop_description(key, lang);
                assert!(!d.is_empty(), "{key} {lang}: missing");
                assert_ne!(d, en, "{key} {lang}: equals English (unwired)");
            }
        }
        // TOOL_LANGS entries all normalize to themselves (table alignment).
        for lang in TOOL_LANGS {
            assert_eq!(normalize_lang(lang), lang, "TOOL_LANGS entry '{lang}' does not normalize to itself");
        }
    }

    /// Byte-pin the FULL English surface: all 31 prop texts + all 10 rag tool
    /// descriptions + the decision text + smart templates/titles, via one FNV
    /// digest. Any English byte change goes red (review the translations then
    /// update the digest deliberately).
    #[test]
    fn en_surface_byte_pinned() {
        let mut buf = Vec::new();
        for key in PROP_KEYS {
            buf.extend_from_slice(prop_description(key, "en").as_bytes());
            buf.push(0x1f);
        }
        for name in RAG_TOOL_KEYS {
            buf.extend_from_slice(rag_tool_description(name, "en").as_bytes());
            buf.push(0x1f);
        }
        buf.extend_from_slice(decision_tool_description("en").as_bytes());
        buf.push(0x1f);
        for progressive in [true, false] {
            buf.extend_from_slice(
                smart_route_search_desc("en", progressive, "{S}", "{L}", "{N}").as_bytes(),
            );
            buf.extend_from_slice(smart_route_describe_desc("en", progressive).as_bytes());
            buf.extend_from_slice(smart_route_call_desc("en", progressive).as_bytes());
            buf.push(0x1f);
        }
        for t in ["search", "describe", "call"] {
            buf.extend_from_slice(smart_tool_title(t, "en").as_bytes());
            buf.push(0x1f);
        }
        for n in [0u32, 1, 7] {
            buf.extend_from_slice(schema_note_full_schema_top_n(n, "en").as_bytes());
            buf.push(0x1f);
        }
        assert_eq!(
            fnv1a64(&buf),
            14481401532452695178,
            "English builtin description surface changed — update translations and re-pin deliberately"
        );
    }

    #[test]
    fn schema_note_localized_all_langs() {
        for n in [0u32, 1, 7] {
            let en = schema_note_full_schema_top_n(n, "en");
            assert!(!en.is_empty());
            // Pin the n-dispatch arms (n=0/1 render distinct texts; a
            // collapsed generic arm must go red).
            match n {
                0 => assert!(en.contains("do not include"), "n=0 dispatch broken: {en}"),
                1 => assert!(en.contains("Only the first result"), "n=1 dispatch broken: {en}"),
                _ => assert!(en.contains(&format!("first {n} results")), "n>1 fill broken: {en}"),
            }
            for lang in ["zh", "fr", "tr", "vi", "es"] {
                let d = schema_note_full_schema_top_n(n, lang);
                assert!(!d.is_empty(), "schema_note({n}) {lang} empty");
                assert_ne!(d, en, "schema_note({n}) {lang} equals English");
                if n > 1 {
                    assert!(d.contains(&n.to_string()), "schema_note({n}) {lang} lost {{n}}");
                }
            }
        }
    }
}
