//! SecondBot's Avatars commands.

use super::reply::{Answer, basic};
use ferro_client::Client;

/// Name2Key: the avatar's key, or "lookup" when the grid does not know the name (SecondBot answers
/// "lookup" until its name cache has it; Ferrobot asks the grid at once and waits).
pub async fn name2key(client: &Client, name: &str) -> Answer {
    match super::avatar(client, name).await {
        Some(id) => basic("Name2Key", &id.to_string(), &[]),
        None => basic("Name2Key", "lookup", &[]),
    }
}
