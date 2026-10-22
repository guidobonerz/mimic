//! TCP server for the JSON-lines protocol.
//!
//! Threads: the emulation thread owns the [`Session`] and never waits on the
//! network; it receives messages through a channel ([`Endpoint`]) and
//! publishes events on a broadcast channel. A second thread runs a
//! single-threaded tokio runtime that accepts connections and moves lines
//! between sockets and channels.
//!
//! An optional second listener speaks the VICE binary monitor protocol
//! ([`crate::vice`]) for tools written for VICE.
//!
//! [`DebugServer::start`] runs its own emulation thread (headless).
//! Frontends with their own emulation loop (desktop) use
//! [`DebugServer::start_attached`] and call [`Endpoint::service`] from it.

use std::io;
use std::net::SocketAddr;
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, oneshot, watch};

use crate::protocol::{self, Command};
use crate::session::Session;
use crate::vice::{self, ViceState};

#[derive(Clone, Copy, Debug)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    /// VICE binary monitor listener (usually port 6502), if wanted.
    pub vice: Option<SocketAddr>,
    /// Start running instead of paused.
    pub run: bool,
    /// Pace emulation to the machine's real frame rate while running.
    pub realtime: bool,
}

/// A job for the emulation thread.
pub(crate) enum Msg {
    Request(protocol::Request, oneshot::Sender<Value>),
    /// Run code against the session (adapters, subscriptions).
    Exec(Box<dyn FnOnce(&mut Session) + Send>),
    Shutdown,
}

/// Runs code against the session on the emulation thread, e.g. remote
/// input from the preview page. Never blocks.
#[derive(Clone)]
pub struct Control {
    tx: mpsc::Sender<Msg>,
}

impl Control {
    /// Queue `f`; false if the emulation thread is gone.
    pub fn exec(&self, f: impl FnOnce(&mut Session) + Send + 'static) -> bool {
        self.tx.send(Msg::Exec(Box::new(f))).is_ok()
    }
}

/// The emulation side of a debug server: owned by whichever thread runs
/// the session.
pub struct Endpoint {
    rx: mpsc::Receiver<Msg>,
    events: broadcast::Sender<Value>,
}

/// What [`Endpoint::service`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    /// Handled zero or more messages; carry on.
    Continue,
    /// The server shut down; stop emulating for it.
    Shutdown,
}

impl Endpoint {
    /// Handle queued messages. Blocks up to `wait` for the first one
    /// (`None`: never blocks). Events the session produced are published.
    pub fn service(&self, session: &mut Session, wait: Option<Duration>) -> Service {
        let mut first = match wait {
            Some(d) => match self.rx.recv_timeout(d) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return Service::Shutdown,
            },
            None => None,
        };
        loop {
            let msg = match first.take() {
                Some(m) => m,
                None => match self.rx.try_recv() {
                    Ok(m) => m,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Service::Shutdown,
                },
            };
            match msg {
                Msg::Shutdown => return Service::Shutdown,
                Msg::Request(req, reply) => {
                    let response = session.handle(req);
                    // Events caused by a command follow its response.
                    let _ = reply.send(response);
                }
                Msg::Exec(f) => f(session),
            }
            self.publish(session);
        }
        Service::Continue
    }

    /// Send the session's pending events to all connections.
    pub fn publish(&self, session: &mut Session) {
        for ev in session.take_events() {
            let _ = self.events.send(ev);
        }
    }
}

/// A running debug server. Dropping it does not stop it; call
/// [`DebugServer::shutdown`] or [`DebugServer::wait`].
pub struct DebugServer {
    addr: SocketAddr,
    vice_addr: Option<SocketAddr>,
    msg_tx: mpsc::Sender<Msg>,
    stop_net: watch::Sender<bool>,
    emu: Option<JoinHandle<()>>,
    net: JoinHandle<()>,
}

impl DebugServer {
    /// Bind and start the network thread and an emulation thread that owns
    /// `session`. Only loopback addresses are accepted: remote access needs
    /// token authentication, which does not exist yet.
    pub fn start(mut session: Session, config: ServerConfig) -> io::Result<Self> {
        let (mut server, endpoint) = Self::start_attached(config, session.machine_id())?;
        session.set_running(config.run);
        server.emu = Some(
            thread::Builder::new()
                .name("emu".into())
                .spawn(move || emulation_loop(session, endpoint, config.realtime))?,
        );
        Ok(server)
    }

