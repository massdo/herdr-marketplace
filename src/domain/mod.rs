pub mod compat;
pub mod index;
pub mod search;
pub mod source;
pub mod text;
pub mod version;

pub const DEFAULT_INDEX_URL: &str = "https://assets.herdr.dev/plugins/index.json";
pub const INDEX_URL_ENV: &str = "HERDR_MARKETPLACE_INDEX_URL";
