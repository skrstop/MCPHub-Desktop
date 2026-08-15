// Server status types
export type ServerStatus = 'connecting' | 'connected' | 'disconnected' | 'oauth_required';

// Market server types
export interface MarketServerRepository {
  type: string;
  url: string;
}

export interface MarketServerAuthor {
  name: string;
}

export interface MarketServerInstallation {
  type: string;
  command: string;
  args: string[];
  env?: Record<string, string>;
}

export interface MarketServerArgument {
  description: string;
  required: boolean;
  example: string;
}

export interface MarketServerExample {
  title: string;
  description: string;
  prompt: string;
}

export interface MarketServerTool {
  name: string;
  description: string;
  inputSchema: Record<string, any>;
}

export interface MarketServer {
  name: string;
  display_name: string;
  description: string;
  repository: MarketServerRepository;
  homepage: string;
  author: MarketServerAuthor;
  license: string;
  categories: string[];
  tags: string[];
  examples: MarketServerExample[];
  installations: {
    [key: string]: MarketServerInstallation;
  };
  arguments: Record<string, MarketServerArgument>;
  tools: MarketServerTool[];
  is_official?: boolean;
}

export type ChangelogCategory = 'feature' | 'fix' | 'breaking' | 'security';

export interface ChangelogEntry {
  product: 'mcphub';
  version: string;
  tagName: string;
  publishedAt: string;
  url: string;
  changelogUrl: string;
  title: string;
  summary: string;
  highlights: string[];
  fixes: string[];
  breakingChanges: string[];
  upgradeNotes: string[];
  categories: ChangelogCategory[];
  locale: 'en' | 'zh';
  bodyMarkdown: string;
  isStructured: boolean;
}

export interface ChangelogUpdateInfo {
  latestVersion: string | null;
  hasUpdate: boolean;
  entries: ChangelogEntry[];
  totalUpdateCount: number;
  /** Link to the latest release's changelog (absent on transient states). */
  changelogUrl?: string;
  /** Link to the full changelog list (absent on transient states). */
  allChangelogUrl?: string;
  /** Where the info came from. Besides the API sources, the About dialog
   *  locally uses transient states: 'checking' (check in flight), 'no-update'
   *  (check finished, up to date), 'error' (check failed), and
   *  'tauri-fallback' (constructed from the Tauri updater result without
   *  structured release notes). */
  source:
    | 'mcphub-web'
    | 'npm-fallback'
    | 'disabled'
    | 'checking'
    | 'no-update'
    | 'error'
    | 'tauri-fallback';
}

// Cloud Server types (for MCPRouter API)
export interface CloudServer {
  created_at: string;
  updated_at: string;
  name: string;
  author_name: string;
  title: string;
  description: string;
  content: string;
  server_key: string;
  config_name: string;
  server_url: string;
  tools?: CloudServerTool[];
}

export interface CloudServerTool {
  name: string;
  description: string;
  inputSchema: Record<string, any>;
}

// Tool input schema types
export interface ToolInputSchema {
  type: string;
  properties?: Record<string, unknown>;
  required?: string[];
}

// Tool types
export interface Tool {
  name: string;
  description: string;
  defaultDescription?: string;
  hasDescriptionOverride?: boolean;
  inputSchema: ToolInputSchema;
  annotations?: Record<string, unknown>;
  _meta?: Record<string, unknown>;
  enabled?: boolean;
}

// Prompt types
export interface Prompt {
  name: string;
  title?: string;
  description?: string;
  arguments?: Array<{
    name: string;
    title?: string;
    description?: string;
    required?: boolean;
  }>;
  enabled?: boolean;
}

// Resource types
export interface Resource {
  uri: string;
  name?: string;
  description?: string;
  mimeType?: string;
  annotations?: Record<string, unknown>;
  _meta?: Record<string, unknown>;
  enabled?: boolean;
}

// Built-in prompt argument definition
export interface PromptArgument {
  name: string;
  title?: string;
  description?: string;
  required?: boolean;
}

