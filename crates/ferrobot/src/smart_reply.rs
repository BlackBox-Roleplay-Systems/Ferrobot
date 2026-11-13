//! SecondBot's SmartCommandReply: where the answer of a command goes when it names a reply
//! target. A channel number in local chat (999 for a negative one), an avatar's key or name by
//! IM, an http(s) address as a form POST of `reply` and `command`, an e-mail address as a message
//! "<command>##commandreply" (when FERRO_SMTP_REPLY_TARGET allows it, crate::services::smtp).

use crate::commands::Ctx;
use ferro_client::Client;
use ferro_client::events::chat_type;
use uuid::Uuid;

/// Sends `output` to `target`.
pub async fn send(ctx: &Ctx, target: &str, output: &str, command: &str) {
    send_with(&ctx.client, &ctx.http, &ctx.smtp, target, output, command).await;
}

/// The same, for a service that has no command context (the interaction events).
pub async fn send_with(
    client: &Client,
    http: &reqwest::Client,
    smtp: &crate::services::smtp::Mailer,
    target: &str,
    output: &str,
    command: &str,
) {
    let target = target.trim();
    let lower = target.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        let body = format!(
            "reply={}&command={}",
            form_encode(output),
            form_encode(command)
        );
        let sent = http
            .post(target)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .timeout(std::time::Duration::from_secs(30))
            .body(body)
            .send()
            .await;
        if let Err(e) = sent {
            log::warn!("command reply by HTTP failed: {}", e.without_url());
        }
        return;
    }
    if let Ok(avatar) = Uuid::parse_str(target) {
        let _ = client.im(avatar, output).await;
        return;
    }
    if target.split_whitespace().count() == 2 {
        if let Ok(Some(avatar)) = client.find_avatar(target).await {
            let _ = client.im(avatar, output).await;
        }
        return;
    }
    // .NET's EmailAddressAttribute, as SmartCommandReply tells an address
    if crate::services::smtp::looks_like_address(target) {
        if let Err(e) = smtp.reply_target(target, command, output).await {
            log::warn!("command reply by e-mail failed: {}", e.for_log());
        }
        return;
    }
    if let Ok(channel) = target.parse::<i32>() {
        // "output Channel must be zero or higher"
        let channel = if channel < 0 { 999 } else { channel };
        let _ = client.chat(output, channel, chat_type::SAY).await;
    }
}

pub fn form_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_owned(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
