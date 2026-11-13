//! Avatar profiles: the AgentProfile capability's reply and the older AvatarPropertiesReply, as
//! LLAvatarPropertiesProcessor reads them, and the picks the capability's reply carries; after
//! Aurora's profile.rs (see NOTICE.md).

use ferro_llsd::Llsd;
use uuid::Uuid;

/// What the bot reads and writes of a profile.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Profile {
    pub id: Uuid,
    pub about: String,
    pub image: Uuid,
    pub first_life_about: String,
    pub first_life_image: Uuid,
    pub allow_publish: bool,
    pub profile_url: String,
}

/// A change to the bot's own profile.
#[derive(Debug, Clone, PartialEq)]
pub enum ProfileChange {
    About(String),
    Image(Uuid),
}

impl ProfileChange {
    /// The AgentProfile capability's PUT body: only what changes (saveAgentUserInfoCoro).
    pub fn body(&self) -> Llsd {
        let mut m = ferro_llsd::Map::new();
        match self {
            ProfileChange::About(t) => m.insert("sl_about_text".into(), t.clone().into()),
            ProfileChange::Image(id) => m.insert("sl_image_id".into(), (*id).into()),
        };
        Llsd::Map(m)
    }

    /// The whole profile with the change, for AvatarPropertiesUpdate.
    pub fn apply(&self, p: &Profile) -> Profile {
        let mut p = p.clone();
        match self {
            ProfileChange::About(t) => p.about = t.clone(),
            ProfileChange::Image(id) => p.image = *id,
        }
        p
    }
}

/// GET <AgentProfile>/<id>'s reply (requestAvatarPropertiesCoro): None when it is about another
/// avatar than the one asked for.
pub fn parse_agent_profile(requested: Uuid, v: &Llsd) -> Option<Profile> {
    if v["id"].as_uuid() != requested {
        return None;
    }
    Some(Profile {
        id: requested,
        about: v["sl_about_text"].to_string_value(),
        image: v["sl_image_id"].as_uuid(),
        first_life_about: v["fl_about_text"].to_string_value(),
        first_life_image: v["fl_image_id"].as_uuid(),
        allow_publish: v["allow_publish"].as_bool(),
        profile_url: v["home_page"].to_string_value(),
    })
}

/// The picks of GET <AgentProfile>/<id>'s reply, (key, name) in the order given, as Aurora's
/// parse_agent_profile reads them (a nil key left out): None when it is about another avatar.
pub fn parse_agent_profile_picks(requested: Uuid, v: &Llsd) -> Option<Vec<(Uuid, String)>> {
    if v["id"].as_uuid() != requested {
        return None;
    }
    Some(
        v["picks"]
            .as_array()
            .iter()
            .map(|p| (p["id"].as_uuid(), p["name"].to_string_value()))
            .filter(|p| !p.0.is_nil())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_capability_reply_of_the_avatar_asked_for() {
        let id = Uuid::from_u128(5);
        let v = ferro_llsd::llsd_map! {
            "id" => id,
            "sl_about_text" => "Hello",
            "sl_image_id" => Uuid::from_u128(9),
            "allow_publish" => true,
        };
        let p = parse_agent_profile(id, &v).expect("a profile");
        assert_eq!(
            (p.about.as_str(), p.image, p.allow_publish),
            ("Hello", Uuid::from_u128(9), true)
        );
        assert!(parse_agent_profile(Uuid::from_u128(6), &v).is_none());
        let c = ProfileChange::About("New".into());
        assert_eq!(c.body()["sl_about_text"].as_str(), "New");
        assert_eq!(c.apply(&p).image, p.image, "the rest kept");
    }

    #[test]
    fn the_picks_of_the_capability_reply() {
        let id = Uuid::from_u128(5);
        let pick = |n: u128, name: &str| {
            ferro_llsd::llsd_map! { "id" => Uuid::from_u128(n), "name" => name }
        };
        let v = ferro_llsd::llsd_map! {
            "id" => id,
            "picks" => Llsd::Array(vec![pick(3, "Beach"), pick(0, "nil"), pick(4, "Club")]),
        };
        assert_eq!(
            parse_agent_profile_picks(id, &v),
            Some(vec![
                (Uuid::from_u128(3), "Beach".to_owned()),
                (Uuid::from_u128(4), "Club".to_owned())
            ])
        );
        let none = ferro_llsd::llsd_map! { "id" => id };
        assert_eq!(parse_agent_profile_picks(id, &none), Some(Vec::new()));
        assert!(parse_agent_profile_picks(Uuid::from_u128(6), &v).is_none());
    }
}
