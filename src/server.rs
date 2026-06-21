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

impl MailerooServer {
    pub async fn bootstrap() -> Result<Self, Box<dyn std::error::Error>> {
        let database_url = config::get_config("DATABASE_URL", "");
        let srs_secret = config::get_config("SRS_SECRET", "");
        let prod_domain = config::get_config("PROD_DOMAIN", "");

        // Runtime validation of required configuration
        let mut missing_envs = Vec::new();
        if database_url.is_empty() {
            missing_envs.push("DATABASE_URL");
        }
        if srs_secret.is_empty() {
            missing_envs.push("SRS_SECRET");
        }
        if prod_domain.is_empty() {
            missing_envs.push("PROD_DOMAIN (This is your web portal domain, e.g., example.com)");
        }

        if !missing_envs.is_empty() {
            eprintln!("❌ Configuration Error: Missing required environment variables:");
            for env in missing_envs {
                eprintln!("   - {}", env);
            }
            eprintln!(
                "\nPlease set these in your environment or in a .env file next to the binary."
            );
            std::process::exit(1);
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
            println!("🔒 Certificates found. Starting with TLS support.");
            Some(inbound::acceptor::HotReloadAcceptor::new(
                cert_path,
                key_path,
                std::time::Duration::from_secs(60),
            )?)
        } else if auto_tls_enabled {
            println!(
                "⚠️ Auto-TLS enabled but certificates not yet retrieved. Starting SMTP with lazy-loading TLS support."
            );
            Some(inbound::acceptor::HotReloadAcceptor::new(
                cert_path,
                key_path,
                std::time::Duration::from_secs(60),
            )?)
        } else {
            println!("⚠️ Certificates not found. Starting in plain mode.");
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
            println!("Creating storage directory: {:?}", storage_dir);
            create_dir_all_sync_with_permissions(&storage_dir)?;
        }

        let identity_domain = prod_domain.clone();
        let db_pool = db::init_pool(&database_url).await?;

        println!("🚀 Checking database schema and running migrations...");
        db::run_migrations(&db_pool).await?;
        println!("✅ Database is fully migrated and ready!");

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

        let web_state = web::AppState {
            db: db_pool.clone(),
            storage_dir: storage_dir.clone(),
            dns_scanner,
            tx,
            outbound: outbound.clone(),
            config: app_config,
        };

        println!("Starting services...");

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
                            if file_path.exists() {
                                if let Err(e) = std::fs::remove_file(&file_path) {
                                    tracing::error!(
                                        "Failed to delete email file {:?}: {}",
                                        file_path,
                                        e
                                    );
                                }
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
                    println!("🚀 Web server running at https://{}", self.web_addr);
                    axum_server::bind_rustls(socket_addr, config)
                        .serve(router.into_make_service())
                        .await
                        .map_err(|e| e.into())
                } else {
                    let listener = tokio::net::TcpListener::bind(&self.web_addr).await?;
                    println!("🚀 Web server running at http://{}", self.web_addr);
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
                    eprintln!("SMTP server error: {}", e);
                }
            }
            res = web_server => {
                if let Err(e) = res {
                    eprintln!("Web server error: {}", e);
                }
            }
        }

        Ok(())
    }
}