// Built-in prompt defined via configuration
export interface BuiltinPrompt {
  id: string;
  name: string;
  title?: string;
  description?: string;
  template: string;
  arguments?: PromptArgument[];
  enabled?: boolean;
}

// Built-in resource defined via configuration
export interface BuiltinResource {
  id: string;
  uri: string;
  name?: string;
  description?: string;
  mimeType?: string;
  content: string;
  enabled?: boolean;
}

// --- Skills (技能) ---
// An AI coding agent (e.g. Claude Code, Cursor) with a directory where it
// stores its skills. Configured in Settings → "Agent 安装路径管理" and
// persisted to system_config.config_json.skills.agents.
export interface SkillAgent {
  id: string;
  name: string;
  skillsPath: string;
  /** false = built-in (from install.json catalog, read-only); true = user-added. */
  custom: boolean;
}

// A skill directory discovered under an agent's skillsPath during import
// scanning. name/description come from the skill's SKILL.md frontmatter.
export interface ScannedSkill {
  agentId: string;
  agentName: string;
  dirName: string;
  name: string;
  description: string;
  path: string;
  isSymlink: boolean;
  alreadyImported: boolean;
}

// A single export of a library skill to an agent, with the method used.
export interface SkillExport {
  agentId: string;
  agentName: string;
  method: 'symlink' | 'copy';
  createdAt?: string;
}

// A skill stored in the app's managed skills library (under $APPDATA/skills).
// exports lists which agents this skill has been exported to (for the View dialog).
export interface Skill {
  id: string;
  dirName: string;
  name: string;
  description: string;
  sourceAgent?: string;
  sourcePath?: string;
  createdAt?: string;
  exports: SkillExport[];
}

// Per (skill, agent) result of an export operation.
export interface ExportResultItem {
  skillId: string;
  agentId: string;
  success: boolean;
  message?: string;
}

// A document stored in the RAG library (under $APPDATA/rag/files).
export interface RagDoc {
  id: string;
  name: string;
  size: number;
  content: string;
  uploadedAt?: string;
  tags?: string[];
  /** Number of chunks indexed (0 if indexed before this field existed). */
  chunkCount?: number;
  fileType?: string;
}

// Document metadata for the list view (no content).
export interface RagDocInfo {
  id: string;
  name: string;
  size: number;
  uploadedAt: string;
  tags: string[];
  /** Number of chunks indexed (0 if indexed before this field existed). */
  chunkCount: number;
  /** Display label from file_support.json (ext→name); "" if unknown. */
  fileType: string;
  /** Content version. 1 on first upload, +1 each update. Legacy docs = 1. */
  version: number;
  /** The actual on-disk filename (uuid for uploads, meta.name for
   *  rag_file_create) — shown under the display name so the user can match the
   *  file when its folder is opened (reveal-in-file-manager). */
  fileName: string;
}

// RAG search settings: weights applied to hybrid search scoring.
export interface RagSettings {
  vectorWeight: number;
  keywordWeight: number;
  maxResults: number;
  /** Minimum similarity score (0..1) for a hit to be shown. Default 0. */
  scoreThreshold: number;
  /** Chunk size in tokens. `0` = "auto" — use the loaded model's deploy.json
   * `chunkSize` (else 1024), capped by maxContext. A positive value is an
   * explicit override. Default 0 (auto). */
  chunkSize: number;
  /** Chunk overlap in tokens. `0` = "auto" — use the model's deploy.json
   * `chunkOverlap` (else 100). A positive value is an explicit override. */
  chunkOverlap: number;
}

/** Model context window (tokens), read from the model's config.json. */
export interface RagModelLimits {
  maxContext: number;
  /** Model-author-recommended chunk size (tokens) from deploy.json `chunkSize`.
   *  Undefined when the model omits it (the backend falls back to 1024). Shown
   *  by Auto mode + used to seed the manual sliders. */
  chunkSize?: number;
  /** Model-author-recommended chunk overlap (tokens) from deploy.json
   *  `chunkOverlap`. Undefined when omitted (fallback 100). */
  chunkOverlap?: number;
}

