use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub pg_port: u16,
    pub bind: String,
    pub token_file: PathBuf,
    pub maintenance_enabled: bool,
    pub maintenance_interval_secs: u64,
    pub read_pool_size: usize,
}

#[derive(Debug, Deserialize, Default)]
struct FileConfig {
    server: Option<ServerSection>,
    maintenance: Option<MaintenanceSection>,
}

#[derive(Debug, Deserialize, Default)]
struct MaintenanceSection {
    enabled: Option<bool>,
    interval_secs: Option<u64>,
}

#[derive(Debug, Deserialize, Default)]
struct ServerSection {
    port: Option<u16>,
    pg_port: Option<u16>,
    bind: Option<String>,
    token_file: Option<String>,
    read_pool_size: Option<usize>,
}

impl ServerConfig {
    /// Load config from ~/.config/ddb/config.toml, with CLI overrides.
    pub fn load(
        port_override: Option<u16>,
        pg_port_override: Option<u16>,
        bind_override: Option<&str>,
    ) -> Self {
        let config_dir = config_dir();
        let file_cfg = Self::read_file(&config_dir);

        let port = port_override
            .or(file_cfg.server.as_ref().and_then(|s| s.port))
            .unwrap_or(2891);
        let pg_port = pg_port_override
            .or(file_cfg.server.as_ref().and_then(|s| s.pg_port))
            .unwrap_or(2892);
        let bind = bind_override
            .map(String::from)
            .or(file_cfg.server.as_ref().and_then(|s| s.bind.clone()))
            .unwrap_or_else(|| "127.0.0.1".into());
        let token_file = file_cfg
            .server
            .as_ref()
            .and_then(|s| s.token_file.as_ref())
            .map(PathBuf::from)
            .unwrap_or_else(|| config_dir.join("token"));

        let maintenance_enabled = file_cfg
            .maintenance
            .as_ref()
            .and_then(|m| m.enabled)
            .unwrap_or(true);
        let maintenance_interval_secs = file_cfg
            .maintenance
            .as_ref()
            .and_then(|m| m.interval_secs)
            .unwrap_or(3600)
            .max(60);

        let read_pool_size = file_cfg
            .server
            .as_ref()
            .and_then(|s| s.read_pool_size)
            .unwrap_or_else(crate::read_pool::ReadPool::default_pool_size);

        Self {
            port,
            pg_port,
            bind,
            token_file,
            maintenance_enabled,
            maintenance_interval_secs,
            read_pool_size,
        }
    }

    fn read_file(config_dir: &std::path::Path) -> FileConfig {
        let path = config_dir.join("config.toml");
        let content = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return FileConfig::default(),
            Err(e) => {
                tracing::warn!("failed to read config {}: {e}", path.display());
                return FileConfig::default();
            }
        };
        match toml::from_str(&content) {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::warn!("failed to parse config {}: {e}", path.display());
                FileConfig::default()
            }
        }
    }
}

fn config_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config/ddb")
}
