//! The session's side of groups: the requests of LLGroupMgr (GroupMemberData and its
//! GroupMembersRequest fallback, GroupRoleDataRequest, GroupRoleMembersRequest, GroupTitlesRequest,
//! sendGroupMemberInvites, sendGroupMemberEjects, sendGroupRoleMemberChanges,
//! sendCreateGroupRequest) and their replies,
//! matched to who waits by the request id the replies repeat. As Aurora's session_groups.rs.

use super::Session;
use crate::events::{Event, dialog};
use crate::groups::{
    self, GroupMember, GroupRole, GroupTitle, RoleChange, eject_messages, invite_messages,
    role_change_messages,
};
use ferro_llsd::{Llsd, llsd_map};
use ferro_msg::msgs::*;
use ferro_msg::{IncomingPacket, Msg};
use std::collections::{HashMap, VecDeque};
use tokio::sync::oneshot;
use uuid::Uuid;

/// IM_GROUP_INVITATION_ACCEPT, IM_GROUP_INVITATION_DECLINE (llinstantmessage.h)
const GROUP_INVITATION_ACCEPT: u8 = 35;
const GROUP_INVITATION_DECLINE: u8 = 36;

/// What a group request asks of the network task.
#[derive(Debug)]
pub(crate) enum GroupRequest {
    Roles {
        group: Uuid,
        reply: oneshot::Sender<Vec<GroupRole>>,
    },
    /// The whole member list; an error when the capability fails.
    Members {
        group: Uuid,
        reply: oneshot::Sender<Result<Vec<GroupMember>, String>>,
    },
    /// Every (role, member) pair.
    RoleMembers {
        group: Uuid,
        reply: oneshot::Sender<Vec<(Uuid, Uuid)>>,
    },
    Titles {
        group: Uuid,
        reply: oneshot::Sender<Vec<GroupTitle>>,
    },
    /// (invitee, role) pairs; the grid does not answer invitations.
    Invite {
        group: Uuid,
        invites: Vec<(Uuid, Uuid)>,
        reply: oneshot::Sender<()>,
    },
    /// Answered by EjectGroupMemberReply's success.
    Eject {
        group: Uuid,
        members: Vec<Uuid>,
        reply: oneshot::Sender<bool>,
    },
    /// The grid does not answer role changes.
    RoleChanges {
        group: Uuid,
        changes: Vec<RoleChange>,
        reply: oneshot::Sender<()>,
    },
    /// Join a group's chat (IM_SESSION_GROUP_START), answered by ChatterBoxSessionStartReply.
    ChatJoin {
        group: Uuid,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// A line in a group's chat (IM_SESSION_SEND).
    ChatSend {
        group: Uuid,
        text: String,
        reply: oneshot::Sender<()>,
    },
    /// Leave a group's chat (IM_SESSION_LEAVE).
    ChatLeave {
        group: Uuid,
        reply: oneshot::Sender<()>,
    },
    /// A group notice (send_group_notice): an IM to the group, an item attached or not.
    Notice {
        group: Uuid,
        subject: String,
        message: String,
        item: Option<Uuid>,
        reply: oneshot::Sender<()>,
    },
    /// Wear the title of one of the bot's roles (GroupTitleUpdate).
    ActivateTitle {
        group: Uuid,
        role: Uuid,
        reply: oneshot::Sender<()>,
    },
    /// Make a group the active one (ActivateGroup; the nil id: none).
    Activate {
        group: Uuid,
        reply: oneshot::Sender<()>,
    },
    /// JoinGroupRequest, answered by JoinGroupReply's success.
    Join {
        group: Uuid,
        reply: oneshot::Sender<bool>,
    },
    /// LeaveGroupRequest, answered by LeaveGroupReply's success.
    Leave {
        group: Uuid,
        reply: oneshot::Sender<bool>,
    },
    /// The ban list (GroupAPIv1); with an action first, ban or unban these.
    Bans {
        group: Uuid,
        action: Option<i32>,
        ids: Vec<Uuid>,
        reply: oneshot::Sender<Result<Vec<Uuid>, String>>,
    },
    /// CreateGroupRequest, answered by CreateGroupReply.
    Create {
        group: Box<groups::NewGroup>,
        reply: oneshot::Sender<groups::GroupCreated>,
    },
    /// Ask again for the group list (AgentDataUpdateRequest).
    Reload { reply: oneshot::Sender<()> },
    /// Answer a group invitation (send_join_group_response): an IM to the group.
    AnswerInvite {
        group: Uuid,
        transaction: Uuid,
        accept: bool,
        reply: oneshot::Sender<()>,
    },
}

/// Replies being gathered: a list comes over several packets, complete once the count the
/// replies announce has arrived.
#[derive(Default)]
pub(super) struct GroupWaits {
    roles: HashMap<Uuid, Gathering<GroupRole>>,
    members: HashMap<Uuid, Gathering<GroupMember>>,
    role_members: HashMap<Uuid, Gathering<(Uuid, Uuid)>>,
    titles: HashMap<Uuid, oneshot::Sender<Vec<GroupTitle>>>,
    /// EjectGroupMemberReply names the group only: answered in order.
    ejects: HashMap<Uuid, VecDeque<oneshot::Sender<bool>>>,
    /// Joins and leaves asked for, by group.
    joins: HashMap<Uuid, Vec<oneshot::Sender<bool>>>,
    leaves: HashMap<Uuid, Vec<oneshot::Sender<bool>>>,
    /// Group chats being joined, by group.
    chat_joins: HashMap<Uuid, Vec<oneshot::Sender<Result<(), String>>>>,
    /// CreateGroupReply names no request: answered in order.
    creates: VecDeque<oneshot::Sender<groups::GroupCreated>>,
}

struct Gathering<T> {
    items: Vec<T>,
    /// Entries received, the null ones the parsers skip included.
    received: usize,
    reply: Reply<T>,
}

enum Reply<T> {
    Plain(oneshot::Sender<Vec<T>>),
    Result(oneshot::Sender<Result<Vec<T>, String>>),
}

impl<T> Reply<T> {
    fn send(self, items: Vec<T>) {
        // a caller that gave up is no error
        match self {
            Reply::Plain(tx) => {
                let _ = tx.send(items);
            }
            Reply::Result(tx) => {
                let _ = tx.send(Ok(items));
            }
        }
    }

