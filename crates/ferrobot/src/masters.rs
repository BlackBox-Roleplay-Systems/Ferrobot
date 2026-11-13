//! The masters: the avatars named in FERRO_MASTERS (SecondBot's MastersCSV).
//!
//! By name, as SecondBot compares them, without case, a username standing for "<name> Resident"
//! (its "dot check"). By key where the grid gives the sender's key (IMs, teleport offers): the
//! names are looked up once and the sender's key compared, so that a name typed in a message
//! cannot pass for a master's. That is stricter than SecondBot, deliberately.

use ferro_client::Client;
use std::sync::Mutex;
use uuid::Uuid;

/// "First Last", lower case: "some" and "Some.Resident" are both "some resident".
fn canonical(name: &str) -> String {
    let parts: Vec<&str> = name.split([' ', '.']).filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        [one] => format!("{one} resident"),
        parts => parts.join(" "),
    }
    .to_lowercase()
}

pub struct Masters {
    names: Vec<String>,
    /// Keys found so far, by the master's name.
    keys: Mutex<Vec<(String, Uuid)>>,
}

impl Masters {
    pub fn new(names: Vec<String>) -> Self {
        Masters {
            names,
            keys: Mutex::new(Vec::new()),
        }
    }

    pub fn by_name(&self, name: &str) -> bool {
        let name = canonical(name);
        !name.is_empty() && self.names.iter().any(|m| canonical(m) == name)
    }

    /// Whether this key is a master's: the masters' names looked up on the grid, once each.
    pub async fn by_key(&self, client: &Client, id: Uuid) -> bool {
        if id.is_nil() {
            return false;
        }
        for name in &self.names {
            let known = self.lock().iter().find(|(n, _)| n == name).map(|(_, k)| *k);
            let key = match known {
                Some(key) => key,
                None => match client.find_avatar(name).await {
                    Ok(Some(key)) => {
                        self.lock().push((name.clone(), key));
                        key
                    }
                    Ok(None) => {
                        log::warn!("master {name}: no such avatar");
                        continue;
                    }
                    Err(e) => {
                        log::debug!("master {name}: not looked up ({e})");
                        continue;
                    }
                },
            };
            if key == id {
                return true;
            }
        }
        false
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<(String, Uuid)>> {
        self.keys
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masters_by_name_or_username_whatever_the_case() {
        let m = Masters::new(vec!["Some Resident".to_owned(), "Other Avatar".to_owned()]);
        assert!(m.by_name("Some Resident"));
        assert!(m.by_name("some"));
        assert!(m.by_name("other.avatar"));
        assert!(m.by_name("OTHER AVATAR"));
        assert!(!m.by_name("Someone Else"));
        assert!(!m.by_name(""));
        assert!(!Masters::new(vec![]).by_name("Some Resident"));
    }
}
