//! Avatar names, both ways, cached for the life of the client (names do not change), and display
//! names, which do, cached for an hour.

use ferro_llsd::Llsd;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// How long a display name is kept (LLAvatarNameCache keeps them for the reply's max-age, an hour
/// by default).
pub const DISPLAY_NAME_TTL: Duration = Duration::from_secs(3600);
/// A legacy name standing for a display name is kept a minute only: the region's capabilities may
/// just not be known yet (the seed answers a moment after the arrival).
pub const LEGACY_DISPLAY_NAME_TTL: Duration = Duration::from_secs(60);

/// Past this many names the cache starts again: a bot that runs for weeks meets many avatars.
const MAX_NAMES: usize = 50_000;

#[derive(Default)]
pub struct Cache {
    by_id: HashMap<Uuid, String>,
    /// lower-case "first last" to key
    by_name: HashMap<String, Uuid>,
    display: HashMap<Uuid, (DisplayName, Instant)>,
}

/// An avatar's display name and account name (the People API's GetDisplayNames, read as
/// LLAvatarName::fromLLSD and Aurora's AvatarNameData::from_llsd).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayName {
    pub id: Uuid,
    pub display_name: String,
    /// Account name: "jane.doe", or "janedoe" for a Resident.
    pub username: String,
    pub legacy_first: String,
    pub legacy_last: String,
    /// No display name chosen: it is made from the username.
    pub is_default: bool,
}

impl DisplayName {
    pub fn from_llsd(v: &Llsd) -> DisplayName {
        let mut d = DisplayName {
            id: v["id"].as_uuid(),
            username: v["username"].to_string_value(),
            display_name: v["display_name"].to_string_value(),
            legacy_first: v["legacy_first_name"].to_string_value(),
            legacy_last: v["legacy_last_name"].to_string_value(),
            is_default: v["is_display_name_default"].as_bool(),
        };
        // older People API replies name it "sl_id"
        if d.username.is_empty() {
            d.username = v["sl_id"].to_string_value();
        }
        // some avatars have no explicit display name: force a legible one
        if d.display_name.is_empty() {
            d.display_name = d.username.clone();
            d.is_default = true;
        }
        d
    }

    /// Without the People API (a grid without GetDisplayNames): the legacy name stands for the
    /// display name, as LLAvatarNameCache's legacy mode does.
    pub fn from_legacy(id: Uuid, name: &str) -> DisplayName {
        let (first, last) = name.split_once(' ').unwrap_or((name, "Resident"));
        let username = if last.eq_ignore_ascii_case("resident") {
            first.to_lowercase()
        } else {
            format!("{first}.{last}").to_lowercase()
        };
        DisplayName {
            id,
            display_name: name.to_owned(),
            username,
            legacy_first: first.to_owned(),
            legacy_last: last.to_owned(),
            is_default: true,
        }
    }
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

    pub fn insert_display(&mut self, d: DisplayName, ttl: Duration) {
        if d.id.is_nil() {
            return;
        }
        if !d.legacy_first.is_empty() {
            self.insert(d.id, &d.legacy_first.clone(), &d.legacy_last.clone());
        }
        if self.display.len() >= MAX_NAMES && !self.display.contains_key(&d.id) {
            self.display.clear();
        }
        self.display.insert(d.id, (d, Instant::now() + ttl));
    }

    pub fn display(&self, id: Uuid) -> Option<&DisplayName> {
        self.display
            .get(&id)
            .filter(|(_, expires)| Instant::now() < *expires)
            .map(|(d, _)| d)
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
    fn display_names_from_the_people_api() {
        let v = ferro_llsd::llsd_map! {
            "id" => Uuid::from_u128(1),
            "username" => "jane.doe",
            "display_name" => "Jane",
            "legacy_first_name" => "Jane",
            "legacy_last_name" => "Doe",
            "is_display_name_default" => false
        };
        let d = DisplayName::from_llsd(&v);
        assert_eq!(
            (d.display_name.as_str(), d.username.as_str()),
            ("Jane", "jane.doe")
        );
        assert!(!d.is_default);
        // no display name: the username, and default
        let v = ferro_llsd::llsd_map! { "id" => Uuid::from_u128(2), "sl_id" => "someone" };
        let d = DisplayName::from_llsd(&v);
        assert_eq!(
            (d.display_name.as_str(), d.username.as_str()),
            ("someone", "someone")
        );
        assert!(d.is_default);
    }

    #[test]
    fn display_names_from_legacy_names() {
        let d = DisplayName::from_legacy(Uuid::from_u128(1), "Some Resident");
        assert_eq!(
            (d.display_name.as_str(), d.username.as_str()),
            ("Some Resident", "some")
        );
        let d = DisplayName::from_legacy(Uuid::from_u128(1), "Other Avatar");
        assert_eq!(d.username, "other.avatar");
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