    fn is_closed(&self) -> bool {
        match self {
            Reply::Plain(tx) => tx.is_closed(),
            Reply::Result(tx) => tx.is_closed(),
        }
    }
}

impl<T> Gathering<T> {
    fn new(reply: Reply<T>) -> Self {
        Gathering {
            items: Vec::new(),
            received: 0,
            reply,
        }
    }
}

/// Add a page; answer once `total` entries have arrived.
fn gather<T>(
    waits: &mut HashMap<Uuid, Gathering<T>>,
    request: Uuid,
    total: usize,
    received: usize,
    items: Vec<T>,
) {
    let Some(g) = waits.get_mut(&request) else {
        // a reply to a request no one waits for any more
        return;
    };
    g.items.extend(items);
    g.received += received;
    if g.received >= total
        && let Some(g) = waits.remove(&request)
    {
        g.reply.send(g.items);
    }
}

impl GroupWaits {
    /// Forget the requests whose caller gave up.
    pub(super) fn retain_open(&mut self) {
        self.roles.retain(|_, g| !g.reply.is_closed());
        self.members.retain(|_, g| !g.reply.is_closed());
        self.role_members.retain(|_, g| !g.reply.is_closed());
        self.titles.retain(|_, tx| !tx.is_closed());
        for q in self.ejects.values_mut() {
            q.retain(|tx| !tx.is_closed());
        }
        self.ejects.retain(|_, q| !q.is_empty());
        for m in [&mut self.joins, &mut self.leaves] {
            for v in m.values_mut() {
                v.retain(|tx| !tx.is_closed());
            }
            m.retain(|_, v| !v.is_empty());
        }
        for v in self.chat_joins.values_mut() {
            v.retain(|tx| !tx.is_closed());
        }
        self.chat_joins.retain(|_, v| !v.is_empty());
        self.creates.retain(|tx| !tx.is_closed());
    }
}

impl Session<'_> {
    pub(super) fn on_group_request(&mut self, r: GroupRequest) {
        let (agent, session) = (self.login.agent_id, self.login.session_id);
        match r {
            GroupRequest::Roles { group, reply } => {
                let request = Uuid::new_v4();
                let mut m = GroupRoleDataRequest::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.group_data.group_id = group;
                m.group_data.request_id = request;
                self.send(&m, true);
                self.groups
                    .roles
                    .insert(request, Gathering::new(Reply::Plain(reply)));
            }
            GroupRequest::Members { group, reply } => self.group_members(group, reply),
            GroupRequest::RoleMembers { group, reply } => {
                let request = Uuid::new_v4();
                let mut m = GroupRoleMembersRequest::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.group_data.group_id = group;
                m.group_data.request_id = request;
                self.send(&m, true);
                self.groups
                    .role_members
                    .insert(request, Gathering::new(Reply::Plain(reply)));
            }
            GroupRequest::Titles { group, reply } => {
                let request = Uuid::new_v4();
                let mut m = GroupTitlesRequest::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.agent_data.group_id = group;
                m.agent_data.request_id = request;
                self.send(&m, true);
                self.groups.titles.insert(request, reply);
            }
            GroupRequest::Invite {
                group,
                invites,
                reply,
            } => {
                for m in invite_messages(agent, session, group, &invites) {
                    self.send(&m, true);
                }
                let _ = reply.send(());
            }
            GroupRequest::Eject {
                group,
                members,
                reply,
            } => {
                let messages = eject_messages(agent, session, group, &members);
                if messages.is_empty() {
                    // only the bot itself: "Can't use 'eject' to leave a group"
                    let _ = reply.send(false);
                    return;
                }
                // one reply per message: the caller hears the first
                let count = messages.len();
                for m in messages {
                    self.send(&m, true);
                }
                let q = self.groups.ejects.entry(group).or_default();
                q.push_back(reply);
                for _ in 1..count {
                    let (tx, _) = oneshot::channel();
                    q.push_back(tx);
                }
            }
            GroupRequest::RoleChanges {
                group,
                changes,
                reply,
            } => {
                for m in role_change_messages(agent, session, group, &changes) {
                    self.send(&m, true);
                }
                let _ = reply.send(());
            }
            GroupRequest::ChatJoin { group, reply } => {
                if self.shared.state().group_chats.contains(&group) {
                    let _ = reply.send(Ok(()));
                    return;
                }
                let waiting = self.groups.chat_joins.entry(group).or_default();
                let first = waiting.is_empty();
                waiting.push(reply);
                if first {
                    // LLIMModel::sendStartSession: the session id is the group's
                    self.send_im(group, dialog::SESSION_GROUP_START, group, "", Vec::new());
                }
            }
            GroupRequest::ChatSend { group, text, reply } => {
                for part in split_message(&text) {
                    self.send_im(group, dialog::SESSION_SEND, group, part, Vec::new());
                }
                let _ = reply.send(());
            }
            GroupRequest::ChatLeave { group, reply } => {
                self.send_im(group, dialog::SESSION_LEAVE, group, "", Vec::new());
                self.shared.state_mut().group_chats.retain(|g| *g != group);
                let _ = reply.send(());
            }
            GroupRequest::Notice {
                group,
                subject,
                message,
                item,
                reply,
            } => {
                let (text, bucket) =
                    groups::notice_to_send(&subject, &message, item.map(|i| (i, agent)));
                self.send_im(group, dialog::GROUP_NOTICE, Uuid::nil(), &text, bucket);
                let _ = reply.send(());
            }
            GroupRequest::ActivateTitle { group, role, reply } => {
                let mut m = GroupTitleUpdate::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.agent_data.group_id = group;
                m.agent_data.title_role_id = role;
                self.send(&m, true);
                let _ = reply.send(());
            }
            GroupRequest::Activate { group, reply } => {
                let mut m = ActivateGroup::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.agent_data.group_id = group;
                self.send(&m, true);
                let _ = reply.send(());
            }
            GroupRequest::Join { group, reply } => {
                let mut m = JoinGroupRequest::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.group_data.group_id = group;
                self.send(&m, true);
                self.groups.joins.entry(group).or_default().push(reply);
            }
            GroupRequest::Leave { group, reply } => {
                let mut m = LeaveGroupRequest::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.group_data.group_id = group;
                self.send(&m, true);
                self.groups.leaves.entry(group).or_default().push(reply);
            }
            GroupRequest::Bans {
                group,
                action,
                ids,
                reply,
            } => self.group_bans(group, action, ids, reply),
            GroupRequest::Create { group, reply } => {
                log::info!("creating the group \"{}\"", group.name.trim());
                self.send(&groups::create_message(agent, session, &group), true);
                self.groups.creates.push_back(reply);
            }
            GroupRequest::Reload { reply } => {
                let mut r = AgentDataUpdateRequest::default();
                r.agent_data.agent_id = agent;
                r.agent_data.session_id = session;
                self.send(&r, true);
                let _ = reply.send(());
            }
            GroupRequest::AnswerInvite {
                group,
                transaction,
                accept,
                reply,
            } => {
                let dialog = if accept {
                    GROUP_INVITATION_ACCEPT
                } else {
                    GROUP_INVITATION_DECLINE
                };
                self.send_im(group, dialog, transaction, "", Vec::new());
                let _ = reply.send(());
            }
        }
    }

