use arc_swap::ArcSwap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::sleep;
use tokio_rustls::{TlsAcceptor, rustls::ServerConfig, server::TlsStream};

#[derive(Clone)]
pub struct HotReloadAcceptor {
    inner: Arc<ArcSwap<Option<ServerConfig>>>,
}

impl HotReloadAcceptor {
    // method for Web server to get the current config
    pub fn config(&self) -> Option<Arc<ServerConfig>> {
        let loaded = self.inner.load_full();
        (*loaded).clone().map(Arc::new)
    }

    fn load_config(
        cert_path: &Path,
        key_path: &Path,
    ) -> Result<Option<ServerConfig>, Box<dyn std::error::Error>> {
        use std::fs::File;
        use std::io::BufReader;

        // Check if the requested files exist on disk
        if !cert_path.exists() || !key_path.exists() {
            return Ok(None);
        }

        let cert_file = match File::open(cert_path) {
            Ok(f) => f,
            Err(_) => return Ok(None),
        };

        let key_file = match File::open(key_path) {
            Ok(f) => f,
            Err(_) => return Ok(None),
        };

        let certs = rustls_pemfile::certs(&mut BufReader::new(cert_file)).collect::<Result<Vec<_>, _>>()?;
        let key = rustls_pemfile::private_key(&mut BufReader::new(key_file))?.ok_or("No private key found")?;

        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)?;

        Ok(Some(config))
    }

    // Keep the SMTP accept method, but derive acceptor from config
    pub async fn accept(&self, stream: TcpStream) -> Result<TlsStream<TcpStream>, std::io::Error> {
        if let Some(config) = self.config() {
            let acceptor = TlsAcceptor::from(config);
            acceptor.accept(stream).await
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "No valid TLS certificate loaded yet. SMTP plain text connections remain active.",
            ))
        }
    }

    /// Creates a new Reloader and spawns a background task to watch files
    pub fn new(cert_path: PathBuf, key_path: PathBuf, reload_interval: Duration) -> Result<Self, Box<dyn std::error::Error>> {
        // 1. Initial Load (Safe to return None if missing on boot)
        let initial_config = Arc::new(Self::load_config(&cert_path, &key_path)?);

        let inner = Arc::new(ArcSwap::new(initial_config));
        // Define the watcher before spawning the task
        let watcher = inner.clone();

        // 2. Spawn the Watcher Task
        tokio::spawn(async move {
            let mut last_modified = std::fs::metadata(&cert_path)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

            loop {
                sleep(reload_interval).await;

                // Check if file changed on disk
                if let Ok(metadata) = std::fs::metadata(&cert_path) {
                    if let Ok(modified) = metadata.modified() {
                        if modified > last_modified || last_modified == std::time::SystemTime::UNIX_EPOCH {
                            // Update the timestamp immediately so we don't get stuck in a reload loop 
                            // if the key file is missing or parsing fails.
                            last_modified = modified;

                            println!("♻️  Detected certificate change on disk. Reloading...");

                            match Self::load_config(&cert_path, &key_path) {
                                Ok(Some(new_config)) => {
                                    let config_arc = Arc::new(Some(new_config));
                                    watcher.store(config_arc);
                                    println!("✅ Certificate reloaded successfully!");
                                }
                                Ok(None) => {
                                    eprintln!("⚠️ Certificate file changed, but key file is missing or invalid. Waiting for next update.");
                                }
                                Err(e) => eprintln!("❌ Failed to reload certs: {}", e),
                            }
                        }
                    }
                }
            }
        });

        Ok(Self { inner })
    }
}
