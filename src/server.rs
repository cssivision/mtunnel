use std::fs::File;
use std::io::{self, BufReader};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use awak::net::{TcpListener, TcpStream};
use awak::time::timeout;
use futures_rustls::{
    rustls,
    rustls::pki_types::{CertificateDer, PrivateKeyDer},
    server::TlsStream,
    TlsAcceptor,
};
use h2::server;
use http::Response;
use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
use tokio_util::compat::*;

use crate::config;
use crate::{other, Stream};

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const HANKSHAKE_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_CONN_WINDOW: u32 = 1024 * 1024 * 8; // 8mb
const DEFAULT_STREAM_WINDOW: u32 = 1024 * 1024; // 1mb
const ACCEPT_TLS_TIMEOUT: Duration = Duration::from_secs(3);

fn tls_config(cfg: &config::Server) -> io::Result<rustls::ServerConfig> {
    let key = load_keys(cfg.server_key.clone())?;
    let certs = load_certs(cfg.server_cert.clone())?;
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    Ok(config)
}

pub async fn run(cfg: config::Server) -> io::Result<()> {
    let config = tls_config(&cfg)?;
    let listener = TcpListener::bind(&cfg.local_addr).await?;
    let tls_acceptor = TlsAcceptor::from(Arc::new(config));
    let remote_addrs = cfg.remote_socket_addrs();
    loop {
        let (stream, addr) = listener.accept().await?;
        log::debug!("accept tcp from {:?}", addr);
        let tls_acceptor = tls_acceptor.clone();
        let remote_addrs = remote_addrs.clone();
        awak::spawn(async move {
            match timeout(ACCEPT_TLS_TIMEOUT, tls_acceptor.accept(stream)).await {
                Ok(stream) => match stream {
                    Ok(stream) => {
                        if let Err(e) = proxy(stream, remote_addrs).await {
                            log::error!("proxy h2 connection fail: {:?}", e);
                        }
                    }
                    Err(e) => {
                        log::error!("accept stream err {:?}", e);
                    }
                },
                Err(e) => {
                    log::error!("accept stream timeout err {:?}", e);
                }
            }
        })
        .detach();
    }
}

async fn proxy(stream: TlsStream<TcpStream>, addrs: Vec<SocketAddr>) -> io::Result<()> {
    let mut h2 = timeout(
        HANKSHAKE_TIMEOUT,
        server::Builder::new()
            .initial_connection_window_size(DEFAULT_CONN_WINDOW)
            .initial_window_size(DEFAULT_STREAM_WINDOW)
            .handshake(stream.compat()),
    )
    .await
    .map_err(|e| other(&e.to_string()))?
    .map_err(|e| other(&e.to_string()))?;

    let mut next: usize = 0;

    while let Some(request) = h2.accept().await {
        next = next.wrapping_add(1);
        let current = next % addrs.len();
        let addr = addrs[current];
        log::debug!("accept h2 stream");
        let (request, mut respond) = request.map_err(|e| other(&e.to_string()))?;
        let recv_stream = request.into_body();
        let send_stream = respond
            .send_response(Response::new(()), false)
            .map_err(|e| other(&e.to_string()))?;

        log::debug!("proxy {:?} to {}", respond.stream_id(), addr);
        awak::spawn(async move {
            match timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
                Ok(stream) => {
                    match stream {
                        Ok(stream) => {
                            crate::proxy(stream, Stream::new(send_stream, recv_stream)).await;
                        }
                        Err(e) => {
                            log::error!("connect to {} err {:?}", &addr, e);
                        }
                    };
                }
                Err(e) => {
                    respond.send_reset(h2::Reason::CANCEL);
                    log::error!("connect to {} err {:?}", &addr, e);
                }
            }
        })
        .detach();
    }
    Ok(())
}

fn load_certs(path: String) -> io::Result<Vec<CertificateDer<'static>>> {
    certs(&mut BufReader::new(File::open(path)?)).collect()
}

fn load_keys(path: String) -> io::Result<PrivateKeyDer<'static>> {
    if let Some(key) = pkcs8_private_keys(&mut BufReader::new(File::open(&path)?)).next() {
        return key.map(Into::into);
    }
    if let Some(key) = rsa_private_keys(&mut BufReader::new(File::open(path)?)).next() {
        return key.map(Into::into);
    }
    Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid key"))
}
