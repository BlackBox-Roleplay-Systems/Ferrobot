//! Files the commands fetch from a URL, as SecondBot's HttpClient.GetByteArrayAsync: http and
//! https only, bounded in size and in time. Redirects are followed to http(s) only, five at most,
//! and never from https down to http.

use std::time::Duration;

/// The whole download, the body included.
const TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_REDIRECTS: usize = 5;

/// May a download start at `url`: SecondBot's check, the scheme as written.
pub fn allowed(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

fn may_redirect(from: Option<&reqwest::Url>, to: &reqwest::Url) -> bool {
    match (from.map(reqwest::Url::scheme), to.scheme()) {
        (Some("https"), "http") => false,
        (_, scheme) => matches!(scheme, "http" | "https"),
    }
}

/// An error and its causes, the URL left out: it is the caller's own, and may carry a token.
fn describe(e: reqwest::Error) -> String {
    if e.is_timeout() {
        return format!("the download took longer than {} s", TIMEOUT.as_secs());
    }
    let e = e.without_url();
    let mut text = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        text.push_str(": ");
        text.push_str(&s.to_string());
        source = s.source();
    }
    text
}

/// The body at `url`, no larger than `max` bytes. Errors say why, as words for a command's reply.
pub async fn fetch(url: &str, max: usize) -> Result<Vec<u8>, String> {
    if !allowed(url) {
        return Err("url must start with http:// or https://".into());
    }
    let policy = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else if may_redirect(attempt.previous().last(), attempt.url()) {
            attempt.follow()
        } else {
            attempt.stop()
        }
    });
    let http = reqwest::Client::builder()
        .redirect(policy)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TIMEOUT)
        .user_agent(format!("Ferrobot/{}", ferro_client::login::version()))
        .build()
        .map_err(describe)?;
    let mut resp = http.get(url).send().await.map_err(describe)?;
    let status = resp.status();
    if !status.is_success() {
        // as .NET's EnsureSuccessStatusCode words it
        return Err(format!(
            "Response status code does not indicate success: {} ({}).",
            status.as_u16(),
            status.canonical_reason().unwrap_or("")
        ));
    }
    let too_large = || format!("the file is larger than {max} bytes");
    if resp.content_length().is_some_and(|n| n > max as u64) {
        return Err(too_large());
    }
    let mut data = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(describe)? {
        if data.len() + chunk.len() > max {
            return Err(too_large());
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_and_https() {
        assert!(allowed("http://example.com/t.raw"));
        assert!(allowed("https://example.com/t.raw"));
        assert!(!allowed("file:///etc/passwd"));
        assert!(!allowed("ftp://example.com/t.raw"));
        assert!(!allowed("HTTP://example.com/t.raw"));
        let u = |s: &str| reqwest::Url::parse(s).expect("url");
        assert!(may_redirect(Some(&u("http://a/")), &u("https://b/")));
        assert!(may_redirect(Some(&u("https://a/")), &u("https://b/")));
        assert!(!may_redirect(Some(&u("https://a/")), &u("http://b/")));
        assert!(!may_redirect(
            Some(&u("http://a/")),
            &u("file:///etc/passwd")
        ));
    }

    #[tokio::test]
    async fn refused_before_any_request() {
        assert!(fetch("file:///etc/passwd", 10).await.is_err());
    }
}
