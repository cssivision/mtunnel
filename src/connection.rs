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
use tokio::time::{sleep, timeout};
use tokio_rustls::{rustls::ClientConfig, webpki::DNSName, TlsConnector};

const DEFAULT_CONNECTION_NUM: usize = 3;
const DELAY_MS: &[u64] = &[50, 75, 100, 250, 500, 750, 1000];

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
    available: Arc<AtomicBool>,
    sleeps: usize,
}

impl Connection {
    fn new(tls_config: Arc<ClientConfig>, addr: SocketAddr, domain_name: DNSName) -> Connection {
        Connection {
            tls_config,
            addr,
            domain_name,
            send_request: None,
            available: Arc::new(AtomicBool::new(false)),
            sleeps: 0,
        }
    }

    async fn new_stream(&mut self) -> io::Result<Stream> {
        if !self.available.load(Ordering::Relaxed) {
            match self.reconnect().await {
                Ok(()) => {
                    self.available.store(true, Ordering::Relaxed);
                    self.sleeps = 0
                }
                Err(e) => {
                    log::error!("reconnect error {:?}", e);
                    self.sleeps += 1;
                    return Err(e);
                }
            }
        }

        self.send_request().await.map_err(|e| {
            self.available.store(false, Ordering::Relaxed);
            log::error!("send request error {:?}", e);
            other(&e.to_string())
        })
    }

    async fn send_request(&mut self) -> Result<Stream, h2::Error> {
        if let Some(send_request) = self.send_request.as_mut() {
            let (response, send_stream) = send_request.send_request(Request::new(()), false)?;
            let recv_stream = response.await?.into_body();
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
        let stream = timeout(Duration::from_secs(3), TcpStream::connect(self.addr)).await??;
        stream.set_nodelay(true)?;
        let tls_stream = tls_connector
            .connect(self.domain_name.as_ref(), stream)
            .await?;
        let (h2, connection) = client::handshake(tls_stream)
            .await
            .map_err(|e| other(&e.to_string()))?;

        let available = self.available.clone();
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                log::error!("h2 underlay connection err {:?}", e);
                available.store(false, Ordering::Relaxed);
            }
        });

        let h2 = h2.ready().await.map_err(|e| other(&e.to_string()))?;
        self.send_request = Some(h2);
        Ok(())
    }
}
