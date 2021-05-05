use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use tokio::io::{copy_bidirectional, AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

macro_rules! ready {
    ($e:expr $(,)?) => {
        match $e {
            std::task::Poll::Ready(t) => t,
            std::task::Poll::Pending => return std::task::Poll::Pending,
        }
    };
}

pub fn other(desc: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Other, desc)
}

pub struct Stream {
    send_stream: SendStream,
    recv_stream: RecvStream,
}

impl Stream {
    pub fn new(s1: h2::SendStream<Bytes>, s2: h2::RecvStream) -> Stream {
        Stream {
            send_stream: SendStream::new(s1),
            recv_stream: RecvStream::new(s2),
        }
    }
}

struct RecvStream {
    inner: h2::RecvStream,
    buf: Bytes,
    read_closed: bool,
}

impl RecvStream {
    fn new(s: h2::RecvStream) -> RecvStream {
        RecvStream {
            inner: s,
            buf: Bytes::new(),
            read_closed: false,
        }
    }

    pub fn poll_data(&mut self, cx: &mut Context<'_>) -> Poll<Option<io::Result<Bytes>>> {
        self.inner
            .poll_data(cx)
            .map_err(|e| other(&format!("poll_data err: {:?}", e.to_string())))
    }

    fn release_capacity(&mut self, size: usize) -> io::Result<()> {
        self.inner
            .flow_control()
            .release_capacity(size)
            .map_err(|e| other(&e.to_string()))
    }
}

struct SendStream {
    inner: h2::SendStream<Bytes>,
}

impl SendStream {
    fn new(s: h2::SendStream<Bytes>) -> SendStream {
        SendStream { inner: s }
    }

    pub fn send_data(&mut self, data: Bytes, end_of_stream: bool) -> io::Result<()> {
        self.inner.send_data(data, end_of_stream).map_err(|e| {
            other(&format!(
                "poll_write err: {:?}, end_of_stream {}",
                e.to_string(),
                end_of_stream
            ))
        })
    }

    pub fn reserve_capacity(&mut self, capacity: usize) {
        self.inner.reserve_capacity(capacity);
    }
}

impl AsyncRead for Stream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            if self.recv_stream.buf.len() != 0 {
                let pos = self.recv_stream.buf.len().min(buf.remaining());
                buf.put_slice(&self.recv_stream.buf.split_to(pos));
                return Poll::Ready(Ok(()));
            }

            if !self.recv_stream.read_closed {
                if let Some(data) = ready!(self.recv_stream.poll_data(cx)) {
                    let data = data?;
                    self.recv_stream.release_capacity(data.len())?;
                    self.recv_stream.buf = data;
                } else {
                    self.recv_stream.read_closed = self.recv_stream.inner.is_end_stream();
                }
            }

            if self.recv_stream.read_closed && self.recv_stream.buf.len() == 0 {
                return Poll::Ready(Ok(()));
            }
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        self.send_stream.reserve_capacity(buf.len());
        self.send_stream
            .send_data(Bytes::copy_from_slice(buf), false)?;
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        self.send_stream.send_data(Bytes::new(), true)?;
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Poll::Ready(Ok(()))
    }
}

pub async fn proxy(mut socket: TcpStream, mut stream: Stream) {
    match copy_bidirectional(&mut socket, &mut stream).await {
        Ok((n1, n2)) => {
            log::debug!("proxy local => remote: {}, remote => local: {}", n1, n2);
        }
        Err(e) => {
            log::error!("copy_bidirectional err: {:?}", e);
            stream.send_stream.inner.send_reset(h2::Reason::CANCEL);
        }
    }
}
