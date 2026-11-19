//! OpenSim's RemoteAdmin (XML-RPC on the grid's public port, OpenSim's RemoteAdminPlugin): the
//! probe creates the accounts of extra bots, restarts regions, lists a region's agents and runs
//! console commands (`kick user`) through it. A local test grid
//! only (AGENTS.md rule 1); its password is read from a file and never logged.

use std::time::Duration;

pub struct Admin {
    http: reqwest::Client,
    url: String,
    password: String,
}

/// A value of an XML-RPC struct member.
enum Value<'a> {
    Str(&'a str),
    Int(i64),
}

impl Admin {
    pub fn new(http: reqwest::Client, url: String, password: String) -> Admin {
        Admin {
            http,
            url,
            password,
        }
    }

    /// Whether the account exists (admin_exists_user).
    pub async fn exists_user(&self, first: &str, last: &str) -> Result<bool, String> {
        self.call(
            "admin_exists_user",
            &[
                ("user_firstname", Value::Str(first)),
                ("user_lastname", Value::Str(last)),
            ],
        )
        .await
    }

    /// A new account (admin_create_user), its home left unset.
    pub async fn create_user(&self, first: &str, last: &str, password: &str) -> Result<(), String> {
        let ok = self
            .call(
                "admin_create_user",
                &[
                    ("user_firstname", Value::Str(first)),
                    ("user_lastname", Value::Str(last)),
                    ("user_password", Value::Str(password)),
                    ("start_region_x", Value::Int(0)),
                    ("start_region_y", Value::Int(0)),
                ],
            )
            .await?;
        if ok {
            Ok(())
        } else {
            Err(format!("the grid did not create {first} {last}"))
        }
    }

    /// Restart a region in `seconds` (admin_restart): its agents are told, then it goes down and
    /// comes back.
    pub async fn restart(&self, region: &str, seconds: u32) -> Result<(), String> {
        // `success` stays false: the answer says `accepted` and `rebooting`
        let body = self
            .post(
                "admin_restart",
                &[
                    ("region_name", Value::Str(region)),
                    ("milliseconds", Value::Int(i64::from(seconds) * 1000)),
                ],
            )
            .await?;
        if member_true(&body, "accepted") {
            Ok(())
        } else {
            Err(format!(
                "the grid refused to restart {region}: {}",
                error(&body)
            ))
        }
    }

    /// Run a region console command (admin_console_command). The answer says nothing but an
    /// error, if there is one; the console's output is not returned.
    pub async fn console(&self, command: &str) -> Result<(), String> {
        let body = self
            .post("admin_console_command", &[("command", Value::Str(command))])
            .await?;
        if body.contains("<name>error</name>") {
            return Err(format!("admin_console_command: {}", error(&body)));
        }
        Ok(())
    }

    /// Whether `region` holds an agent with this id, root or child (admin_get_agents).
    pub async fn has_agent(&self, region: &str, agent: &str) -> Result<bool, String> {
        let body = self
            .post(
                "admin_get_agents",
                &[
                    ("region_name", Value::Str(region)),
                    ("include_children", Value::Str("true")),
                ],
            )
            .await?;
        if !member_true(&body, "success") {
            return Err(format!("admin_get_agents in {region}: {}", error(&body)));
        }
        Ok(body.contains(&format!("<string>{agent}</string>")))
    }

    /// A call whose answer says `success`.
    async fn call(&self, method: &str, members: &[(&str, Value<'_>)]) -> Result<bool, String> {
        let body = self.post(method, members).await?;
        if body.contains("<name>error</name>") {
            return Err(format!("{method}: {}", error(&body)));
        }
        Ok(member_true(&body, "success"))
    }

    async fn post(&self, method: &str, members: &[(&str, Value<'_>)]) -> Result<String, String> {
        let mut xml = format!(
            "<?xml version=\"1.0\"?><methodCall><methodName>{method}</methodName><params><param>\
             <value><struct>{}",
            member("password", &Value::Str(&self.password))
        );
        for (name, value) in members {
            xml.push_str(&member(name, value));
        }
        xml.push_str("</struct></value></param></params></methodCall>");
        let resp = self
            .http
            .post(&self.url)
            .header("Content-Type", "text/xml")
            .timeout(Duration::from_secs(30))
            .body(xml)
            .send()
            .await
            .map_err(|e| format!("{method}: {}", e.without_url()))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| format!("{method}: {}", e.without_url()))?;
        if !status.is_success() {
            return Err(format!("{method}: HTTP {status}"));
        }
        if body.contains("<fault>") {
            return Err(format!("{method}: {}", fault(&body)));
        }
        Ok(body)
    }
}

fn member(name: &str, value: &Value<'_>) -> String {
    let v = match value {
        Value::Str(s) => format!("<string>{}</string>", escape(s)),
        Value::Int(i) => format!("<i4>{i}</i4>"),
    };
    format!("<member><name>{name}</name><value>{v}</value></member>")
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Whether the answer's struct has `name` set to true.
fn member_true(body: &str, name: &str) -> bool {
    let key = format!("<name>{name}</name>");
    body.split(&key).nth(1).is_some_and(|rest| {
        let rest = rest.trim_start();
        rest.starts_with("<value><boolean>1</boolean>")
            || rest.starts_with("<value><boolean>true</boolean>")
    })
}

/// The text of the answer's `error` member.
fn error(body: &str) -> String {
    string_after(body, "<name>error</name>").unwrap_or_else(|| "no reason given".into())
}

fn fault(body: &str) -> String {
    string_after(body, "<name>faultString</name>").unwrap_or_else(|| "a fault".into())
}

fn string_after(body: &str, key: &str) -> Option<String> {
    let rest = body.split(key).nth(1)?;
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")?;
    Some(rest[start..start + end].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_member_is_escaped() {
        assert_eq!(
            member("user_lastname", &Value::Str("A<&>B")),
            "<member><name>user_lastname</name><value><string>A&lt;&amp;&gt;B</string></value></member>"
        );
        assert_eq!(
            member("milliseconds", &Value::Int(5000)),
            "<member><name>milliseconds</name><value><i4>5000</i4></value></member>"
        );
    }

    #[test]
    fn answers_are_read() {
        let yes = "<struct><member><name>success</name><value><boolean>1</boolean></value></member></struct>";
        let no = "<struct><member><name>success</name><value><boolean>0</boolean></value></member>\
                  <member><name>error</name><value><string>no such region</string></value></member></struct>";
        assert!(member_true(yes, "success"));
        assert!(!member_true(no, "success"));
        assert!(!member_true(yes, "accepted"));
        assert_eq!(error(no), "no such region");
        assert_eq!(
            fault(
                "<fault><member><name>faultString</name><value><string>bad</string></value></member></fault>"
            ),
            "bad"
        );
    }
}
