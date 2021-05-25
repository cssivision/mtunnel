use std::fs::File;
use std::io::{self, BufReader};
use std::sync::Arc;

use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::webpki::DNSNameRef;

use mtunnel::args::parse_args;
use mtunnel::config::Config;
use mtunnel::connection::Connection;
use mtunnel::ALPN_HTTP2;

fn tls_config(cfg: &Config) -> io::Result<ClientConfig> {
    let mut config = ClientConfig::new();
    let mut pem = BufReader::new(File::open(&cfg.ca_certificate)?);
    config.root_store.add_pem_file(&mut pem).map_err(|e| {
        log::error!("add pem file err {:?}", e);
        io::Error::new(io::ErrorKind::InvalidInput, "invalid cert")
    })?;

    config.set_protocols(&[ALPN_HTTP2.to_vec()]);
    Ok(config)
}

#[tokio::main]
pub async fn main() -> io::Result<()> {
    env_logger::init();
    let config = parse_args("mtunnel-client").expect("invalid config");
    log::info!("{}", serde_json::to_string_pretty(&config).unwrap());

    let tls_config = tls_config(&config)?;
    let listener = TcpListener::bind(&config.local_addr).await?;
    let remote_addr = config.remote_addr.parse().expect("invalid remote addr");
    let domain_name = DNSNameRef::try_from_ascii_str(&config.domain_name)
        .expect("invalid domain name")
        .to_owned();
    let h2 = Connection::new(Arc::new(tls_config), remote_addr, domain_name);

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                log::debug!("accept tcp from {:?}", addr);
                let h2 = h2.clone();
                tokio::spawn(async move {
                    if let Err(e) = proxy(stream, h2).await {
                        log::error!("proxy error {:?}", e);
                    }
                });
            }
            Err(e) => {
                log::error!("accept fail: {:?}", e);
            }
        }
    }
}

async fn proxy(socket: TcpStream, h2: Connection) -> io::Result<()> {
    log::debug!("new h2 stream");
    let stream = h2.new_stream().await?;
    log::debug!("proxy to {:?}", stream.stream_id());
    mtunnel::proxy(socket, stream).await;
    Ok(())
}
