use std::io;

use tokio::io::copy_bidirectional;
use tokio::net::TcpStream;

macro_rules! ready {
    ($e:expr $(,)?) => {
        match $e {
            std::task::Poll::Ready(t) => t,
            std::task::Poll::Pending => return std::task::Poll::Pending,
        }
    };
}

pub mod args;
pub mod config;
pub mod connection;
mod stream;

pub use stream::Stream;

pub const ALPN_HTTP2: &[u8] = b"h2";

pub fn other(desc: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Other, desc)
}

pub async fn proxy(mut socket: TcpStream, mut stream: Stream) {
    match copy_bidirectional(&mut socket, &mut stream).await {
        Ok((n1, n2)) => {
            log::debug!("proxy local => remote: {}, remote => local: {}", n1, n2);
        }
        Err(e) => {
            log::error!("copy_bidirectional err: {:?}", e);
            stream.send_reset(h2::Reason::CANCEL);
        }
    }
}
