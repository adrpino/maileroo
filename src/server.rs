use crate::fs::create_dir_all_sync_with_permissions;
use crate::{config, db, inbound, outbound, web};
use hickory_resolver::TokioResolver;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct MailerooServer {
    pub state: web::AppState,
    pub smtp_addr: String,
    pub web_addr: String,
    pub tls_acceptor: Option<inbound::acceptor::HotReloadAcceptor>,
    pub storage_dir: PathBuf,
}

/// Pure helper that validates required startup environment variables and returns
/// a plain, log-friendly error message when any are missing.
pub fn validate_required_startup_env(
    database_url: &str,
    srs_secret: &str,
    prod_domain: &str,
) -> Result<(), String> {
    let mut missing_envs = Vec::new();
    if database_url.trim().is_empty() {
        missing_envs.push("DATABASE_URL");
    }
    if srs_secret.trim().is_empty() {
        missing_envs.push("SRS_SECRET");
    }
    if prod_domain.trim().is_empty() {
        missing_envs.push("PROD_DOMAIN (web portal domain, e.g., example.com)");
    }

    if !missing_envs.is_empty() {
        return Err(format!(
            "Configuration error: missing required environment variables: {}. Set these in your environment or .env file.",
            missing_envs.join(", ")
        ));
    }
    Ok(())
}

