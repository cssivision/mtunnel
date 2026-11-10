use std::time::Duration;

use awak::net::TcpStream;
use awak::util::{copy_bidirectional_with_sizes, IdleTimeout};

pub mod args;
pub mod client;
pub mod config;
pub mod connection;
pub mod server;
mod stream;

pub use stream::Stream;

const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(3);
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Buffer size used by [`proxy`] when the config does not set `buffer_size`.
///
/// The throughput plateau starts at 16KB, which is exactly one h2 DATA frame
/// (`DEFAULT_MAX_FRAME_SIZE`), so a larger buffer only costs memory: each
/// proxied connection holds four of them on a single host (client and server
/// side, one per direction).
pub const DEFAULT_COPY_BUFFER_SIZE: usize = 16 * 1024;
const MIN_COPY_BUFFER_SIZE: usize = 1024;

pub fn copy_buffer_size(configured: Option<usize>) -> usize {
    match configured {
        Some(size) if size >= MIN_COPY_BUFFER_SIZE => size,
        Some(size) => {
            log::warn!(
                "buffer_size {} is too small, falling back to {}",
                size,
                MIN_COPY_BUFFER_SIZE
            );
            MIN_COPY_BUFFER_SIZE
        }
        None => DEFAULT_COPY_BUFFER_SIZE,
    }
}

pub async fn proxy(mut socket: TcpStream, mut stream: Stream, buffer_size: usize) {
    match IdleTimeout::new(
        copy_bidirectional_with_sizes(&mut socket, &mut stream, buffer_size),
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
