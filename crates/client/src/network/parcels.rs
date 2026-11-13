//! The session's side of parcels: the bot's own parcel (ParcelProperties with sequence 0, which
//! the region sends on arrival and when the bot walks into another one, and which the bot asks
//! for with ParcelPropertiesRequest when it has none), its dwell (ParcelDwellRequest), its
//! global key (the RemoteParcelRequest capability) and its changes (the ParcelPropertiesUpdate
//! capability). As LLViewerParcelMgr does, and Aurora's session_land.rs.

use super::Session;
use crate::caps;
use crate::parcels::{self, AccessEntry, ParcelInfo};
use ferro_llsd::Llsd;
use ferro_msg::msgs::*;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use uuid::Uuid;

/// An access list's replies stop coming: the list is complete.
const ACCESS_LIST_QUIET: Duration = Duration::from_secs(2);

/// What the bot asks of the region about a parcel, without an answer to wait for.
#[derive(Debug, Clone, PartialEq)]
pub enum ParcelAction {
    /// EjectUser: off the parcel (and banned, `ban`).
    Eject {
        target: Uuid,
        ban: bool,
    },
    /// FreezeUser.
    Freeze {
        target: Uuid,
        freeze: bool,
    },
    /// ParcelAccessListUpdate: the whole list (AL_ACCESS or AL_BAN) as it should be.
    AccessList {
        local_id: i32,
        flags: u32,
        entries: Vec<AccessEntry>,
    },
    /// ParcelReturnObjects: the objects of these owners (RT_LIST).
    ReturnObjects {
        local_id: i32,
        owners: Vec<Uuid>,
    },
    DeedToGroup {
        local_id: i32,
        group: Uuid,
    },
    /// ParcelRelease: abandon it.
    Release {
        local_id: i32,
    },
    Buy {
        local_id: i32,
        price: i32,
        area: i32,
    },
    /// ParcelReclaim: an estate manager takes it back.
    Reclaim {
        local_id: i32,
    },
}

/// One row of ParcelObjectOwnersReply.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectOwner {
    pub id: Uuid,
    pub is_group: bool,
    pub count: i32,
}

/// The sequence id of the agent's own parcel (LLViewerParcelMgr::processParcelProperties).
const AGENT_PARCEL: i32 = 0;
/// PARCEL_GRID_STEP_METERS: a parcel is made of 4 m cells.
const CELL: f32 = 4.0;

#[derive(Debug)]
pub(crate) enum ParcelRequest {
    /// The parcel the bot stands on, asked again.
    Properties { reply: oneshot::Sender<ParcelInfo> },
    /// A parcel's dwell (traffic).
    Dwell {
        local_id: i32,
        reply: oneshot::Sender<f32>,
    },
    /// The global key of the parcel the bot stands on.
    GlobalId {
        reply: oneshot::Sender<Result<Uuid, String>>,
    },
    /// The parcel changed as given (its local id says which).
    Update {
        parcel: Box<ParcelInfo>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// An access list (AL_ACCESS or AL_BAN).
    AccessList {
        local_id: i32,
        flags: u32,
        reply: oneshot::Sender<Vec<AccessEntry>>,
    },
    /// Who owns objects on the parcel.
    ObjectOwners {
        local_id: i32,
        reply: oneshot::Sender<Vec<ObjectOwner>>,
    },
    Act {
        action: ParcelAction,
        reply: oneshot::Sender<()>,
    },
}

#[derive(Debug)]
struct AccessWait {
    local_id: i32,
    flags: u32,
    entries: Vec<AccessEntry>,
    last: Option<Instant>,
    asked: Instant,
    reply: oneshot::Sender<Vec<AccessEntry>>,
}

#[derive(Debug, Default)]
pub(super) struct ParcelWaits {
    properties: Vec<oneshot::Sender<ParcelInfo>>,
    dwell: Vec<(i32, oneshot::Sender<f32>)>,
    access: Vec<AccessWait>,
    owners: Vec<(i32, oneshot::Sender<Vec<ObjectOwner>>)>,
}

impl ParcelWaits {
    pub fn retain_open(&mut self) {
        self.properties.retain(|r| !r.is_closed());
        self.dwell.retain(|(_, r)| !r.is_closed());
        self.owners.retain(|(_, r)| !r.is_closed());
        // an access list is complete once its replies stop; an empty one may not come at all
        let (done, waiting): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.access).into_iter().partition(|w| {
                w.reply.is_closed()
                    || w.last
                        .map_or(w.asked.elapsed() >= ACCESS_LIST_QUIET * 2, |t| {
                            t.elapsed() >= ACCESS_LIST_QUIET
                        })
            });
        self.access = waiting;
        for w in done {
            let _ = w.reply.send(w.entries);
        }
    }
}

