use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use h2::{Reason, StreamId};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::other;

pub struct Stream {
    pub send_stream: SendStream,
    pub recv_stream: RecvStream,
}

impl Stream {
    pub fn new(s1: h2::SendStream<Bytes>, s2: h2::RecvStream) -> Stream {
        Stream {
            send_stream: SendStream::new(s1),
            recv_stream: RecvStream::new(s2),
        }
    }

    pub(crate) fn send_reset(&mut self, reason: Reason) {
        self.send_stream.send_reset(reason);
    }

    pub fn stream_id(&self) -> StreamId {
        self.send_stream.inner.stream_id()
    }
}

pub struct RecvStream {
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

pub struct SendStream {
    inner: h2::SendStream<Bytes>,
}

impl SendStream {
    fn new(s: h2::SendStream<Bytes>) -> SendStream {
        SendStream { inner: s }
    }

    fn send_data(&mut self, data: Bytes, end_of_stream: bool) -> io::Result<()> {
        self.inner.send_data(data, end_of_stream).map_err(|e| {
            other(&format!(
                "poll_write err: {:?}, end_of_stream {}",
                e.to_string(),
                end_of_stream
            ))
        })
    }

    fn reserve_capacity(&mut self, capacity: usize) {
        self.inner.reserve_capacity(capacity);
    }

    fn poll_capacity(&mut self, cx: &mut Context<'_>) -> Poll<Option<io::Result<usize>>> {
        if self.inner.capacity() > 0 {
            return Poll::Ready(Some(Ok(self.inner.capacity())));
        }
        self.inner
            .poll_capacity(cx)
            .map_err(|e| other(&e.to_string()))
    }

    fn poll_reset(&mut self, cx: &mut Context) -> Poll<io::Result<Reason>> {
        self.inner.poll_reset(cx).map_err(|e| other(&e.to_string()))
    }

    pub(crate) fn send_reset(&mut self, reason: Reason) {
        self.inner.send_reset(reason);
    }
}

impl AsyncRead for Stream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            if !self.recv_stream.buf.is_empty() {
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

            if self.recv_stream.read_closed && self.recv_stream.buf.is_empty() {
                return Poll::Ready(Ok(()));
            }
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.send_stream.reserve_capacity(buf.len());
        let n = match ready!(self.send_stream.poll_capacity(cx)) {
            Some(v) => v?,
            None => return Poll::Ready(Err(other("poll capacity unexpectedly closed"))),
        };

        if let Poll::Ready(reason) = self.send_stream.poll_reset(cx)? {
            log::debug!("stream received RST_STREAM: {:?}", reason);
            return Poll::Ready(Err(other(&format!(
                "stream reset for reason: {:?}",
                reason
            ))));
        }

        let size = n.min(buf.len());
        self.send_stream
            .send_data(Bytes::copy_from_slice(&buf[..size]), false)?;
        Poll::Ready(Ok(size))
    }

    fn poll_flush(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.send_stream.reserve_capacity(0);
        self.send_stream.send_data(Bytes::default(), true)?;
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
