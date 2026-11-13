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

/// A number as System.Text.Json writes a C# float or double: no ".0" when it is whole.
pub fn cs_number(v: f64) -> serde_json::Value {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        serde_json::Value::from(v as i64)
    } else {
        serde_json::Number::from_f64(v).map_or(serde_json::Value::Null, serde_json::Value::Number)
    }
}

/// A C# float as System.Text.Json writes it: the shortest form that reads back the same float
/// (125.11583, not the double 125.11582946777344), no ".0" when it is whole.
pub fn cs_single(v: f32) -> serde_json::Value {
    let shortest: f64 = format!("{v}").parse().unwrap_or(0.0);
    cs_number(shortest)
}

/// A JSON object of text values in the order given, as C# writes a Dictionary<string, string>.
pub fn json_pairs(pairs: &[(&str, String)]) -> String {
    let body: Vec<String> = pairs
        .iter()
        .map(|(k, v)| format!("{}:{}", json(k), json(v)))
        .collect();
    format!("{{{}}}", body.join(","))
}

/// A float as C#'s ToString writes it: "128", "64.5".
pub fn cs_float(v: f32) -> String {
    if v.fract() == 0.0 && v.abs() < 1e9 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// A bool as C#'s ToString writes it.
pub fn cs_bool_text(b: bool) -> String {
    if b { "True" } else { "False" }.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_as_csharp_writes_them() {
        assert_eq!(cs_single(125.115_83).to_string(), "125.11583");
        assert_eq!(cs_single(100.0).to_string(), "100");
        assert_eq!(cs_number(0.5).to_string(), "0.5");
    }

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
