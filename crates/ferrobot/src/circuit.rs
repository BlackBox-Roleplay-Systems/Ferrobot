//! Reliable UDP circuit to one region: sequence numbers, resends, acks, duplicates.
//!
//! Port of aurora-net/src/circuit.rs (Aurora Viewer, see NOTICE.md), itself a port of the
//! essentials of `llmessage/llcircuit.cpp`. Trimmed for a bot: no statistics or loss counting.

use ferro_msg::packet::{PacketFlags, build_raw};
use ferro_msg::{Msg, Writer};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

const MAX_RESENDS: u8 = 4;
const DEDUPE_WINDOW: usize = 2048;
const ACK_FLUSH_INTERVAL: Duration = Duration::from_millis(100);

/// Resend policy of a reliable packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resend {
    /// Fixed resend timeout; `None` derives it from the ping time.
    pub timeout: Option<Duration>,
    /// Resends before the packet is given up on.
    pub max_retries: u8,
}

impl Resend {
    pub const DEFAULT: Resend = Resend {
        timeout: None,
        max_retries: MAX_RESENDS,
    };
    /// The login UseCircuitCode (llstartup.cpp STATE_WORLD_INIT: UseCircuitCodeMaxRetries = 3,
    /// UseCircuitCodeTimeout = 5 s).
    pub const LOGIN_USE_CIRCUIT_CODE: Resend = Resend {
        timeout: Some(Duration::from_secs(5)),
        max_retries: 3,
    };
}

struct Unacked {
    payload: Vec<u8>,
    zerocoded: bool,
    sent: Instant,
    retries: u8,
    policy: Resend,
}

pub struct Circuit {
    pub addr: SocketAddr,
    next_seq: u32,
    unacked: HashMap<u32, Unacked>,
    pending_acks: Vec<u32>,
    last_ack_flush: Instant,
    recent: VecDeque<u32>,
    recent_set: HashSet<u32>,
    pub last_recv: Instant,
    pub rtt: Duration,
    ping_id: u8,
    ping_sent: Option<(u8, Instant)>,
    buf: Vec<u8>,
    dropped: Vec<u32>,
}

