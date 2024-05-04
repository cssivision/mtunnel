use std::io;
use std::time::Duration;

use awak::net::TcpStream;
use awak::util::{copy_bidirectional, IdleTimeout};

pub mod args;
pub mod client;
pub mod config;
pub mod connection;
pub mod server;
mod stream;

pub use stream::Stream;

const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(3);
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

pub fn other(desc: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Other, desc)
}

pub async fn proxy(mut socket: TcpStream, mut stream: Stream) {
    match IdleTimeout::new(
        copy_bidirectional(&mut socket, &mut stream),
        DEFAULT_IDLE_TIMEOUT,
        DEFAULT_CHECK_INTERVAL,
    )
    .await
    {
        Ok(v) => match v {
            Ok((n1, n2)) => {
                log::debug!("proxy local => remote: {}, remote => local: {}", n1, n2);
            }
            Err(e) => {
                log::error!("copy_bidirectional err: {:?}", e);
                stream.send_reset(h2::Reason::CANCEL);
            }
        },
        Err(_) => {
            log::error!("copy_bidirectional idle timeout");
        }
    }
}
