//! E-mail through an SMTP server (SecondBot's SmtpService and SmtpConfig): the SendEmail command,
//! e-mail as a command's reply target, and the grid's alerts and the login told by e-mail. Off
//! unless configured; each part has its own switch, off by default, as SecondBot's.
//!
//! A message is sent as SmtpService.SendMail sends it: from the reply address (named
//! "<address> via Ferrobot"), to one address (named by itself), its subject, a plain text body. The
//! reply address must be set, and with an allowed list (UseAllowedRecivers, AllowedReciversCSV)
//! only the addresses on it are mailed. One connection per message, as SecondBot's; every step is
//! under a timeout, so an unreachable server costs a failed send, never a stuck bot.
//!
//! Neither the password nor an address is logged: a failure is logged by its kind, and the
//! caller is told SecondBot's text.
//!
//! Deliberate differences: the TLS mode is FERRO_SMTP_TLS (`tls`, `starttls`, `none`); without it,
//! SecondBot's useSSL picks implicit TLS, else STARTTLS is required (MailKit uses it only when the
//! server offers it, which lets a password go out in clear); the login notice follows
//! AllowSendLoginNotice (SecondBot reads AllowSendAlertStatus for both and never reads
//! AllowSendLoginNotice); the allowed list ignores spaces and case; HideStatusOutput has nothing to
//! hide (no status output).

use ferro_client::{Client, Event};
use lettre::message::header::ContentType;
use lettre::message::{Mailbox, Message};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{Address, AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

/// How long one SMTP step (connecting, a command) may take.
const STEP_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a whole message may take.
const SEND_TIMEOUT: Duration = Duration::from_secs(60);

/// How the connection to the server is secured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// TLS from the first byte (SMTPS, usually port 465; SecondBot's useSSL).
    Tls,
    /// A plain connection upgraded by STARTTLS, which must succeed (usually port 587).
    StartTls,
    /// No encryption: only for a relay on the same host or network.
    None,
}

impl Security {
    pub fn parse(s: &str) -> Option<Security> {
        match s.trim().to_ascii_lowercase().as_str() {
            "tls" | "ssl" => Some(Security::Tls),
            "starttls" => Some(Security::StartTls),
            "none" => Some(Security::None),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub security: Security,
    /// The user and the password (SecondBot's User and Token); none when the server takes mail
    /// without logging in (Token "none").
    pub login: Option<(String, String)>,
    /// The address mail comes from (MailReplyAddress); every send fails while it is not set.
    pub from: Option<Address>,
    /// SendEmail is allowed (AllowCommandSendMail).
    pub command: bool,
    /// A command's reply target may be an address (AllowEmailAsReplyTarget).
    pub reply_target: bool,
    /// Only these addresses are mailed, when set (UseAllowedRecivers, AllowedReciversCSV).
    pub allowed: Option<Vec<String>>,
    /// The grid's alerts are mailed (AllowSendAlertStatus).
    pub alerts: bool,
    /// The login is mailed (AllowSendLoginNotice).
    pub login_notice: bool,
    /// Where alerts and login notices go (SendAlertsAndLoginsTo).
    pub notify: Option<Address>,
}

impl std::fmt::Debug for Settings {
    // never prints the password, nor the addresses
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("smtp::Settings")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("security", &self.security)
            .field("login", &self.login.is_some())
            .field("from", &self.from.is_some())
            .field("command", &self.command)
            .field("reply_target", &self.reply_target)
            .field("allowed", &self.allowed.as_ref().map(Vec::len))
            .field("alerts", &self.alerts)
            .field("login_notice", &self.login_notice)
            .field("notify", &self.notify.is_some())
            .finish()
    }
}

/// Why a message was not sent: SecondBot's texts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    #[error("SMTP service not enabled")]
    NotEnabled,
    #[error("Send email via commands not enabled")]
    CommandOff,
    #[error("Send email via command reply is not enabled")]
    ReplyTargetOff,
    #[error("you need to set the reply address")]
    NoReplyAddress,
    #[error("{0} is not in allowed recivers")]
    NotAllowed(String),
    /// What went wrong, for the caller; `log` is the same without what the server echoed back
    /// (it may name the address).
    #[error("Failed to send email: {reply}")]
    Failed { reply: String, log: String },
}

impl SendError {
    /// The error as it may be logged: no address.
    pub fn for_log(&self) -> String {
        match self {
            SendError::NotAllowed(_) => "the address is not in the allowed list".into(),
            SendError::Failed { log, .. } => log.clone(),
            other => other.to_string(),
        }
    }
}

