mod doogat;
mod schema;
pub mod value;

pub use doogat::*;
pub use schema::*;
pub use value::*;

use serde::{Deserialize, Serialize};

/// Repository-level configuration stored in `.ddb.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepoConfig {
    #[serde(default)]
    pub compaction: CompactionConfig,
    #[serde(default)]
    pub crdt: CrdtConfig,
    #[serde(default)]
    pub maintenance: MaintenanceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    /// Days before a non-syncing node is considered stale.
    #[serde(default = "default_stale_ttl_days")]
    pub stale_ttl_days: u32,
    /// CRDT temp cleanup threshold in MB.
    #[serde(default = "default_threshold_mb")]
    pub threshold_mb: u32,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            stale_ttl_days: default_stale_ttl_days(),
            threshold_mb: default_threshold_mb(),
        }
    }
}

fn default_stale_ttl_days() -> u32 {
    90
}
fn default_threshold_mb() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrdtConfig {
    /// Fallback CRDT strategy when typedef doesn't specify one.
    #[serde(default = "default_crdt_strategy")]
    pub default_strategy: String,
}

impl Default for CrdtConfig {
    fn default() -> Self {
        Self {
            default_strategy: default_crdt_strategy(),
        }
    }
}

fn default_crdt_strategy() -> String {
    "preset:default".to_string()
}

fn default_write_threshold() -> u32 {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenanceConfig {
    #[serde(default)]
    pub auto_enabled: bool,
    #[serde(default = "default_write_threshold")]
    pub write_threshold: u32,
}

impl Default for MaintenanceConfig {
    fn default() -> Self {
        Self {
            auto_enabled: false,
            write_threshold: default_write_threshold(),
        }
    }
}
