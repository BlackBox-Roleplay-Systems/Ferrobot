//! Avatar names, both ways, cached for the life of the client (names do not change; display
//! names, which do, are not kept here).

use std::collections::HashMap;
use uuid::Uuid;

/// Past this many names the cache starts again: a bot that runs for weeks meets many avatars.
const MAX_NAMES: usize = 50_000;

#[derive(Default)]
pub struct Cache {
    by_id: HashMap<Uuid, String>,
    /// lower-case "first last" to key
    by_name: HashMap<String, Uuid>,
}

impl Cache {
    pub fn insert(&mut self, id: Uuid, first: &str, last: &str) {
        let name = normalize(&format!("{first} {last}"));
        if id.is_nil() || name.is_empty() {
            return;
        }
        if self.by_id.len() >= MAX_NAMES && !self.by_id.contains_key(&id) {
            self.by_id.clear();
            self.by_name.clear();
        }
        self.by_name.insert(name.to_lowercase(), id);
        self.by_id.insert(id, name);
    }

    pub fn name(&self, id: Uuid) -> Option<&str> {
        self.by_id.get(&id).map(String::as_str)
    }

    pub fn key(&self, name: &str) -> Option<Uuid> {
        self.by_name.get(&normalize(name).to_lowercase()).copied()
    }
}

/// "First Last": a username ("first.last" or "name") is the legacy name it stands for, "Resident"
/// when it has one part, as SecondBot's NormalizeAvatarName and the login do.
pub fn normalize(name: &str) -> String {
    let parts: Vec<&str> = name.split([' ', '.']).filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        [] => String::new(),
        [first] => format!("{first} Resident"),
        parts => parts.join(" "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_normalize_like_secondbot() {
        assert_eq!(normalize("someone"), "someone Resident");
        assert_eq!(normalize("  Some   One "), "Some One");
        assert_eq!(normalize("some.one"), "some one");
        assert_eq!(normalize(""), "");
    }

    #[test]
    fn the_cache_works_both_ways_whatever_the_case() {
        let mut c = Cache::default();
        let id = Uuid::from_u128(7);
        c.insert(id, "Some", "Resident");
        assert_eq!(c.name(id), Some("Some Resident"));
        assert_eq!(c.key("some"), Some(id));
        assert_eq!(c.key("SOME RESIDENT"), Some(id));
        assert_eq!(c.key("other"), None);
    }
}
