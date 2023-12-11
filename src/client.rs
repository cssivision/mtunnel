use std::fs::File;
use std::io::{self, BufReader};
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::ServerName;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

use crate::config;
use crate::connection::Connection;

fn tls_config(cfg: &config::Client) -> io::Result<rustls::ClientConfig> {
    let mut root_cert_store = rustls::RootCertStore::empty();
    let mut pem = BufReader::new(File::open(&cfg.ca_certificate)?);
    for cert in rustls_pemfile::certs(&mut pem) {
        root_cert_store.add(cert?).unwrap();
    }
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_cert_store)
        .with_no_client_auth();
    Ok(config)
}

pub async fn run(config: config::Client) -> io::Result<()> {
    let tls_config = tls_config(&config)?;
    let listener = TcpListener::bind(&config.local_addr).await?;
    let remote_addr = config.remote_addr.parse().expect("invalid remote addr");
    let domain_name = ServerName::try_from(config.domain_name.as_str())
        .expect("invalid domain name")
        .to_owned();
    let h2 = Connection::new(Arc::new(tls_config), remote_addr, domain_name);

    loop {
        let (stream, addr) = listener.accept().await?;
        log::debug!("accept tcp from {:?}", addr);
        let h2 = h2.clone();
        tokio::spawn(async move {
            if let Err(e) = proxy(stream, h2).await {
                log::error!("proxy error {:?}", e);
            }
        });
    }
}

async fn proxy(socket: TcpStream, h2: Connection) -> io::Result<()> {
    log::debug!("new h2 stream");
    let stream = timeout(Duration::from_secs(3), h2.new_stream()).await??;
    log::debug!("proxy to {:?}", stream.stream_id());
    crate::proxy(socket, stream).await;
    Ok(())
}
