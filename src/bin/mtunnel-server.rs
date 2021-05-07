use std::fs::File;
use std::io::{self, BufReader};
use std::net::SocketAddr;
use std::sync::Arc;

use h2::server;
use http::Response;
use mtunnel::{other, Stream};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::rustls::internal::pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
use tokio_rustls::rustls::{Certificate, NoClientAuth, PrivateKey, ServerConfig};
use tokio_rustls::{server::TlsStream, TlsAcceptor};

use mtunnel::args::parse_args;
use mtunnel::config::Config;
use mtunnel::ALPN_HTTP2;

fn tls_config(cfg: &Config) -> io::Result<ServerConfig> {
    let key = load_keys(&cfg.server_key)?;
    let certs = load_certs(&cfg.server_cert)?;
    let mut config = ServerConfig::new(NoClientAuth::new());
    config
        .set_single_cert(certs, key)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    config.set_protocols(&[ALPN_HTTP2.to_vec()]);
    Ok(config)
}

#[tokio::main]
pub async fn main() -> io::Result<()> {
    env_logger::init();

    let cfg = parse_args("mtunnel-server").expect("invalid config");
    log::info!("{}", serde_json::to_string_pretty(&cfg).unwrap());

    let config = tls_config(&cfg)?;
    let listener = TcpListener::bind(&cfg.local_addr).await?;
    let tls_acceptor = TlsAcceptor::from(Arc::new(config));
    let remote_addr = cfg.remote_addr.parse().expect("invalid remote addr");
    loop {
        if let Ok((stream, addr)) = listener.accept().await {
            log::debug!("accept tcp stream from {:?}", addr);

            match tls_acceptor.accept(stream).await {
                Ok(stream) => {
                    tokio::spawn(async move {
                        if let Err(e) = proxy(stream, remote_addr).await {
                            log::error!("proxy h2 connection fail: {:?}", e);
                        }
                    });
                }
                Err(e) => {
                    log::error!("accept stream err {:?}", e);
                }
            }
        }
    }
}

async fn proxy(stream: TlsStream<TcpStream>, addr: SocketAddr) -> io::Result<()> {
    let mut h2 = server::handshake(stream)
        .await
        .map_err(|e| other(&e.to_string()))?;

    while let Some(request) = h2.accept().await {
        log::debug!("accept h2 stream");
        let (request, mut respond) = request.map_err(|e| other(&e.to_string()))?;
        let recv_stream = request.into_body();
        let send_stream = respond
            .send_response(Response::new(()), false)
            .map_err(|e| other(&e.to_string()))?;

        log::debug!("proxy tcp stream to {}", addr);
        tokio::spawn(async move {
            match TcpStream::connect(addr).await {
                Ok(stream) => {
                    mtunnel::proxy(stream, Stream::new(send_stream, recv_stream)).await;
                }
                Err(e) => {
                    log::error!("connect to {} err {:?}", &addr, e);
                }
            };
        });
    }
    Ok(())
}

fn load_certs(path: &str) -> io::Result<Vec<Certificate>> {
    certs(&mut BufReader::new(File::open(path)?))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid cert"))
}

fn load_keys(path: &str) -> io::Result<PrivateKey> {
    if let Ok(mut keys) = pkcs8_private_keys(&mut BufReader::new(File::open(path)?)) {
        if !keys.is_empty() {
            return Ok(keys.remove(0));
        }
    }
    if let Ok(mut keys) = rsa_private_keys(&mut BufReader::new(File::open(path)?)) {
        if !keys.is_empty() {
            return Ok(keys.remove(0));
        }
    }
    Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid key"))
}