/** A single chunk of a document (for the "view chunks" dialog): its 0-based
 *  index + the text. No embedding is returned (view-only). */
export interface RagChunk {
  chunkIndex: number;
  chunkText: string;
}

// A single search result fragment returned by a similarity search.
export interface RagSearchResult {
  docId: string;
  docName: string;
  title: string;
  snippet: string;
  score: number;
}

// RAG runtime status reported to the switch.
export interface RagStatus {
  enabled: boolean;
  initializing: boolean;
  /** True iff the vector table was recreated on the last enable because the
   *  loaded model's embedding dim differs from the on-disk table (model
   *  swapped). The frontend must trigger `reindexAll` to re-embed docs with
   *  the new model — old embeddings are gone. */
  needsReindex?: boolean;
}

/** One selectable model size (scanned from runtimes/rag/model/<family>/<size>/).
 *  `status`: "ready" (selectable) | "downloadable" (has download.url, fetch
 *  first) | "unavailable". `format`: "gguf" | "" (drives the backend
 *  strategy + a dropdown badge). `fileSize`: model payload bytes (ready) or 0. */
export interface RagModelInfo {
  size: string;
  label: string;
  status: string;
  ready: boolean;
  downloadable: boolean;
  format?: string;
  fileSize?: number;
  description?: string;
  /** True if this size's deploy.json has "default": true (the out-of-box +
   *  fallback model). */
  default?: boolean;
}

// Result of an upload batch.
export interface RagUploadResult {
  successCount: number;
  failureCount: number;
}

// A tag with the number of documents that carry it.
export interface RagTagStat {
  tag: string;
  fileCount: number;
}

// A file picked from the OS file dialog (by path) — the backend reads bytes
// from disk, so large files never go through JSON/base64.
export interface RagPickedFile {
  path: string;
  name: string;
}




// Proxychains4 configuration for STDIO servers (Linux/macOS only)
export interface ProxychainsConfig {
  enabled?: boolean; // Enable/disable proxychains4 proxy routing
  type?: 'socks4' | 'socks5' | 'http'; // Proxy protocol type
  host?: string; // Proxy server hostname or IP address
  port?: number; // Proxy server port
  username?: string; // Proxy authentication username (optional)
  password?: string; // Proxy authentication password (optional)
  configPath?: string; // Path to custom proxychains4 configuration file (optional)
}

