//! Ferro-client: a Second Life client as a library, for the bot's services to build on.
//!
//! - [`Client`] is a cheap handle (clone it freely): it sends requests to the network task and
//!   awaits their replies, hands out [`Event`]s, and reads the [`State`] the client keeps.
//! - [`Network`] is the network task: it owns the UDP socket, the circuit and the event queue,
//!   and runs one session at a time ([`Network::session`]). Nothing else touches them.
//!
//! The relog policy is the caller's: log in ([`login::login`]), run the session, decide what to
//! do when it ends. Requests made between sessions fail with [`ClientError::NotConnected`].
//! ARCHITECTURE.md, "How the pieces talk".

mod caps;
mod circuit;
pub mod events;
mod group_api;
mod inventory_api;
pub use inventory_api::FolderNode;
pub mod groups;
pub mod inventory;
pub mod login;
pub mod names;
mod network;
pub mod objects;
pub mod region_restart;
pub mod state;
mod xmlrpc;

pub use events::{ChatMessage, Event, InstantMessage, ScriptDialog};
pub use login::{LoginError, LoginRequest, LoginResponse, StartLocation};
pub use names::DisplayName;
pub use network::{End, Network};
pub use state::{Group, Health, Home, State};

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};
use uuid::Uuid;

/// How long a request waits for the grid's answer.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a teleport may take, from the request to the arrival.
pub const TELEPORT_TIMEOUT: Duration = Duration::from_secs(60);
/// Events a slow subscriber may fall behind by before it misses some.
const EVENT_BUFFER: usize = 1024;
const REQUEST_BUFFER: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClientError {
    #[error("the bot is not connected")]
    NotConnected,
    #[error("no answer from the grid in time")]
    Timeout,
    #[error("{0}")]
    Refused(String),
    #[error("the bot is not in that group")]
    UnknownGroup,
}

