use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
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

type ClientTx = oneshot::Sender<(client::ResponseFuture, h2::SendStream<Bytes>)>;

#[derive(Clone)]
pub struct Connection(Arc<Inner>);

pub struct Inner {
    tls_config: Arc<ClientConfig>,
    addr: SocketAddr,
    domain_name: DNSName,
    tx: UnboundedSender<ClientTx>,
}

impl Connection {
    pub fn new(
        tls_config: Arc<ClientConfig>,
        addr: SocketAddr,
        domain_name: DNSName,
    ) -> Connection {
        let (tx, rx) = unbounded_channel();
        let conn = Connection(Arc::new(Inner {
            tls_config,
            addr,
            domain_name,
            tx,
        }));
        tokio::spawn(conn.clone().main_loop(rx));
        conn
    }

    async fn main_loop(mut self, mut rx: UnboundedReceiver<ClientTx>) {
        loop {
            let (h2, conn) = self.connect().await;
            self.recv_send_loop(h2, conn, &mut rx).await;
        }
    }

    async fn recv_send_loop(
        &mut self,
        mut h2: SendRequest<Bytes>,
        mut conn: client::Connection<TlsStream<TcpStream>, Bytes>,
        rx: &mut UnboundedReceiver<ClientTx>,
    ) {
        poll_fn(|cx| {
            if let Poll::Ready(v) = Pin::new(&mut conn).poll(cx) {
                log::error!("underly connection close {:?}", v);
                return Poll::Ready(());
            }

            loop {
                if let Err(e) = ready!(h2.poll_ready(cx)) {
                    log::error!("poll ready error {}", e);
                    return Poll::Ready(());
                }

                match ready!(rx.poll_recv(cx)) {
                    None => unreachable!(),
                    Some(req_tx) => {
                        log::debug!("recv new stream request");
                        match h2.send_request(Request::new(()), false) {
                            Err(e) => {
                                log::error!("send request error {:?}", e);
                                return Poll::Ready(());
                            }
                            Ok(v) => {
                                let _ = req_tx.send(v);
                            }
                        }
                    }
                }
            }
        })
        .await
    }

    pub async fn new_stream(&self) -> io::Result<Stream> {
        let (tx, rx) = oneshot::channel();
        self.0
            .tx
            .send(tx)
            .map_err(|e| other(&format!("new stream request err: {}", e.to_string())))?;
        let (response, send_stream) = rx
            .await
            .map_err(|e| other(&format!("new stream response err: {}", e.to_string())))?;

        let recv_stream = response
            .await
            .map_err(|e| other(&format!("recv stream err: {}", e.to_string())))?
            .into_body();
        Ok(Stream::new(send_stream, recv_stream))
    }

    async fn connect(
        &self,
    ) -> (
        SendRequest<Bytes>,
        client::Connection<TlsStream<TcpStream>, Bytes>,
    ) {
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
                    let delay = DELAY_MS.get(sleeps as usize).unwrap_or(&1000);
                    sleeps += 1;
                    sleep(Duration::from_millis(*delay)).await;
                }
            }
        }
    }
}
