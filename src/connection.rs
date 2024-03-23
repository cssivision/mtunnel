use std::future::{poll_fn, Future};
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Poll};
use std::time::Duration;

use awak::net::TcpStream;
use awak::time::{delay_for, timeout};
use bytes::Bytes;
use futures_channel::{mpsc, oneshot};
use futures_rustls::{
    self, client::TlsStream, rustls, rustls::pki_types::ServerName, TlsConnector,
};
use futures_util::sink::SinkExt;
use futures_util::stream::Stream;
use h2::client::{self, SendRequest};
use http::Request;
use tokio_util::compat::*;

use crate::other;

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const DELAY_MS: &[u64] = &[50, 75, 100, 250, 500, 750, 1000];
const DEFAULT_CONN_WINDOW: u32 = 1024 * 1024 * 16; // 16mb
const DEFAULT_STREAM_WINDOW: u32 = 1024 * 1024 * 2; // 2mb

type ClientTx = oneshot::Sender<(client::ResponseFuture, h2::SendStream<Bytes>)>;

#[derive(Clone)]
pub struct Connection(Arc<Inner>);

pub struct Inner {
    tls_config: Arc<rustls::ClientConfig>,
    addr: SocketAddr,
    server_name: ServerName<'static>,
    tx: mpsc::Sender<ClientTx>,
}

impl Connection {
    pub fn new(
        tls_config: Arc<rustls::ClientConfig>,
        addr: SocketAddr,
        server_name: ServerName<'static>,
    ) -> Connection {
        let (tx, rx) = mpsc::channel(100);
        let conn = Connection(Arc::new(Inner {
            tls_config,
            addr,
            server_name,
            tx,
        }));
        awak::spawn(conn.clone().main_loop(rx)).detach();
        conn
    }

    async fn main_loop(mut self, mut rx: mpsc::Receiver<ClientTx>) {
        loop {
            let (h2, conn) = self.connect().await;
            self.recv_send_loop(h2, conn, &mut rx).await;
        }
    }

    async fn recv_send_loop(
        &mut self,
        mut h2: SendRequest<Bytes>,
        mut conn: client::Connection<Compat<TlsStream<TcpStream>>, Bytes>,
        mut rx: &mut mpsc::Receiver<ClientTx>,
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

                match ready!(Pin::new(&mut rx).poll_next(cx)) {
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

    pub async fn new_stream(&mut self) -> io::Result<crate::Stream> {
        let (tx, rx) = oneshot::channel();
        self.0
            .tx
            .clone()
            .send(tx)
            .await
            .map_err(|e| other(&format!("new stream request err: {e}")))?;
        let (response, send_stream) = rx
            .await
            .map_err(|e| other(&format!("new stream response err: {e}")))?;

        let recv_stream = response
            .await
            .map_err(|e| other(&format!("recv stream err: {e}")))?
            .into_body();
        Ok(crate::Stream::new(send_stream, recv_stream))
    }

    async fn connect(
        &self,
    ) -> (
        SendRequest<Bytes>,
        client::Connection<Compat<TlsStream<TcpStream>>, Bytes>,
    ) {
        let mut sleeps = 0;

        loop {
            let fut = async move {
                let tls_connector = TlsConnector::from(self.0.tls_config.clone());
                let stream =
                    timeout(DEFAULT_CONNECT_TIMEOUT, TcpStream::connect(self.0.addr)).await??;
                let _ = stream.set_nodelay(true);
                let tls_stream = tls_connector
                    .connect(self.0.server_name.clone(), stream)
                    .await?;
                client::Builder::new()
                    .initial_connection_window_size(DEFAULT_CONN_WINDOW)
                    .initial_window_size(DEFAULT_STREAM_WINDOW)
                    .handshake(tls_stream.compat())
                    .await
                    .map_err(|e| other(&e.to_string()))
            };

            match fut.await {
                Ok((s, c)) => {
                    return (s, c);
                }
                Err(e) => {
                    log::error!("reconnect err: {:?} fail: {:?}", self.0.addr, e);
                    let delay = DELAY_MS.get(sleeps as usize).unwrap_or(&1000);
                    sleeps += 1;
                    delay_for(Duration::from_millis(*delay)).await;
                }
            }
        }
    }
}