/// What the network task is asked to do. Each request that has an answer carries the
/// `oneshot` it goes back on.
#[derive(Debug)]
pub(crate) enum Request {
    /// Names of avatars (UUIDNameRequest); answered once all are known.
    Names {
        ids: Vec<Uuid>,
        reply: oneshot::Sender<()>,
    },
    /// Avatars whose name matches (AvatarPickerRequest).
    FindAvatars {
        name: String,
        reply: oneshot::Sender<Vec<(Uuid, String)>>,
    },
    /// Display names (the GetDisplayNames capability); an error when the region has none or it
    /// fails.
    DisplayNames {
        ids: Vec<Uuid>,
        reply: oneshot::Sender<Result<Vec<DisplayName>, String>>,
    },
    /// Local chat (ChatFromViewer): whisper, say or shout, on a channel.
    Chat {
        message: String,
        channel: i32,
        chat_type: u8,
        reply: oneshot::Sender<()>,
    },
    /// A plain IM to an avatar (IM_NOTHING_SPECIAL).
    Im {
        to: Uuid,
        message: String,
        reply: oneshot::Sender<()>,
    },
    /// The hover height (AgentPreferences).
    HoverHeight {
        height: f32,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Ask the region to bake the bot's outfit (UpdateAvatarAppearance); answered with the outfit
    /// version baked, or why not.
    Rebake {
        reply: oneshot::Sender<Result<i32, String>>,
    },
    /// A region's handle by its name (MapNameRequest); `None` when there is no such region.
    RegionHandle {
        name: String,
        reply: oneshot::Sender<Option<u64>>,
    },
    /// Answers to offers: friendships, inventory, teleports.
    Interact {
        interaction: Interaction,
        reply: oneshot::Sender<()>,
    },
    /// The inventory: fetches, gifts, changes.
    Inventory(network::InventoryRequest),
    /// Groups: roles, members, invitations, ejections, role changes.
    Group(network::GroupRequest),
    /// A teleport; answered with the region arrived in, or the grid's reason.
    Teleport {
        to: TeleportTo,
        reply: oneshot::Sender<Result<String, String>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Interaction {
    /// AcceptFriendship, the calling card in the inventory's root.
    AcceptFriendship { from: Uuid, transaction: Uuid },
    /// IM_INVENTORY_ACCEPTED / DECLINED (TASK_ for an object's offer), the item to the root.
    AnswerInventory {
        to: Uuid,
        transaction: Uuid,
        accept: bool,
        from_task: bool,
    },
    /// ScriptDialogReply: a button pressed.
    DialogReply {
        object_id: Uuid,
        channel: i32,
        index: i32,
        label: String,
    },
    /// MoneyBalanceRequest.
    RequestBalance,
    /// Movement, through the agent updates and the messages the viewer sends.
    Move(Move),
    /// MoneyTransferRequest: to an avatar (a gift), or to an object (its name given).
    Pay {
        to: Uuid,
        amount: i32,
        object_name: Option<String>,
    },
    /// IM_TELEPORT_REQUEST: ask an avatar for a teleport to them.
    RequestTeleport { to: Uuid, message: String },
    /// IM_FRIENDSHIP_OFFERED: offer an avatar friendship.
    OfferFriendship { to: Uuid },
    /// TerminateFriendship.
    EndFriendship { to: Uuid },
    /// GrantUserRights: what a friend may see and do (LLRelationship's rights).
    GrantRights { to: Uuid, rights: i32 },
    /// StartLure: offer an avatar a teleport to the bot.
    OfferTeleport { to: Uuid, message: String },
    /// IM_LURE_DECLINED: no to a teleport request.
    DeclineTeleportRequest { to: Uuid, transaction: Uuid },
}

/// What the bot can be asked to do with its body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Move {
    /// AGENT_CONTROL_FLY in every update, or not.
    Fly(bool),
    /// SetAlwaysRun.
    AlwaysRun(bool),
    /// AGENT_CONTROL_SIT_ON_GROUND, once.
    SitOnGround,
    /// AgentRequestSit on an object (AgentSit when the region asks for it).
    SitOn(Uuid),
    /// AGENT_CONTROL_STAND_UP, once.
    Stand,
    /// The region walks the bot there (GenericMessage "autopilot", LibreMetaverse's AutoPilot):
    /// a place in the bot's region.
    AutoPilot(glam::Vec3),
    /// AGENT_CONTROL_STOP, once: the autopilot stops.
    Stop,
    /// Face this way: a heading in degrees, clockwise from north.
    Face(f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TeleportTo {
    /// Accept a teleport offer (TeleportLureRequest).
    Lure(Uuid),
    /// TeleportLandmarkRequest with no landmark: home.
    Home,
    /// TeleportLocationRequest.
    Location {
        region_handle: u64,
        position: glam::Vec3,
    },
}

/// What the client and the network task share.
pub(crate) struct Shared {
    pub events: broadcast::Sender<Event>,
    state: RwLock<State>,
    names: Mutex<names::Cache>,
    /// The names of the inventory folders seen in fetches.
    folder_names: Mutex<HashMap<Uuid, String>>,
    /// Group member lists and when they were fetched.
    group_members: Mutex<HashMap<Uuid, (Vec<groups::GroupMember>, std::time::Instant)>>,
}

impl Shared {
    pub fn state(&self) -> std::sync::RwLockReadGuard<'_, State> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn state_mut(&self) -> std::sync::RwLockWriteGuard<'_, State> {
        self.state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn names(&self) -> MutexGuard<'_, names::Cache> {
        self.names
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn emit(&self, e: Event) {
        // no subscriber is not an error
        let _ = self.events.send(e);
    }
}

#[derive(Clone)]
pub struct Client {
    requests: mpsc::Sender<Request>,
    shared: Arc<Shared>,
}

impl Client {
    /// A client and its network task. `http` must be built with [`login::redirect_policy`].
    pub fn new(http: reqwest::Client) -> (Client, Network) {
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let shared = Arc::new(Shared {
            events,
            state: RwLock::new(State::default()),
            names: Mutex::new(names::Cache::default()),
            group_members: Mutex::new(HashMap::new()),
            folder_names: Mutex::new(HashMap::new()),
        });
        let (tx, rx) = mpsc::channel(REQUEST_BUFFER);
        (
            Client {
                requests: tx,
                shared: shared.clone(),
            },
            Network::new(http, rx, shared),
        )
    }

    /// Everything the client reports from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.shared.events.subscribe()
    }

    /// A copy of the client's state.
    pub fn state(&self) -> State {
        self.shared.state().clone()
    }

    pub fn health(&self) -> Health {
        self.shared.state().health()
    }

    async fn ask<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<T>) -> Request,
    ) -> Result<T, ClientError> {
        self.ask_within(REQUEST_TIMEOUT, make).await
    }

    async fn ask_within<T>(
        &self,
        timeout: Duration,
        make: impl FnOnce(oneshot::Sender<T>) -> Request,
    ) -> Result<T, ClientError> {
        if !self.shared.state().online {
            return Err(ClientError::NotConnected);
        }
        let (tx, rx) = oneshot::channel();
        self.requests
            .send(make(tx))
            .await
            .map_err(|_| ClientError::NotConnected)?;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(v)) => Ok(v),
            // the session ended before the answer
            Ok(Err(_)) => Err(ClientError::NotConnected),
            Err(_) => Err(ClientError::Timeout),
        }
    }

