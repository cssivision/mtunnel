use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use crate::{other, Stream};
use bytes::Bytes;
use h2::client::{self, SendRequest};
use http::Request;
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};
use tokio_rustls::{rustls::ClientConfig, webpki::DNSName, TlsConnector};

const DEFAULT_CONNECTION_NUM: usize = 1;
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const DELAY_MS: &[u64] = &[50, 75, 100, 250, 500, 750, 1000];
const DEFAULT_CONN_WINDOW: u32 = 1024 * 1024 * 16; // 16mb
const DEFAULT_STREAM_WINDOW: u32 = 1024 * 1024 * 2; // 2mb

pub struct Multiplexed {
    conns: Vec<Connection>,
    ticks: usize,
}

impl Multiplexed {
    pub fn new(
        tls_config: ClientConfig,
        addr: SocketAddr,
        domain_name: DNSName,
        n: usize,
    ) -> Multiplexed {
        let n = if n != 0 { n } else { DEFAULT_CONNECTION_NUM };
        let tls_config = Arc::new(tls_config);
        Multiplexed {
            conns: (0..n)
                .map(|_| Connection::new(tls_config.clone(), addr, domain_name.clone()))
                .collect(),
            ticks: 0,
        }
    }

    pub async fn new_stream(&mut self) -> io::Result<Stream> {
        self.ticks += 1;
        let mask = self.conns.len();
        self.conns[self.ticks % mask].new_stream().await
    }
}

struct Connection {
    tls_config: Arc<ClientConfig>,
    addr: SocketAddr,
    domain_name: DNSName,
    send_request: Option<SendRequest<Bytes>>,
    available: bool,
    sleeps: usize,
}

impl Connection {
    fn new(tls_config: Arc<ClientConfig>, addr: SocketAddr, domain_name: DNSName) -> Connection {
        Connection {
            tls_config,
            addr,
            domain_name,
            send_request: None,
            available: false,
            sleeps: 0,
        }
    }

    async fn new_stream(&mut self) -> io::Result<Stream> {
        if !self.available {
            match self.reconnect().await {
                Ok(()) => {
                    self.available = true;
                    self.sleeps = 0;
                }
                Err(e) => {
                    log::error!("reconnect error {:?}", e);
                    self.sleeps += 1;
                    return Err(e);
                }
            }
        }

        log::debug!("send h2 request");
        self.send_request().await.map_err(|e| {
            self.available = false;
            log::error!("send request error {:?}", e);
            other(&e.to_string())
        })
    }

    async fn send_request(&mut self) -> Result<Stream, h2::Error> {
        if let Some(send_request) = self.send_request.take() {
            log::debug!("waiting for send request ready");
            let mut send_request = send_request.ready().await?;
            let (response, send_stream) = send_request.send_request(Request::new(()), false)?;

            log::debug!("waiting for {:?} ready", response.stream_id());
            let recv_stream = response.await?.into_body();

            self.send_request = Some(send_request);

            Ok(Stream::new(send_stream, recv_stream))
        } else {
            unreachable!();
        }
    }

    async fn reconnect(&mut self) -> io::Result<()> {
        if self.sleeps > 0 {
            let delay_ms = DELAY_MS.get(self.sleeps as usize).unwrap_or(&1000);
            sleep(Duration::from_millis(*delay_ms)).await;
        }

        let tls_connector = TlsConnector::from(self.tls_config.clone());
        let stream = timeout(DEFAULT_CONNECT_TIMEOUT, TcpStream::connect(self.addr)).await??;
        stream.set_nodelay(true)?;
        let tls_stream = tls_connector
            .connect(self.domain_name.as_ref(), stream)
            .await?;
        let (h2, connection) = client::Builder::new()
            .initial_connection_window_size(DEFAULT_CONN_WINDOW)
            .initial_window_size(DEFAULT_STREAM_WINDOW)
            .handshake(tls_stream)
            .await
            .map_err(|e| other(&e.to_string()))?;

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                log::error!("h2 underlay connection err {:?}", e);
            }
        });

        let h2 = h2.ready().await.map_err(|e| other(&e.to_string()))?;
        self.send_request = Some(h2);
        Ok(())
    }
}
