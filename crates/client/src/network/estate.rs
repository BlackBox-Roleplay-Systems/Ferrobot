//! The session's side of the estate: EstateOwnerMessage (its `getinfo`, answered by
//! `estateupdateinfo` with the same invoice, and the changes), the EstateAccess capability, the
//! covenant (EstateCovenantRequest, then its notecard by TransferRequest from the estate), and the
//! estate's capabilities. As Aurora's session/estate.rs and session_land.rs.

use super::Session;
use crate::caps;
use crate::estate::{self, Covenant, EstateAccess, EstateInfo};
use ferro_llsd::Llsd;
use ferro_msg::msgs::*;
use ferro_msg::str_field;
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
}

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
