pub const APP_NAME: &str = "memini";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const OPENAI_KEY_VAR: &str = "openai_api_key";
pub const ACTIVE_MCP_VAR: &str = "active_mcp";

pub const DEFAULT_RUN_ID: &str = "memini";
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-4o-mini";
pub const DEFAULT_OPENAI_EMBED_MODEL: &str = "text-embedding-3-small";
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

pub const MAX_TOOL_LOOPS: usize = 6;
pub const DEFAULT_MEMORY_LIMIT: u64 = 6;
pub const MAX_LOGS: usize = 1000;
