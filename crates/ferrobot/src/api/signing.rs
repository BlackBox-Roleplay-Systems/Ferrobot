//! The signature of a command, as SecondBot's SignedCommand checks it (Validate, with the time
//! window its HTTP endpoint always asks for): sha1, in lower-case hex, of the command's name, its
//! arguments joined with `~#~`, the unix time it was signed at and the shared secret. A command
//! older than the window is refused; one from the future is not (SecondBot does the same).
//!
//! Also SignedCommand's own unpacking of a command sent as one text (by IM, on Discord, to
//! `/json`): `command|||arg~#~arg#|#reply target@@@signature;|;unixtime`, or a JSON object
//! `{cmd, args, reply, signing, unixtime}`.

use serde_json::Value;
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

/// A command as SignedCommand unpacks it.
#[derive(Debug, Default, PartialEq)]
pub struct Unpacked {
    pub command: String,
    pub args: Vec<String>,
    pub reply: Option<String>,
    pub signing: Option<String>,
    pub unixtime: i64,
}

/// SignedCommand.UnpackInput: the JSON form when the text is an object that parses, else the
/// text form. None when it is malformed or names no command (SecondBot never accepts those).
pub fn unpack(input: &str) -> Option<Unpacked> {
    let trimmed = input.trim();
    let json = (trimmed.starts_with('{') && trimmed.ends_with('}'))
        .then(|| serde_json::from_str::<Value>(trimmed).ok())
        .flatten();
    let u = match json {
        Some(v) => from_json(&v)?,
        // IsValidJson: an object that does not parse is read as the text form
        None => from_text(input)?,
    };
    (!u.command.trim().is_empty()).then_some(u)
}

/// SignedCommand.UnpackStringToClass: each separator splits only when it is there once, and
/// what comes before it is kept (C#'s Split, then `bits.Length == 2`).
fn from_text(input: &str) -> Option<Unpacked> {
    fn two<'a>(text: &'a str, separator: &str) -> (&'a str, Option<&'a str>) {
        let bits: Vec<&str> = text.split(separator).collect();
        (bits[0], (bits.len() == 2).then(|| bits[1]))
    }
    let mut u = Unpacked::default();
    let (rest, time) = two(input, ";|;");
    if let Some(time) = time {
        // long.TryParse: surrounding white space and a sign are allowed
        u.unixtime = time.trim().parse().ok()?;
    }
    let (rest, signing) = two(rest, "@@@");
    u.signing = signing.map(str::to_owned);
    let (rest, reply) = two(rest, "#|#");
    u.reply = reply.map(str::to_owned);
    let (command, args) = two(rest, "|||");
    u.command = command.to_owned();
    u.args = args
        .map(|a| a.split("~#~").map(str::to_owned).collect())
        .unwrap_or_default();
    Some(u)
}

/// SignedCommand.UnpackJsonToClass, with Newtonsoft's conversions (JToken.ToObject): a field
/// that is missing or null takes its default, one of the wrong type refuses the command.
fn from_json(v: &Value) -> Option<Unpacked> {
    let text = |key: &str| match &v[key] {
        Value::Null => Some(None),
        other => scalar(other).map(Some),
    };
    let args = match &v["args"] {
        Value::Null => Vec::new(),
        Value::Array(items) => items
            .iter()
            .map(|x| match x {
                Value::Null => Some(String::new()),
                other => scalar(other),
            })
            .collect::<Option<_>>()?,
        _ => return None,
    };
    let unixtime = match &v["unixtime"] {
        Value::Null => 0,
        Value::Bool(b) => i64::from(*b),
        Value::Number(n) => match n.as_i64() {
            Some(t) => t,
            // Convert.ToInt64 rounds a double to the nearest, halves to even
            None => {
                let f = n.as_f64()?.round_ties_even();
                if !(i64::MIN as f64..i64::MAX as f64).contains(&f) {
                    return None;
                }
                f as i64
            }
        },
        Value::String(s) => s.trim().parse().ok()?,
        _ => return None,
    };
    Some(Unpacked {
        command: text("cmd")?.unwrap_or_default(),
        args,
        reply: text("reply")?,
        signing: text("signing")?,
        unixtime,
    })
}

