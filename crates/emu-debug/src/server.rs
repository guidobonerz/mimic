//! TCP server for the JSON-lines protocol.
//!
//! Threads: the emulation thread owns the [`Session`] and never waits on the
//! network; it receives requests through a channel and publishes events on a
//! broadcast channel. A second thread runs a single-threaded tokio runtime
//! that accepts connections and moves lines between sockets and channels.

use std::io;
use std::net::SocketAddr;
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, oneshot, watch};

use crate::protocol;
use crate::session::Session;

#[derive(Clone, Copy, Debug)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    /// Start running instead of paused.
    pub run: bool,
    /// Pace emulation to the machine's real frame rate while running.
    pub realtime: bool,
}

enum Msg {
    Request(protocol::Request, oneshot::Sender<Value>),
    Shutdown,
}

/// A running debug server. Dropping it does not stop it; call
/// [`DebugServer::shutdown`] or [`DebugServer::wait`].
pub struct DebugServer {
    addr: SocketAddr,
    msg_tx: mpsc::Sender<Msg>,
    stop_net: watch::Sender<bool>,
    emu: JoinHandle<()>,
    net: JoinHandle<()>,
}

impl DebugServer {
    /// Bind and start both threads. Only loopback addresses are accepted:
    /// remote access needs token authentication, which does not exist yet.
    pub fn start(mut session: Session, config: ServerConfig) -> io::Result<Self> {
        if !config.bind.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "debug server binds to loopback only (remote access needs a token, not yet supported)",
            ));
        }
        let std_listener = std::net::TcpListener::bind(config.bind)?;
        std_listener.set_nonblocking(true)?;
        let addr = std_listener.local_addr()?;

        let machine = session.machine_id().to_string();
        let (msg_tx, msg_rx) = mpsc::channel::<Msg>();
        let (event_tx, _) = broadcast::channel::<Value>(256);
        let (stop_net, stop_rx) = watch::channel(false);

        session.set_running(config.run);
        let events = event_tx.clone();
        let emu = thread::Builder::new()
            .name("emu".into())
            .spawn(move || emulation_loop(session, msg_rx, events, config.realtime))?;

        let net_msg_tx = msg_tx.clone();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let net = thread::Builder::new()
            .name("debug-net".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let listener = match TcpListener::from_std(std_listener) {
                        Ok(l) => l,
                        Err(_) => return,
                    };
                    accept_loop(listener, net_msg_tx, event_tx, machine, stop_rx).await;
                });
            })?;

        Ok(Self {
            addr,
            msg_tx,
            stop_net,
            emu,
            net,
        })
    }

    /// The bound address (useful with port 0).
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stop both threads and wait for them.
    pub fn shutdown(self) {
        let _ = self.msg_tx.send(Msg::Shutdown);
        let _ = self.stop_net.send(true);
        let _ = self.emu.join();
        let _ = self.net.join();
    }

    /// Block until the emulation thread ends (e.g. the process is killed).
    pub fn wait(self) {
        let _ = self.emu.join();
        let _ = self.stop_net.send(true);
        let _ = self.net.join();
    }
}

fn emulation_loop(
    mut session: Session,
    rx: mpsc::Receiver<Msg>,
    events: broadcast::Sender<Value>,
    realtime: bool,
) {
    let frame = session.frame_duration();
    let mut deadline = Instant::now();
    loop {
        // Commands first; block only while paused.
        let msg = if session.is_running() {
            match rx.try_recv() {
                Ok(m) => Some(m),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return,
            }
        } else {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        };
        match msg {
            Some(Msg::Shutdown) => return,
            Some(Msg::Request(req, reply)) => {
                let was_running = session.is_running();
                let _ = reply.send(session.handle(req));
                if !was_running && session.is_running() {
                    deadline = Instant::now();
                }
                continue;
            }
            None => {}
        }

        if let Some(ev) = session.run_slice() {
            let _ = events.send(ev);
        }
        if realtime && session.is_running() {
            deadline += frame;
            let now = Instant::now();
            if deadline > now {
                thread::sleep(deadline - now);
            } else if now - deadline > frame * 5 {
                // Fell far behind (debugger stop, slow host): do not catch up in a burst.
                deadline = now;
            }
        }
    }
}

async fn accept_loop(
    listener: TcpListener,
    msg_tx: mpsc::Sender<Msg>,
    events: broadcast::Sender<Value>,
    machine: String,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let hello = protocol::hello(&machine);
                tokio::spawn(client(stream, msg_tx.clone(), events.subscribe(), hello));
            }
            _ = stop.changed() => return,
        }
    }
}

async fn send_line(w: &mut (impl AsyncWriteExt + Unpin), v: &Value) -> io::Result<()> {
    let mut line = v.to_string();
    line.push('\n');
    w.write_all(line.as_bytes()).await
}

/// Longest accepted request line; longer lines close the connection.
const MAX_LINE: usize = 64 * 1024;

async fn client(
    stream: TcpStream,
    msg_tx: mpsc::Sender<Msg>,
    mut events: broadcast::Receiver<Value>,
    hello: Value,
) {
    let _ = stream.set_nodelay(true);
    let (r, mut w) = stream.into_split();
    let mut lines = BufReader::new(r).lines();
    if send_line(&mut w, &hello).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Ok(Some(line)) = line else { return };
                if line.len() > MAX_LINE {
                    return;
                }
                if line.trim().is_empty() {
                    continue;
                }
                let response = match protocol::parse_request(&line) {
                    Ok(req) => {
                        let (tx, rx) = oneshot::channel();
                        if msg_tx.send(Msg::Request(req, tx)).is_err() {
                            return;
                        }
                        match rx.await {
                            Ok(v) => v,
                            Err(_) => return,
                        }
                    }
                    Err(e) => e,
                };
                if send_line(&mut w, &response).await.is_err() {
                    return;
                }
            }
            ev = events.recv() => match ev {
                Ok(ev) => {
                    if send_line(&mut w, &ev).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
        }
    }
}
