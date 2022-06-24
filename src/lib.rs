use std::future::Future;
use std::io;
use std::ops::{Add, Sub};
use std::pin::Pin;
use std::task::{Context, Poll};

use pin_project_lite::pin_project;
use tokio::io::copy_bidirectional;
use tokio::net::TcpStream;
use tokio::time::{sleep, Duration, Instant, Sleep};

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

const DEFAULT_VISITED_GAP: Duration = Duration::from_secs(3);
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

pub fn other(desc: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Other, desc)
}

pub async fn proxy(mut socket: TcpStream, mut stream: Stream) {
    match IdleTimeout::new(
        copy_bidirectional(&mut socket, &mut stream),
        DEFAULT_IDLE_TIMEOUT,
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

pin_project! {
    /// A future with timeout time set
    pub struct IdleTimeout<S: Future> {
        #[pin]
        inner: S,
        #[pin]
        sleep: Sleep,
        idle_timeout: Duration,
        last_visited: Instant,
    }
}

impl<S: Future> IdleTimeout<S> {
    pub fn new(inner: S, idle_timeout: Duration) -> Self {
        let sleep = sleep(idle_timeout);

        Self {
            inner,
            sleep,
            idle_timeout,
            last_visited: Instant::now(),
        }
    }
}

impl<S: Future> Future for IdleTimeout<S> {
    type Output = io::Result<S::Output>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        match this.inner.poll(cx) {
            Poll::Ready(v) => Poll::Ready(Ok(v)),
            Poll::Pending => match Pin::new(&mut this.sleep).poll(cx) {
                Poll::Ready(_) => Poll::Ready(Err(io::ErrorKind::TimedOut.into())),
                Poll::Pending => {
                    let now = Instant::now();
                    if now.sub(*this.last_visited) >= DEFAULT_VISITED_GAP {
                        *this.last_visited = now;
                        this.sleep.reset(now.add(*this.idle_timeout));
                    }
                    Poll::Pending
                }
            },
        }
    }
}
