//! RabbitMQ, as SecondBot's RabbitService (SecondBotEvents/Services/RabbitService.cs and
//! Config/RabbitConfig.cs): one AMQP connection to a broker, through which
//! - the bot publishes: SecondBot's SendMessageToQ command and the relays' `rabbit` target, each
//!   message to a queue by its name (the default exchange), the queue declared first as SecondBot
//!   declares it (durable, not exclusive, not auto-deleted);
//! - the bot reads SecondBot's four queues (notecards, commands, IMs, group IMs): see
//!   [`inbound`], where each message must be signed.
//!
//! Off unless configured. The connection lives in its own task: a broker that is down, refuses
//! the login or goes away never blocks the bot. Messages published meanwhile fail at once
//! ("Connection is not open", as SecondBot's SendMessage), and the task connects again with a
//! growing wait (1 s doubled up to a minute). SecondBot connects once per login and stops for
//! good when that fails (deliberate difference).
//!
//! Commands and relays talk to the connection through [`Rabbit`], a channel: tests put an
//! in-memory stand-in on its other end.

pub mod inbound;

use amqprs::BasicProperties;
use amqprs::channel::{
    BasicAckArguments, BasicConsumeArguments, BasicPublishArguments, BasicQosArguments, Channel,
    ConsumerMessage, QueueDeclareArguments,
};
use amqprs::connection::{Connection, OpenConnectionArguments};
use amqprs::tls::TlsAdaptor;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio::task::JoinSet;

/// The connection's settings (SecondBot's RabbitConfig, with a virtual host and TLS added).
#[derive(Clone)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub username: String,
    /// Never logged.
    pub password: String,
    pub vhost: String,
    /// amqps: TLS, with the same certificate checks as the bot's HTTPS.
    pub tls: bool,
    pub queues: Queues,
}

impl std::fmt::Debug for Settings {
    // never prints the password
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Settings")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("vhost", &self.vhost)
            .field("tls", &self.tls)
            .field("queues", &self.queues)
            .finish_non_exhaustive()
    }
}

/// The queues the bot reads (SecondBot's NotecardQueue, CommandQueue, ImQueue, GroupImQueue).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Queues {
    pub notecards: String,
    pub commands: String,
    pub ims: String,
    pub group_ims: String,
}

impl Default for Queues {
    /// SecondBot's defaults.
    fn default() -> Queues {
        Queues {
            notecards: "notecards".into(),
            commands: "commands".into(),
            ims: "ims".into(),
            group_ims: "groupims".into(),
        }
    }
}

impl Queues {
    fn each(&self) -> [(inbound::Kind, &str); 4] {
        [
            (inbound::Kind::Notecards, &self.notecards),
            (inbound::Kind::Commands, &self.commands),
            (inbound::Kind::Ims, &self.ims),
            (inbound::Kind::GroupIms, &self.group_ims),
        ]
    }
}

/// Whether the broker takes this queue name: an AMQP short string (at most 255 bytes, which the
/// AMQP library would otherwise panic on), not one of the broker's own `amq.` names.
pub fn check_queue_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        Err("the queue name is empty".into())
    } else if name.len() > 255 {
        Err("the queue name is longer than 255 bytes".into())
    } else if name.starts_with("amq.") {
        Err("queue names starting with \"amq.\" are the broker's".into())
    } else {
        Ok(())
    }
}

/// SecondBot's answer when there is no connection.
pub const NOT_OPEN: &str = "Connection is not open";
/// How long a publish may wait for the connection's answer.
const PUBLISH_WAIT: Duration = Duration::from_secs(10);
/// How long a connection, or a request on it, may take.
const BROKER_WAIT: Duration = Duration::from_secs(15);
/// How many messages of a queue are taken before they are handled (AMQP's prefetch).
const PREFETCH: u16 = 16;

/// A message to publish, and where its outcome goes.
#[derive(Debug)]
pub struct Publish {
    pub queue: String,
    pub body: Vec<u8>,
    pub done: oneshot::Sender<Result<(), String>>,
}

/// The connection's other end, for the commands and the relays.
#[derive(Debug)]
pub struct Rabbit {
    outbox: mpsc::Sender<Publish>,
}

