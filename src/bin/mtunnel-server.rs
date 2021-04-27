use std::fs::File;
use std::io::{self, BufReader};
use std::sync::Arc;

use h2::server;
use http::Response;
use mtunnel::{other, Stream};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::rustls::internal::pemfile::{certs, pkcs8_private_keys};
use tokio_rustls::rustls::{Certificate, NoClientAuth, PrivateKey, ServerConfig};
use tokio_rustls::TlsAcceptor;

#[tokio::main]
pub async fn main() -> io::Result<()> {
    env_logger::init();

    let mut keys = load_keys("./server.key")?;
    let certs = load_certs("./server.pem")?;
    let mut config = ServerConfig::new(NoClientAuth::new());
    config
        .set_single_cert(certs, keys.remove(0))
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    config.set_protocols(&[b"h2".to_vec()]);

    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:8081").await?;

    loop {
        if let Ok((stream, addr)) = listener.accept().await {
            log::debug!("accept stream from {:?}", addr);
            let stream = acceptor.accept(stream).await?;
            tokio::spawn(async move {
                if let Err(e) = proxy(stream).await {
                    log::error!("proxy h2 connection fail: {:?}", e);
                }
            });
        }
    }
}

async fn proxy<T: AsyncRead + AsyncWrite + Unpin>(stream: T) -> io::Result<()> {
    let mut h2 = server::handshake(stream)
        .await
        .map_err(|e| other(&e.to_string()))?;

    while let Some(request) = h2.accept().await {
        log::debug!("accept h2 stream from {:?}", request);
        let (request, mut respond) = request.map_err(|e| other(&e.to_string()))?;
        let recv_stream = request.into_body();
        let send_stream = respond
            .send_response(Response::new(()), false)
            .map_err(|e| other(&e.to_string()))?;

        log::debug!("proxy tcp stream to 127.0.0.1:8082");
        tokio::spawn(async move {
            match TcpStream::connect("127.0.0.1:8082").await {
                Ok(stream) => {
                    mtunnel::proxy(stream, Stream::new(send_stream, recv_stream)).await;
                }
                Err(e) => {
                    log::error!("connect to 127.0.0.1:8082 err {:?}", e);
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

fn load_keys(path: &str) -> io::Result<Vec<PrivateKey>> {
    pkcs8_private_keys(&mut BufReader::new(File::open(path)?))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid key"))
}