// Server config types
export interface ServerConfig {
  type?: 'stdio' | 'sse' | 'streamable-http' | 'openapi' | 'builtin';
  description?: string;
  url?: string;
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  headers?: Record<string, string>;
  passthroughHeaders?: string[];
  enabled?: boolean;
  // Per-server visibility for non-admin users. See issue #817. 'group' reserved.
  visibility?: 'private' | 'group' | 'public';
  enableKeepAlive?: boolean; // Enable remote health checks and automatic reconnect attempts
  keepAliveInterval?: number; // Health check and reconnect interval in milliseconds (default: 60000ms)
  perSessionClient?: boolean; // Create a dedicated upstream client per downstream session instead of sharing one connection (for stateful servers like Playwright)
  // On-demand spawning (stdio only): skip startup connect, spawn the process
  // lazily on the first tool call, and shut it down automatically after the
  // idle timeout. Reduces persistent memory usage for rarely-used servers.
  startOnDemand?: boolean;
  idleTimeoutMs?: number; // Milliseconds of inactivity before shutting down (default: 300000 = 5 min)
  tools?: Record<string, { enabled: boolean; description?: string }>; // Tool-specific configurations with enable/disable state and custom descriptions
  prompts?: Record<string, { enabled: boolean; description?: string }>; // Prompt-specific configurations with enable/disable state and custom descriptions
  options?: {
    timeout?: number; // Request timeout in milliseconds
    resetTimeoutOnProgress?: boolean; // Reset timeout on progress notifications
    maxTotalTimeout?: number; // Maximum total timeout in milliseconds
  }; // MCP request options configuration
  // Proxychains4 proxy configuration for STDIO servers (Linux/macOS only, Windows not supported)
  proxy?: ProxychainsConfig;
  // OAuth authentication for upstream MCP servers
  oauth?: {
    clientId?: string; // OAuth client ID
    clientSecret?: string; // OAuth client secret
    scopes?: string[]; // Required OAuth scopes
    accessToken?: string; // Pre-obtained access token (if available)
    refreshToken?: string; // Refresh token for renewing access
    dynamicRegistration?: {
      enabled?: boolean; // Enable/disable dynamic registration
      issuer?: string; // OAuth issuer URL for discovery
      registrationEndpoint?: string; // Direct registration endpoint URL
      metadata?: {
        client_name?: string;
        client_uri?: string;
        logo_uri?: string;
        scope?: string;
        redirect_uris?: string[];
        grant_types?: string[];
        response_types?: string[];
        token_endpoint_auth_method?: string;
        contacts?: string[];
        software_id?: string;
        software_version?: string;
        [key: string]: any;
      };
      initialAccessToken?: string;
    };
    resource?: string; // OAuth resource parameter (RFC8707)
    authorizationEndpoint?: string; // Authorization endpoint (authorization code flow)
    tokenEndpoint?: string; // Token endpoint for exchanging authorization codes for tokens
    revocationEndpoint?: string; // Token revocation endpoint (RFC 7009)
    pendingAuthorization?: {
      authorizationUrl?: string;
      state?: string;
      codeVerifier?: string;
      createdAt?: number;
    };
  };
  // OpenAPI specific configuration
  openapi?: {
    url?: string; // OpenAPI specification URL
    schema?: Record<string, any>; // Complete OpenAPI JSON schema
    version?: string; // OpenAPI version (default: '3.1.0')
    security?: OpenAPISecurityConfig; // Security configuration for API calls
    passthroughHeaders?: string[]; // Header names to pass through from tool call requests to upstream OpenAPI endpoints
  };
}

// OpenAPI Security Configuration
export interface OpenAPISecurityConfig {
  type: 'none' | 'apiKey' | 'http' | 'oauth2' | 'openIdConnect';
  // API Key authentication
  apiKey?: {
    name: string; // Header/query/cookie name
    in: 'header' | 'query' | 'cookie';
    value: string; // The API key value
  };
  // HTTP authentication (Basic, Bearer, etc.)
  http?: {
    scheme: 'basic' | 'bearer' | 'digest'; // HTTP auth scheme
    bearerFormat?: string; // Bearer token format (e.g., JWT)
    credentials?: string; // Base64 encoded credentials for basic auth or bearer token
  };
  // OAuth2 (simplified - mainly for bearer tokens)
  oauth2?: {
    tokenUrl?: string; // Token endpoint for client credentials flow
    clientId?: string;
    clientSecret?: string;
    scopes?: string[]; // Required scopes
    token?: string; // Pre-obtained access token
    expiresAt?: number; // Access token expiration timestamp in milliseconds
  };
  // OpenID Connect
  openIdConnect?: {
    url: string; // OpenID Connect discovery URL
    clientId?: string;
    clientSecret?: string;
    token?: string; // Pre-obtained ID token
  };
}

// Server types
export interface Server {
  name: string;
  owner?: string;
  visibility?: 'private' | 'group' | 'public';
  status: ServerStatus;
  error?: string;
  /** Running version reported by the MCP handshake (stdio servers). */
  version?: string;
  tools?: Tool[];
  prompts?: Prompt[];
  resources?: Resource[];
  config?: ServerConfig;
  enabled?: boolean;
  oauth?: {
    authorizationUrl?: string;
    state?: string;
    connected?: boolean;
    clientIdConfigured?: boolean; // A static oauth.clientId is set (dynamic registration suppressed)
  };
}