impl Circuit {
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            next_seq: 1,
            unacked: HashMap::new(),
            pending_acks: Vec::new(),
            last_ack_flush: Instant::now(),
            recent: VecDeque::with_capacity(DEDUPE_WINDOW),
            recent_set: HashSet::with_capacity(DEDUPE_WINDOW),
            last_recv: Instant::now(),
            rtt: Duration::from_millis(300),
            ping_id: 0,
            ping_sent: None,
            buf: Vec::with_capacity(1500),
            dropped: Vec::new(),
        }
    }

    fn raw_send(&mut self, sock: &UdpSocket) {
        if let Err(e) = sock.try_send_to(&self.buf, self.addr) {
            log::trace!("udp send to {} failed: {e}", self.addr);
        }
    }

    /// Encode and send a message, piggybacking pending acks. Returns its sequence number.
    pub fn send<M: Msg>(&mut self, sock: &UdpSocket, msg: &M, reliable: bool) -> u32 {
        self.send_with(sock, msg, reliable.then_some(Resend::DEFAULT))
    }

    /// Send a reliable message with its own resend policy.
    pub fn send_reliable_with<M: Msg>(&mut self, sock: &UdpSocket, msg: &M, policy: Resend) -> u32 {
        self.send_with(sock, msg, Some(policy))
    }

    fn send_with<M: Msg>(&mut self, sock: &UdpSocket, msg: &M, reliable: Option<Resend>) -> u32 {
        let mut w = Writer::new();
        M::ID.encode(&mut w.buf);
        msg.encode_body(&mut w);
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        let mut buf = std::mem::take(&mut self.buf);
        let n = build_raw(
            &w.buf,
            M::ZEROCODED,
            seq,
            reliable.is_some(),
            false,
            &self.pending_acks,
            &mut buf,
        );
        self.pending_acks.drain(..n);
        self.buf = buf;
        self.raw_send(sock);
        if let Some(policy) = reliable {
            self.unacked.insert(
                seq,
                Unacked {
                    payload: w.buf,
                    zerocoded: M::ZEROCODED,
                    sent: Instant::now(),
                    retries: 0,
                    policy,
                },
            );
        }
        seq
    }

    /// True while a reliable packet is still waiting for its ack.
    pub fn is_unacked(&self, seq: u32) -> bool {
        self.unacked.contains_key(&seq)
    }

    /// Process the header of an incoming packet (the acks it carries). Returns `false` for a resend
    /// of a packet already processed: it is acked again and must not be processed. A new packet is
    /// acked only once processed, see [`Circuit::accept`].
    pub fn on_receive(&mut self, flags: u8, seq: u32, acks: &[u32]) -> bool {
        self.last_recv = Instant::now();
        for a in acks {
            self.unacked.remove(a);
        }
        // Like LL (message.cpp), only packets flagged RESENT can be duplicates.
        if flags & PacketFlags::RESENT != 0 && self.recent_set.contains(&seq) {
            if flags & PacketFlags::RELIABLE != 0 {
                self.pending_acks.push(seq);
            }
            return false;
        }
        true
    }

    /// A packet was decoded and handled: ack it if reliable and remember it. A packet that fails to
    /// decode is neither acked nor remembered, so the region resends it.
    pub fn accept(&mut self, flags: u8, seq: u32) {
        if flags & PacketFlags::RELIABLE != 0 {
            self.pending_acks.push(seq);
        }
        if self.recent_set.contains(&seq) {
            return;
        }
        if self.recent.len() >= DEDUPE_WINDOW
            && let Some(old) = self.recent.pop_front()
        {
            self.recent_set.remove(&old);
        }
        self.recent.push_back(seq);
        self.recent_set.insert(seq);
    }

    /// Reliable packets given up on since the last call.
    pub fn take_dropped(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.dropped)
    }

    pub fn ack_received(&mut self, ids: impl IntoIterator<Item = u32>) {
        for id in ids {
            self.unacked.remove(&id);
        }
    }

    /// Resend timed-out reliable packets and flush standalone acks.
    pub fn tick(&mut self, sock: &UdpSocket) {
        let now = Instant::now();
        let ping_timeout = (self.rtt * 3).clamp(Duration::from_millis(500), Duration::from_secs(3));
        let mut to_resend = Vec::new();
        let mut to_drop = Vec::new();
        for (seq, u) in &self.unacked {
            if now.duration_since(u.sent) > u.policy.timeout.unwrap_or(ping_timeout) {
                if u.retries >= u.policy.max_retries {
                    to_drop.push(*seq);
                } else {
                    to_resend.push(*seq);
                }
            }
        }
        for seq in to_drop {
            self.unacked.remove(&seq);
            self.dropped.push(seq);
        }
        for seq in to_resend {
            let Some(u) = self.unacked.get_mut(&seq) else {
                continue;
            };
            u.retries += 1;
            u.sent = now;
            let mut buf = std::mem::take(&mut self.buf);
            build_raw(&u.payload, u.zerocoded, seq, true, true, &[], &mut buf);
            self.buf = buf;
            self.raw_send(sock);
        }
        if !self.pending_acks.is_empty()
            && now.duration_since(self.last_ack_flush) >= ACK_FLUSH_INTERVAL
        {
            self.flush_acks(sock);
        }
    }

    pub fn flush_acks(&mut self, sock: &UdpSocket) {
        self.last_ack_flush = Instant::now();
        while !self.pending_acks.is_empty() {
            let n = self.pending_acks.len().min(250);
            let ids: Vec<u32> = self.pending_acks.drain(..n).collect();
            let m = ferro_msg::msgs::PacketAck {
                packets: ids
                    .into_iter()
                    .map(|id| ferro_msg::msgs::packet_ack::Packets { id })
                    .collect(),
            };
            let mut w = Writer::new();
            ferro_msg::msgs::PacketAck::ID.encode(&mut w.buf);
            m.encode_body(&mut w);
            let seq = self.next_seq;
            self.next_seq = self.next_seq.wrapping_add(1);
            let mut buf = std::mem::take(&mut self.buf);
            build_raw(&w.buf, false, seq, false, false, &[], &mut buf);
            self.buf = buf;
            self.raw_send(sock);
        }
    }

    /// Start a ping; returns the ping id and the oldest unacked sequence.
    pub fn next_ping(&mut self) -> (u8, u32) {
        self.ping_id = self.ping_id.wrapping_add(1);
        self.ping_sent = Some((self.ping_id, Instant::now()));
        let oldest = self.unacked.keys().min().copied().unwrap_or(self.next_seq);
        (self.ping_id, oldest)
    }

    pub fn on_ping_reply(&mut self, id: u8) -> Option<Duration> {
        if let Some((pid, at)) = self.ping_sent
            && pid == id
        {
            let rtt = at.elapsed();
            self.rtt = (self.rtt * 7 + rtt) / 8;
            self.ping_sent = None;
            return Some(rtt);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELIABLE: u8 = PacketFlags::RELIABLE;
    const RESENT: u8 = PacketFlags::RELIABLE | PacketFlags::RESENT;

    fn circuit() -> Circuit {
        Circuit::new("127.0.0.1:9".parse().expect("address"))
    }

    #[test]
    fn undecodable_packet_is_not_acked_and_its_resend_is_processed() {
        let mut c = circuit();
        assert!(c.on_receive(RELIABLE, 7, &[]));
        assert!(c.pending_acks.is_empty());
        assert!(c.on_receive(RESENT, 7, &[]));
        c.accept(RESENT, 7);
        assert_eq!(c.pending_acks, vec![7]);
    }

    #[test]
    fn duplicate_resend_is_acked_but_not_processed_again() {
        let mut c = circuit();
        assert!(c.on_receive(RELIABLE, 3, &[]));
        c.accept(RELIABLE, 3);
        assert!(!c.on_receive(RESENT, 3, &[]));
        assert_eq!(c.pending_acks, vec![3, 3]);
        assert!(c.on_receive(RELIABLE, 3, &[]));
    }

    #[test]
    fn unreliable_packets_are_never_acked() {
        let mut c = circuit();
        assert!(c.on_receive(0, 1, &[]));
        c.accept(0, 1);
        assert!(c.pending_acks.is_empty());
    }

    #[tokio::test]
    async fn login_use_circuit_code_keeps_its_own_resend_policy() {
        let sock = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
        let mut c = circuit();
        let m = ferro_msg::msgs::UseCircuitCode::default();
        let login = c.send_reliable_with(&sock, &m, Resend::LOGIN_USE_CIRCUIT_CODE);
        let other = c.send(&sock, &m, true);
        for u in c.unacked.values_mut() {
            u.sent -= Duration::from_secs(4);
        }
        c.tick(&sock);
        assert_eq!(c.unacked[&login].retries, 0);
        assert_eq!(c.unacked[&other].retries, 1);
        for _ in 0..4 {
            c.unacked.get_mut(&login).expect("unacked").sent -= Duration::from_secs(6);
            c.tick(&sock);
        }
        assert!(!c.is_unacked(login));
        assert_eq!(c.take_dropped(), vec![login]);
    }
}