    /// The names ("First Last") of these avatars: from the cache, else asked of the grid. Ids the
    /// grid does not know are left out.
    pub async fn names(&self, ids: &[Uuid]) -> Result<HashMap<Uuid, String>, ClientError> {
        let missing: Vec<Uuid> = {
            let cache = self.shared.names();
            ids.iter()
                .copied()
                .filter(|id| !id.is_nil() && cache.name(*id).is_none())
                .collect()
        };
        if !missing.is_empty() {
            self.ask(|reply| Request::Names {
                ids: missing,
                reply,
            })
            .await?;
        }
        let cache = self.shared.names();
        Ok(ids
            .iter()
            .filter_map(|id| cache.name(*id).map(|n| (*id, n.to_owned())))
            .collect())
    }

    /// The name of one avatar, `None` when the grid does not know it.
    pub async fn name(&self, id: Uuid) -> Result<Option<String>, ClientError> {
        Ok(self.names(&[id]).await?.remove(&id))
    }

    /// Display names of these avatars: from the cache (an hour), else the People API
    /// (GetDisplayNames); a grid without it gives the legacy names instead, as the viewer shows
    /// them. Ids the grid does not know are left out.
    pub async fn display_names(
        &self,
        ids: &[Uuid],
    ) -> Result<HashMap<Uuid, DisplayName>, ClientError> {
        let missing: Vec<Uuid> = {
            let cache = self.shared.names();
            ids.iter()
                .copied()
                .filter(|id| !id.is_nil() && cache.display(*id).is_none())
                .collect()
        };
        if !missing.is_empty() {
            let asked = missing.clone();
            match self
                .ask(|reply| Request::DisplayNames { ids: asked, reply })
                .await?
            {
                Ok(found) => {
                    let mut cache = self.shared.names();
                    for d in found {
                        cache.insert_display(d, names::DISPLAY_NAME_TTL);
                    }
                }
                Err(why) => {
                    log::debug!("display names: {why}; legacy names instead");
                    let legacy = self.names(&missing).await?;
                    let mut cache = self.shared.names();
                    for (id, name) in legacy {
                        cache.insert_display(
                            DisplayName::from_legacy(id, &name),
                            names::LEGACY_DISPLAY_NAME_TTL,
                        );
                    }
                }
            }
        }
        let cache = self.shared.names();
        Ok(ids
            .iter()
            .filter_map(|id| cache.display(*id).map(|d| (*id, d.clone())))
            .collect())
    }

    /// The display name of one avatar.
    pub async fn display_name(&self, id: Uuid) -> Result<Option<DisplayName>, ClientError> {
        Ok(self.display_names(&[id]).await?.remove(&id))
    }

