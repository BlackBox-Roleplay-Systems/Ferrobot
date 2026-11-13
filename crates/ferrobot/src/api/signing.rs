//! The signature of a command, as SecondBot's SignedCommand checks it (Validate, with the time
//! window its HTTP endpoint always asks for): sha1, in lower-case hex, of the command's name, its
//! arguments joined with `~#~`, the unix time it was signed at and the shared secret. A command
//! older than the window is refused; one from the future is not (SecondBot does the same).

use sha1::{Digest, Sha1};

/// The arguments as SecondBot reads them: split on `~#~`, the empty ones dropped.
pub fn split_args(args: Option<&str>) -> Vec<String> {
    args.map(|a| {
        a.split("~#~")
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect()
    })
    .unwrap_or_default()
}

pub fn sign(command: &str, args: &[String], unixtime: i64, secret: &str) -> String {
    sign_with(command, args, Some(unixtime), secret)
}

/// The signature with the time left out when no time window is asked for (SignedCommand
/// without EnforceTimeWindow, for commands by IM).
pub fn sign_with(command: &str, args: &[String], unixtime: Option<i64>, secret: &str) -> String {
    let time = unixtime.map(|t| t.to_string()).unwrap_or_default();
    let raw = format!("{command}{}{time}{secret}", args.join("~#~"));
    Sha1::digest(raw.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether a command is signed with the secret and recent enough.
pub fn accepted(
    command: &str,
    args: &[String],
    unixtime: i64,
    signing: Option<&str>,
    secret: &str,
    window: u64,
    now: i64,
) -> bool {
    if command.trim().is_empty() || unixtime == 0 {
        return false;
    }
    if now.saturating_sub(unixtime) > i64::try_from(window).unwrap_or(i64::MAX) {
        return false;
    }
    let Some(signing) = signing else {
        return false;
    };
    same(&sign(command, args, unixtime, secret), signing)
}

/// Two signatures compared in constant time: the time taken says nothing about the right one.
pub fn same(expected: &str, signing: &str) -> bool {
    expected.len() == signing.len()
        && expected
            .bytes()
            .zip(signing.bytes())
            .fold(0u8, |d, (a, b)| d | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn the_signature_is_secondbots() {
        // python3 -c "import hashlib;print(hashlib.sha1(b'GroupInvite'
        //   b'g~#~a~#~everyone1800000000secret').hexdigest())"
        let args = vec!["g".to_owned(), "a".to_owned(), "everyone".to_owned()];
        assert_eq!(
            sign("GroupInvite", &args, NOW, "secret"),
            "e7b78b4e3092d82d680ed58145ee671fd8a735f6"
        );
    }

    #[test]
    fn signed_recent_commands_only() {
        let args = split_args(Some("a~#~~#~b"));
        assert_eq!(args, vec!["a", "b"], "empty arguments are dropped");
        let good = sign("Name", &args, NOW, "secret");
        let ok = |sig: Option<&str>, time: i64, now: i64| {
            accepted("Name", &args, time, sig, "secret", 35, now)
        };
        assert!(ok(Some(&good), NOW, NOW + 35));
        assert!(!ok(Some(&good), NOW, NOW + 36), "too old");
        assert!(ok(Some(&good), NOW, NOW - 100), "the future is not checked");
        assert!(!ok(None, NOW, NOW));
        assert!(!ok(Some(&sign("Name", &args, NOW, "other")), NOW, NOW));
        assert!(!ok(Some(&good.to_uppercase()), NOW, NOW), "lower-case hex");
        assert!(!accepted("", &[], NOW, Some(&good), "secret", 35, NOW));
        assert!(split_args(None).is_empty());
    }
}
