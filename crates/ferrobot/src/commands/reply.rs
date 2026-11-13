//! A command's answer, as SecondBot's CommandsAPI builds it: BasicReply `{command, args, reply,
//! status: true}`, Failure `{command, args, reply, status: false, whyfailed: null}`, serialized
//! in that order.

use serde::Serialize;

#[derive(Serialize)]
struct Basic<'a> {
    command: &'a str,
    args: &'a [String],
    reply: &'a str,
    status: bool,
}

#[derive(Serialize)]
struct Failure<'a> {
    command: &'a str,
    args: &'a [String],
    reply: &'a str,
    status: bool,
    whyfailed: Option<&'a str>,
}

/// The JSON text of a command's answer.
pub type Answer = String;

pub fn basic(command: &str, reply: &str, args: &[String]) -> Answer {
    serde_json::to_string(&Basic {
        command,
        args,
        reply,
        status: true,
    })
    .unwrap_or_default()
}

pub fn failure(command: &str, reply: &str, args: &[String]) -> Answer {
    serde_json::to_string(&Failure {
        command,
        args,
        reply,
        status: false,
        whyfailed: None,
    })
    .unwrap_or_default()
}

/// A list or a map, as the JSON text a list command's `reply` holds.
pub fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_secondbots() {
        assert_eq!(
            basic("Name", "Test Bot", &[]),
            r#"{"command":"Name","args":[],"reply":"Test Bot","status":true}"#
        );
        assert_eq!(
            failure("GroupEject", "Unknown group", &["g".into(), "a".into()]),
            r#"{"command":"GroupEject","args":["g","a"],"reply":"Unknown group","status":false,"whyfailed":null}"#
        );
    }
}
