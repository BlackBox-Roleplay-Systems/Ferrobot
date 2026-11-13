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
