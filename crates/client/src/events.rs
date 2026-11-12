//! What the client reports, on a broadcast channel ([`crate::Client::subscribe`]).

use glam::Vec3;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum Event {
    /// In world: AgentMovementComplete from the region.
    Arrived { region: String, position: Vec3 },
    /// Local chat heard, the bot's own included (ChatFromSimulator).
    Chat(ChatMessage),
    /// An instant message of any kind: `dialog` says which (ImprovedInstantMessage).
    InstantMessage(InstantMessage),
    /// A message from the grid for the user (AlertMessage, AgentAlertMessage).
    Alert(String),
    /// The region announced a restart (AlertMessage): `seconds` left; `region` when the grid
    /// names it (Second Life does, OpenSim does not).
    RegionRestart {
        seconds: i32,
        region: Option<String>,
    },
    /// The announced restart was cancelled.
    RegionRestartCancelled,
    /// The groups the bot is in changed (AgentGroupDataUpdate, AgentDropGroup).
    GroupsChanged,
    /// A teleport failed; the bot stays where it is (TeleportFailed).
    TeleportFailed { reason: String },
    /// The session ended; a new login comes next, or not, as the caller decides.
    Disconnected { reason: String },
}

/// Chat types (`EChatType`, llchat.h).
pub mod chat_type {
    pub const WHISPER: u8 = 0;
    pub const SAY: u8 = 1;
    pub const SHOUT: u8 = 2;
    pub const START_TYPING: u8 = 4;
    pub const STOP_TYPING: u8 = 5;
    pub const OWNER_SAY: u8 = 8;
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub from_name: String,
    pub source_id: Uuid,
    pub owner_id: Uuid,
    /// 0 system, 1 avatar, 2 object (`EChatSourceType`)
    pub source_type: u8,
    pub chat_type: u8,
    pub audible: u8,
    pub position: Vec3,
    pub message: String,
}

/// IM dialog types (`EInstantMessage`, llinstantmessage.h), those the bot meets first.
pub mod dialog {
    pub const NOTHING_SPECIAL: u8 = 0;
    pub const MESSAGEBOX: u8 = 1;
    pub const GROUP_INVITATION: u8 = 3;
    pub const INVENTORY_OFFERED: u8 = 4;
    pub const GROUP_NOTICE: u8 = 32;
    pub const FRIENDSHIP_OFFERED: u8 = 38;
    pub const LURE_USER: u8 = 22;
    pub const SESSION_SEND: u8 = 17;
    pub const TYPING_START: u8 = 41;
    pub const TYPING_STOP: u8 = 42;
}

#[derive(Debug, Clone)]
pub struct InstantMessage {
    pub from_agent_id: Uuid,
    pub from_name: String,
    pub to_agent_id: Uuid,
    pub dialog: u8,
    /// The IM session, or the id of what is offered (a group, an item, a lure).
    pub id: Uuid,
    pub message: String,
    pub from_group: bool,
    pub offline: bool,
    pub timestamp: u32,
    pub region_id: Uuid,
    pub position: Vec3,
    pub binary_bucket: Vec<u8>,
}