    /// The key of the avatar with this name ("First Last", or a username, "Resident" then
    /// implied, like SecondBot's Name2Key): from the cache, else searched on the grid.
    pub async fn find_avatar(&self, name: &str) -> Result<Option<Uuid>, ClientError> {
        let wanted = names::normalize(name);
        if wanted.is_empty() {
            return Ok(None);
        }
        if let Some(id) = self.shared.names().key(&wanted) {
            return Ok(Some(id));
        }
        let found = self
            .ask(|reply| Request::FindAvatars {
                name: wanted.clone(),
                reply,
            })
            .await?;
        Ok(found
            .into_iter()
            .find(|(_, n)| names::normalize(n).eq_ignore_ascii_case(&wanted))
            .map(|(id, _)| id))
    }

    async fn teleport(&self, to: TeleportTo) -> Result<String, ClientError> {
        self.ask_within(TELEPORT_TIMEOUT, |reply| Request::Teleport { to, reply })
            .await?
            .map_err(ClientError::Refused)
    }

    /// Accept a teleport offer (the `id` of an IM_LURE_USER instant message). Answers with the
    /// region arrived in.
    pub async fn accept_lure(&self, lure: Uuid) -> Result<String, ClientError> {
        self.teleport(TeleportTo::Lure(lure)).await
    }

    /// Teleport home.
    pub async fn teleport_home(&self) -> Result<String, ClientError> {
        self.teleport(TeleportTo::Home).await
    }

    /// Teleport to a place in a region, by the region's handle.
    pub async fn teleport_to(
        &self,
        region_handle: u64,
        position: glam::Vec3,
    ) -> Result<String, ClientError> {
        self.teleport(TeleportTo::Location {
            region_handle,
            position,
        })
        .await
    }

    /// Say something in local chat: `chat_type` is [`events::chat_type`]'s WHISPER, SAY or SHOUT;
    /// channel 0 is the one avatars hear.
    pub async fn chat(
        &self,
        message: &str,
        channel: i32,
        chat_type: u8,
    ) -> Result<(), ClientError> {
        let message = message.to_owned();
        self.ask(|reply| Request::Chat {
            message,
            channel,
            chat_type,
            reply,
        })
        .await
    }

    /// Send an IM to an avatar.
    pub async fn im(&self, to: Uuid, message: &str) -> Result<(), ClientError> {
        let message = message.to_owned();
        self.ask(|reply| Request::Im { to, message, reply }).await
    }

    async fn interact(&self, i: Interaction) -> Result<(), ClientError> {
        self.ask(|reply| Request::Interact {
            interaction: i,
            reply,
        })
        .await
    }

    /// Accept a friendship offer: the IM_FRIENDSHIP_OFFERED's sender and id.
    pub async fn accept_friendship(
        &self,
        from: Uuid,
        transaction: Uuid,
    ) -> Result<(), ClientError> {
        self.interact(Interaction::AcceptFriendship { from, transaction })
            .await?;
        let mut st = self.shared.state_mut();
        if !st.friends.contains(&from) {
            st.friends.push(from);
        }
        Ok(())
    }

    /// Answer an inventory offer (IM_INVENTORY_OFFERED, or IM_TASK_INVENTORY_OFFERED from an
    /// object): its sender and id. Accepted items go to the inventory's root folder.
    pub async fn answer_inventory_offer(
        &self,
        from: Uuid,
        transaction: Uuid,
        accept: bool,
        from_task: bool,
    ) -> Result<(), ClientError> {
        self.interact(Interaction::AnswerInventory {
            to: from,
            transaction,
            accept,
            from_task,
        })
        .await
    }

    /// Offer an avatar a teleport to where the bot is (StartLure).
    pub async fn offer_teleport(&self, to: Uuid, message: &str) -> Result<(), ClientError> {
        self.interact(Interaction::OfferTeleport {
            to,
            message: message.to_owned(),
        })
        .await
    }

    /// Say no to a teleport request (IM_TELEPORT_REQUEST): its sender and id.
    pub async fn decline_teleport_request(
        &self,
        to: Uuid,
        transaction: Uuid,
    ) -> Result<(), ClientError> {
        self.interact(Interaction::DeclineTeleportRequest { to, transaction })
            .await
    }

