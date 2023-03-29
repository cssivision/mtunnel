use std::fs::File;
use std::io::{self, BufReader};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use h2::server;
use http::Response;
use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_rustls::rustls::{Certificate, PrivateKey, ServerConfig};
use tokio_rustls::{server::TlsStream, TlsAcceptor};

use crate::config;
use crate::{other, Stream};

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const HANKSHAKE_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_CONN_WINDOW: u32 = 1024 * 1024 * 8; // 8mb
const DEFAULT_STREAM_WINDOW: u32 = 1024 * 1024; // 1mb
const ACCEPT_TLS_TIMEOUT: Duration = Duration::from_secs(3);

fn tls_config(cfg: &config::Server) -> io::Result<ServerConfig> {
    let key = load_keys(&cfg.server_key)?;
    let certs = load_certs(&cfg.server_cert)?;
    let config = ServerConfig::builder()
        .with_safe_defaults()
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
        tokio::spawn(async move {
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
        });
    }
}

async fn proxy(stream: TlsStream<TcpStream>, addrs: Vec<SocketAddr>) -> io::Result<()> {
    let mut h2 = timeout(
        HANKSHAKE_TIMEOUT,
        server::Builder::new()
            .initial_connection_window_size(DEFAULT_CONN_WINDOW)
            .initial_window_size(DEFAULT_STREAM_WINDOW)
            .handshake(stream),
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
        tokio::spawn(async move {
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
        });
    }
    Ok(())
}

fn load_certs(path: &str) -> io::Result<Vec<Certificate>> {
    certs(&mut BufReader::new(File::open(path)?))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid cert"))
        .map(|mut certs| certs.drain(..).map(Certificate).collect())
}

fn load_keys(path: &str) -> io::Result<PrivateKey> {
    if let Ok(mut keys) = pkcs8_private_keys(&mut BufReader::new(File::open(path)?))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid key"))
        .map(|mut keys| keys.drain(..).map(PrivateKey).collect::<Vec<PrivateKey>>())
    {
        if !keys.is_empty() {
            return Ok(keys.remove(0));
        }
    }
    if let Ok(mut keys) = rsa_private_keys(&mut BufReader::new(File::open(path)?))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid key"))
        .map(|mut keys| keys.drain(..).map(PrivateKey).collect::<Vec<PrivateKey>>())
    {
        if !keys.is_empty() {
            return Ok(keys.remove(0));
        }
    }
    Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid key"))
}
