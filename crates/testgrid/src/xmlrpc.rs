//! The grid's side of XML-RPC: read a `methodCall`, write a `methodResponse`. Written apart from
//! the bot's codec on purpose, so that a mistake in one is not hidden by the same one in the
//! other.

use ferro_llsd::xml::{self, Element};
use ferro_llsd::{Llsd, Map};

/// The method name and the first parameter of a `methodCall`.
pub fn parse_call(body: &[u8]) -> Option<(String, Llsd)> {
    let root = xml::parse(body).ok()?;
    if root.name != "methodCall" {
        return None;
    }
    let method = root.child("methodName")?.text();
    let value = root.child("params")?.child("param")?.child("value")?;
    Some((method, value_of(value)))
}

fn value_of(el: &Element) -> Llsd {
    let Some(typed) = el.elements().next() else {
        return Llsd::String(el.text());
    };
    match typed.name.as_str() {
        "int" | "i4" => Llsd::Integer(typed.text().trim().parse().unwrap_or(0)),
        "boolean" => Llsd::Boolean(typed.text().trim() == "1"),
        "double" => Llsd::Real(typed.text().trim().parse().unwrap_or(0.0)),
        "array" => Llsd::Array(
            typed
                .child("data")
                .map(|d| {
                    d.elements()
                        .filter(|e| e.name == "value")
                        .map(value_of)
                        .collect()
                })
                .unwrap_or_default(),
        ),
        "struct" => {
            let mut m = Map::new();
            for member in typed.elements().filter(|e| e.name == "member") {
                if let (Some(name), Some(v)) = (member.child("name"), member.child("value")) {
                    m.insert(name.text(), value_of(v));
                }
            }
            Llsd::Map(m)
        }
        _ => Llsd::String(typed.text()),
    }
}

/// A `methodResponse` with one struct parameter. Values are written the way the grid writes
/// them: strings, ints and arrays of structs.
pub fn response(v: &Llsd) -> String {
    let mut out = String::from("<?xml version=\"1.0\"?><methodResponse><params><param>");
    write(v, &mut out);
    out.push_str("</param></params></methodResponse>");
    out
}

fn write(v: &Llsd, out: &mut String) {
    out.push_str("<value>");
    match v {
        Llsd::Integer(i) => out.push_str(&format!("<int>{i}</int>")),
        Llsd::Boolean(b) => out.push_str(&format!("<boolean>{}</boolean>", u8::from(*b))),
        Llsd::Real(r) => out.push_str(&format!("<double>{r}</double>")),
        Llsd::Array(a) => {
            out.push_str("<array><data>");
            for e in a {
                write(e, out);
            }
            out.push_str("</data></array>");
        }
        Llsd::Map(m) => {
            out.push_str("<struct>");
            for (k, e) in m {
                out.push_str("<member><name>");
                xml::escape(k, out);
                out.push_str("</name>");
                write(e, out);
                out.push_str("</member>");
            }
            out.push_str("</struct>");
        }
        other => {
            out.push_str("<string>");
            xml::escape(&other.to_string_value(), out);
            out.push_str("</string>");
        }
    }
    out.push_str("</value>");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_login_call() {
        let call = b"<?xml version=\"1.0\"?><methodCall><methodName>login_to_simulator</methodName><params><param><value><struct><member><name>first</name><value><string>Test</string></value></member><member><name>n</name><value><int>3</int></value></member><member><name>options</name><value><array><data><value><string>a</string></value></data></array></value></member></struct></value></param></params></methodCall>";
        let (method, p) = parse_call(call).expect("a call");
        assert_eq!(method, "login_to_simulator");
        assert_eq!(p["first"].as_str(), "Test");
        assert_eq!(p["n"].as_i32(), 3);
        assert_eq!(p["options"][0].as_str(), "a");
    }

    #[test]
    fn writes_a_response() {
        let mut m = Map::new();
        m.insert("login".into(), "true".into());
        m.insert("sim_port".into(), 13000.into());
        let r = response(&Llsd::Map(m));
        assert!(r.contains("<name>login</name><value><string>true</string></value>"));
        assert!(r.contains("<name>sim_port</name><value><int>13000</int></value>"));
    }
}
