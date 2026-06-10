pub mod acceptor;
pub mod blocklist;
pub mod protocol;
pub mod rate_limit;

use self::acceptor::HotReloadAcceptor;
use self::protocol::SmtpSession;
use self::rate_limit::RateLimiter;
use crate::outbound::OutboundService;

use crate::db::DbPool;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

pub async fn run_server(
    addr: &str,
    db_pool: DbPool,
    storage_dir: std::path::PathBuf,
    outbound: Arc<OutboundService>,
    acceptor: Option<HotReloadAcceptor>,
    tx: tokio::sync::broadcast::Sender<crate::web::DashboardEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(addr).await?;
    println!("SMTP server listening on {}", &addr);

    let rate_limiter = Arc::new(RateLimiter::new());
    let limits = rate_limit::InboundLimits::from_env();

    let blocklist_path = crate::config::get_config("BLOCKIPS_FILE", "blockips.conf");
    let blocklist = Arc::new(blocklist::Blocklist::new(std::path::PathBuf::from(
        blocklist_path,
    )));

    let limiter_clone = rate_limiter.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(600)).await; // Run cleanup every 10 minutes
            limiter_clone.cleanup(Duration::from_secs(3600)); // Remove entries older than 1 hour
        }
    });

    loop {
        let (socket, peer_addr) = listener.accept().await?;
        let peer_ip = peer_addr.ip();

        // Immediate block check!
        if blocklist.is_blocked(peer_ip) {
            tracing::warn!(
                "Connection from blocked IP {} dropped immediately.",
                peer_ip
            );
            continue; // Drop the TCP connection instantly by letting 'socket' go out of scope
        }

        let acceptor_clone = acceptor.clone();
        let pool_clone = db_pool.clone();
        let storage_dir_clone = storage_dir.clone();
        let outbound_clone = outbound.clone();
        let tx_clone = tx.clone();
        let rate_limiter_clone = rate_limiter.clone();
        let blocklist_clone = blocklist.clone();
        let limits_clone = limits;

        tokio::spawn(async move {
            let mut session = SmtpSession::new(
                socket,
                acceptor_clone,
                pool_clone,
                storage_dir_clone,
                outbound_clone,
                tx_clone,
                peer_ip,
                rate_limiter_clone,
                blocklist_clone,
                limits_clone,
            );
            if let Err(e) = session.handle().await {
                eprintln!("SMTP error from {}: {}", peer_ip, e)
            }
        });
    }
}
