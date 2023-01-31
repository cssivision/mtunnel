use std::fs::File;
use std::io::{self, BufReader};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_rustls::rustls::{ClientConfig, OwnedTrustAnchor, RootCertStore, ServerName};
use tokio_rustls::webpki;

use crate::config;
use crate::connection::Connection;

fn tls_config(cfg: &config::Client) -> io::Result<ClientConfig> {
    let mut root_cert_store = RootCertStore::empty();
    let mut pem = BufReader::new(File::open(&cfg.ca_certificate)?);
    let certs = rustls_pemfile::certs(&mut pem)?;
    let trust_anchors = certs.iter().map(|cert| {
        let ta = webpki::TrustAnchor::try_from_cert_der(&cert[..]).unwrap();
        OwnedTrustAnchor::from_subject_spki_name_constraints(
            ta.subject,
            ta.spki,
            ta.name_constraints,
        )
    });
    root_cert_store.add_server_trust_anchors(trust_anchors);
    let config = ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(root_cert_store)
        .with_no_client_auth();
    Ok(config)
}

pub async fn run(config: config::Client) -> io::Result<()> {
    let tls_config = tls_config(&config)?;
    let listener = TcpListener::bind(&config.local_addr).await?;
    let remote_addr = config.remote_addr.parse().expect("invalid remote addr");
    let domain_name =
        ServerName::try_from(config.domain_name.as_str()).expect("invalid domain name");
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