    /// Press a script dialog's button (ScriptDialogReply): the dialog's object and channel, the
    /// button's index and label.
    pub async fn reply_dialog(
        &self,
        object_id: Uuid,
        channel: i32,
        index: i32,
        label: &str,
    ) -> Result<(), ClientError> {
        self.interact(Interaction::DialogReply {
            object_id,
            channel,
            index,
            label: label.to_owned(),
        })
        .await
    }

    /// A region's handle by its name, from the world map (MapNameRequest); `None` when the grid
    /// has no region of that name.
    pub async fn region_handle(&self, name: &str) -> Result<Option<u64>, ClientError> {
        let name = name.trim().to_owned();
        self.ask(|reply| Request::RegionHandle { name, reply })
            .await
    }

    /// Teleport to a place in a region given by name (LibreMetaverse's Teleport(simName, …)).
    pub async fn teleport_to_region(
        &self,
        region: &str,
        position: glam::Vec3,
    ) -> Result<String, ClientError> {
        match self.region_handle(region).await? {
            Some(handle) => self.teleport_to(handle, position).await,
            None => Err(ClientError::Refused(format!("no region named {region}"))),
        }
    }

    /// Ask the region to bake the bot's outfit again (server-side baking), as SecondBot's
    /// RebakeAppearance; answers with the outfit version baked. The bot asks once by itself
    /// after each login.
    pub async fn rebake(&self) -> Result<i32, ClientError> {
        self.ask_within(Duration::from_secs(60), |reply| Request::Rebake { reply })
            .await?
            .map_err(ClientError::Refused)
    }

    /// The L$ balance: asked of the grid (MoneyBalanceRequest), answered when it says.
    pub async fn balance(&self) -> Result<i32, ClientError> {
        let before = self.shared.state().balance_updates;
        self.interact(Interaction::RequestBalance).await?;
        let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
        loop {
            {
                let st = self.shared.state();
                if st.balance_updates > before
                    && let Some(b) = st.balance
                {
                    return Ok(b);
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ClientError::Timeout);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Pay an avatar (a gift), or an object by its key and name (LLGiveMoney, PayObject).
    pub async fn pay(
        &self,
        to: Uuid,
        amount: i32,
        object_name: Option<&str>,
    ) -> Result<(), ClientError> {
        self.interact(Interaction::Pay {
            to,
            amount,
            object_name: object_name.map(str::to_owned),
        })
        .await
    }

    /// Ask an avatar for a teleport to them (IM_TELEPORT_REQUEST).
    pub async fn request_teleport(&self, to: Uuid, message: &str) -> Result<(), ClientError> {
        self.interact(Interaction::RequestTeleport {
            to,
            message: message.to_owned(),
        })
        .await
    }

    /// Offer an avatar friendship.
    pub async fn offer_friendship(&self, to: Uuid) -> Result<(), ClientError> {
        self.interact(Interaction::OfferFriendship { to }).await
    }

    /// End a friendship.
    pub async fn end_friendship(&self, to: Uuid) -> Result<(), ClientError> {
        self.interact(Interaction::EndFriendship { to }).await?;
        let mut st = self.shared.state_mut();
        st.friends.retain(|f| *f != to);
        st.friends_online.retain(|f| *f != to);
        Ok(())
    }

    /// What a friend may do: see the bot online (1), on the map (2), edit its objects (4)
    /// (LLRelationship::GRANT_*).
    pub async fn grant_rights(&self, to: Uuid, rights: i32) -> Result<(), ClientError> {
        self.interact(Interaction::GrantRights { to, rights }).await
    }

    /// Move the bot's body ([`Move`]).
    pub async fn move_bot(&self, m: Move) -> Result<(), ClientError> {
        self.interact(Interaction::Move(m)).await
    }

    /// The avatar's hover height, -2 to 2 metres (the AgentPreferences capability, as
    /// LLAgent::sendHoverHeight and Aurora's set_hover_height).
    pub async fn set_hover_height(&self, height: f32) -> Result<(), ClientError> {
        self.ask(|reply| Request::HoverHeight { height, reply })
            .await?
            .map_err(ClientError::Refused)
    }
}
