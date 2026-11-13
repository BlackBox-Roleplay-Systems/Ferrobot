//! A stand-in SMTP server on localhost, plain text, for the bot's e-mail: it greets, answers EHLO
//! (offering AUTH PLAIN and LOGIN), takes AUTH, MAIL, RCPT, DATA and QUIT, and keeps each message
//! it accepted. A recipient whose address starts with "refused" gets a 550, as a server refuses a
//! mailbox it does not know.

use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// A message the server accepted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mail {
    /// MAIL FROM's address.
    pub from: String,
    /// RCPT TO's addresses.
    pub to: Vec<String>,
    /// The message: headers and body, as sent (dot-stuffing undone).
    pub data: String,
    /// The AUTH line's argument, when the client logged in (base64, as sent).
    pub auth: Option<String>,
}

pub struct FakeSmtp {
    pub port: u16,
    mails: mpsc::UnboundedReceiver<Mail>,
    /// Every line the client sent, for checks on what crossed the wire.
    pub lines: Arc<Mutex<Vec<String>>>,
}

impl FakeSmtp {
    pub async fn start() -> std::io::Result<FakeSmtp> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let (tx, mails) = mpsc::unbounded_channel();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let seen = lines.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (tx, seen) = (tx.clone(), seen.clone());
                tokio::spawn(async move {
                    if let Err(e) = session(stream, tx, seen).await {
                        log::debug!("fake SMTP: {e}");
                    }
                });
            }
        });
        Ok(FakeSmtp { port, mails, lines })
    }

    /// The next message accepted, waiting up to `wait`.
    pub async fn next(&mut self, wait: std::time::Duration) -> Option<Mail> {
        tokio::time::timeout(wait, self.mails.recv())
            .await
            .ok()
            .flatten()
    }
}

/// The address inside `<…>`.
fn address(arg: &str) -> String {
    let start = arg.find('<').map_or(0, |i| i + 1);
    let end = arg.rfind('>').unwrap_or(arg.len());
    arg.get(start..end).unwrap_or_default().to_owned()
}

async fn session(
    stream: TcpStream,
    tx: mpsc::UnboundedSender<Mail>,
    seen: Arc<Mutex<Vec<String>>>,
) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    write.write_all(b"220 testgrid ESMTP\r\n").await?;
    let mut mail = Mail::default();
    let mut line = String::new();
    loop {
        line.clear();
        if read.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let text = line.trim_end().to_owned();
        if let Ok(mut s) = seen.lock() {
            s.push(text.clone());
        }
        let upper = text.to_ascii_uppercase();
        let reply: &[u8] = if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            b"250-testgrid\r\n250-AUTH PLAIN LOGIN\r\n250 8BITMIME\r\n"
        } else if upper.starts_with("AUTH ") {
            mail.auth = text.get(5..).map(str::to_owned);
            b"235 2.7.0 Authentication successful\r\n"
        } else if upper.starts_with("MAIL FROM:") {
            mail.from = address(&text[10..]);
            b"250 2.1.0 Ok\r\n"
        } else if upper.starts_with("RCPT TO:") {
            let to = address(&text[8..]);
            if to.starts_with("refused") {
                b"550 5.1.1 No such mailbox\r\n"
            } else {
                mail.to.push(to);
                b"250 2.1.5 Ok\r\n"
            }
        } else if upper == "DATA" {
            write
                .write_all(b"354 End data with <CR><LF>.<CR><LF>\r\n")
                .await?;
            let mut data = String::new();
            loop {
                line.clear();
                if read.read_line(&mut line).await? == 0 {
                    return Ok(());
                }
                if line == ".\r\n" || line == ".\n" {
                    break;
                }
                data.push_str(line.strip_prefix('.').unwrap_or(&line));
            }
            mail.data = data;
            let _ = tx.send(std::mem::take(&mut mail));
            b"250 2.0.0 Ok: queued\r\n"
        } else if upper == "QUIT" {
            write.write_all(b"221 2.0.0 Bye\r\n").await?;
            return Ok(());
        } else if upper == "RSET" || upper == "NOOP" {
            b"250 2.0.0 Ok\r\n"
        } else {
            b"502 5.5.2 Command not recognized\r\n"
        };
        write.write_all(reply).await?;
    }
}