    /// LLGroupMgr::sendCapGroupMembersRequest: the whole list from the GroupMemberData
    /// capability, or GroupMembersRequest when the region has none.
    fn group_members(
        &mut self,
        group: Uuid,
        reply: oneshot::Sender<Result<Vec<GroupMember>, String>>,
    ) {
        let request = Uuid::new_v4();
        let Some(url) = self.caps.get("GroupMemberData").cloned() else {
            log::debug!("group members: no GroupMemberData capability, by UDP");
            let mut m = GroupMembersRequest::default();
            m.agent_data.agent_id = self.login.agent_id;
            m.agent_data.session_id = self.login.session_id;
            m.group_data.group_id = group;
            m.group_data.request_id = request;
            self.send(&m, true);
            self.groups
                .members
                .insert(request, Gathering::new(Reply::Result(reply)));
            return;
        };
        let http = self.http.clone();
        tokio::spawn(async move {
            let answer = match crate::caps::post_llsd(&http, &url, &groups::member_data_body(group))
                .await
            {
                Ok(v) => match groups::member_data_response(&v, request) {
                    Some(page) => {
                        log::debug!("group members: {} from GroupMemberData", page.members.len());
                        Ok(page.members)
                    }
                    None => Err("GroupMemberData: no member data received".to_owned()),
                },
                Err(e) => Err(format!("GroupMemberData: {e}")),
            };
            let _ = reply.send(answer);
        });
    }