impl Rabbit {
    /// The handle, and the outbox the connection task (or a test's stand-in) answers.
    pub fn new() -> (Arc<Rabbit>, mpsc::Receiver<Publish>) {
        let (outbox, rx) = mpsc::channel(256);
        (Arc::new(Rabbit { outbox }), rx)
    }

    /// Publish a message to a queue (SecondBot's RabbitService.SendMessage). Never waits long:
    /// without a connection it fails at once.
    pub async fn send(&self, queue: &str, message: &str) -> Result<(), String> {
        check_queue_name(queue)?;
        let (done, answer) = oneshot::channel();
        self.outbox
            .try_send(Publish {
                queue: queue.to_owned(),
                body: message.as_bytes().to_vec(),
                done,
            })
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => "too many messages waiting".to_owned(),
                mpsc::error::TrySendError::Closed(_) => NOT_OPEN.to_owned(),
            })?;
        match tokio::time::timeout(PUBLISH_WAIT, answer).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => Err(NOT_OPEN.into()),
            Err(_) => Err("no answer from the broker".into()),
        }
    }
}

/// Runs `work`, refusing what is published meanwhile; `None` when the bot dropped its side.
async fn refusing<T>(
    outbox: &mut mpsc::Receiver<Publish>,
    work: impl Future<Output = T>,
) -> Option<T> {
    tokio::pin!(work);
    loop {
        tokio::select! {
            out = &mut work => return Some(out),
            p = outbox.recv() => match p {
                Some(p) => {
                    let _ = p.done.send(Err(NOT_OPEN.into()));
                }
                None => return None,
            },
        }
    }
}

/// The TLS of amqps: rustls with aws-lc-rs and the platform's certificate checks, as reqwest's.
fn tls(host: &str) -> Result<TlsAdaptor, String> {
    use rustls_platform_verifier::BuilderVerifierExt;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_platform_verifier()
        .map_err(|e| e.to_string())?
        .with_no_client_auth();
    Ok(TlsAdaptor::new(
        tokio_rustls::TlsConnector::from(Arc::new(config)),
        host.to_owned(),
    ))
}

async fn connect(s: Settings) -> Result<Connection, String> {
    let mut args = OpenConnectionArguments::new(&s.host, s.port, &s.username, &s.password);
    args.virtual_host(&s.vhost).connection_name("Ferrobot");
    if s.tls {
        args.tls_adaptor(tls(&s.host)?);
    }
    match tokio::time::timeout(BROKER_WAIT, Connection::open(&args)).await {
        Ok(Ok(c)) => Ok(c),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("no answer".into()),
    }
}

/// A queue to read: its channel (kept open while it is read) and its messages.
async fn start_consumer(
    conn: &Connection,
    queue: &str,
) -> Result<(Channel, mpsc::UnboundedReceiver<ConsumerMessage>), String> {
    let setup = async {
        let channel = conn.open_channel(None).await?;
        channel
            .queue_declare(QueueDeclareArguments::durable_client_named(queue))
            .await?;
        channel
            .basic_qos(BasicQosArguments::new(0, PREFETCH, false))
            .await?;
        let (_tag, rx) = channel
            .basic_consume_rx(
                BasicConsumeArguments::new(queue, "")
                    .manual_ack(true)
                    .finish(),
            )
            .await?;
        Ok::<_, amqprs::error::Error>((channel, rx))
    };
    match tokio::time::timeout(BROKER_WAIT, setup).await {
        Ok(r) => r.map_err(|e| e.to_string()),
        Err(_) => Err("no answer".into()),
    }
}

/// Reads a queue. Each message is acknowledged as it is taken, then handled, as SecondBot's
/// autoAck: at most once, a command never runs twice. Deliberate difference: SecondBot's broker
/// pushes every waiting message at once; here at most PREFETCH are taken and not yet handled.
async fn consume(
    channel: Channel,
    mut messages: mpsc::UnboundedReceiver<ConsumerMessage>,
    kind: inbound::Kind,
    queue: String,
    inbound: inbound::Inbound,
) {
    let handling = Arc::new(Semaphore::new(usize::from(PREFETCH)));
    while let Some(m) = messages.recv().await {
        let Some(deliver) = m.deliver else {
            continue;
        };
        let Ok(permit) = handling.clone().acquire_owned().await else {
            return;
        };
        let ack = BasicAckArguments::new(deliver.delivery_tag(), false);
        if let Err(e) = channel.basic_ack(ack).await {
            log::warn!("RabbitMQ: {queue}: {e}");
            return;
        }
        let inbound = inbound.clone();
        let queue = queue.clone();
        let body = m.content.unwrap_or_default();
        tokio::spawn(async move {
            inbound.handle(kind, &queue, &body).await;
            drop(permit);
        });
    }
    // the broker cancelled the consumer (the queue deleted): read again on the next connection
    log::warn!("RabbitMQ: {queue} is no longer read");
}

