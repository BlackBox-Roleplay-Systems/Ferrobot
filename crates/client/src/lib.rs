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
pub mod estate;
pub mod events;
pub mod gesture;
mod group_api;
mod inventory_api;
pub use inventory_api::FolderNode;
pub mod groups;
pub mod inventory;
pub mod login;
pub mod mutes;
pub mod names;
mod network;
pub use network::{ObjectOwner, ParcelAction};
pub mod objects;
mod outfit;
pub mod parcels;
pub mod profile;
pub mod region_restart;
pub mod state;
mod transfer;
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
    Parcel(network::ParcelRequest),
    Estate(network::EstateRequest),
    Profile(network::ProfileRequest),
    Object(network::ObjectRequest),
    /// An asset from the ViewerAsset capability (`kind` its query key, gesture_id…).
    ViewerAsset {
        kind: &'static str,
        asset: Uuid,
        reply: oneshot::Sender<Result<Vec<u8>, String>>,
    },
    FindRegion {
        name: String,
        reply: oneshot::Sender<Option<MapRegion>>,
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

/// A region on the world map (MapBlockReply).
#[derive(Debug, Clone, PartialEq)]
pub struct MapRegion {
    pub name: String,
    /// Its place on the grid, in regions.
    pub x: u16,
    pub y: u16,
    pub handle: u64,
    /// Its map tile.
    pub map_image: Uuid,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Interaction {
    /// FindAgent: where a friend is on the map.
    FindAgent { prey: Uuid },
    /// MuteListRequest, without a cached copy.
    RequestMuteList,
    /// AgentAnimation: animations started (true) or stopped.
    Animate { anims: Vec<(Uuid, bool)> },
    /// SoundTrigger where the bot stands.
    Sound { asset: Uuid },
    /// ViewerEffect: point at an avatar, and a beam to it.
    PointAt { target: Uuid },
    /// ScriptAnswerYes: the permissions granted to a script.
    ScriptAnswer {
        task: Uuid,
        item: Uuid,
        questions: i32,
    },
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
    /// Away (or back), as the viewer's setAFK and clearAFK.
    Away(bool),
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
    /// The items seen in fetches, counted (InventoryPreloadCount).
    seen_items: Mutex<std::collections::HashSet<Uuid>>,
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
            seen_items: Mutex::new(std::collections::HashSet::new()),
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
        Ok(self.find_region(name).await?.map(|r| r.handle))
    }

    /// A region by its name, from the world map (MapNameRequest): where it is on the grid and
    /// its map tile.
    pub async fn find_region(&self, name: &str) -> Result<Option<MapRegion>, ClientError> {
        let name = name.trim().to_owned();
        self.ask(|reply| Request::FindRegion { name, reply }).await
    }

    /// The parcel the bot stands on: the one the region told, else asked for (waiting up to
    /// `wait`).
    pub async fn parcel(
        &self,
        wait: std::time::Duration,
    ) -> Result<Option<parcels::ParcelInfo>, ClientError> {
        if let Some(p) = self.shared.state().parcel.clone() {
            return Ok(Some(p));
        }
        match self
            .ask_within(wait, |reply| {
                Request::Parcel(network::ParcelRequest::Properties { reply })
            })
            .await
        {
            Ok(p) => Ok(Some(p)),
            Err(ClientError::Timeout) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Every parcel of the region, as LibreMetaverse's RequestAllSimParcels learns them: the
    /// parcel of a 4 m cell no parcel known covers yet, asked, then the cells its bitmap covers
    /// marked, until the whole map is known. Kept in the state (`region_parcels`).
    pub async fn region_parcels(&self) -> Result<parcels::RegionParcels, ClientError> {
        let size = parcels::MAP_SIZE;
        let mut out = parcels::RegionParcels {
            parcels: Vec::new(),
            map: vec![0; size * size],
        };
        // a region that stops answering: what is known so far, not 4096 waits
        let mut silent = 0;
        'cells: for y in 0..size {
            for x in 0..size {
                if out.map[y * size + x] != 0 {
                    continue;
                }
                let answer = self
                    .ask_within(Duration::from_secs(2), |reply| {
                        Request::Parcel(network::ParcelRequest::Cell { x, y, reply })
                    })
                    .await;
                let p = match answer {
                    Ok(p) => {
                        silent = 0;
                        p
                    }
                    // no answer for this cell: the next ones may still come
                    Err(ClientError::Timeout) => {
                        silent += 1;
                        if silent >= 3 {
                            log::warn!("parcels: the region stopped answering");
                            break 'cells;
                        }
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                let mut marked = false;
                for cy in 0..size {
                    for cx in 0..size {
                        if parcels::covers(&p.bitmap, cx, cy) {
                            out.map[cy * size + cx] = p.local_id;
                            marked = true;
                        }
                    }
                }
                if !marked {
                    // a parcel without its bitmap: at least the cell asked is known
                    out.map[y * size + x] = p.local_id;
                }
                if !out.parcels.iter().any(|q| q.local_id == p.local_id) {
                    out.parcels.push(p);
                }
            }
        }
        self.shared.state_mut().region_parcels = Some(out.clone());
        Ok(out)
    }

    /// Land for sale (DirLandQuery): LibreMetaverse's StartLandSearch, its flags and kind.
    pub async fn land_search(
        &self,
        flags: u32,
        kind: u32,
        price: i32,
        area: i32,
        start: i32,
        wait: Duration,
    ) -> Result<Vec<parcels::LandForSale>, ClientError> {
        self.ask_within(wait, |reply| {
            Request::Parcel(network::ParcelRequest::LandSearch {
                flags,
                kind,
                price,
                area,
                start,
                reply,
            })
        })
        .await
    }

    /// A parcel's dwell, its traffic.
    pub async fn parcel_dwell(&self, local_id: i32) -> Result<f32, ClientError> {
        self.ask(|reply| Request::Parcel(network::ParcelRequest::Dwell { local_id, reply }))
            .await
    }

    /// The global key of the parcel the bot stands on.
    pub async fn parcel_global_id(&self) -> Result<Uuid, ClientError> {
        self.ask(|reply| Request::Parcel(network::ParcelRequest::GlobalId { reply }))
            .await?
            .map_err(ClientError::Refused)
    }

    /// An EstateOwnerMessage: `method` and its strings (kickestate, estateaccessdelta, restart,
    /// simulatormessage, teleporthomeuser, setregioninfo, estatechangeinfo,
    /// estatechangecovenantid…).
    pub async fn estate_message(
        &self,
        method: &str,
        strings: Vec<String>,
    ) -> Result<(), ClientError> {
        let method = method.to_owned();
        self.ask(|reply| {
            Request::Estate(network::EstateRequest::Owner {
                method,
                strings,
                reply,
            })
        })
        .await
    }

    /// The estate's info (`getinfo`).
    pub async fn estate_info(&self) -> Result<estate::EstateInfo, ClientError> {
        self.ask(|reply| Request::Estate(network::EstateRequest::Info { reply }))
            .await
    }

    /// The estate's managers and bans (the EstateAccess capability).
    pub async fn estate_access(&self) -> Result<estate::EstateAccess, ClientError> {
        self.ask(|reply| Request::Estate(network::EstateRequest::Access { reply }))
            .await?
            .map_err(ClientError::Refused)
    }

    pub async fn estate_covenant(&self) -> Result<estate::Covenant, ClientError> {
        self.ask(|reply| Request::Estate(network::EstateRequest::Covenant { reply }))
            .await
    }

    /// The covenant's text, downloaded from the estate.
    pub async fn estate_covenant_text(&self) -> Result<String, ClientError> {
        let data = self
            .ask(|reply| Request::Estate(network::EstateRequest::CovenantText { reply }))
            .await?
            .map_err(ClientError::Refused)?;
        crate::transfer::notecard_text(&data, "[ATTACHMENT]")
            .ok_or_else(|| ClientError::Refused("not a notecard".into()))
    }

    /// A POST to an estate capability (DispatchRegionInfo, EstateChangeInfo); refused when the
    /// region has none.
    pub async fn estate_post(
        &self,
        cap: &'static str,
        body: ferro_llsd::Llsd,
    ) -> Result<(), ClientError> {
        self.ask(|reply| Request::Estate(network::EstateRequest::Post { cap, body, reply }))
            .await?
            .map_err(ClientError::Refused)
    }

    /// A parcel's access list (parcels::AL_ACCESS) or ban list (parcels::AL_BAN).
    pub async fn parcel_access_list(
        &self,
        local_id: i32,
        flags: u32,
    ) -> Result<Vec<parcels::AccessEntry>, ClientError> {
        self.ask(|reply| {
            Request::Parcel(network::ParcelRequest::AccessList {
                local_id,
                flags,
                reply,
            })
        })
        .await
    }

    /// Who owns objects on a parcel, and how many.
    pub async fn parcel_object_owners(
        &self,
        local_id: i32,
    ) -> Result<Vec<network::ObjectOwner>, ClientError> {
        self.ask(|reply| Request::Parcel(network::ParcelRequest::ObjectOwners { local_id, reply }))
            .await
    }

    /// Eject, freeze, change an access list, return objects, deed, abandon, buy, reclaim.
    pub async fn parcel_act(&self, action: network::ParcelAction) -> Result<(), ClientError> {
        self.ask(|reply| Request::Parcel(network::ParcelRequest::Act { action, reply }))
            .await
    }

    /// Change a parcel: `parcel` is what it should be, from a ParcelInfo the region sent.
    pub async fn update_parcel(&self, parcel: parcels::ParcelInfo) -> Result<(), ClientError> {
        let sent = Box::new(parcel.clone());
        self.ask(|reply| {
            Request::Parcel(network::ParcelRequest::Update {
                parcel: sent,
                reply,
            })
        })
        .await?
        .map_err(ClientError::Refused)?;
        // kept as changed at once: a next change starts from it, before the region's answer
        let mut st = self.shared.state_mut();
        if st
            .parcel
            .as_ref()
            .is_some_and(|p| p.local_id == parcel.local_id)
        {
            st.parcel = Some(parcel);
        }
        Ok(())
    }

    /// Ask where a friend is, when they let the bot see them on the map (FindAgent); the answer
    /// lands in the state's `friend_locations`.
    pub async fn map_friend(&self, friend: Uuid) -> Result<(), ClientError> {
        self.interact(Interaction::FindAgent { prey: friend }).await
    }

    /// An object the region told of, by its key (a prim of a linkset, an attachment…).
    pub async fn find_object(&self, key: Uuid) -> Result<Option<objects::Known>, ClientError> {
        self.ask(|reply| Request::Object(network::ObjectRequest::Find { key, reply }))
            .await
    }

    /// The region's root objects the bot knows of, and where they stand.
    pub async fn root_objects(&self) -> Result<Vec<objects::Root>, ClientError> {
        self.ask(|reply| Request::Object(network::ObjectRequest::Roots { reply }))
            .await
    }

    /// Learn the owners and names of these root objects (ObjectSelect, as Aurora's Area Search):
    /// done once the region told of them all, or they are gone; a timeout after `wait`, with
    /// what came by then kept. Objects already known are not asked about again.
    pub async fn learn_objects(&self, keys: Vec<Uuid>, wait: Duration) -> Result<(), ClientError> {
        self.ask_within(wait, |reply| {
            Request::Object(network::ObjectRequest::Learn { keys, reply })
        })
        .await
    }

    /// The sculpted prims the bot knows of in its region (SecondBot's ListSculptys).
    pub async fn sculpts(&self) -> Result<Vec<objects::Sculpt>, ClientError> {
        self.ask(|reply| Request::Object(network::ObjectRequest::Sculpts { reply }))
            .await
    }

    /// Click (touch) an object.
    pub async fn click_object(&self, local_id: u32) -> Result<(), ClientError> {
        self.ask(|reply| Request::Object(network::ObjectRequest::Click { local_id, reply }))
            .await
    }

    /// Rez an item of the bot's at a place in its region; answers the request's id.
    pub async fn rez(&self, item: inventory::InvItem, at: glam::Vec3) -> Result<Uuid, ClientError> {
        let item = Box::new(item);
        self.ask(|reply| Request::Object(network::ObjectRequest::Rez { item, at, reply }))
            .await
    }

    /// Take an object into a folder of the bot's.
    pub async fn derez(&self, local_id: u32, folder: Uuid) -> Result<(), ClientError> {
        self.ask(|reply| {
            Request::Object(network::ObjectRequest::DeRez {
                local_id,
                folder,
                reply,
            })
        })
        .await
    }

    /// Put an item of the bot's into an object (a script, running or not).
    pub async fn give_to_object(
        &self,
        local_id: u32,
        item: inventory::InvItem,
        running: bool,
    ) -> Result<(), ClientError> {
        let item = Box::new(item);
        self.ask(|reply| {
            Request::Object(network::ObjectRequest::ToTask {
                local_id,
                item,
                running,
                reply,
            })
        })
        .await
    }

    /// An avatar's profile.
    pub async fn profile(&self, id: Uuid) -> Result<profile::Profile, ClientError> {
        // the region does not answer for an avatar it does not know: a short wait
        self.ask_within(Duration::from_secs(5), |reply| {
            Request::Profile(network::ProfileRequest::Get { id, reply })
        })
        .await?
        .map_err(ClientError::Refused)
    }

    /// Change the bot's own profile (its about text or its picture).
    pub async fn update_profile(&self, change: profile::ProfileChange) -> Result<(), ClientError> {
        let me = self.state().agent_id;
        let current = Box::new(self.profile(me).await?);
        self.ask(|reply| {
            Request::Profile(network::ProfileRequest::Save {
                current,
                change,
                reply,
            })
        })
        .await?
        .map_err(ClientError::Refused)
    }

    /// Start and stop animations (AgentAnimation).
    pub async fn animate(&self, anims: Vec<(Uuid, bool)>) -> Result<(), ClientError> {
        self.interact(Interaction::Animate { anims }).await
    }

    /// Play a sound where the bot stands (SoundTrigger).
    pub async fn trigger_sound(&self, asset: Uuid) -> Result<(), ClientError> {
        self.interact(Interaction::Sound { asset }).await
    }

    /// Point at an avatar: the point-at effect and a beam (ViewerEffect).
    pub async fn point_at(&self, target: Uuid) -> Result<(), ClientError> {
        self.interact(Interaction::PointAt { target }).await
    }

    /// Grant a script the permissions it asked for (ScriptAnswerYes; 0 grants none).
    pub async fn answer_script(
        &self,
        task: Uuid,
        item: Uuid,
        questions: i32,
    ) -> Result<(), ClientError> {
        self.interact(Interaction::ScriptAnswer {
            task,
            item,
            questions,
        })
        .await
    }

    /// An asset from the ViewerAsset capability: `kind` is its query key (gesture_id,
    /// animatn_id…).
    pub async fn viewer_asset(
        &self,
        kind: &'static str,
        asset: Uuid,
    ) -> Result<Vec<u8>, ClientError> {
        self.ask_within(Duration::from_secs(45), |reply| Request::ViewerAsset {
            kind,
            asset,
            reply,
        })
        .await?
        .map_err(ClientError::Refused)
    }

    /// Ask the grid for the mute list again (MuteListRequest); it lands in the state's
    /// `mute_list`.
    pub async fn request_mute_list(&self) -> Result<(), ClientError> {
        self.interact(Interaction::RequestMuteList).await
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
