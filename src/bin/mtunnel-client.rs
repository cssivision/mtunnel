use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use h2::{client, RecvStream, SendStream};
use http::Request;
use tokio::io::{copy, AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{rustls::ClientConfig, webpki::DNSNameRef, TlsConnector};

#[tokio::main]
pub async fn main() -> io::Result<()> {
    let listener = TcpListener::bind("0.0.0.0:8080").await?;
    let mut config = ClientConfig::new();
    config
        .root_store
        .add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
    let connector = TlsConnector::from(Arc::new(config));

    let stream = TcpStream::connect("127.0.0.1:8081").await?;
    let domain = DNSNameRef::try_from_ascii_str("example.com")
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid dnsname"))?;
    let stream = connector.connect(domain, stream).await?;

    let (h2, connection) = client::handshake(stream)
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

    tokio::spawn(async move {
        connection.await.unwrap();
    });

    let mut h2 = h2
        .ready()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

    loop {
        let (socket, _) = listener.accept().await?;
        let request = Request::new(());
        let (response, send_stream) = h2.send_request(request, false).unwrap();
        let recv_stream = response.await.unwrap().into_body();
        let remote_stream = Stream {
            recv_stream,
            send_stream,
        };

        tokio::spawn(async move {
            if let Err(e) = proxy(socket, remote_stream).await {
                log::error!("proxy fail: {:?}", e);
            }
        });
    }
}

struct Stream {
    recv_stream: RecvStream,
    send_stream: SendStream<Bytes>,
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        unimplemented!();
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        unimplemented!();
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        unimplemented!();
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        unimplemented!();
    }
}

async fn proxy(mut socket: TcpStream, mut stream: Stream) -> io::Result<()> {
    let (mut socket_reader, mut socket_writer) = socket.split();
    copy(&mut socket_reader, &mut stream).await;
    Ok(())
}
