use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use crate::{other, Stream};
use bytes::Bytes;
use futures_util::future::poll_fn;
use h2::client::{self, SendRequest};
use http::Request;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout};
use tokio_rustls::{client::TlsStream, rustls::ClientConfig, webpki::DNSName, TlsConnector};

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const DELAY_MS: &[u64] = &[50, 75, 100, 250, 500, 750, 1000];
const DEFAULT_CONN_WINDOW: u32 = 1024 * 1024 * 16; // 16mb
const DEFAULT_STREAM_WINDOW: u32 = 1024 * 1024 * 2; // 2mb
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone)]
pub struct Connection(Arc<Inner>);

pub struct Inner {
    tls_config: Arc<ClientConfig>,
    addr: SocketAddr,
    domain_name: DNSName,
    sender: UnboundedSender<oneshot::Sender<Stream>>,
    receiver: UnboundedReceiver<oneshot::Sender<Stream>>,
}

impl Connection {
    pub fn new(
        tls_config: Arc<ClientConfig>,
        addr: SocketAddr,
        domain_name: DNSName,
    ) -> Connection {
        let (sender, receiver) = unbounded_channel();
        let conn = Connection(Arc::new(Inner {
            tls_config,
            addr,
            domain_name,
            sender,
            receiver,
        }));
        tokio::spawn(conn.clone().main_loop());
        conn
    }

    async fn main_loop(mut self) {
        loop {
            let (h2, conn) = self.connect().await;
            self.recv_send_loop(h2, conn).await;
        }
    }

    async fn recv_send_loop(
        &mut self,
        h2: SendRequest<Bytes>,
        conn: client::Connection<TlsStream<TcpStream>, Bytes>,
    ) {
        poll_fn(|cx| {
            loop {
                match ready!(self.0.receiver.poll_recv(cx)) {
                    None => unreachable!(),
                    Some(v) => {}
                }
            }
            return Poll::Ready(());
        })
        .await
    }

    pub async fn new_stream(&self) -> io::Result<Stream> {
        let (sender, receiver) = oneshot::channel();
        self.0
            .sender
            .send(sender)
            .map_err(|e| other(&format!("new stream request err: {}", e.to_string())))?;
        receiver
            .await
            .map_err(|e| other(&format!("new stream response err: {}", e.to_string())))
    }

    async fn connect(
        &self,
    ) -> (
        SendRequest<Bytes>,
        client::Connection<TlsStream<TcpStream>, Bytes>,
    ) {
        let delay_ms = [50, 75, 100, 250, 500, 750, 1000, 2500, 3000];
        let mut sleeps = 0;

        loop {
            let fut = async move {
                let tls_connector = TlsConnector::from(self.0.tls_config.clone());
                let stream =
                    timeout(DEFAULT_CONNECT_TIMEOUT, TcpStream::connect(self.0.addr)).await??;
                let _ = stream.set_nodelay(true);
                let tls_stream = tls_connector
                    .connect(self.0.domain_name.as_ref(), stream)
                    .await?;
                client::Builder::new()
                    .initial_connection_window_size(DEFAULT_CONN_WINDOW)
                    .initial_window_size(DEFAULT_STREAM_WINDOW)
                    .handshake(tls_stream)
                    .await
                    .map_err(|e| other(&e.to_string()))
            };

            match fut.await {
                Ok(v) => return v,
                Err(e) => {
                    log::trace!("reconnect err: {:?} fail: {:?}", self.0.addr, e);
                    let delay = delay_ms.get(sleeps as usize).unwrap_or(&5000);
                    sleeps += 1;
                    sleep(Duration::from_millis(*delay)).await;
                }
            }
        }
    }
}
