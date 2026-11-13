//! SecondBot's Avatars commands.

use super::reply::{Answer, basic, cs_number};
use ferro_client::Client;

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