impl MailerooServer {
    pub async fn bootstrap() -> Result<Self, Box<dyn std::error::Error>> {
        let database_url = config::get_config("DATABASE_URL", "");
        let srs_secret = config::get_config("SRS_SECRET", "");
        let prod_domain = config::get_config("PROD_DOMAIN", "");

        if let Err(msg) = validate_required_startup_env(&database_url, &srs_secret, &prod_domain) {
            tracing::error!("{}", msg);
            return Err(msg.into());
        }

        let smtp_addr = config::get_config("SMTP_ADDR", "0.0.0.0:2525");
        let web_addr = config::get_config("WEB_ADDR", "0.0.0.0:3000");
        let storage_dir = PathBuf::from(config::get_config("STORAGE_DIR", "./storage/emails"));
        let email_retention_days: i64 = config::get_config("EMAIL_RETENTION_DAYS", "30")
            .parse()
            .unwrap_or(30);

        // 1. Parse AppConfig from environment
        let auto_tls_enabled = config::get_config("AUTO_TLS", "false") == "true";
        let auto_tls = if auto_tls_enabled {
            let domain = prod_domain.clone();
            let email = config::get_config("ACME_EMAIL", "");
            let cache_dir = std::path::PathBuf::from(config::get_config(
                "ACME_CACHE_DIR",
                "./storage/certs/acme",
            ));
            let acme_directory = config::get_config(
                "ACME_DIRECTORY",
                "https://acme-v02.api.letsencrypt.org/directory",
            );
            let http_port: u16 = config::get_config("AUTO_TLS_HTTP_PORT", "80")
                .parse()
                .unwrap_or(80);
            let https_port: u16 = config::get_config("AUTO_TLS_HTTPS_PORT", "443")
                .parse()
                .unwrap_or(443);
            Some(config::AutoTlsConfig {
                domain,
                email,
                cache_dir,
                acme_directory,
                http_port,
                https_port,
            })
        } else {
            None
        };

        let app_config = config::AppConfig { auto_tls };

        // 2. Resolve cert paths depending on Auto-TLS mode
        let (cert_path, key_path) = if auto_tls_enabled {
            let cache_dir = config::get_config("ACME_CACHE_DIR", "./storage/certs/acme");
            let domain = prod_domain.clone();
            let acme_dir = config::get_config(
                "ACME_DIRECTORY",
                "https://acme-v02.api.letsencrypt.org/directory",
            );
            let path = web::autotls::resolve_acme_cert_path(
                std::path::Path::new(&cache_dir),
                &domain,
                &acme_dir,
            );
            (path.clone(), path)
        } else {
            let certs_path = config::get_config("CERTS_PATH", ".");
            (
                PathBuf::from(format!("{}/smtp_cert.pem", &certs_path)),
                PathBuf::from(format!("{}/smtp_key.pem", &certs_path)),
            )
        };

        // 3. Initialize HotReloadAcceptor
        let tls_acceptor = if cert_path.exists() && key_path.exists() {
            tracing::info!("Certificates found. Starting with TLS support.");
            Some(inbound::acceptor::HotReloadAcceptor::new(
                cert_path,
                key_path,
                std::time::Duration::from_secs(60),
            )?)
        } else if auto_tls_enabled {
            tracing::warn!(
                "Auto-TLS enabled but certificates not yet retrieved. Starting SMTP with lazy-loading TLS support."
            );
            Some(inbound::acceptor::HotReloadAcceptor::new(
                cert_path,
                key_path,
                std::time::Duration::from_secs(60),
            )?)
        } else {
            tracing::warn!("Certificates not found. Starting in plain mode.");
            None
        };

        // Resolver
        let resolver = TokioResolver::builder_tokio()
            .expect("Failed to create resolver builder")
            .build()
            .expect("Failed to build resolver");
        let dns_scanner = crate::dns::DnsScanner::new(resolver.clone());

        // Ensure storage directory exists
        if !Path::new(&storage_dir).exists() {
            tracing::info!("Creating storage directory: {:?}", storage_dir);
            create_dir_all_sync_with_permissions(&storage_dir)?;
        }

        let identity_domain = prod_domain.clone();
        let db_pool = db::init_pool(&database_url).await?;

        tracing::info!("Checking database schema and running migrations...");
        db::run_migrations(&db_pool).await?;
        tracing::info!("Database is fully migrated and ready.");

        // Validate credentials and seed/verify administrator at startup
        let admin_config = config::AdminConfig::from_env();
        db::seed_admin_on_startup(&db_pool, &admin_config).await?;

        let outbound = Arc::new(outbound::OutboundService::new(
            srs_secret,
            resolver.clone(),
            identity_domain,
            db_pool.clone(),
            storage_dir.clone(),
        ));

        // Start background outbound queue daemon with a 30-second interval
        outbound::queue::start_queue_daemon(
            db_pool.clone(),
            storage_dir.clone(),
            outbound.clone(),
            std::time::Duration::from_secs(30),
        );

        let (tx, _) = tokio::sync::broadcast::channel::<web::DashboardEvent>(100);
        let filter_engine = crate::filter::FilterEngine::new();
        let (backfill_engine, backfill_rx) = crate::filter::BackfillEngine::new();

        crate::filter::BackfillEngine::start_worker(
            backfill_engine.active_tenants(),
            db_pool.clone(),
            storage_dir.clone(),
            backfill_rx,
        );

        let web_state = web::AppState {
            db: db_pool.clone(),
            storage_dir: storage_dir.clone(),
            dns_scanner,
            tx,
            outbound: outbound.clone(),
            config: app_config,
            filter_engine,
            backfill_engine,
        };

        tracing::info!("Starting services...");

        // Spawn background cleanup task
        let cleanup_pool = db_pool.clone();
        let cleanup_storage_dir = storage_dir.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(86400)); // Once a day
            loop {
                interval.tick().await;
                if email_retention_days <= 0 {
                    continue;
                }

                tracing::info!(
                    "Running background cleanup for emails older than {} days",
                    email_retention_days
                );
                match db::delete_old_emails(&cleanup_pool, email_retention_days).await {
                    Ok(body_keys) => {
                        for key in &body_keys {
                            let file_path = cleanup_storage_dir.join(key.to_string());
                            if file_path.exists()
                                && let Err(e) = std::fs::remove_file(&file_path)
                            {
                                tracing::error!(
                                    "Failed to delete email file {:?}: {}",
                                    file_path,
                                    e
                                );
                            }
                        }
                        if !body_keys.is_empty() {
                            tracing::info!("Cleaned up {} old emails", body_keys.len());
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to delete old emails from DB: {}", e);
                    }
                }
            }
        });

        Ok(Self {
            state: web_state,
            smtp_addr,
            web_addr,
            tls_acceptor,
            storage_dir,
        })
    }

    pub async fn get_router(&self) -> axum::Router {
        crate::web::create_app(self.state.clone()).await
    }

    pub async fn start(
        self,
        custom_router: Option<axum::Router>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let smtp_server = crate::inbound::run_server(
            &self.smtp_addr,
            self.state.db.clone(),
            self.storage_dir.clone(),
            self.state.outbound.clone(),
            self.tls_acceptor.clone(),
            self.state.tx.clone(),
            self.state.filter_engine.clone(),
        );

        let web_server = async move {
            let res: Result<(), Box<dyn std::error::Error>> = if let Some(router) = custom_router {
                if let Some(ref auto_tls) = self.state.config.auto_tls {
                    crate::web::autotls::run_auto_tls_web_server(router, auto_tls)
                        .await
                        .map_err(|e| e.into())
                } else if let Some(acceptor) = self.tls_acceptor {
                    use axum_server::tls_rustls::RustlsConfig;
                    let config = RustlsConfig::from_config(
                        acceptor
                            .config()
                            .expect("Manual TLS mode requires active certificates on boot"),
                    );
                    let socket_addr: std::net::SocketAddr = self.web_addr.parse()?;
                    tracing::info!("Web server running at https://{}", self.web_addr);
                    axum_server::bind_rustls(socket_addr, config)
                        .serve(router.into_make_service())
                        .await
                        .map_err(|e| e.into())
                } else {
                    let listener = tokio::net::TcpListener::bind(&self.web_addr).await?;
                    tracing::info!("Web server running at http://{}", self.web_addr);
                    axum::serve(listener, router).await.map_err(|e| e.into())
                }
            } else {
                crate::web::run_web_server(&self.web_addr, self.state, self.tls_acceptor)
                    .await
                    .map_err(|e| e.into())
            };
            res
        };

        tokio::select! {
            res = smtp_server => {
                if let Err(e) = res {
                    tracing::error!("SMTP server error: {}", e);
                }
            }
            res = web_server => {
                if let Err(e) = res {
                    tracing::error!("Web server error: {}", e);
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_required_startup_env() {
        assert!(validate_required_startup_env("sqlite::memory:", "secret", "example.com").is_ok());

        let err = validate_required_startup_env("", "secret", "").unwrap_err();
        assert!(err.contains("DATABASE_URL"));
        assert!(err.contains("PROD_DOMAIN"));
        assert!(!err.contains("SRS_SECRET"));
    }
}