    /// Bind and start only the network thread. The caller runs the session
    /// and calls [`Endpoint::service`] regularly (at least once per frame,
    /// blocking with a timeout while paused).
    pub fn start_attached(config: ServerConfig, machine: &str) -> io::Result<(Self, Endpoint)> {
        let loopback = |a: &SocketAddr| a.ip().is_loopback();
        if !loopback(&config.bind) || config.vice.is_some_and(|a| !loopback(&a)) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "debug server binds to loopback only (remote access needs a token, not yet supported)",
            ));
        }
        let std_listener = std::net::TcpListener::bind(config.bind)?;
        std_listener.set_nonblocking(true)?;
        let addr = std_listener.local_addr()?;
        let vice_listener = match config.vice {
            Some(a) => {
                let l = std::net::TcpListener::bind(a)?;
                l.set_nonblocking(true)?;
                Some(l)
            }
            None => None,
        };
        let vice_addr = vice_listener.as_ref().map(|l| l.local_addr()).transpose()?;

        let machine = machine.to_string();
        let (msg_tx, msg_rx) = mpsc::channel::<Msg>();
        let (event_tx, _) = broadcast::channel::<Value>(1024);
        let (stop_net, stop_rx) = watch::channel(false);
        let endpoint = Endpoint {
            rx: msg_rx,
            events: event_tx.clone(),
        };

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
                    if let Some(l) = vice_listener.and_then(|l| TcpListener::from_std(l).ok()) {
                        tokio::spawn(vice_accept_loop(
                            l,
                            net_msg_tx.clone(),
                            event_tx.clone(),
                            stop_rx.clone(),
                        ));
                    }
                    accept_loop(listener, net_msg_tx, event_tx, machine, stop_rx).await;
                });
            })?;

        Ok((
            Self {
                addr,
                vice_addr,
                msg_tx,
                stop_net,
                emu: None,
                net,
            },
            endpoint,
        ))
    }

    /// The bound address (useful with port 0).
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// A handle for running code on the emulation thread.
    pub fn control(&self) -> Control {
        Control {
            tx: self.msg_tx.clone(),
        }
    }

    /// The bound VICE binary monitor address, if enabled.
    pub fn vice_addr(&self) -> Option<SocketAddr> {
        self.vice_addr
    }

    /// Stop both threads and wait for them.
    pub fn shutdown(self) {
        let _ = self.msg_tx.send(Msg::Shutdown);
        let _ = self.stop_net.send(true);
        if let Some(emu) = self.emu {
            let _ = emu.join();
        }
        let _ = self.net.join();
    }

    /// Block until the emulation thread ends (e.g. the process is killed).
    pub fn wait(self) {
        if let Some(emu) = self.emu {
            let _ = emu.join();
        }
        let _ = self.stop_net.send(true);
        let _ = self.net.join();
    }
}

