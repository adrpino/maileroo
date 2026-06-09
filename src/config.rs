use std::env;

/// Retrieves a configuration value transparently from the runtime environment.
pub fn get_config(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

#[derive(Clone, Debug)]
pub struct AutoTlsConfig {
    pub domain: String,
    pub email: String,
    pub cache_dir: std::path::PathBuf,
    pub acme_directory: String,
    pub http_port: u16,
    pub https_port: u16,
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub auto_tls: Option<AutoTlsConfig>,
}

#[derive(Clone, Debug)]
pub struct AdminConfig {
    pub email: String,
    pub password: String,
}

impl AdminConfig {
    pub fn from_env() -> Self {
        Self {
            email: get_config("ADMIN_EMAIL", ""),
            password: get_config("ADMIN_PASSWORD", ""),
        }
    }
}
