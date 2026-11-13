//! The state the client keeps, read by the bot's services: who it is, where, its groups.

use glam::Vec3;
use std::time::{Duration, Instant, SystemTime};
use uuid::Uuid;

#[derive(Debug, Clone, Default)]
pub struct State {
    /// In world: arrived in a region and not disconnected since.
    pub online: bool,
    pub agent_id: Uuid,
    /// "First Last"
    pub name: String,
    pub region: Option<Region>,
    /// In the region, as the region last said.
    pub position: Vec3,
    pub home: Option<Home>,
    pub active_group: Option<ActiveGroup>,
    pub groups: Vec<Group>,
    /// The bot's friends (the login's buddy list, then the friendships it accepts).
    pub friends: Vec<Uuid>,
    /// The group chats the bot is in (their session id is the group's).
    pub group_chats: Vec<Uuid>,
    /// Logins that reached the region since the start.
    pub logins: u32,
    pub online_since: Option<SystemTime>,
    pub last_packet: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub name: String,
    pub handle: u64,
}

/// Where "home" is (the login reply's `home`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Home {
    pub region_handle: u64,
    pub position: Vec3,
    pub look_at: Vec3,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActiveGroup {
    pub id: Uuid,
    pub name: String,
    pub title: String,
}

/// A group the bot is in (AgentGroupDataUpdate).
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub id: Uuid,
    pub name: String,
    pub insignia: Uuid,
    /// GP_* bits of the bot's roles in the group.
    pub powers: u64,
    pub accept_notices: bool,
    pub contribution: i32,
}

/// What a health check reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Health {
    pub online: bool,
    pub region: Option<String>,
    pub since_last_packet: Option<Duration>,
    pub logins: u32,
}

impl State {
    pub fn health(&self) -> Health {
        Health {
            online: self.online,
            region: self.region.as_ref().map(|r| r.name.clone()),
            since_last_packet: self.last_packet.map(|t| t.elapsed()),
            logins: self.logins,
        }
    }
}

/// A region handle from the region's corner on the grid, in metres.
pub fn region_handle(x: u32, y: u32) -> u64 {
    (u64::from(x) << 32) | u64::from(y)
}
