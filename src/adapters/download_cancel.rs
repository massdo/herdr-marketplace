//! Cancellation of a video request, including a blocked TLS/body read.
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, Connector, LazyBuffers, NextTimeout, RustlsConnector, Transport,
};

#[derive(Clone, Debug, Default)]
pub struct Cancellation(Arc<State>);

#[derive(Debug, Default)]
struct State {
    stopped: AtomicBool,
    sockets: Mutex<Vec<TcpStream>>,
}

impl Cancellation {
    pub fn cancel(&self) {
        self.0.stopped.store(true, Ordering::Release);
        for socket in self.0.sockets.lock().unwrap().iter() {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }

    pub fn cancelled(&self) -> bool {
        self.0.stopped.load(Ordering::Acquire)
    }

    pub(crate) fn check(&self) -> Result<(), ureq::Error> {
        if self.cancelled() {
            // TLS retries Interrupted automatically; cancellation must abort
            // the connection, including a handshake in progress.
            Err(io::Error::new(io::ErrorKind::ConnectionAborted, "video download cancelled").into())
        } else {
            Ok(())
        }
    }

    pub(crate) fn agent(&self, config: Config, resolver: impl Resolver) -> ureq::Agent {
        ureq::Agent::with_parts(
            config,
            CancelConnector(self.clone()).chain(RustlsConnector::default()),
            CancelResolver(Arc::new(resolver), self.clone()),
        )
    }

    fn wait<T>(&self, result: mpsc::Receiver<Result<T, ureq::Error>>) -> Result<T, ureq::Error> {
        loop {
            self.check()?;
            match result.recv_timeout(Duration::from_millis(20)) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other("video connection stopped").into());
                }
            }
        }
    }
}

#[derive(Debug)]
struct CancelResolver<R>(Arc<R>, Cancellation);

impl<R: Resolver> Resolver for CancelResolver<R> {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        self.1.check()?;
        let (sender, result) = mpsc::channel();
        let (resolver, uri, config) = (self.0.clone(), uri.clone(), config.clone());
        // DNS cannot be interrupted portably. On cancellation only the bounded
        // lookup finishes; it cannot open a connection or retain the video file.
        thread::spawn(move || {
            let _ = sender.send(resolver.resolve(&uri, &config, timeout));
        });
        self.1.wait(result)
    }
}

#[derive(Debug)]
struct CancelConnector(Cancellation);

impl Connector for CancelConnector {
    type Out = CancelTransport;

    fn connect(
        &self,
        details: &ConnectionDetails,
        _: Option<()>,
    ) -> Result<Option<CancelTransport>, ureq::Error> {
        self.0.check()?;
        let addresses: Vec<_> = details.addrs.iter().copied().collect();
        let budget = details
            .timeout
            .not_zero()
            .map(|time| *time)
            .unwrap_or(Duration::from_secs(30));
        let (sender, result) = mpsc::channel();
        let cancelled = self.0.clone();
        thread::spawn(move || {
            let deadline = Instant::now() + budget;
            let mut result = Err(io::Error::other("no video endpoint"));
            for address in addresses {
                if cancelled.cancelled() {
                    break;
                }
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                result = TcpStream::connect_timeout(&address, left.min(Duration::from_secs(5)));
                if result.is_ok() {
                    break;
                }
            }
            // A cancelled caller drops the receiver: a late socket closes
            // here, without sending HTTP requests or holding the video file.
            let _ = sender.send(result.map_err(ureq::Error::from));
        });
        let stream = self.0.wait(result)?;
        let mut sockets = self.0.0.sockets.lock().unwrap();
        self.0.check()?;
        sockets.push(stream.try_clone()?);
        drop(sockets);
        stream.set_nodelay(true)?;
        let buffers = LazyBuffers::new(
            details.config.input_buffer_size(),
            details.config.output_buffer_size(),
        );
        Ok(Some(CancelTransport {
            stream,
            buffers,
            cancellation: self.0.clone(),
        }))
    }
}

#[derive(Debug)]
struct CancelTransport {
    stream: TcpStream,
    buffers: LazyBuffers,
    cancellation: Cancellation,
}

impl Transport for CancelTransport {
    fn buffers(&mut self) -> &mut dyn Buffers {
        &mut self.buffers
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        self.cancellation.check()?;
        self.stream
            .set_write_timeout(timeout.not_zero().map(|time| *time))?;
        self.stream.write_all(&self.buffers.output()[..amount])?;
        Ok(())
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        self.cancellation.check()?;
        self.stream
            .set_read_timeout(timeout.not_zero().map(|time| *time))?;
        let amount = self.stream.read(self.buffers.input_append_buf())?;
        self.cancellation.check()?;
        self.buffers.input_appended(amount);
        Ok(amount > 0)
    }

    fn is_open(&mut self) -> bool {
        if self.cancellation.cancelled() || self.stream.set_nonblocking(true).is_err() {
            return false;
        }
        let open = matches!(self.stream.peek(&mut [0]), Err(error) if error.kind() == io::ErrorKind::WouldBlock);
        self.stream.set_nonblocking(false).is_ok() && open
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn cancellation_interrupts_a_blocked_response_or_body_read() {
        for headers in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let (started, ready) = mpsc::channel();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                if headers {
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 65536\r\n\r\nabc")
                        .unwrap();
                }
                started.send(()).unwrap();
                match stream.read(&mut [0]) {
                    Ok(0) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
                        ) => {}
                    other => panic!("the cancelled connection stays open: {other:?}"),
                }
            });
            let cancellation = Cancellation::default();
            let worker_cancel = cancellation.clone();
            let download = thread::spawn(move || {
                let agent = worker_cancel.agent(
                    Config::builder()
                        .timeout_global(Some(Duration::from_secs(30)))
                        .build(),
                    ureq::unversioned::resolver::DefaultResolver::default(),
                );
                agent
                    .get(&url)
                    .call()
                    .and_then(|mut response| response.body_mut().read_to_vec())
            });
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
            let started = Instant::now();
            cancellation.cancel();
            assert!(download.join().unwrap().is_err());
            assert!(started.elapsed() < Duration::from_secs(1));
            server.join().unwrap();
        }
    }

    #[test]
    fn a_cancelled_request_opens_no_connection() {
        let cancellation = Cancellation::default();
        cancellation.cancel();
        let agent = cancellation.agent(
            Config::default(),
            ureq::unversioned::resolver::DefaultResolver::default(),
        );
        assert!(agent.get("http://127.0.0.1:1").call().is_err());
        assert!(cancellation.0.sockets.lock().unwrap().is_empty());
    }

    #[test]
    fn cancellation_interrupts_a_stalled_tls_handshake() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("https://{}", listener.local_addr().unwrap());
        let (started, ready) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0; 4096];
            assert!(stream.read(&mut buffer).unwrap() > 0);
            started.send(()).unwrap();
            while stream.read(&mut buffer).unwrap() > 0 {}
        });
        let cancellation = Cancellation::default();
        let worker_cancel = cancellation.clone();
        let (done, result) = mpsc::channel();
        let download = thread::spawn(move || {
            let agent = worker_cancel.agent(
                Config::builder()
                    .timeout_global(Some(Duration::from_secs(30)))
                    .build(),
                ureq::unversioned::resolver::DefaultResolver::default(),
            );
            let _ = done.send(agent.get(&url).call().is_err());
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        cancellation.cancel();
        assert!(result.recv_timeout(Duration::from_secs(1)).unwrap());
        download.join().unwrap();
        server.join().unwrap();
    }
}