/// A JSON value read as a C# string: objects and arrays are not one, booleans are "True" and
/// "False" (bool.ToString).
fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Bool(true) => Some("True".into()),
        Value::Bool(false) => Some("False".into()),
        Value::Number(n) => Some(n.to_string()),
        Value::Null => Some(String::new()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

/// SHA-1 of a text, in lower-case hex (SecondbotHelpers.GetSHA1).
pub fn sha1_hex(text: &str) -> String {
    Sha1::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn sign(command: &str, args: &[String], unixtime: i64, secret: &str) -> String {
    sign_with(command, args, Some(unixtime), secret)
}

/// The signature with the time left out when no time window is asked for (SignedCommand
/// without EnforceTimeWindow, for commands by IM).
pub fn sign_with(command: &str, args: &[String], unixtime: Option<i64>, secret: &str) -> String {
    let time = unixtime.map(|t| t.to_string()).unwrap_or_default();
    sha1_hex(&format!("{command}{}{time}{secret}", args.join("~#~")))
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

    #[test]
    fn the_text_form() {
        let u = unpack("Say|||0~#~hello there#|#42@@@abc;|;1800000000").expect("a command");
        assert_eq!(
            u,
            Unpacked {
                command: "Say".into(),
                args: vec!["0".into(), "hello there".into()],
                reply: Some("42".into()),
                signing: Some("abc".into()),
                unixtime: 1_800_000_000,
            }
        );
        let u = unpack("Name").expect("a bare command");
        assert_eq!((u.command.as_str(), u.args.len()), ("Name", 0));
        assert!(unpack("   ").is_none());
        assert!(unpack("Name;|;soon").is_none(), "a bad time");
        assert_eq!(unpack("Name;|; 5 ").expect("a command").unixtime, 5);
        // empty arguments are kept, unlike /api/Run's
        let u = unpack("Say|||0~#~").expect("a command");
        assert_eq!(u.args, vec!["0", ""]);
        // a separator found twice is not one: what comes before the first is kept
        let u = unpack("Say|||a@@@b@@@c;|;1;|;2").expect("a command");
        assert_eq!((u.signing, u.unixtime), (None, 0));
        assert_eq!(u.args, vec!["a"]);
        // not trimmed, as SignedCommand
        assert_eq!(unpack("Name ").expect("a command").command, "Name ");
    }

    #[test]
    fn the_json_form() {
        let u = unpack(
            r#"{"cmd":"IM","args":["Some Resident","hi"],"reply":"0","signing":"x","unixtime":5}"#,
        )
        .expect("a command");
        assert_eq!(u.command, "IM");
        assert_eq!(u.args, vec!["Some Resident", "hi"]);
        assert_eq!((u.reply.as_deref(), u.unixtime), (Some("0"), 5));
        assert_eq!(u.signing.as_deref(), Some("x"));

        let u = unpack(r#" {"cmd":"Ping","unixtime":"1800000000"} "#).expect("a command");
        assert_eq!((u.unixtime, u.args.len()), (1_800_000_000, 0));
        let u = unpack(r#"{"cmd":"Say","args":[7,true,null,""],"reply":null}"#).expect("a command");
        assert_eq!(u.args, vec!["7", "True", "", ""]);
        assert_eq!(u.reply, None);
        assert_eq!(
            unpack(r#"{"cmd":"X","unixtime":2.5}"#).map(|u| u.unixtime),
            Some(2)
        );

        // SecondBot's Json_WrongFieldTypes_AreRejectedWithoutThrowing, Json_MissingCommand
        for bad in [
            r#"{"cmd":"Say","args":{"not":"an array"}}"#,
            r#"{"cmd":"Say","args":"a"}"#,
            r#"{"cmd":"Say","args":[["nested"]]}"#,
            r#"{"cmd":"Say","unixtime":"abc"}"#,
            r#"{"cmd":{"nested":1}}"#,
            r#"{"args":["a"]}"#,
        ] {
            assert!(unpack(bad).is_none(), "{bad}");
        }
        // an object that does not parse is the text form
        let u = unpack("{not json}").expect("a command");
        assert_eq!(u.command, "{not json}");
    }

    #[test]
    fn empty_arguments_of_the_form_are_left_out_of_the_signature() {
        // /api/Run as SecondBot's Run: split with RemoveEmptyEntries, then the signature over
        // what is left. One made over the raw text with an empty argument is refused there too.
        let args = split_args(Some("a~#~"));
        assert_eq!(args, vec!["a"]);
        let over_raw = sha1_hex(&format!("Cmda~#~{NOW}secret"));
        assert!(!accepted(
            "Cmd",
            &args,
            NOW,
            Some(&over_raw),
            "secret",
            35,
            NOW
        ));
        let over_kept = sha1_hex(&format!("Cmda{NOW}secret"));
        assert!(accepted(
            "Cmd",
            &args,
            NOW,
            Some(&over_kept),
            "secret",
            35,
            NOW
        ));
    }
}
