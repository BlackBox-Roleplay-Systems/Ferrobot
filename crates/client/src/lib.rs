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
pub mod groups;
pub mod login;
pub mod names;
mod network;
pub mod region_restart;
pub mod state;
mod xmlrpc;

pub use events::{ChatMessage, Event, InstantMessage};
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
    /// Groups: roles, members, invitations, ejections, role changes.
    Group(network::GroupRequest),
    /// A teleport; answered with the region arrived in, or the grid's reason.
    Teleport {
        to: TeleportTo,
        reply: oneshot::Sender<Result<String, String>>,
    },
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
}
