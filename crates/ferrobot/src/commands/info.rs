//! SecondBot's Info commands.

use super::reply::{Answer, basic, failure};
use ferro_client::Client;

/// SimName: the region's name, or "Error not in a sim".
pub fn sim_name(client: &Client) -> Answer {
    match client.state().region {
        Some(region) if client.health().online => basic("SimName", &region.name, &[]),
        _ => failure("SimName", "Error not in a sim", &[]),
    }
}

/// Name: "Firstname Lastname".
pub fn name(client: &Client) -> Answer {
    basic("Name", &client.state().name, &[])
}

/// Hello: "world" (Core).
pub fn hello() -> Answer {
    basic("Hello", "world", &[])
}

/// Version: the bot's build.
pub fn version() -> Answer {
    basic("Version", &ferro_client::login::version(), &[])
}

/// UnixTimeNow: the bot's clock (for signing commands).
pub fn unix_time_now() -> Answer {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    basic("UnixTimeNow", &t.to_string(), &[])
}

/// GetPosition: `{x, y, z}` in the region, rounded.
pub fn get_position(client: &Client) -> Answer {
    let st = client.state();
    if !st.online {
        return failure("GetPosition", "Error not in a sim", &[]);
    }
    let p = st.position;
    let pos = serde_json::json!({
        "x": p.x.round() as i64,
        "y": p.y.round() as i64,
        "z": p.z.round() as i64,
    });
    basic("GetPosition", &pos.to_string(), &[])
}
