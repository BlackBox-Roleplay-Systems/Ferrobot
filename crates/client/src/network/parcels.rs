//! The session's side of parcels: the bot's own parcel (ParcelProperties with sequence 0, which
//! the region sends on arrival and when the bot walks into another one, and which the bot asks
//! for with ParcelPropertiesRequest when it has none), its dwell (ParcelDwellRequest), its
//! global key (the RemoteParcelRequest capability) and its changes (the ParcelPropertiesUpdate
//! capability). As LLViewerParcelMgr does, and Aurora's session_land.rs.

use super::Session;
use crate::caps;
use crate::parcels::{self, ParcelInfo};
use ferro_llsd::Llsd;
use ferro_msg::msgs::*;
use tokio::sync::oneshot;
use uuid::Uuid;

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
}

#[derive(Debug, Default)]
pub(super) struct ParcelWaits {
    properties: Vec<oneshot::Sender<ParcelInfo>>,
    dwell: Vec<(i32, oneshot::Sender<f32>)>,
}

impl ParcelWaits {
    pub fn retain_open(&mut self) {
        self.properties.retain(|r| !r.is_closed());
        self.dwell.retain(|(_, r)| !r.is_closed());
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
}