/// Where messages are published: one channel, the queues it declared.
#[derive(Default)]
struct Publisher {
    channel: Option<Channel>,
    declared: HashSet<String>,
}

impl Publisher {
    async fn publish(
        &mut self,
        conn: &Connection,
        queue: &str,
        body: Vec<u8>,
    ) -> Result<(), String> {
        let channel = match self.channel.take() {
            Some(c) if c.is_open() => c,
            _ => {
                // a channel the broker closed (a refused declaration) forgot its queues
                self.declared.clear();
                match tokio::time::timeout(BROKER_WAIT, conn.open_channel(None)).await {
                    Ok(Ok(c)) => c,
                    Ok(Err(e)) => return Err(e.to_string()),
                    Err(_) => return Err("no answer".into()),
                }
            }
        };
        if !self.declared.contains(queue) {
            let declare = channel.queue_declare(QueueDeclareArguments::durable_client_named(queue));
            match tokio::time::timeout(BROKER_WAIT, declare).await {
                Ok(Ok(_)) => {
                    self.declared.insert(queue.to_owned());
                }
                // the channel is dropped: the broker closes it after a refusal
                Ok(Err(e)) => return Err(e.to_string()),
                Err(_) => return Err("no answer".into()),
            }
        }
        let sent = channel
            .basic_publish(
                BasicProperties::default(),
                body,
                BasicPublishArguments::new("", queue),
            )
            .await
            .map_err(|e| e.to_string());
        self.channel = Some(channel);
        sent
    }
}

/// One connection, until it is lost: the queues read, the messages published. `None` when the
/// bot dropped its side.
async fn serve(
    conn: Connection,
    outbox: &mut mpsc::Receiver<Publish>,
    settings: &Settings,
    inbound: Option<&inbound::Inbound>,
) -> Option<String> {
    let mut consumers = JoinSet::new();
    if let Some(inbound) = inbound {
        for (kind, queue) in settings.queues.each() {
            match start_consumer(&conn, queue).await {
                Ok((channel, rx)) => {
                    log::debug!("RabbitMQ: reading {queue}");
                    consumers.spawn(consume(
                        channel,
                        rx,
                        kind,
                        queue.to_owned(),
                        inbound.clone(),
                    ));
                }
                Err(e) => log::warn!("RabbitMQ: {queue} is not read: {e}"),
            }
        }
    }
    let mut publisher = Publisher::default();
    let failure = conn.listen_network_io_failure();
    tokio::pin!(failure);
    let mut check = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            _ = &mut failure => return Some("network failure".into()),
            _ = check.tick() => {
                if !conn.is_open() {
                    return Some("closed by the broker".into());
                }
            }
            p = outbox.recv() => {
                let p = p?;
                let outcome = publisher.publish(&conn, &p.queue, p.body).await;
                if let Err(e) = &outcome {
                    log::warn!("RabbitMQ: not sent to {}: {e}", p.queue);
                }
                let _ = p.done.send(outcome);
            }
        }
    }
}

