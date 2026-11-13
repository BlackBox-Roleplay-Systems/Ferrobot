//! SecondBot's Avatars commands.

use super::reply::{Answer, basic, cs_number};
use ferro_client::Client;
use ferro_client::profile::ProfileChange;

/// Name2Key: the avatar's key, or "lookup" when the grid does not know the name (SecondBot answers
/// "lookup" until its name cache has it; Ferrobot asks the grid at once and waits).
pub async fn name2key(client: &Client, name: &str) -> Answer {
    match super::avatar(client, name).await {
        Some(id) => basic("Name2Key", &id.to_string(), &[]),
        None => basic("Name2Key", "lookup", &[]),
    }
}

/// NearmeWithDetails: the avatars the object updates place (within the draw distance), each
/// `{id, name, x, y, z, range}`, the range in metres to two places.
pub async fn nearme_with_details(client: &Client) -> Answer {
    const C: &str = "NearmeWithDetails";
    let st = client.state();
    if !st.online {
        return basic(C, "Error not in a sim", &[]);
    }
    let ids: Vec<uuid::Uuid> = st.avatars.keys().copied().collect();
    let names = client.names(&ids).await.unwrap_or_default();
    let mut list: Vec<(uuid::Uuid, glam::Vec3)> =
        st.avatars.iter().map(|(k, v)| (*k, *v)).collect();
    list.sort_by_key(|(id, _)| *id);
    let out: Vec<NearMeDetails> = list
        .iter()
        .map(|(id, at)| NearMeDetails {
            id: id.to_string(),
            name: names.get(id).cloned().unwrap_or_default(),
            x: at.x.round() as i64,
            y: at.y.round() as i64,
            z: at.z.round() as i64,
            range: cs_number((f64::from(at.distance(st.position)) * 100.0).round() / 100.0),
        })
        .collect();
    basic(C, &super::reply::json(&out), &[])
}

/// SecondBot's NearMeDetails, its fields in their order.
#[derive(serde::Serialize)]
struct NearMeDetails {
    id: String,
    name: String,
    x: i64,
    y: i64,
    z: i64,
    range: serde_json::Value,
}

/// GetProfilePicks: the avatar's picks, `[{id, name}]` in the grid's order; "No reply from the
/// grid [Retry later]" after SecondBot's 10 s.
pub async fn profile_picks(client: &Client, who: &str) -> Answer {
    const C: &str = "GetProfilePicks";
    let args = super::owned(&[who]);
    let Some(id) = super::avatar(client, who).await else {
        return super::reply::failure(C, "Invaild avatar uuid", &args);
    };
    match client.picks(id).await {
        Ok(picks) => {
            let list: Vec<Pick> = picks
                .into_iter()
                .map(|(id, name)| Pick {
                    id: id.to_string(),
                    name,
                })
                .collect();
            basic(C, &super::reply::json(&list), &args)
        }
        Err(e) => {
            log::debug!("{C}: {e}");
            super::reply::failure(C, "No reply from the grid [Retry later]", &args)
        }
    }
}

/// A pick as SecondBot lists it: a Dictionary<string, string> of id, then name.
#[derive(serde::Serialize)]
struct Pick {
    id: String,
    name: String,
}

/// SecondBot's text when the profile has not come (two spaces, as it writes it).
const RETRY: &str = "Requesting avatar details  [Retry later]";

pub async fn profile_about(client: &Client, who: &str) -> Answer {
    const C: &str = "GetProfileAbout";
    let args = super::owned(&[who]);
    let Some(id) = super::avatar(client, who).await else {
        return super::reply::failure(C, "Invaild avatar uuid", &args);
    };
    match client.profile(id).await {
        Ok(p) => basic(C, &p.about, &[]),
        Err(e) => {
            log::debug!("{C}: {e}");
            super::reply::failure(C, RETRY, &args)
        }
    }
}

pub async fn profile_image(client: &Client, who: &str) -> Answer {
    const C: &str = "GetProfileImage";
    let args = super::owned(&[who]);
    let Some(id) = super::avatar(client, who).await else {
        return super::reply::failure(C, "Invaild avatar uuid", &args);
    };
    match client.profile(id).await {
        Ok(p) => basic(C, &p.image.to_string(), &[]),
        Err(e) => {
            log::debug!("{C}: {e}");
            super::reply::failure(C, RETRY, &args)
        }
    }
}

/// SetProfileAbout: 10 to 300 characters.
pub async fn set_profile_about(client: &Client, text: &str) -> Answer {
    const C: &str = "SetProfileAbout";
    let args = super::owned(&[text]);
    let n = text.chars().count();
    if n < 10 {
        return super::reply::failure(C, "About text to short", &args);
    }
    if n > 300 {
        return super::reply::failure(C, "About text to long", &args);
    }
    save(client, C, ProfileChange::About(text.to_owned()), &args).await
}

pub async fn set_profile_image(client: &Client, texture: &str) -> Answer {
    const C: &str = "SetProfileImage";
    let args = super::owned(&[texture]);
    let Some(id) = super::key(texture) else {
        // SecondBot's text, its stray quote too
        return super::reply::failure(C, "Invaild texture uuid\"", &args);
    };
    save(client, C, ProfileChange::Image(id), &args).await
}

async fn save(client: &Client, c: &str, change: ProfileChange, args: &[String]) -> Answer {
    match client.update_profile(change).await {
        Ok(()) => basic(c, "ok", &[]),
        Err(e) => {
            log::warn!("{c}: {e}");
            super::reply::failure(c, RETRY, args)
        }
    }
}
