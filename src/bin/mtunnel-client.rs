use std::fs::File;
use std::io::{self, BufReader};
use std::sync::Arc;

use bytes::Bytes;
use h2::client::{self, SendRequest};
use http::Request;
use mtunnel::{other, Stream};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{rustls::ClientConfig, webpki::DNSNameRef, TlsConnector};

use mtunnel::args::parse_args;
use mtunnel::ALPN_HTTP2;

#[tokio::main]
pub async fn main() -> io::Result<()> {
    env_logger::init();
    let cfg = parse_args("mtunnel-client").expect("invalid config");
    log::info!("{}", serde_json::to_string_pretty(&cfg).unwrap());

    let listener = TcpListener::bind(&cfg.local_addr).await?;
    let mut config = ClientConfig::new();
    let mut pem = BufReader::new(File::open(&cfg.ca_certificate)?);
    config
        .root_store
        .add_pem_file(&mut pem)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid cert"))?;

    config.set_protocols(&[ALPN_HTTP2.to_vec()]);
    let connector = TlsConnector::from(Arc::new(config));

    let stream = TcpStream::connect(&cfg.remote_addr).await?;
    let domain = DNSNameRef::try_from_ascii_str(&cfg.domain_name)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid domain name"))?;
    let stream = connector.connect(domain, stream).await?;

    let (h2, connection) = client::handshake(stream)
        .await
        .map_err(|e| other(&e.to_string()))?;

    tokio::spawn(async move {
        if let Err(e) = connection.await {
            log::error!("h2 underlay connection err {:?}", e);
        }
    });

    let mut h2 = h2.ready().await.map_err(|e| other(&e.to_string()))?;
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                log::debug!("accept tcp stream from {:?}", addr);
                if let Err(e) = proxy(stream, &mut h2).await {
                    log::error!("proxy error {:?}", e);
                }
            }
            Err(e) => {
                log::error!("accept fail: {:?}", e);
            }
        }
    }
}

async fn proxy(stream: TcpStream, h2: &mut SendRequest<Bytes>) -> io::Result<()> {
    match h2.send_request(Request::new(()), false) {
        Ok((response, send_stream)) => {
            let recv_stream = response
                .await
                .map_err(|e| other(&e.to_string()))?
                .into_body();

            log::debug!("proxy to h2 stream");
            tokio::spawn(async move {
                mtunnel::proxy(stream, Stream::new(send_stream, recv_stream)).await;
            });
        }
        Err(e) => {
            log::error!("send stream error {:?}", e);
        }
    }
    Ok(())
}
