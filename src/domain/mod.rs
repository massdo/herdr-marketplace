pub mod compat;
pub mod error;
pub mod fiche;
pub mod geometry;
pub mod ids;
pub mod index;
pub mod listing;
pub mod pane;
pub mod registry;
pub mod search;
pub mod sidebar_decision;
pub mod source;
pub mod text;
pub mod version;

pub const PLUGIN_ID: &str = "herdr-marketplace";
pub const SIDEBAR_ENTRYPOINT: &str = "sidebar";
pub const SIDEBAR_TOKEN_KEY: &str = "herdr_marketplace_sidebar";
pub const FICHE_ENTRYPOINT: &str = "fiche";
pub const FICHE_TOKEN_KEY: &str = "herdr_marketplace_fiche";
/// JSON `FicheTarget` handed to a fiche pane.
pub const FICHE_ENV: &str = "HERDR_MARKETPLACE_FICHE";
pub const TOKEN_VALUE: &str = "v1";
pub const EXPLORER_TOKEN_KEY: &str = "herdr-sidebar-explorer";
pub const PREFERRED_OUTER_COLUMNS: u16 = 32;
pub const MIN_SIDEBAR_SHARE: f64 = 0.15;
pub const MAX_SIDEBAR_SHARE: f64 = 0.50;
pub const DEFAULT_INDEX_URL: &str = "https://assets.herdr.dev/plugins/index.json";
pub const INDEX_URL_ENV: &str = "HERDR_MARKETPLACE_INDEX_URL";