    /// The ban list through GroupAPIv1 (Aurora's group_bans: the group in the query; a POST bans
    /// or unbans, then the list is fetched again).
    fn group_bans(
        &mut self,
        group: Uuid,
        action: Option<i32>,
        ids: Vec<Uuid>,
        reply: oneshot::Sender<Result<Vec<Uuid>, String>>,
    ) {
        let Some(cap) = self.caps.get("GroupAPIv1").cloned() else {
            let _ = reply.send(Err("no GroupAPIv1 capability".into()));
            return;
        };
        let url = format!("{cap}?group_id={group}");
        let http = self.http.clone();
        tokio::spawn(async move {
            if let Some(action) = action
                && let Err(e) =
                    crate::caps::post_llsd(&http, &url, &groups::ban_body(action, &ids)).await
            {
                let _ = reply.send(Err(format!("GroupAPIv1: {e}")));
                return;
            }
            let answer = match crate::caps::get_llsd(&http, &url).await {
                Ok(v) => groups::ban_list(&v)
                    .map(|(_, bans)| bans.into_iter().map(|(id, _)| id).collect())
                    .ok_or_else(|| "GroupAPIv1: no ban list".to_owned()),
                Err(e) => Err(format!("GroupAPIv1: {e}")),
            };
            let _ = reply.send(answer);
        });
    }