// Group types
// Group server configuration - supports tool selection
export interface IGroupServerConfig {
  name: string; // Server name
  alias?: string; // Optional exposed name for this server within the group
  tools?: string[] | 'all'; // Array of specific tool names to include, or 'all' for all tools (default: 'all')
  prompts?: string[] | 'all'; // Array of specific prompt names to include, or 'all' for all prompts (default: 'all')
  resources?: string[] | 'all'; // Array of specific resource URIs to include, or 'all' for all resources (default: 'all')
}

export interface Group {
  id: string;
  name: string;
  description?: string;
  servers: string[] | IGroupServerConfig[]; // Supports both old and new format
}

// Environment variable types
export interface EnvVar {
  key: string;
  value: string;
}

// Form data types
export interface ServerFormData {
  name: string;
  description?: string;
  url: string;
  command: string;
  arguments: string;
  args?: string[]; // Added explicit args field
  type?: 'stdio' | 'sse' | 'streamable-http' | 'openapi'; // Added type field with openapi support
  env: EnvVar[];
  headers: EnvVar[];
  passthroughHeaders?: string;
  // Visibility for non-admin users. See issue #817. 'group' is reserved.
  visibility?: 'private' | 'group' | 'public';
  options?: {
    timeout?: number;
    resetTimeoutOnProgress?: boolean;
    maxTotalTimeout?: number;
  };
  keepAlive?: {
    enabled?: boolean;
    interval?: number;
  };
  // Create a dedicated upstream client per downstream session (stateful servers)
  perSessionClient?: boolean;
  // On-demand spawning (stdio only): spawn lazily on first tool call, shut down after idle timeout
  startOnDemand?: boolean;
  idleTimeoutMs?: number;
  oauth?: {
    clientId?: string;
    clientSecret?: string;
    scopes?: string;
    accessToken?: string;
    refreshToken?: string;
    authorizationEndpoint?: string;
    tokenEndpoint?: string;
    resource?: string;
  };
  // OpenAPI specific fields
  openapi?: {
    url?: string;
    schema?: string; // JSON schema as string for form input
    inputMode?: 'url' | 'schema'; // Mode to determine input type
    version?: string;
    securityType?: 'none' | 'apiKey' | 'http' | 'oauth2' | 'openIdConnect';
    // API Key fields
    apiKeyName?: string;
    apiKeyIn?: 'header' | 'query' | 'cookie';
    apiKeyValue?: string;
    // HTTP auth fields
    httpScheme?: 'basic' | 'bearer' | 'digest';
    httpCredentials?: string;
    // OAuth2 fields
    oauth2TokenUrl?: string;
    oauth2ClientId?: string;
    oauth2ClientSecret?: string;
    oauth2Token?: string;
    // OpenID Connect fields
    openIdConnectUrl?: string;
    openIdConnectClientId?: string;
    openIdConnectClientSecret?: string;
    openIdConnectToken?: string;
    // Passthrough headers
    passthroughHeaders?: string; // Comma-separated list of header names
  };
}

// Group form data types
export interface GroupFormData {
  name: string;
  description: string;
  servers: string[] | IGroupServerConfig[]; // Updated to support new format
}

// API response types
export interface ApiResponse<T = any> {
  success: boolean;
  message?: string;
  data?: T;
}

// Bearer authentication key configuration (frontend view model)
export type BearerKeyAccessType = 'all' | 'groups' | 'servers' | 'custom';
export type BearerKeyKind = 'system' | 'user';

export interface BearerKey {
  id: string;
  name: string;
  token: string;
  enabled: boolean;
  kind?: BearerKeyKind;
  owner?: string;
  accessType: BearerKeyAccessType;
  allowedGroups?: string[];
  allowedServers?: string[];
}

// Auth types
export interface IUser {
  username: string;
  isAdmin?: boolean;
  permissions?: string[];
}

// User management types
export interface User {
  username: string;
  isAdmin: boolean;
  email?: string;
}

