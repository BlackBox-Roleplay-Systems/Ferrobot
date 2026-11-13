//! The session's side of profiles: the AgentProfile capability (GET, PUT), else
//! AvatarPropertiesRequest / AvatarPropertiesUpdate, as Aurora's session does for OpenSim; the
//! picks from the same capability, else the GenericMessage "avatarpicksrequest" answered by
//! AvatarPicksReply (LLAvatarPropertiesProcessor::sendAvatarPicksRequest, as Aurora's session).

use super::Session;
use crate::profile::{self, Profile, ProfileChange};
use ferro_msg::msgs::*;
use ferro_msg::{field_str, str_field};

/// A profile's picks: (key, name).
pub(crate) type Picks = Vec<(Uuid, String)>;
use tokio::sync::oneshot;
use uuid::Uuid;

/// AVATAR_ALLOW_PUBLISH (llavatarpropertiesprocessor.h)
const ALLOW_PUBLISH: u32 = 1;

#[derive(Debug)]
pub(crate) enum ProfileRequest {
    Get {
        id: Uuid,
        reply: oneshot::Sender<Result<Profile, String>>,
    },
    /// An avatar's picks.
    Picks {
        id: Uuid,
        reply: oneshot::Sender<Result<Picks, String>>,
    },
    /// The bot's own profile changed: `current` is what it is now (for the whole update).
    Save {
        current: Box<Profile>,
        change: ProfileChange,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

#[derive(Debug, Default)]
pub(super) struct ProfileWaits {
    get: Vec<(Uuid, oneshot::Sender<Result<Profile, String>>)>,
    picks: Vec<(Uuid, oneshot::Sender<Result<Picks, String>>)>,
}

impl ProfileWaits {
    pub fn retain_open(&mut self) {
        self.get.retain(|(_, r)| !r.is_closed());
        self.picks.retain(|(_, r)| !r.is_closed());
    }
}

impl Session<'_> {
    pub(super) fn on_profile_request(&mut self, r: ProfileRequest) {
        let cap = self.caps.get("AgentProfile").cloned();
        match r {
            ProfileRequest::Get { id, reply } => {
                let Some(cap) = cap else {
                    let mut m = AvatarPropertiesRequest::default();
                    m.agent_data.agent_id = self.login.agent_id;
                    m.agent_data.session_id = self.login.session_id;
                    m.agent_data.avatar_id = id;
                    self.send(&m, true);
                    self.profile_waits.get.push((id, reply));
                    return;
                };
                let http = self.http.clone();
                tokio::spawn(async move {
                    let url = format!("{}/{id}", cap.trim_end_matches('/'));
                    let answer = match crate::caps::get_llsd(&http, &url).await {
                        Ok(v) => profile::parse_agent_profile(id, &v)
                            .ok_or_else(|| "a profile of another avatar".to_owned()),
                        Err(e) => Err(format!("AgentProfile: {e}")),
                    };
                    let _ = reply.send(answer);
                });
            }
            ProfileRequest::Picks { id, reply } => {
                let Some(cap) = cap else {
                    // what AgentProfile would have carried (sendAvatarPicksRequest)
                    let mut m = GenericMessage::default();
                    m.agent_data.agent_id = self.login.agent_id;
                    m.agent_data.session_id = self.login.session_id;
                    m.method_data.method = str_field("avatarpicksrequest");
                    m.param_list = vec![generic_message::ParamList {
                        parameter: str_field(&id.to_string()),
                    }];
                    self.send(&m, true);
                    self.profile_waits.picks.push((id, reply));
                    return;
                };
                let http = self.http.clone();
                tokio::spawn(async move {
                    let url = format!("{}/{id}", cap.trim_end_matches('/'));
                    let answer = match crate::caps::get_llsd(&http, &url).await {
                        Ok(v) => profile::parse_agent_profile_picks(id, &v)
                            .ok_or_else(|| "a profile of another avatar".to_owned()),
                        Err(e) => Err(format!("AgentProfile: {e}")),
                    };
                    let _ = reply.send(answer);
                });
            }
            ProfileRequest::Save {
                current,
                change,
                reply,
            } => {
                let Some(cap) = cap else {
                    // sendAvatarPropertiesUpdate: the whole profile, never mature
                    let p = change.apply(&current);
                    let mut m = AvatarPropertiesUpdate::default();
                    m.agent_data.agent_id = self.login.agent_id;
                    m.agent_data.session_id = self.login.session_id;
                    let d = &mut m.properties_data;
                    d.image_id = p.image;
                    d.fl_image_id = p.first_life_image;
                    d.about_text = str_field(&p.about);
                    d.fl_about_text = str_field(&p.first_life_about);
                    d.allow_publish = p.allow_publish;
                    d.mature_publish = false;
                    d.profile_url = str_field(&p.profile_url);
                    self.send(&m, true);
                    let _ = reply.send(Ok(()));
                    return;
                };
                let http = self.http.clone();
                let me = self.login.agent_id;
                tokio::spawn(async move {
                    let url = format!("{}/{me}", cap.trim_end_matches('/'));
                    let sent = http
                        .put(&url)
                        .header("Content-Type", "application/llsd+xml")
                        .header("Accept", "application/llsd+xml")
                        .body(ferro_llsd::to_xml(&change.body()))
                        .send()
                        .await;
                    let answer = match sent {
                        Ok(r) if r.status().is_success() => Ok(()),
                        Ok(r) => Err(format!("AgentProfile: status {}", r.status().as_u16())),
                        Err(e) => Err(format!("AgentProfile: {}", e.without_url())),
                    };
                    let _ = reply.send(answer);
                });
            }
        }
    }

    pub(super) fn on_avatar_properties(&mut self, m: &AvatarPropertiesReply) {
        let d = &m.properties_data;
        let p = Profile {
            id: m.agent_data.avatar_id,
            about: field_str(&d.about_text),
            image: d.image_id,
            first_life_about: field_str(&d.fl_about_text),
            first_life_image: d.fl_image_id,
            allow_publish: d.flags & ALLOW_PUBLISH != 0,
            profile_url: field_str(&d.profile_url),
        };
        let (now, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.profile_waits.get)
            .into_iter()
            .partition(|(id, _)| *id == p.id);
        self.profile_waits.get = later;
        for (_, w) in now {
            let _ = w.send(Ok(p.clone()));
        }
    }

    /// AvatarPicksReply (processAvatarPicksReply): the picks of the avatar it names.
    pub(super) fn on_avatar_picks(&mut self, m: &AvatarPicksReply) {
        let target = m.agent_data.target_id;
        let picks: Picks = m
            .data
            .iter()
            .map(|p| (p.pick_id, field_str(&p.pick_name)))
            .collect();
        let (now, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.profile_waits.picks)
            .into_iter()
            .partition(|(id, _)| *id == target);
        self.profile_waits.picks = later;
        for (_, w) in now {
            let _ = w.send(Ok(picks.clone()));
        }
    }
}