/// The SMTP service: its settings, and what it sent.
#[derive(Debug, Default)]
pub struct Mailer {
    settings: Option<Settings>,
    sent: AtomicU64,
    failed: AtomicU64,
}

impl Mailer {
    pub fn new(settings: Option<Settings>) -> Mailer {
        Mailer {
            settings,
            ..Mailer::default()
        }
    }

    pub fn settings(&self) -> Option<&Settings> {
        self.settings.as_ref()
    }

    /// Messages sent and failed (SmtpService.Status).
    pub fn counts(&self) -> (u64, u64) {
        (
            self.sent.load(Ordering::Relaxed),
            self.failed.load(Ordering::Relaxed),
        )
    }

    /// SmtpService.commandEmail, for SendEmail.
    pub async fn command_email(
        &self,
        to: &str,
        subject: &str,
        body: &str,
    ) -> Result<(), SendError> {
        let s = self.settings.as_ref().ok_or(SendError::NotEnabled)?;
        if !s.command {
            return Err(SendError::CommandOff);
        }
        self.send(s, subject, body, to).await
    }

    /// SmtpService.sendReplyTarget: a command's answer, under "<command>##commandreply".
    pub async fn reply_target(
        &self,
        to: &str,
        command: &str,
        output: &str,
    ) -> Result<(), SendError> {
        let s = self.settings.as_ref().ok_or(SendError::NotEnabled)?;
        if !s.reply_target {
            return Err(SendError::ReplyTargetOff);
        }
        self.send(s, &format!("{command}##commandreply"), output, to)
            .await
    }

    /// SmtpService.SendMail: the checks, then the message, under the timeouts.
    async fn send(
        &self,
        s: &Settings,
        subject: &str,
        body: &str,
        to: &str,
    ) -> Result<(), SendError> {
        let done = match check(s, to) {
            Ok(from) => {
                match tokio::time::timeout(SEND_TIMEOUT, deliver(s, from, subject, body, to)).await
                {
                    Ok(done) => done,
                    Err(_) => Err(SendError::Failed {
                        reply: "timed out".into(),
                        log: "timed out".into(),
                    }),
                }
            }
            Err(e) => Err(e),
        };
        match &done {
            Ok(()) => {
                self.sent.fetch_add(1, Ordering::Relaxed);
                log::info!("e-mail sent");
            }
            // SecondBot counts a refused address as neither
            Err(SendError::NotAllowed(_)) => {}
            Err(e) => {
                self.failed.fetch_add(1, Ordering::Relaxed);
                log::warn!("e-mail not sent: {}", e.for_log());
            }
        }
        done
    }
}

/// The reply address, and the recipient on the allowed list (SendMail's checks, in its order).
fn check<'a>(s: &'a Settings, to: &str) -> Result<&'a Address, SendError> {
    let from = s.from.as_ref().ok_or(SendError::NoReplyAddress)?;
    if let Some(allowed) = &s.allowed {
        let to = to.trim();
        if !allowed.iter().any(|a| a.eq_ignore_ascii_case(to)) {
            return Err(SendError::NotAllowed(to.to_owned()));
        }
    }
    Ok(from)
}

/// The message as SendMail builds it.
pub fn message(from: &Address, to: &str, subject: &str, body: &str) -> Result<Message, SendError> {
    let to: Address = to
        .trim()
        .parse()
        .map_err(|e: lettre::address::AddressError| SendError::Failed {
            reply: format!("invalid address: {e}"),
            log: format!("invalid address: {e}"),
        })?;
    // a line break in the subject would start a header of its own
    let subject: String = subject
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let id = format!("<{}@{}>", uuid::Uuid::new_v4().simple(), from.domain());
    Message::builder()
        .from(Mailbox::new(
            Some(format!("{from} via Ferrobot")),
            from.clone(),
        ))
        .to(Mailbox::new(Some(to.to_string()), to))
        .subject(subject)
        .message_id(Some(id))
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_owned())
        .map_err(|e| SendError::Failed {
            reply: e.to_string(),
            log: "the message could not be built".into(),
        })
}

/// The transport the settings describe: one connection per message (no pool).
fn transport(s: &Settings) -> Result<AsyncSmtpTransport<Tokio1Executor>, SendError> {
    let tls = match s.security {
        Security::None => Tls::None,
        Security::Tls | Security::StartTls => {
            let params = TlsParameters::new(s.host.clone()).map_err(|e| failed(&e))?;
            if s.security == Security::Tls {
                Tls::Wrapper(params)
            } else {
                Tls::Required(params)
            }
        }
    };
    let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(s.host.clone())
        .port(s.port)
        .tls(tls)
        .timeout(Some(STEP_TIMEOUT));
    if let Some((user, password)) = &s.login {
        builder = builder.credentials(Credentials::new(user.clone(), password.clone()));
    }
    Ok(builder.build())
}

