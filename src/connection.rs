use std::io;
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use crate::{other, Stream};
use bytes::Bytes;
use h2::client::{self, SendRequest};
use http::Request;
use tokio::net::TcpStream;
use tokio::time::sleep;
use tokio_rustls::{rustls::ClientConfig, webpki::DNSNameRef, TlsConnector};

pub struct Connection {
    tls_config: Arc<ClientConfig>,
    addr: SocketAddr,
    domain_name: String,
    send_request: Option<SendRequest<Bytes>>,
    available: Arc<AtomicBool>,
    sleeps: usize,
}

impl Connection {
    pub async fn new(
        tls_config: ClientConfig,
        addr: SocketAddr,
        domain_name: String,
    ) -> io::Result<Connection> {
        let mut conn = Connection {
            tls_config: Arc::new(tls_config),
            addr,
            domain_name,
            send_request: None,
            available: Arc::new(AtomicBool::new(false)),
            sleeps: 0,
        };

        conn.connect().await?;
        Ok(conn)
    }

    pub async fn new_stream(&mut self) -> io::Result<Stream> {
        if !self.available.load(Ordering::Relaxed) {
            match self.reconnect().await {
                Ok(()) => self.sleeps = 0,
                Err(e) => {
                    self.sleeps += 1;
                    return Err(e);
                }
            }
        }

        if let Some(send_request) = self.send_request.as_mut() {
            let (response, send_stream) = send_request
                .send_request(Request::new(()), false)
                .map_err(|e| {
                    self.available.store(false, Ordering::Relaxed);
                    log::error!("send stream error {:?}", e);
                    other(&e.to_string())
                })?;

            let recv_stream = response
                .await
                .map_err(|e| {
                    self.available.store(false, Ordering::Relaxed);
                    log::error!("response err {}", e);
                    other(&e.to_string())
                })?
                .into_body();

            return Ok(Stream::new(send_stream, recv_stream));
        }

        panic!("this should not happend");
    }

    async fn connect(&mut self) -> io::Result<()> {
        self.reconnect().await
    }

    async fn reconnect(&mut self) -> io::Result<()> {
        if self.sleeps > 0 {
            let delay_ms = [50, 75, 100, 250, 500, 750, 1000]
                .get(self.sleeps as usize)
                .unwrap_or(&1000);
            sleep(Duration::from_millis(*delay_ms)).await;
        }

        self.available.store(false, Ordering::Relaxed);

        let tls_connector = TlsConnector::from(self.tls_config.clone());
        let domain = DNSNameRef::try_from_ascii_str(&self.domain_name).map_err(|e| {
            log::error!("domain err {:?}", e);
            io::Error::new(io::ErrorKind::InvalidInput, "invalid domain name")
        })?;

        let stream = TcpStream::connect(self.addr).await?;
        stream.set_nodelay(true)?;
        let tls_stream = tls_connector.connect(domain, stream).await?;
        let (h2, connection) = client::handshake(tls_stream).await.map_err(|e| {
            log::error!("handshake err {:?}", e);
            other(&e.to_string())
        })?;

        let available = self.available.clone();

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                log::error!("h2 underlay connection err {:?}", e);
                available.store(false, Ordering::Relaxed);
            }
        });

        let h2 = h2.ready().await.map_err(|e| {
            log::error!("h2 ready err {:?}", e);
            other(&e.to_string())
        })?;

        self.send_request = Some(h2);
        self.available.store(true, Ordering::Relaxed);

        Ok(())
    }
}
