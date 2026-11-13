//! The session's side of the estate: EstateOwnerMessage (its `getinfo`, answered by
//! `estateupdateinfo` with the same invoice, and the changes), the EstateAccess capability, the
//! covenant (EstateCovenantRequest, then its notecard by TransferRequest from the estate), and the
//! estate's capabilities, and a terrain RAW file's upload by xfer (`terrain` "upload filename",
//! then the region's RequestXfer). As Aurora's session/estate.rs, session_land.rs and
//! session/terrain_raw.rs.

use super::Session;
use crate::caps;
use crate::estate::{self, Covenant, EstateAccess, EstateInfo};
use ferro_llsd::Llsd;
use ferro_msg::msgs::*;
use ferro_msg::{field_str, str_field};
use std::time::Instant;
use tokio::sync::oneshot;
use uuid::Uuid;

#[derive(Debug)]
pub(crate) enum EstateRequest {
    /// An EstateOwnerMessage, with the last invoice.
    Owner {
        method: String,
        strings: Vec<String>,
        reply: oneshot::Sender<()>,
    },
    Info {
        reply: oneshot::Sender<EstateInfo>,
    },
    Access {
        reply: oneshot::Sender<Result<EstateAccess, String>>,
    },
    Covenant {
        reply: oneshot::Sender<Covenant>,
    },
    /// The covenant's notecard, downloaded from the estate.
    CovenantText {
        reply: oneshot::Sender<Result<Vec<u8>, String>>,
    },
    /// A POST to an estate capability; Err when the region has none (the caller falls back).
    Post {
        cap: &'static str,
        body: Llsd,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// A terrain RAW file to upload: answered once asked for, with how it will end.
    TerrainUpload {
        name: String,
        data: Vec<u8>,
        reply: oneshot::Sender<Result<TerrainDone, String>>,
    },
}

/// How a terrain upload ended, once it has.
pub(crate) type TerrainDone = oneshot::Receiver<Result<(), String>>;

#[derive(Debug, Default)]
pub(super) struct EstateWaits {
    info: Vec<oneshot::Sender<EstateInfo>>,
    covenant: Vec<oneshot::Sender<Covenant>>,
}

impl EstateWaits {
    pub fn retain_open(&mut self) {
        self.info.retain(|r| !r.is_closed());
        self.covenant.retain(|r| !r.is_closed());
    }
}

/// LLTST_SIM_ESTATE and its parameters (LLTransferSourceParamsEstate: agent, session, ET_Covenant
/// as an S32, little-endian), as Aurora's covenant_transfer_params.
fn covenant_transfer(agent: Uuid, session: Uuid) -> TransferRequest {
    let mut m = TransferRequest::default();
    let t = &mut m.transfer_info;
    t.transfer_id = Uuid::new_v4();
    t.channel_type = crate::transfer::ASSET_CHANNEL;
    t.source_type = 4;
    t.priority = 100.0;
    t.params.extend_from_slice(agent.as_bytes());
    t.params.extend_from_slice(session.as_bytes());
    t.params.extend_from_slice(&0i32.to_le_bytes());
    m
}

impl Session<'_> {
    /// sendEstateOwnerMessage: no strings is a single empty parameter.
    fn estate_owner_message(&mut self, method: &str, strings: &[String]) {
        let mut m = EstateOwnerMessage::default();
        m.agent_data.agent_id = self.login.agent_id;
        m.agent_data.session_id = self.login.session_id;
        m.method_data.method = str_field(method);
        m.method_data.invoice = self.estate_invoice;
        m.param_list = if strings.is_empty() {
            vec![estate_owner_message::ParamList {
                parameter: Vec::new(),
            }]
        } else {
            strings
                .iter()
                .map(|s| estate_owner_message::ParamList {
                    parameter: str_field(s),
                })
                .collect()
        };
        self.send(&m, true);
    }

    pub(super) fn on_estate_request(&mut self, r: EstateRequest) {
        match r {
            EstateRequest::Owner {
                method,
                strings,
                reply,
            } => {
                self.estate_owner_message(&method, &strings);
                let _ = reply.send(());
            }
            EstateRequest::Info { reply } => {
                // a new invoice: its answer is told apart from older ones
                self.estate_invoice = Uuid::new_v4();
                self.estate_owner_message("getinfo", &[]);
                self.estate_waits.info.push(reply);
            }
            EstateRequest::Access { reply } => {
                let Some(url) = self.caps.get("EstateAccess").cloned() else {
                    let _ = reply.send(Err("no EstateAccess capability".into()));
                    return;
                };
                let http = self.http.clone();
                tokio::spawn(async move {
                    let answer = caps::get_llsd(&http, &url)
                        .await
                        .map(|v| estate::parse_estate_access(&v))
                        .map_err(|e| format!("EstateAccess: {e}"));
                    let _ = reply.send(answer);
                });
            }
            EstateRequest::Covenant { reply } => {
                let mut m = EstateCovenantRequest::default();
                m.agent_data.agent_id = self.login.agent_id;
                m.agent_data.session_id = self.login.session_id;
                self.send(&m, true);
                self.estate_waits.covenant.push(reply);
            }
            EstateRequest::CovenantText { reply } => {
                let m = covenant_transfer(self.login.agent_id, self.login.session_id);
                if self.transfers.start(m.transfer_info.transfer_id, reply) {
                    self.send(&m, true);
                }
            }
            EstateRequest::Post { cap, body, reply } => {
                let Some(url) = self.caps.get(cap).cloned() else {
                    let _ = reply.send(Err(format!("no {cap} capability")));
                    return;
                };
                let http = self.http.clone();
                tokio::spawn(async move {
                    let answer = caps::post_llsd(&http, &url, &body)
                        .await
                        .map(|_| ())
                        .map_err(|e| format!("{cap}: {e}"));
                    let _ = reply.send(answer);
                });
            }
            EstateRequest::TerrainUpload { name, data, reply } => {
                // one at a time, as the region takes them (OpenSim: "Another Terrain Upload is in
                // progress")
                if self.terrain_upload.is_some() {
                    let _ = reply.send(Err("a terrain upload is already under way".into()));
                    return;
                }
                let (tx, rx) = oneshot::channel();
                log::info!("terrain: asking to upload {name} ({} bytes)", data.len());
                // onUploadRawFilepickerCB: the file's name, then the region asks for it by xfer
                let strings = ["upload filename".to_owned(), name.clone()];
                self.terrain_upload = Some(crate::terrain::Upload::new(name, data, tx));
                self.estate_owner_message("terrain", &strings);
                let _ = reply.send(Ok(rx));
            }
        }
    }