impl Session<'_> {
    pub(super) fn on_parcel_request(&mut self, r: ParcelRequest) {
        let (agent, session) = (self.login.agent_id, self.login.session_id);
        match r {
            ParcelRequest::Properties { reply } => {
                // the 4 m cell the bot stands in, as a point selection does
                let p = self.position;
                let (west, south) = ((p.x / CELL).floor() * CELL, (p.y / CELL).floor() * CELL);
                let mut m = ParcelPropertiesRequest::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.parcel_data.sequence_id = AGENT_PARCEL;
                m.parcel_data.west = west;
                m.parcel_data.south = south;
                m.parcel_data.east = west + CELL;
                m.parcel_data.north = south + CELL;
                m.parcel_data.snap_selection = false;
                self.send(&m, true);
                self.parcel_waits.properties.push(reply);
            }
            ParcelRequest::Dwell { local_id, reply } => {
                let mut m = ParcelDwellRequest::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.data.local_id = local_id;
                self.send(&m, true);
                self.parcel_waits.dwell.push((local_id, reply));
            }
            ParcelRequest::GlobalId { reply } => {
                // LLRemoteParcelInfoProcessor::regionParcelInfoCoro: the point, the region's id
                // and its handle (8 bytes, big-endian)
                let Some(url) = self.caps.get("RemoteParcelRequest").cloned() else {
                    let _ = reply.send(Err("no RemoteParcelRequest capability".into()));
                    return;
                };
                let (region_id, handle) = {
                    let st = self.shared.state();
                    let r = st.region.as_ref();
                    (r.map(|r| r.id), r.map_or(0, |r| r.handle))
                };
                let p = self.position;
                let mut body = ferro_llsd::Map::new();
                body.insert(
                    "location".into(),
                    Llsd::Array(vec![
                        Llsd::Real(f64::from(p.x)),
                        Llsd::Real(f64::from(p.y)),
                        Llsd::Real(f64::from(p.z)),
                    ]),
                );
                if let Some(id) = region_id.filter(|id| !id.is_nil()) {
                    body.insert("region_id".into(), id.into());
                }
                body.insert(
                    "region_handle".into(),
                    Llsd::Binary(handle.to_be_bytes().to_vec()),
                );
                let http = self.http.clone();
                tokio::spawn(async move {
                    let answer = match caps::post_llsd(&http, &url, &Llsd::Map(body)).await {
                        Ok(v) => Some(v["parcel_id"].as_uuid())
                            .filter(|id| !id.is_nil())
                            .ok_or_else(|| "no parcel there".to_owned()),
                        Err(e) => Err(format!("RemoteParcelRequest: {e}")),
                    };
                    let _ = reply.send(answer);
                });
            }
            ParcelRequest::AccessList {
                local_id,
                flags,
                reply,
            } => {
                let mut m = ParcelAccessListRequest::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.data.sequence_id = 0;
                m.data.flags = flags;
                m.data.local_id = local_id;
                self.send(&m, true);
                self.parcel_waits.access.push(AccessWait {
                    local_id,
                    flags,
                    entries: Vec::new(),
                    last: None,
                    asked: Instant::now(),
                    reply,
                });
            }
            ParcelRequest::ObjectOwners { local_id, reply } => {
                let mut m = ParcelObjectOwnersRequest::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.parcel_data.local_id = local_id;
                self.send(&m, true);
                self.parcel_waits.owners.push((local_id, reply));
            }
            ParcelRequest::Act { action, reply } => {
                self.parcel_act(action);
                let _ = reply.send(());
            }
            ParcelRequest::Update { parcel, reply } => {
                let Some(url) = self.caps.get("ParcelPropertiesUpdate").cloned() else {
                    let _ = reply.send(Err("no ParcelPropertiesUpdate capability".into()));
                    return;
                };
                let body = parcels::update_body(&parcel);
                let http = self.http.clone();
                tokio::spawn(async move {
                    let answer = caps::post_llsd(&http, &url, &body)
                        .await
                        .map(|_| ())
                        .map_err(|e| format!("ParcelPropertiesUpdate: {e}"));
                    let _ = reply.send(answer);
                });
            }
        }
    }

    /// ParcelProperties on the event queue: the bot's own parcel when its sequence is 0.
    pub(super) fn on_parcel_properties(&mut self, body: &Llsd) {
        let p = parcels::parse_parcel_properties(body);
        if p.sequence_id != AGENT_PARCEL {
            return;
        }
        for w in self.parcel_waits.properties.drain(..) {
            let _ = w.send(p.clone());
        }
        self.shared.state_mut().parcel = Some(p);
    }

    pub(super) fn on_dwell_reply(&mut self, m: &ParcelDwellReply) {
        let local_id = m.data.local_id;
        let (now, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.parcel_waits.dwell)
            .into_iter()
            .partition(|(id, _)| *id == local_id);
        self.parcel_waits.dwell = later;
        for (_, w) in now {
            let _ = w.send(m.data.dwell);
        }
    }

    /// ParcelAccessListReply: a part of a list asked for.
    pub(super) fn on_access_list_reply(&mut self, m: &ParcelAccessListReply) {
        let Some(w) = self
            .parcel_waits
            .access
            .iter_mut()
            .find(|w| w.local_id == m.data.local_id && w.flags & m.data.flags != 0)
        else {
            return;
        };
        w.entries.extend(
            m.list
                .iter()
                .filter(|e| !e.id.is_nil())
                .map(|e| AccessEntry {
                    id: e.id,
                    time: e.time,
                    flags: e.flags,
                }),
        );
        w.last = Some(Instant::now());
        // a part not full is the last one
        if m.list.len() < parcels::PARCEL_MAX_ENTRIES_PER_PACKET {
            let i = self
                .parcel_waits
                .access
                .iter()
                .position(|w| w.local_id == m.data.local_id && w.flags & m.data.flags != 0);
            if let Some(i) = i {
                let w = self.parcel_waits.access.remove(i);
                let _ = w.reply.send(w.entries);
            }
        }
    }

    /// ParcelObjectOwnersReply (UDP, or the event queue on Second Life).
    pub(super) fn on_object_owners(&mut self, owners: Vec<ObjectOwner>) {
        // the reply does not say which parcel: the oldest question
        if !self.parcel_waits.owners.is_empty() {
            let (_, w) = self.parcel_waits.owners.remove(0);
            let _ = w.send(owners);
        }
    }

    fn parcel_act(&mut self, action: ParcelAction) {
        let (agent, session) = (self.login.agent_id, self.login.session_id);
        match action {
            ParcelAction::Eject { target, ban } => {
                // LLPanelLandAccess / LLFloaterEject: flags 1 also bans
                let mut m = EjectUser::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.data.target_id = target;
                m.data.flags = u32::from(ban);
                self.send(&m, true);
            }
            ParcelAction::Freeze { target, freeze } => {
                // flags 0 freezes, 1 frees (LLAvatarActions::freeze)
                let mut m = FreezeUser::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.data.target_id = target;
                m.data.flags = u32::from(!freeze);
                self.send(&m, true);
            }
            ParcelAction::AccessList {
                local_id,
                flags,
                entries,
            } => {
                // LLViewerParcelMgr::sendParcelAccessListUpdate
                let (sections, chunks) = parcels::access_list_chunks(&entries);
                let transaction = Uuid::new_v4();
                for (i, chunk) in chunks.iter().enumerate() {
                    let mut m = ParcelAccessListUpdate::default();
                    m.agent_data.agent_id = agent;
                    m.agent_data.session_id = session;
                    m.data.flags = flags;
                    m.data.local_id = local_id;
                    m.data.transaction_id = transaction;
                    m.data.sequence_id = i as i32 + 1;
                    m.data.sections = sections;
                    m.list = chunk
                        .iter()
                        .map(|e| parcel_access_list_update::List {
                            id: e.id,
                            time: e.time,
                            flags: e.flags,
                        })
                        .collect();
                    self.send(&m, true);
                }
            }
            ParcelAction::ReturnObjects { local_id, owners } => {
                // LLPanelLandObjects::callbackReturnOwnerList: a null task, then the owners
                let mut m = ParcelReturnObjects::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.parcel_data.local_id = local_id;
                m.parcel_data.return_type = parcels::RT_LIST;
                m.task_i_ds = vec![parcel_return_objects::TaskIDs {
                    task_id: Uuid::nil(),
                }];
                m.owner_i_ds = owners
                    .into_iter()
                    .map(|owner_id| parcel_return_objects::OwnerIDs { owner_id })
                    .collect();
                self.send(&m, true);
            }
            ParcelAction::DeedToGroup { local_id, group } => {
                let mut m = ParcelDeedToGroup::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.data.group_id = group;
                m.data.local_id = local_id;
                self.send(&m, true);
            }
            ParcelAction::Release { local_id } => {
                let mut m = ParcelRelease::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.data.local_id = local_id;
                self.send(&m, true);
            }
            ParcelAction::Buy {
                local_id,
                price,
                area,
            } => {
                // LLViewerParcelMgr::sendParcelBuy, for the bot itself
                let mut m = ParcelBuy::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.data.local_id = local_id;
                m.data.is_group_owned = false;
                m.data.remove_contribution = false;
                m.data.final_ = true;
                m.parcel_data.price = price;
                m.parcel_data.area = area;
                self.send(&m, true);
            }
            ParcelAction::Reclaim { local_id } => {
                let mut m = ParcelReclaim::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.data.local_id = local_id;
                self.send(&m, true);
            }
        }
    }
}