/// The connection task: connects, serves, and connects again, for as long as the bot runs.
/// `inbound` is `None` when the queues are not read (no shared secret to check them with).
pub async fn run(
    settings: Settings,
    mut outbox: mpsc::Receiver<Publish>,
    inbound: Option<inbound::Inbound>,
) {
    let place = format!("{}:{}", settings.host, settings.port);
    let mut wait = Duration::from_secs(1);
    loop {
        // in a task of its own: a panic in the AMQP library is a failed attempt, not the end
        let attempt = tokio::spawn(connect(settings.clone()));
        let Some(attempt) = refusing(&mut outbox, attempt).await else {
            return;
        };
        match attempt {
            Ok(Ok(conn)) => {
                log::info!("RabbitMQ: connected to {place}");
                wait = Duration::from_secs(1);
                let why = serve(conn, &mut outbox, &settings, inbound.as_ref()).await;
                let Some(why) = why else {
                    return;
                };
                log::warn!("RabbitMQ: connection to {place} lost ({why})");
            }
            Ok(Err(e)) => log::warn!("RabbitMQ: no connection to {place}: {e}"),
            Err(e) => log::warn!("RabbitMQ: no connection to {place}: {e}"),
        }
        log::info!("RabbitMQ: connecting again in {} s", wait.as_secs());
        if refusing(&mut outbox, tokio::time::sleep(wait))
            .await
            .is_none()
        {
            return;
        }
        wait = (wait * 2).min(Duration::from_secs(60));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_names_the_broker_takes() {
        assert!(check_queue_name("exampleq").is_ok());
        assert!(check_queue_name(&"q".repeat(255)).is_ok());
        assert!(check_queue_name(&"q".repeat(256)).is_err());
        assert!(check_queue_name("").is_err());
        assert!(check_queue_name("amq.direct").is_err());
    }

    #[test]
    fn the_password_is_never_printed() {
        let s = Settings {
            host: "broker.invalid".into(),
            port: 5672,
            username: "bot".into(),
            password: "do not print".into(),
            vhost: "/".into(),
            tls: false,
            queues: Queues::default(),
        };
        let printed = format!("{s:?}");
        assert!(printed.contains("broker.invalid"), "{printed}");
        assert!(!printed.contains("do not print"), "{printed}");
    }

    #[test]
    fn the_tls_settings_build() {
        assert!(tls("broker.invalid").is_ok());
    }

    #[tokio::test]
    async fn publishing_goes_through_the_outbox() {
        let (rabbit, mut outbox) = Rabbit::new();
        let stand_in = tokio::spawn(async move {
            let p = outbox.recv().await.expect("a message");
            let seen = (
                p.queue.clone(),
                String::from_utf8(p.body.clone()).expect("text"),
            );
            p.done.send(Ok(())).expect("the sender waits");
            seen
        });
        assert_eq!(rabbit.send("exampleq", "Hello world").await, Ok(()));
        assert_eq!(
            stand_in.await.expect("the stand-in"),
            ("exampleq".to_owned(), "Hello world".to_owned())
        );
        // a name the broker would refuse never reaches it
        assert!(rabbit.send("amq.x", "Hello").await.is_err());
    }

    #[tokio::test]
    async fn without_a_connection_publishing_fails_at_once() {
        let (rabbit, outbox) = Rabbit::new();
        // nothing listens on this port: the task keeps trying, and refuses meanwhile
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .expect("a free port")
            .port();
        let settings = Settings {
            host: "127.0.0.1".into(),
            port,
            username: "guest".into(),
            password: "guest".into(),
            vhost: "/".into(),
            tls: false,
            queues: Queues::default(),
        };
        let task = tokio::spawn(run(settings, outbox, None));
        let started = std::time::Instant::now();
        for _ in 0..3 {
            assert_eq!(rabbit.send("q", "m").await, Err(NOT_OPEN.to_owned()));
        }
        assert!(started.elapsed() < Duration::from_secs(5));
        task.abort();
    }

    #[tokio::test]
    async fn a_broker_that_never_answers_blocks_nothing() {
        // accepts the connection, then says nothing: the connection waits, publishing does not
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("address").port();
        let silent = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        let (rabbit, outbox) = Rabbit::new();
        let settings = Settings {
            host: "127.0.0.1".into(),
            port,
            username: "guest".into(),
            password: "guest".into(),
            vhost: "/".into(),
            tls: false,
            queues: Queues::default(),
        };
        let task = tokio::spawn(run(settings, outbox, None));
        tokio::time::sleep(Duration::from_millis(200)).await;
        let started = std::time::Instant::now();
        assert_eq!(rabbit.send("q", "m").await, Err(NOT_OPEN.to_owned()));
        assert!(started.elapsed() < Duration::from_secs(1));
        task.abort();
        silent.abort();
    }
}
