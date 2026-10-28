//! Reproducible, unprivileged network faults for loopback integration tests.
//!
//! UDP and TCP share the advertised port, but have independent upstreams.
//! Counters and `skip_first` are per direction across all UDP clients. A burst
//! drops the first M packets of each N-packet block, after the skipped prefix.
//! UDP forwarding counts physical datagrams (including duplicate copies).

use std::collections::{BTreeMap, HashMap};
use std::future::{pending, poll_fn};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream, UdpSocket};
use tokio::sync::{mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{sleep, sleep_until, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    ToHost,
    ToClient,
}

impl Dir {
    pub fn index(self) -> usize {
        match self {
            Self::ToHost => 0,
            Self::ToClient => 1,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UdpRule {
    pub loss_pct: f32,
    pub burst: Option<(u32, u32)>,
    pub blackhole: bool,
    pub delay_ms: u32,
    pub jitter_ms: u32,
    pub duplicate_pct: f32,
    pub reorder_pct: f32,
    pub corrupt_pct: f32,
    pub max_datagram: Option<usize>,
    /// Kilobits per second (1000 bits); the token bucket holds 100 ms of credit.
    pub rate_kbps: Option<u32>,
    pub skip_first: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TcpMode {
    #[default]
    Forward,
    Refuse,
    Blackhole,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TcpRule {
    pub mode: TcpMode,
    pub delay_ms: u32,
    pub rate_kbps: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirStats {
    pub received: u64,
    pub received_bytes: u64,
    pub forwarded_bytes: u64,
    /// Rate-limit queue delay in 10 ms buckets (last bucket includes overflow).
    pub rate_queue_histogram: [u64; 21],
    pub rate_queue_dropped: u64,
    /// Actual proxy residence (including scheduling), in 10 ms buckets.
    pub forward_delay_histogram: [u64; 21],
    pub forward_delay_max_us: u64,
    /// High water of the UDP engine's scheduled packets, in either direction.
    pub scheduled_queue_max: u64,
    pub reply_queue_max: u64,
    pub reply_delay_max_us: u64,
    pub forwarded: u64,
    pub dropped: u64,
    pub duplicated: u64,
    pub reordered: u64,
    pub corrupted: u64,
    pub oversize_dropped: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxyStats {
    pub to_host: DirStats,
    pub to_client: DirStats,
    pub tcp_accepted: u64,
    pub tcp_bytes_to_host: u64,
    pub tcp_bytes_to_client: u64,
    pub udp_clients: u64,
}

impl ProxyStats {
    pub fn direction(&mut self, dir: Dir) -> &mut DirStats {
        match dir {
            Dir::ToHost => &mut self.to_host,
            Dir::ToClient => &mut self.to_client,
        }
    }
}

/// Xorshift64*: zero seeds are mapped to a fixed nonzero state.
#[derive(Debug)]
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            seed
        })
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub fn chance(&mut self, percent: f32) -> bool {
        // Disabled/certain faults consume no entropy, so unrelated default
        // reverse traffic cannot change a forward-direction loss sequence.
        if percent >= 100.0 {
            true
        } else if percent > 0.0 {
            (self.next_u64() as f64 / (u64::MAX as f64 + 1.0)) * 100.0 < f64::from(percent)
        } else {
            false
        }
    }

    pub fn delay(&mut self, rule: &UdpRule) -> Duration {
        let jitter = i64::from(rule.jitter_ms);
        let offset = if jitter == 0 {
            0
        } else {
            (self.next_u64() % (2 * jitter as u64 + 1)) as i64 - jitter
        };
        Duration::from_millis((i64::from(rule.delay_ms) + offset).max(0) as u64)
    }
}

#[derive(Debug)]
pub struct State {
    pub udp: [UdpRule; 2],
    pub udp_revision: [u64; 2],
    pub tcp: TcpRule,
    pub cut: bool,
    pub stopped: bool,
    pub listener: Option<TcpListener>,
    pub stats: ProxyStats,
}

#[derive(Debug)]
pub struct Shared {
    pub state: Mutex<State>,
    pub rng: Mutex<Rng>,
    pub changed: watch::Sender<u64>,
}

impl Shared {
    pub fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn notify(&self) {
        self.changed
            .send_modify(|version| *version = version.wrapping_add(1));
    }
}

/// Owns all sockets and tasks. `shutdown` waits for their release; dropping the
/// proxy cancels them without waiting for the runtime to poll their destruction.
pub struct FaultProxy {
    addr: SocketAddr,
    shared: Arc<Shared>,
    tasks: Vec<JoinHandle<()>>,
}

impl FaultProxy {
    pub async fn start(
        udp_upstream: SocketAddr,
        tcp_upstream: Option<SocketAddr>,
        seed: u64,
    ) -> io::Result<Self> {
        let mut attempts = 0;
        loop {
            attempts += 1;
            match Self::start_on(
                SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
                udp_upstream,
                tcp_upstream,
                seed,
            )
            .await
            {
                // UDP :0 can select a port already used by TCP. Retry only
                // ephemeral allocation; start_on must preserve explicit binds.
                Err(error) if error.kind() == io::ErrorKind::AddrInUse && attempts < 8 => {}
                result => return result,
            }
        }
    }

    /// Start the proxy on an explicit loopback address. This is an additive
    /// alternative for callers that need to stay inside an allocated port
    /// range; [`Self::start`] keeps its ephemeral-port behavior.
    pub async fn start_on(
        bind: SocketAddr,
        udp_upstream: SocketAddr,
        tcp_upstream: Option<SocketAddr>,
        seed: u64,
    ) -> io::Result<Self> {
        let udp = Arc::new(UdpSocket::bind(bind).await?);
        let addr = udp.local_addr()?;
        let listener = Self::listen(addr)?;
        let (changed, _) = watch::channel(0);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                udp: [UdpRule::default(), UdpRule::default()],
                udp_revision: [0; 2],
                tcp: TcpRule::default(),
                cut: false,
                stopped: false,
                listener: Some(listener),
                stats: ProxyStats::default(),
            }),
            rng: Mutex::new(Rng::new(seed)),
            changed,
        });
        let udp_task = tokio::spawn(UdpEngine::run(udp, udp_upstream, Arc::clone(&shared)));
        let tcp_task = tokio::spawn(Self::accept_tcp(addr, tcp_upstream, Arc::clone(&shared)));
        Ok(Self {
            addr,
            shared,
            tasks: vec![udp_task, tcp_task],
        })
    }

    pub fn listen(addr: SocketAddr) -> io::Result<TcpListener> {
        let socket = TcpSocket::new_v4()?;
        socket.set_reuseaddr(true)?;
        socket.bind(addr)?;
        socket.listen(128)
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn set_udp(&self, dir: Dir, rule: UdpRule) {
        let mut state = self.shared.lock();
        state.udp[dir.index()] = rule;
        state.udp_revision[dir.index()] = state.udp_revision[dir.index()].wrapping_add(1);
        drop(state);
        self.shared.notify();
    }

    pub fn set_tcp(&self, rule: TcpRule) {
        let mut state = self.shared.lock();
        state.tcp = rule;
        if state.tcp.mode == TcpMode::Refuse {
            // No accept task owns a cloned listener: dropping this closes the
            // actual port before set_tcp returns, including its accept backlog.
            state.listener = None;
        } else if state.listener.is_none() {
            state.listener = Self::listen(self.addr).ok();
        }
        drop(state);
        self.shared.notify();
    }

    pub fn cut(&self) {
        let mut state = self.shared.lock();
        state.udp[0].blackhole = true;
        state.udp[1].blackhole = true;
        state.udp_revision = state.udp_revision.map(|revision| revision.wrapping_add(1));
        state.cut = true;
        state.tcp.mode = TcpMode::Blackhole;
        if state.listener.is_none() {
            state.listener = Self::listen(self.addr).ok();
        }
        drop(state);
        self.shared.notify();
    }

    /// Reset rules without resetting counters or NAT mappings. TCP data held
    /// during a cut is retained and forwarded on restoration, in stream order.
    pub fn restore(&self) {
        let mut state = self.shared.lock();
        state.udp = [UdpRule::default(), UdpRule::default()];
        state.udp_revision = state.udp_revision.map(|revision| revision.wrapping_add(1));
        state.tcp = TcpRule::default();
        state.cut = false;
        if state.listener.is_none() {
            state.listener = Self::listen(self.addr).ok();
        }
        drop(state);
        self.shared.notify();
    }

    pub fn stats(&self) -> ProxyStats {
        self.shared.lock().stats.clone()
    }

    pub async fn shutdown(mut self) {
        self.shared.lock().stopped = true;
        self.shared.notify();
        for task in self.tasks.drain(..) {
            let _ = task.await;
        }
    }

    pub async fn accept_tcp(addr: SocketAddr, upstream: Option<SocketAddr>, shared: Arc<Shared>) {
        let mut changed = shared.changed.subscribe();
        let mut connections = JoinSet::new();
        loop {
            if shared.lock().stopped {
                break;
            }
            // Retry the rare bind failure (e.g. another process temporarily
            // owns the TCP port). The public setters cannot return io::Error.
            {
                let mut state = shared.lock();
                if state.listener.is_none() && state.tcp.mode != TcpMode::Refuse {
                    state.listener = Self::listen(addr).ok();
                }
            }
            tokio::select! {
                biased;
                _ = changed.changed() => {}
                accepted = poll_fn(|cx| {
                    let state = shared.lock();
                    match &state.listener {
                        Some(listener) => listener.poll_accept(cx),
                        None => std::task::Poll::Pending,
                    }
                }) => {
                    if let Ok((client, _)) = accepted {
                        shared.lock().stats.tcp_accepted += 1;
                        connections.spawn(Self::tcp_connection(client, upstream, Arc::clone(&shared)));
                    }
                }
                Some(_) = connections.join_next(), if !connections.is_empty() => {}
                _ = sleep(Duration::from_millis(10)) => {}
            }
        }
        shared.lock().listener = None;
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    }

    pub async fn tcp_connection(
        client: TcpStream,
        upstream: Option<SocketAddr>,
        shared: Arc<Shared>,
    ) -> io::Result<()> {
        let mut changed = shared.changed.subscribe();
        loop {
            let ready = {
                let state = shared.lock();
                !state.cut && state.tcp.mode != TcpMode::Blackhole
            };
            if ready {
                break;
            }
            if changed.changed().await.is_err() {
                return Ok(());
            }
        }
        let Some(upstream) = upstream else {
            // No TCP upstream represents a reachable blackhole, rather than
            // an unrelated service or an immediate FIN.
            pending::<()>().await;
            return Ok(());
        };
        let host = TcpStream::connect(upstream).await?;
        let (client_read, client_write) = client.into_split();
        let (host_read, host_write) = host.into_split();
        tokio::try_join!(
            Self::pump(client_read, host_write, Dir::ToHost, Arc::clone(&shared)),
            Self::pump(host_read, client_write, Dir::ToClient, shared),
        )?;
        Ok(())
    }

    pub async fn pump<R, W>(
        mut read: R,
        mut write: W,
        dir: Dir,
        shared: Arc<Shared>,
    ) -> io::Result<()>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut changed = shared.changed.subscribe();
        let mut buffer = vec![0; 16 * 1024];
        let mut bucket = TokenBucket::default();
        loop {
            while shared.lock().cut {
                if changed.changed().await.is_err() {
                    return Ok(());
                }
            }
            let size = tokio::select! {
                biased;
                _ = changed.changed() => continue,
                size = read.read(&mut buffer) => size?,
            };
            if size == 0 {
                write.shutdown().await?;
                return Ok(());
            }
            let arrived = Instant::now();
            let mut offset = 0;
            let mut reservation = None;
            while offset < size {
                let (cut, rule) = {
                    let state = shared.lock();
                    (state.cut, state.tcp.clone())
                };
                if cut {
                    reservation = None;
                    changed
                        .changed()
                        .await
                        .map_err(|_| io::ErrorKind::Interrupted)?;
                    continue;
                }
                let due = match &reservation {
                    Some((previous, due)) if previous == &rule => *due,
                    _ => {
                        let base = (arrived + Duration::from_millis(u64::from(rule.delay_ms)))
                            .max(Instant::now());
                        let due = bucket.reserve(base, size - offset, rule.rate_kbps);
                        reservation = Some((rule, due));
                        due
                    }
                };
                tokio::select! {
                    biased;
                    _ = changed.changed() => continue,
                    _ = sleep_until(due) => {}
                }
                let written = tokio::select! {
                    biased;
                    _ = changed.changed() => continue,
                    written = write.write(&buffer[offset..size]) => written?,
                };
                if written == 0 {
                    return Err(io::ErrorKind::WriteZero.into());
                }
                offset += written;
                let mut state = shared.lock();
                match dir {
                    Dir::ToHost => state.stats.tcp_bytes_to_host += written as u64,
                    Dir::ToClient => state.stats.tcp_bytes_to_client += written as u64,
                }
            }
        }
    }
}

impl Drop for FaultProxy {
    fn drop(&mut self) {
        self.shared.lock().listener = None;
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// A virtual token bucket: reservations include queued bytes, preventing a
/// burst from bypassing the rate limit. Changing the rate starts a fresh bucket.
#[derive(Debug)]
pub struct TokenBucket {
    pub rate: Option<u32>,
    pub available: Instant,
    pub credit: f64,
}

impl Default for TokenBucket {
    fn default() -> Self {
        Self {
            rate: None,
            available: Instant::now(),
            credit: 0.0,
        }
    }
}

impl TokenBucket {
    pub fn reserve(&mut self, now: Instant, bytes: usize, rate: Option<u32>) -> Instant {
        if self.rate != rate {
            self.rate = rate;
            self.available = now;
            self.credit = f64::from(rate.unwrap_or(0)) * 125.0 * 0.1;
        }
        let Some(rate) = rate else { return now };
        if rate == 0 {
            // Zero bandwidth has no tokens. UDP rejects this as >200 ms of
            // queuing; TCP waits, with rule changes able to interrupt the wait.
            return now + Duration::from_secs(24 * 60 * 60);
        }
        let bytes_per_second = f64::from(rate) * 125.0;
        if now > self.available {
            self.credit = (self.credit + (now - self.available).as_secs_f64() * bytes_per_second)
                .min(bytes_per_second * 0.1);
            self.available = now;
        }
        let missing = (bytes as f64 - self.credit).max(0.0);
        self.credit = (self.credit - bytes as f64).max(0.0);
        self.available += Duration::from_secs_f64(missing / bytes_per_second);
        self.available
    }
}

#[derive(Debug)]
pub struct Packet {
    pub dir: Dir,
    pub client: SocketAddr,
    pub upstream: Arc<UdpSocket>,
    pub data: Vec<u8>,
    pub arrived: Instant,
    pub ordinal: u64,
    pub sequence: u64,
}

#[derive(Debug)]
pub struct Planned {
    pub packet: Packet,
    pub data: Vec<u8>,
    pub due: Instant,
    pub copies: usize,
    pub corrupted: bool,
    pub reordered: bool,
}

#[derive(Default)]
pub struct UdpEngine {
    pub queue: BTreeMap<(Instant, u64), Planned>,
    pub held: [Option<Planned>; 2],
    pub buckets: [TokenBucket; 2],
    pub revision: [u64; 2],
    pub sequence: u64,
}

impl UdpEngine {
    pub fn enqueue(&mut self, packet: Packet, shared: &Shared) {
        let index = packet.dir.index();
        let (rule, cut) = {
            let state = shared.lock();
            (state.udp[index].clone(), state.cut)
        };
        if cut {
            shared.lock().stats.direction(packet.dir).dropped += 1;
            return;
        }
        let skip = packet.ordinal <= u64::from(rule.skip_first);
        let mut data = packet.data.clone();
        let mut copies = 1;
        let mut corrupted = false;
        let mut reorder = false;
        let mut due = packet.arrived;
        if !skip {
            let oversize = rule.max_datagram.is_some_and(|max| data.len() > max);
            let burst = rule.burst.is_some_and(|(n, m)| {
                n > 0
                    && (packet.ordinal - u64::from(rule.skip_first) - 1) % u64::from(n)
                        < u64::from(m)
            });
            let mut rng = shared.rng.lock().unwrap_or_else(|e| e.into_inner());
            if rule.blackhole || oversize || burst || rng.chance(rule.loss_pct) {
                let mut state = shared.lock();
                let stats = state.stats.direction(packet.dir);
                stats.dropped += 1;
                stats.oversize_dropped += u64::from(oversize);
                return;
            }
            due += rng.delay(&rule);
            if rng.chance(rule.duplicate_pct) {
                copies = 2;
            }
            if !data.is_empty() && rng.chance(rule.corrupt_pct) {
                let byte = (rng.next_u64() % data.len() as u64) as usize;
                data[byte] ^= 1 << (rng.next_u64() % 8);
                corrupted = true;
            }
            if self.held[index].is_none() {
                reorder = rng.chance(rule.reorder_pct);
            }
        }
        let mut planned = Planned {
            packet,
            data,
            due,
            copies,
            corrupted,
            reordered: reorder,
        };
        if reorder {
            self.held[index] = Some(planned);
            return;
        }
        self.schedule(&mut planned, skip, &rule, shared);
        let first_due = planned.due;
        self.queue
            .insert((planned.due, planned.packet.sequence), planned);
        if let Some(mut held) = self.held[index].take() {
            // Even differing jitter must put the held datagram after its
            // successor. Sequence alone cannot break ties in the right order.
            held.due = held.due.max(first_due) + Duration::from_nanos(1);
            self.schedule(&mut held, false, &rule, shared);
            self.queue.insert((held.due, held.packet.sequence), held);
        }
    }

    pub fn schedule(&mut self, planned: &mut Planned, skip: bool, rule: &UdpRule, shared: &Shared) {
        if skip {
            return;
        }
        let base = planned.due.max(Instant::now());
        let bucket = &mut self.buckets[planned.packet.dir.index()];
        // Rejected reservations must not consume future tokens.
        let snapshot = (bucket.rate, bucket.available, bucket.credit);
        let due = bucket.reserve(base, planned.data.len() * planned.copies, rule.rate_kbps);
        let queue_delay = due.saturating_duration_since(base);
        if rule.rate_kbps.is_some() {
            let bin = (queue_delay.as_millis() / 10).min(20) as usize;
            shared
                .lock()
                .stats
                .direction(planned.packet.dir)
                .rate_queue_histogram[bin] += 1;
        }
        if queue_delay > Duration::from_millis(200) {
            (bucket.rate, bucket.available, bucket.credit) = snapshot;
            let mut state = shared.lock();
            let stats = state.stats.direction(planned.packet.dir);
            stats.dropped += 1;
            stats.rate_queue_dropped += 1;
            planned.copies = 0;
        } else {
            planned.due = due;
        }
    }

    pub fn refresh(&mut self, shared: &Shared) {
        let revision = shared.lock().udp_revision;
        if revision == self.revision {
            return;
        }
        let changed = [
            revision[0] != self.revision[0],
            revision[1] != self.revision[1],
        ];
        self.revision = revision;
        let mut packets = Vec::new();
        for (key, planned) in std::mem::take(&mut self.queue) {
            if !changed[planned.packet.dir.index()] {
                self.queue.insert(key, planned);
            } else if planned.copies > 0 {
                packets.push(planned.packet);
            }
        }
        for (index, changed) in changed.into_iter().enumerate() {
            if changed {
                self.buckets[index] = TokenBucket::default();
                if let Some(held) = self.held[index].take() {
                    packets.push(held.packet);
                }
            }
        }
        packets.sort_by_key(|packet| packet.sequence);
        for packet in packets {
            self.enqueue(packet, shared);
        }
    }

    pub async fn run(main: Arc<UdpSocket>, upstream: SocketAddr, shared: Arc<Shared>) {
        let mut changed = shared.changed.subscribe();
        let (replies_tx, mut replies_rx) = mpsc::channel(1024);
        let mut readers = JoinSet::new();
        let mut clients: HashMap<SocketAddr, Arc<UdpSocket>> = HashMap::new();
        let mut buffer = vec![0; 65_536];
        let mut engine = Self::default();
        loop {
            if shared.lock().stopped {
                break;
            }
            engine.refresh(&shared);
            let deadline = engine.queue.first_key_value().map(|(key, _)| key.0);
            tokio::select! {
                _ = changed.changed() => {}
                // Fair selection keeps sustained client traffic from starving
                // replies, while delay/jitter never block the receive path.
                _ = async {
                    match deadline {
                        Some(deadline) => sleep_until(deadline).await,
                        None => pending::<()>().await,
                    }
                } => {
                    if let Some((_, planned)) = engine.queue.pop_first() {
                        engine.forward(planned, &main, &shared).await;
                    }
                }
                received = main.recv_from(&mut buffer) => {
                    let Ok((size, client)) = received else { break };
                    let socket = if let Some(socket) = clients.get(&client) {
                        Arc::clone(socket)
                    } else {
                        let Ok(socket) = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await else { continue };
                        if socket.connect(upstream).await.is_err() { continue; }
                        let socket = Arc::new(socket);
                        clients.insert(client, Arc::clone(&socket));
                        shared.lock().stats.udp_clients += 1;
                        let reply_socket = Arc::clone(&socket);
                        let replies = replies_tx.clone();
                        readers.spawn(async move {
                            let mut buffer = vec![0; 65_536];
                            while let Ok(size) = reply_socket.recv(&mut buffer).await {
                                if replies.send((client, Arc::clone(&reply_socket), buffer[..size].to_vec(), Instant::now())).await.is_err() {
                                    break;
                                }
                            }
                        });
                        socket
                    };
                    engine.receive(Dir::ToHost, client, socket, buffer[..size].to_vec(), &shared);
                }
                Some((client, socket, data, received_at)) = replies_rx.recv() => {
                    {
                        let mut state = shared.lock();
                        let stats = state.stats.direction(Dir::ToClient);
                        stats.reply_queue_max = stats.reply_queue_max.max(replies_rx.len() as u64);
                        stats.reply_delay_max_us = stats.reply_delay_max_us.max(received_at.elapsed().as_micros() as u64);
                    }
                    engine.receive(Dir::ToClient, client, socket, data, &shared);
                }
                Some(_) = readers.join_next(), if !readers.is_empty() => {}
            }
        }
        readers.abort_all();
        while readers.join_next().await.is_some() {}
    }

    pub fn receive(
        &mut self,
        dir: Dir,
        client: SocketAddr,
        upstream: Arc<UdpSocket>,
        data: Vec<u8>,
        shared: &Shared,
    ) {
        let ordinal = {
            let mut state = shared.lock();
            let stats = state.stats.direction(dir);
            stats.received += 1;
            stats.received_bytes += data.len() as u64;
            stats.received
        };
        self.sequence += 1;
        self.enqueue(
            Packet {
                dir,
                client,
                upstream,
                data,
                arrived: Instant::now(),
                ordinal,
                sequence: self.sequence,
            },
            shared,
        );
    }

    pub async fn forward(&mut self, planned: Planned, main: &UdpSocket, shared: &Shared) {
        if planned.copies == 0 {
            return;
        }
        let index = planned.packet.dir.index();
        let socket = match planned.packet.dir {
            Dir::ToHost => planned.packet.upstream.as_ref(),
            Dir::ToClient => main,
        };
        let mut changed = shared.changed.subscribe();
        let mut sent = 0;
        'copies: for _ in 0..planned.copies {
            loop {
                tokio::select! {
                    biased;
                    _ = changed.changed() => {}
                    ready = socket.writable() => {
                        if ready.is_err() { break 'copies; }
                    }
                }
                let state = shared.lock();
                if state.stopped {
                    return;
                }
                // Check the rule before *each* physical send, including the
                // duplicate. Holding the lock over a nonblocking syscall makes
                // setters take effect before their return, even under pressure.
                if state.udp_revision[index] != self.revision[index] {
                    drop(state);
                    if sent == 0 {
                        self.queue
                            .insert((planned.due, planned.packet.sequence), planned);
                        self.refresh(shared);
                        return;
                    }
                    // The original already went out; suppress a stale duplicate
                    // rather than replaying the original under the new rules.
                    break 'copies;
                }
                let result = match planned.packet.dir {
                    Dir::ToHost => socket.try_send(&planned.data),
                    Dir::ToClient => socket.try_send_to(&planned.data, planned.packet.client),
                };
                drop(state);
                match result {
                    Ok(_) => {
                        sent += 1;
                        break;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                    Err(_) => break,
                }
            }
        }
        let mut state = shared.lock();
        let stats = state.stats.direction(planned.packet.dir);
        let delay = planned.packet.arrived.elapsed();
        stats.forward_delay_histogram[(delay.as_millis() / 10).min(20) as usize] += 1;
        stats.forward_delay_max_us = stats.forward_delay_max_us.max(delay.as_micros() as u64);
        stats.scheduled_queue_max = stats.scheduled_queue_max.max(self.queue.len() as u64 + 1);
        stats.forwarded += sent;
        stats.forwarded_bytes += sent * planned.data.len() as u64;
        if sent == 0 {
            stats.dropped += 1;
        } else {
            stats.duplicated += sent.saturating_sub(1);
            stats.corrupted += u64::from(planned.corrupted);
            stats.reordered += u64::from(planned.reordered);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pure_jitter_keeps_a_burst_within_twice_the_jitter() {
        let socket = Arc::new(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap());
        let addr = socket.local_addr().unwrap();
        let (changed, _) = watch::channel(0);
        let rule = UdpRule {
            delay_ms: 100,
            jitter_ms: 20,
            ..UdpRule::default()
        };
        let shared = Shared {
            state: Mutex::new(State {
                udp: [rule.clone(), rule],
                udp_revision: [0; 2],
                tcp: TcpRule::default(),
                cut: false,
                stopped: false,
                listener: None,
                stats: ProxyStats::default(),
            }),
            rng: Mutex::new(Rng::new(0xa17f_2301)),
            changed,
        };
        for dir in [Dir::ToHost, Dir::ToClient] {
            let mut engine = UdpEngine::default();
            let arrived = Instant::now();
            for sequence in 0..256 {
                engine.enqueue(
                    Packet {
                        dir,
                        client: addr,
                        upstream: Arc::clone(&socket),
                        data: vec![0x23; 1200],
                        arrived,
                        ordinal: sequence + 1,
                        sequence,
                    },
                    &shared,
                );
            }
            assert_eq!(engine.queue.len(), 256);
            let (&(first, _), _) = engine.queue.first_key_value().unwrap();
            let (&(last, _), _) = engine.queue.last_key_value().unwrap();
            assert!(last - first <= Duration::from_millis(40));
        }
    }
}