    /// What the terrain upload does next: a packet, or its end.
    fn terrain_step(&mut self, step: crate::terrain::Step) {
        use crate::terrain::Step;
        let Some(u) = self.terrain_upload.as_mut() else {
            return;
        };
        match step {
            Step::Ignore => {}
            Step::Send(packet, data) => {
                let Some(id) = u.xfer_id() else {
                    return;
                };
                let mut m = SendXferPacket::default();
                m.xfer_id.id = id;
                m.xfer_id.packet = packet;
                m.data_packet.data = data;
                // unreliable: the xfer confirms and resends its own packets (LLXfer)
                self.send(&m, false);
            }
            Step::Done => {
                log::info!("terrain: {} uploaded", u.name());
                u.finish(Ok(()));
                self.terrain_upload = None;
            }
            Step::Failed(why) => self.terrain_failed(why),
        }
    }

    pub(super) fn terrain_failed(&mut self, why: String) {
        if let Some(mut u) = self.terrain_upload.take() {
            log::warn!("terrain: the upload of {} failed: {why}", u.name());
            u.finish(Err(why));
        }
    }

    /// RequestXfer: the region asks for the terrain file it was told of.
    pub(super) fn on_request_xfer(&mut self, m: &RequestXfer) {
        let Some(u) = self.terrain_upload.as_mut() else {
            return;
        };
        let x = &m.xfer_id;
        let step = u.requested(
            x.id,
            &field_str(&x.filename),
            x.file_path,
            x.v_file_id.is_nil(),
            x.use_big_packets,
            Instant::now(),
        );
        if matches!(step, crate::terrain::Step::Send(..)) {
            log::info!(
                "terrain: the region asked for {}, sending {} bytes",
                u.name(),
                u.len()
            );
        } else {
            log::debug!("RequestXfer for a file the bot does not send");
        }
        self.terrain_step(step);
    }

    pub(super) fn on_confirm_xfer(&mut self, m: &ConfirmXferPacket) {
        let Some(u) = self.terrain_upload.as_mut() else {
            return;
        };
        let step = u.confirmed(m.xfer_id.id, m.xfer_id.packet, Instant::now());
        self.terrain_step(step);
    }

    /// AbortXfer: true when it ended the terrain upload.
    pub(super) fn on_abort_terrain_xfer(&mut self, m: &AbortXfer) -> bool {
        let ours = self
            .terrain_upload
            .as_ref()
            .and_then(|u| u.xfer_id())
            .is_some_and(|id| id == m.xfer_id.id);
        if ours {
            self.terrain_failed(format!("the region aborted it ({})", m.xfer_id.result));
        }
        ours
    }

    pub(super) fn tick_terrain_upload(&mut self, now: Instant) {
        if let Some(u) = self.terrain_upload.as_mut() {
            let step = u.tick(now);
            self.terrain_step(step);
        }
    }

    /// processEstateOwnerRequest: `estateupdateinfo` of the last invoice.
    pub(super) fn on_estate_owner_message(&mut self, m: &EstateOwnerMessage) {
        let method = estate::parameter_string(&m.method_data.method);
        if method != "estateupdateinfo" || m.method_data.invoice != self.estate_invoice {
            return;
        }
        let strings: Vec<String> = m
            .param_list
            .iter()
            .map(|p| estate::parameter_string(&p.parameter))
            .collect();
        match estate::parse_estate_update_info(&strings) {
            Some(info) => {
                for w in self.estate_waits.info.drain(..) {
                    let _ = w.send(info.clone());
                }
            }
            None => log::warn!("estate: estateupdateinfo with {} strings", strings.len()),
        }
    }

    pub(super) fn on_covenant_reply(&mut self, m: &EstateCovenantReply) {
        let d = &m.data;
        let c = Covenant {
            id: d.covenant_id,
            timestamp: d.covenant_timestamp,
            estate_name: ferro_msg::field_str(&d.estate_name),
            estate_owner: d.estate_owner_id,
        };
        for w in self.estate_waits.covenant.drain(..) {
            let _ = w.send(c.clone());
        }
    }
}