export interface UserFormData {
  username: string;
  password: string;
  isAdmin: boolean;
  email?: string;
}

export interface UserUpdateData {
  isAdmin?: boolean;
  newPassword?: string;
  email?: string;
}

export interface UserStats {
  totalUsers: number;
  adminUsers: number;
  regularUsers: number;
}

export interface AuthState {
  isAuthenticated: boolean;
  user: IUser | null;
  loading: boolean;
  error: string | null;
  skipAuth?: boolean;
}

export interface LoginCredentials {
  username: string;
  password: string;
}

export interface RegisterCredentials extends LoginCredentials {
  isAdmin?: boolean;
}

export interface ChangePasswordCredentials {
  currentPassword: string;
  newPassword: string;
}

export interface AuthResponse {
  success: boolean;
  token?: string;
  user?: IUser;
  message?: string;
  isUsingDefaultPassword?: boolean;
}

// Official Registry types (from registry.modelcontextprotocol.io)
export interface RegistryVariable {
  choices?: string[];
  default?: string;
  description?: string;
  format?: string;
  isRequired?: boolean;
  isSecret?: boolean;
  value?: string;
}

export interface RegistryVariables {
  [key: string]: RegistryVariable;
}

export interface RegistryEnvironmentVariable {
  choices?: string[];
  default?: string;
  description?: string;
  format?: string;
  isRequired?: boolean;
  isSecret?: boolean;
  name: string;
  value?: string;
  variables?: RegistryVariables;
}

export interface RegistryPackageArgument {
  choices?: string[];
  default?: string;
  description?: string;
  format?: string;
  isRepeated?: boolean;
  isRequired?: boolean;
  isSecret?: boolean;
  name: string;
  type?: string;
  value?: string;
  valueHint?: string;
  variables?: RegistryVariables;
}

export interface RegistryTransportHeader {
  choices?: string[];
  default?: string;
  description?: string;
  format?: string;
  isRequired?: boolean;
  isSecret?: boolean;
  name: string;
  value?: string;
  variables?: RegistryVariables;
}

export interface RegistryTransport {
  headers?: RegistryTransportHeader[];
  type: string;
  url?: string;
}

export interface RegistryPackage {
  environmentVariables?: RegistryEnvironmentVariable[];
  fileSha256?: string;
  identifier: string;
  packageArguments?: RegistryPackageArgument[];
  registryBaseUrl?: string;
  registryType: string;
  runtimeArguments?: RegistryPackageArgument[];
  runtimeHint?: string;
  transport?: RegistryTransport;
  version?: string;
}

export interface RegistryRemote {
  headers?: RegistryTransportHeader[];
  type: string;
  url: string;
}

export interface RegistryRepository {
  id?: string;
  source?: string;
  subfolder?: string;
  url?: string;
}

export interface RegistryIcon {
  mimeType: string;
  sizes?: string[];
  src: string;
  theme?: string;
}

export interface RegistryServerData {
  $schema?: string;
  _meta?: {
    'io.modelcontextprotocol.registry/publisher-provided'?: Record<string, any>;
  };
  description: string;
  icons?: RegistryIcon[];
  name: string;
  packages?: RegistryPackage[];
  remotes?: RegistryRemote[];
  repository?: RegistryRepository;
  title: string;
  version: string;
  websiteUrl?: string;
}

export interface RegistryOfficialMeta {
  isLatest?: boolean;
  publishedAt?: string;
  status?: string;
  updatedAt?: string;
}

export interface RegistryServerEntry {
  _meta?: {
    'io.modelcontextprotocol.registry/official'?: RegistryOfficialMeta;
  };
  server: RegistryServerData;
}

export interface RegistryMetadata {
  count: number;
  nextCursor?: string;
}

export interface RegistryServersResponse {
  metadata: RegistryMetadata;
  servers: RegistryServerEntry[];
}

export interface RegistryServerVersionsResponse {
  metadata: RegistryMetadata;
  servers: RegistryServerEntry[];
}