async fn deliver(
    s: &Settings,
    from: &Address,
    subject: &str,
    body: &str,
    to: &str,
) -> Result<(), SendError> {
    let message = message(from, to, subject, body)?;
    transport(s)?
        .send(message)
        .await
        .map(|_| ())
        .map_err(|e| failed(&e))
}

/// An SMTP failure: in full for the caller, as SecondBot's; for the log, the server's code alone
/// when the server answered (its text may name the address).
fn failed(e: &lettre::transport::smtp::Error) -> SendError {
    let log = match e.status() {
        Some(code) => format!("the server answered {code}"),
        None if e.is_timeout() => "timed out".to_owned(),
        None => e.to_string(),
    };
    SendError::Failed {
        reply: e.to_string(),
        log,
    }
}

/// .NET's EmailAddressAttribute.IsValid, which SmartCommandReply uses to tell an address: one
/// `@`, neither first nor last, and no line break.
pub fn looks_like_address(target: &str) -> bool {
    let Some(at) = target.find('@') else {
        return false;
    };
    at > 0
        && at + 1 != target.len()
        && target.rfind('@') == Some(at)
        && !target.contains(['\r', '\n'])
}

/// The alerts and the login notice (BotAlertMessage, BotLoggedIn): run only when one of them is
/// on and has an address to go to.
pub async fn run(mailer: Arc<Mailer>, client: Client, mut events: Receiver<Event>) {
    let Some(s) = mailer.settings().cloned() else {
        return;
    };
    let Some(notify) = s.notify.clone() else {
        return;
    };
    let notify = notify.to_string();
    // SmtpService's botConnected: the first arrival after a login is the login
    let mut connected = false;
    loop {
        let (subject, body) = match events.recv().await {
            Ok(Event::Arrived { .. }) if !connected => {
                connected = true;
                if !s.login_notice {
                    continue;
                }
                (
                    "Bot - Login message",
                    format!("{} has signed in", client.state().name),
                )
            }
            Ok(Event::Disconnected { .. }) => {
                connected = false;
                continue;
            }
            Ok(Event::Alert(message)) if s.alerts => ("Bot - Alert message", message),
            Ok(_) => continue,
            Err(RecvError::Lagged(n)) => {
                log::warn!("e-mail service: {n} events missed");
                continue;
            }
            Err(RecvError::Closed) => return,
        };
        let (mailer, s, notify) = (mailer.clone(), s.clone(), notify.clone());
        tokio::spawn(async move {
            let _ = mailer.send(&s, subject, &body, &notify).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            host: "smtp.example.invalid".into(),
            port: 587,
            security: Security::StartTls,
            login: Some(("bot@example.invalid".into(), "a secret password".into())),
            from: Some("bot@example.invalid".parse().expect("an address")),
            command: true,
            reply_target: true,
            allowed: None,
            alerts: false,
            login_notice: false,
            notify: None,
        }
    }

    #[test]
    fn addresses_as_dotnet_tells_them() {
        assert!(looks_like_address("someone@example.invalid"));
        assert!(looks_like_address("a@b"));
        assert!(!looks_like_address("@example.invalid"));
        assert!(!looks_like_address("someone@"));
        assert!(!looks_like_address("a@b@c"));
        assert!(!looks_like_address("someone\n@example.invalid"));
        assert!(!looks_like_address("42"));
    }

    #[test]
    fn the_password_is_never_printed() {
        let text = format!("{:?}", settings());
        assert!(!text.contains("secret"), "{text}");
        assert!(!text.contains("bot@"), "{text}");
    }

    #[test]
    fn the_message_is_secondbots() {
        let from: Address = "bot@example.invalid".parse().expect("an address");
        let m = message(
            &from,
            " someone@example.invalid ",
            "Hi\r\nBcc: x",
            "Hello there",
        )
        .expect("a message");
        let text = String::from_utf8(m.formatted()).expect("text");
        // the names hold an "@", so they go as RFC 2047 encoded words: base64 of
        // "bot@example.invalid via Ferrobot" and of "someone@example.invalid"
        assert!(
            text.contains(
                "From: =?utf-8?b?Ym90QGV4YW1wbGUuaW52YWxpZCB2aWEgRmVycm9ib3Q=?= <bot@example.invalid>"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "To: =?utf-8?b?c29tZW9uZUBleGFtcGxlLmludmFsaWQ=?= <someone@example.invalid>"
            ),
            "{text}"
        );
        // the line break did not make a header of its own
        assert!(text.contains("Subject: Hi  Bcc: x\r\n"), "{text}");
        assert!(!text.contains("\r\nBcc:"), "{text}");
        assert!(
            text.contains("Content-Type: text/plain; charset=utf-8"),
            "{text}"
        );
        assert!(text.contains("Message-ID: <"), "{text}");
        assert!(text.ends_with("Hello there"), "{text}");
        let envelope = m.envelope();
        assert_eq!(envelope.to().len(), 1);
        assert!(matches!(
            message(&from, "not an address", "s", "b"),
            Err(SendError::Failed { .. })
        ));
    }

    #[tokio::test]
    async fn the_checks_come_first() {
        let off = Mailer::new(None);
        assert_eq!(
            off.command_email("someone@example.invalid", "s", "b").await,
            Err(SendError::NotEnabled)
        );
        let mut s = settings();
        s.command = false;
        s.reply_target = false;
        let m = Mailer::new(Some(s.clone()));
        assert_eq!(
            m.command_email("someone@example.invalid", "s", "b").await,
            Err(SendError::CommandOff)
        );
        assert_eq!(
            m.reply_target("someone@example.invalid", "Name", "{}")
                .await,
            Err(SendError::ReplyTargetOff)
        );
        s.command = true;
        s.from = None;
        let m = Mailer::new(Some(s.clone()));
        assert_eq!(
            m.command_email("someone@example.invalid", "s", "b").await,
            Err(SendError::NoReplyAddress)
        );
        assert_eq!(m.counts(), (0, 1));
        s.from = settings().from;
        s.allowed = Some(vec!["Friend@Example.invalid".into()]);
        let m = Mailer::new(Some(s));
        let refused = m.command_email("someone@example.invalid", "s", "b").await;
        assert_eq!(
            refused.as_ref().map_err(SendError::to_string),
            Err("someone@example.invalid is not in allowed recivers".to_owned())
        );
        assert!(
            !refused
                .err()
                .is_some_and(|e| e.for_log().contains("someone"))
        );
        assert_eq!(m.counts(), (0, 0));
    }

    #[tokio::test]
    async fn sent_through_a_server() {
        let mut server = ferro_testgrid::smtp::FakeSmtp::start()
            .await
            .expect("a fake SMTP server");
        let mut s = settings();
        s.host = "127.0.0.1".into();
        s.port = server.port;
        s.security = Security::None;
        let m = Mailer::new(Some(s));
        m.command_email(
            "someone@example.invalid",
            "A subject",
            "line one\r\n.line two",
        )
        .await
        .expect("sent");
        let mail = server
            .next(Duration::from_secs(5))
            .await
            .expect("a message");
        assert_eq!(mail.from, "bot@example.invalid");
        assert_eq!(mail.to, vec!["someone@example.invalid".to_owned()]);
        assert!(mail.auth.is_some(), "logged in");
        assert!(
            mail.data.contains("Subject: A subject\r\n"),
            "{}",
            mail.data
        );
        assert!(mail.data.contains("line one\r\n.line two"), "{}", mail.data);
        m.reply_target("someone@example.invalid", "Name", "Test Bot")
            .await
            .expect("sent");
        let mail = server
            .next(Duration::from_secs(5))
            .await
            .expect("a message");
        assert!(
            mail.data.contains("Subject: Name##commandreply\r\n"),
            "{}",
            mail.data
        );
        // the server's refusal: in full for the caller, its code alone for the log
        let refused = m
            .command_email("refused@example.invalid", "s", "b")
            .await
            .expect_err("refused");
        assert!(refused.to_string().starts_with("Failed to send email: "));
        assert_eq!(refused.for_log(), "the server answered 550");
        assert_eq!(m.counts(), (2, 1));
    }

    #[tokio::test]
    async fn an_unreachable_server_is_a_failure() {
        // a port nobody listens on: the connection is refused at once
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .expect("a free port")
            .port();
        let mut s = settings();
        s.host = "127.0.0.1".into();
        s.port = port;
        s.security = Security::None;
        let m = Mailer::new(Some(s));
        let e = m
            .command_email("someone@example.invalid", "s", "b")
            .await
            .expect_err("no server");
        assert!(e.to_string().starts_with("Failed to send email: "), "{e}");
    }

    #[test]
    fn security_modes() {
        assert_eq!(Security::parse("TLS"), Some(Security::Tls));
        assert_eq!(Security::parse("starttls"), Some(Security::StartTls));
        assert_eq!(Security::parse(" none "), Some(Security::None));
        assert_eq!(Security::parse("maybe"), None);
    }
}