fn emulation_loop(mut session: Session, endpoint: Endpoint, realtime: bool) {
    let frame = session.frame_duration();
    let mut deadline = Instant::now();
    loop {
        // Commands first; block only while paused.
        let was_running = session.is_running();
        let wait = (!was_running).then_some(Duration::from_millis(250));
        if endpoint.service(&mut session, wait) == Service::Shutdown {
            return;
        }
        if !session.is_running() {
            continue;
        }
        if !was_running {
            deadline = Instant::now();
        }
        session.run_slice();
        endpoint.publish(&mut session);
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
/// Large enough for a base64 savestate.
const MAX_LINE: usize = 16 * 1024 * 1024;

/// Per-connection event filter.
#[derive(Default)]
struct Subscriptions {
    /// Optional events this connection asked for.
    events: Vec<String>,
}

impl Subscriptions {
    fn wants(&self, ev: &Value) -> bool {
        let name = ev["event"].as_str().unwrap_or_default();
        !protocol::OPTIONAL_EVENTS.contains(&name) || self.events.iter().any(|e| e == name)
    }
}

async fn client(
    stream: TcpStream,
    msg_tx: mpsc::Sender<Msg>,
    mut events: broadcast::Receiver<Value>,
    hello: Value,
) {
    let _ = stream.set_nodelay(true);
    let (r, mut w) = stream.into_split();
    let mut lines = BufReader::new(r).lines();
    let mut subs = Subscriptions::default();
    if send_line(&mut w, &hello).await.is_ok() {
        loop {
            tokio::select! {
                line = lines.next_line() => {
                    let Ok(Some(line)) = line else { break };
                    if line.len() > MAX_LINE {
                        break;
                    }
                    if line.trim().is_empty() {
                        continue;
                    }
                    let response = match protocol::parse_request(&line) {
                        Ok(protocol::Request { id, cmd: Command::Subscribe { events } }) => {
                            subscribe(&msg_tx, &mut subs, id, events)
                        }
                        Ok(req) => {
                            let (tx, rx) = oneshot::channel();
                            if msg_tx.send(Msg::Request(req, tx)).is_err() {
                                break;
                            }
                            match rx.await {
                                Ok(v) => v,
                                Err(_) => break,
                            }
                        }
                        Err(e) => e,
                    };
                    if send_line(&mut w, &response).await.is_err() {
                        break;
                    }
                }
                ev = events.recv() => match ev {
                    Ok(ev) => {
                        if subs.wants(&ev) && send_line(&mut w, &ev).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            }
        }
    }
    for event in subs.events {
        let _ = msg_tx.send(Msg::Exec(Box::new(move |s| s.subscriber(&event, false))));
    }
}

/// `subscribe` replaces this connection's set of optional events.
fn subscribe(
    msg_tx: &mpsc::Sender<Msg>,
    subs: &mut Subscriptions,
    id: u64,
    events: Vec<String>,
) -> Value {
    if let Some(bad) = events
        .iter()
        .find(|e| !protocol::OPTIONAL_EVENTS.contains(&e.as_str()))
    {
        return protocol::err(Some(id), format!("unknown optional event '{bad}'"));
    }
    let mut wanted: Vec<String> = events.clone();
    wanted.sort();
    wanted.dedup();
    for e in subs.events.iter().filter(|e| !wanted.contains(e)) {
        let e = e.clone();
        let _ = msg_tx.send(Msg::Exec(Box::new(move |s| s.subscriber(&e, false))));
    }
    for e in wanted.iter().filter(|e| !subs.events.contains(e)) {
        let e = e.clone();
        let _ = msg_tx.send(Msg::Exec(Box::new(move |s| s.subscriber(&e, true))));
    }
    subs.events = wanted;
    protocol::ok(id, serde_json::json!({"events": events}))
}

async fn vice_accept_loop(
    listener: TcpListener,
    msg_tx: mpsc::Sender<Msg>,
    events: broadcast::Sender<Value>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                tokio::spawn(vice_client(stream, msg_tx.clone(), events.subscribe()));
            }
            _ = stop.changed() => return,
        }
    }
}

/// Run adapter code with this connection's state on the emulation thread.
/// `None` if the emulation thread is gone.
async fn vice_exec(
    msg_tx: &mpsc::Sender<Msg>,
    state: &mut Option<ViceState>,
    f: impl FnOnce(&mut ViceState, &mut Session) -> Vec<u8> + Send + 'static,
) -> Option<Vec<u8>> {
    let mut st = state.take()?;
    let (tx, rx) = oneshot::channel();
    let job = Msg::Exec(Box::new(move |s| {
        let out = f(&mut st, s);
        let _ = tx.send((st, out));
    }));
    msg_tx.send(job).ok()?;
    let (st, out) = rx.await.ok()?;
    *state = Some(st);
    Some(out)
}

async fn vice_client(
    stream: TcpStream,
    msg_tx: mpsc::Sender<Msg>,
    mut events: broadcast::Receiver<Value>,
) {
    let _ = stream.set_nodelay(true);
    let (mut r, mut w) = stream.into_split();
    let mut buf = Vec::new();
    let mut state = Some(ViceState::default());
    loop {
        tokio::select! {
            n = r.read_buf(&mut buf) => {
                if !matches!(n, Ok(n) if n > 0) {
                    break;
                }
                loop {
                    let (req, used) = match vice::parse(&buf) {
                        Ok(Some(r)) => r,
                        Ok(None) => break,
                        Err(_) => return,
                    };
                    buf.drain(..used);
                    let Some(out) =
                        vice_exec(&msg_tx, &mut state, move |st, s| vice::handle(st, s, &req)).await
                    else {
                        return;
                    };
                    if w.write_all(&out).await.is_err() {
                        return;
                    }
                }
            }
            ev = events.recv() => match ev {
                Ok(ev) => {
                    if !matches!(ev["event"].as_str(), Some("stopped" | "resumed")) {
                        continue;
                    }
                    let Some(out) =
                        vice_exec(&msg_tx, &mut state, move |st, s| vice::on_event(st, s, &ev)).await
                    else {
                        return;
                    };
                    if w.write_all(&out).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
        }
    }
    // Checkpoints belong to the connection here (VICE keeps them);
    // remove them so they do not stop a machine nobody watches.
    let _ = vice_exec(&msg_tx, &mut state, |st, s| {
        vice::disconnect(st, s);
        Vec::new()
    })
    .await;
}