export interface RegistryServerVersionResponse {
  _meta?: {
    'io.modelcontextprotocol.registry/official'?: RegistryOfficialMeta;
  };
  server: RegistryServerData;
}

// Activity types for tool call tracking
export type ActivityStatus = 'success' | 'error';

export interface Activity {
  id: string;
  createdAt: string;
  server: string;
  tool: string;
  duration: number;
  status: ActivityStatus;
  input?: string;
  output?: string;
  group?: string;
  username?: string;
  keyId?: string;
  keyName?: string;
  sourceIp?: string;
  errorMessage?: string;
}

export interface ActivityStats {
  totalCalls: number;
  successCount: number;
  errorCount: number;
  avgDuration: number;
}

export interface ActivityFilter {
  server?: string;
  tool?: string;
  status?: ActivityStatus;
  group?: string;
  username?: string;
  keyId?: string;
  keyName?: string;
  startDate?: string;
  endDate?: string;
}

export interface ActivityFilterOptions {
  servers: string[];
  tools: string[];
  groups: string[];
  usernames: string[];
  keyNames: string[];
}

// Configuration template types for team sharing
export interface ConfigTemplate {
  version: string;
  name: string;
  description?: string;
  createdAt: string;
  servers: Record<string, TemplateServerConfig>;
  groups: TemplateGroup[];
  requiredEnvVars: string[];
}

export interface TemplateServerConfig {
  type?: ServerConfig['type'];
  description?: string;
  url?: string;
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  headers?: Record<string, string>;
  passthroughHeaders?: string[];
  enabled?: boolean;
  enableKeepAlive?: boolean;
  keepAliveInterval?: number;
  tools?: Record<string, { enabled: boolean; description?: string }>;
  prompts?: Record<string, { enabled: boolean; description?: string }>;
  resources?: Record<string, { enabled: boolean; description?: string }>;
  options?: ServerConfig['options'];
  proxy?: ProxychainsConfig;
  oauth?: {
    clientId?: string;
    clientSecret?: string;
    scopes?: string[];
    accessToken?: string;
    refreshToken?: string;
    dynamicRegistration?: {
      enabled?: boolean;
      issuer?: string;
      registrationEndpoint?: string;
      metadata?: {
        client_name?: string;
        client_uri?: string;
        logo_uri?: string;
        scope?: string;
        redirect_uris?: string[];
        grant_types?: string[];
        response_types?: string[];
        token_endpoint_auth_method?: string;
        contacts?: string[];
        software_id?: string;
        software_version?: string;
        [key: string]: any;
      };
      initialAccessToken?: string;
    };
    resource?: string;
    authorizationEndpoint?: string;
    tokenEndpoint?: string;
  };
  openapi?: {
    url?: string;
    schema?: Record<string, any>;
    version?: string;
    security?: OpenAPISecurityConfig;
    passthroughHeaders?: string[];
  };
}

export interface TemplateGroup {
  name: string;
  description?: string;
  servers: IGroupServerConfig[];
}

export interface TemplateImportResult {
  success: boolean;
  serversCreated: number;
  serversSkipped: number;
  groupsCreated: number;
  groupsSkipped: number;
  requiredEnvVars: string[];
  details: TemplateImportDetail[];
}

export interface TemplateImportDetail {
  type: 'server' | 'group';
  name: string;
  action: 'created' | 'skipped' | 'failed';
  message?: string;
}

// Context Footprint cost DTOs
export interface ItemCost {
  kind: 'tool' | 'prompt' | 'resource';
  name: string;
  cost: number;
  enabled: boolean;
}

export interface ServerCost {
  name: string;
  connected: boolean;
  exposed: number;
  gross: number;
  items: ItemCost[];
}

export interface SmartRoutingCost {
  base: number;
  progressiveDisclosure: number;
}

export interface GroupCost {
  id: string;
  name: string;
  connectedCount: number;
  totalCount: number;
  direct: { exposed: number; gross: number };
  smartRouting: SmartRoutingCost | null;
}