    /// Replies of this module; false when the packet is not one of them. As in LLGroupMgr, a
    /// reply addressed to another agent is dropped.
    pub(super) fn dispatch_groups(
        &mut self,
        pkt: &IncomingPacket,
    ) -> Result<bool, ferro_msg::DecodeError> {
        let id = pkt.id;
        let me = self.login.agent_id;
        if id == GroupMembersReply::ID {
            let m: GroupMembersReply = pkt.decode()?;
            if m.agent_data.agent_id == me {
                let received = m.member_data.len();
                let page = groups::members_reply(&m);
                let total = usize::try_from(page.total).unwrap_or(0);
                gather(
                    &mut self.groups.members,
                    page.request,
                    total,
                    received,
                    page.members,
                );
            }
        } else if id == GroupRoleDataReply::ID {
            let m: GroupRoleDataReply = pkt.decode()?;
            if m.agent_data.agent_id == me {
                let received = m.role_data.len();
                let page = groups::roles_reply(&m);
                let total = usize::try_from(page.total).unwrap_or(0);
                gather(
                    &mut self.groups.roles,
                    page.request,
                    total,
                    received,
                    page.roles,
                );
            }
        } else if id == GroupRoleMembersReply::ID {
            let m: GroupRoleMembersReply = pkt.decode()?;
            if m.agent_data.agent_id == me {
                let received = m.member_data.len();
                let page = groups::role_members_reply(&m);
                let pairs = page
                    .pairs
                    .into_iter()
                    .filter(|(role, member)| !(role.is_nil() && member.is_nil()))
                    .collect();
                gather(
                    &mut self.groups.role_members,
                    page.request,
                    page.total_pairs as usize,
                    received,
                    pairs,
                );
            }
        } else if id == GroupTitlesReply::ID {
            let m: GroupTitlesReply = pkt.decode()?;
            if m.agent_data.agent_id == me
                && let Some(tx) = self.groups.titles.remove(&m.agent_data.request_id)
            {
                let _ = tx.send(groups::titles_reply(&m));
            }
        } else if id == JoinGroupReply::ID {
            let m: JoinGroupReply = pkt.decode()?;
            if m.agent_data.agent_id == me {
                let g = m.group_data.group_id;
                for tx in self.groups.joins.remove(&g).unwrap_or_default() {
                    let _ = tx.send(m.group_data.success);
                }
            }
        } else if id == LeaveGroupReply::ID {
            let m: LeaveGroupReply = pkt.decode()?;
            if m.agent_data.agent_id == me {
                let g = m.group_data.group_id;
                for tx in self.groups.leaves.remove(&g).unwrap_or_default() {
                    let _ = tx.send(m.group_data.success);
                }
            }
        } else if id == CreateGroupReply::ID {
            let m: CreateGroupReply = pkt.decode()?;
            if m.agent_data.agent_id == me {
                let created = groups::created_reply(&m);
                log::info!(
                    "group creation: success {}, {:?}",
                    created.success,
                    created.message
                );
                if let Some(tx) = self.groups.creates.pop_front() {
                    let _ = tx.send(created);
                }
            }
        } else if id == EjectGroupMemberReply::ID {
            let m: EjectGroupMemberReply = pkt.decode()?;
            if m.agent_data.agent_id == me {
                let group = m.group_data.group_id;
                if let Some(q) = self.groups.ejects.get_mut(&group)
                    && let Some(tx) = q.pop_front()
                {
                    let _ = tx.send(m.eject_data.success);
                }
            }
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    fn joined_chat(&mut self, group: Uuid) {
        let mut st = self.shared.state_mut();
        if !st.group_chats.contains(&group) {
            st.group_chats.push(group);
        }
    }

    fn in_group(&self, id: Uuid) -> bool {
        self.shared.state().groups.iter().any(|g| g.id == id)
    }

    /// ChatterBoxSessionStartReply: the group chat the bot asked to join is open, or refused.
    pub(super) fn on_session_start_reply(&mut self, body: &Llsd) {
        let group = body["session_id"].as_uuid();
        let group = if group.is_nil() {
            body["temp_session_id"].as_uuid()
        } else {
            group
        };
        let answer = if body["success"].as_bool() {
            self.joined_chat(group);
            log::info!("in the chat of group {group}");
            Ok(())
        } else {
            let why = body["error"].as_str().to_owned();
            log::warn!("group chat {group} refused: {why}");
            Err(if why.is_empty() {
                "the grid refused the group chat".to_owned()
            } else {
                why
            })
        };
        for tx in self.groups.chat_joins.remove(&group).unwrap_or_default() {
            let _ = tx.send(answer.clone());
        }
    }

    /// ChatterBoxInvitation: someone spoke first in a group chat. The bot joins it ("accept
    /// invitation" to ChatSessionRequest, chatterBoxInvitationCoro) and hears the line. Other
    /// sessions (ad-hoc conferences) are left alone.
    pub(super) fn on_session_invitation(&mut self, body: &Llsd) {
        let p = &body["instantmessage"]["message_params"];
        let group = p["id"].as_uuid();
        if group.is_nil() || !self.in_group(group) {
            log::debug!("chat session invitation for {group} ignored: not a group of the bot");
            return;
        }
        if !self.shared.state().group_chats.contains(&group) {
            match self.caps.get("ChatSessionRequest").cloned() {
                Some(url) => {
                    let http = self.http.clone();
                    tokio::spawn(async move {
                        let body =
                            llsd_map! { "method" => "accept invitation", "session-id" => group };
                        if let Err(e) = crate::caps::post_llsd(&http, &url, &body).await {
                            log::warn!("joining the chat of group {group}: {e}");
                        }
                    });
                    self.joined_chat(group);
                }
                None => {
                    log::warn!("no ChatSessionRequest capability: group chat {group} not joined")
                }
            }
        }
        self.shared.emit(Event::GroupChat {
            group,
            from_id: p["from_id"].as_uuid(),
            from_name: p["from_name"].as_str().to_owned(),
            message: p["message"].as_str().to_owned(),
        });
    }

    /// IM_SESSION_SEND in a group's session: a line of its chat.
    pub(super) fn on_session_message(&mut self, im: &crate::InstantMessage) {
        if im.dialog != dialog::SESSION_SEND || !self.in_group(im.id) {
            return;
        }
        self.joined_chat(im.id);
        self.shared.emit(Event::GroupChat {
            group: im.id,
            from_id: im.from_agent_id,
            from_name: im.from_name.clone(),
            message: im.message.clone(),
        });
    }
}

/// MAX_MSG_BUF_SIZE (1024, with its terminating zero): longer texts go in parts, cut between
/// characters.
pub(super) fn split_message(text: &str) -> Vec<&str> {
    const MAX: usize = 1023;
    let mut parts = Vec::new();
    let mut rest = text;
    while rest.len() > MAX {
        let mut cut = MAX;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        parts.push(&rest[..cut]);
        rest = &rest[cut..];
    }
    parts.push(rest);
    parts
}

#[cfg(test)]
mod tests {
    use super::split_message;

    #[test]
    fn long_messages_go_in_parts_between_characters() {
        assert_eq!(split_message("hello"), vec!["hello"]);
        let long = "é".repeat(600); // 1200 bytes
        let parts = split_message(&long);
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|p| p.len() <= 1023));
        assert_eq!(parts.concat(), long);
    }
}
