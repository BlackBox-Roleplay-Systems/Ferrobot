//! The client's group requests, for the commands BB's SL group sync sends through SecondBot's
//! API: the roles, the members (on demand, cached a minute, unlike SecondBot's list from the login
//! only), who holds a role, invitations with a role, ejections and role changes.

use crate::groups::{GroupMember, GroupRole, GroupTitle, RoleChange};
use crate::network::GroupRequest;
use crate::{Client, ClientError, Request};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use uuid::Uuid;

/// A member list younger than this is answered from the cache (SecondBot calls a list older than
/// 60 s "Updating").
pub const MEMBERS_MAX_AGE: Duration = Duration::from_secs(60);
/// A big group's member list comes over many packets when the region has no GroupMemberData.
const MEMBERS_TIMEOUT: Duration = Duration::from_secs(30);

impl Client {
    fn check_group(&self, group: Uuid) -> Result<(), ClientError> {
        let st = self.shared.state();
        if !st.online {
            return Err(ClientError::NotConnected);
        }
        if group.is_nil() || !st.groups.iter().any(|g| g.id == group) {
            return Err(ClientError::UnknownGroup);
        }
        Ok(())
    }

    async fn group_ask<T>(
        &self,
        timeout: Duration,
        make: impl FnOnce(oneshot::Sender<T>) -> GroupRequest,
    ) -> Result<T, ClientError> {
        self.ask_within(timeout, |reply| Request::Group(make(reply)))
            .await
    }

    /// The group's roles (GroupRoleDataRequest); the "everyone" role has the nil id.
    pub async fn group_roles(&self, group: Uuid) -> Result<Vec<GroupRole>, ClientError> {
        self.check_group(group)?;
        self.group_ask(crate::REQUEST_TIMEOUT, |reply| GroupRequest::Roles {
            group,
            reply,
        })
        .await
    }

    /// The titles the bot can wear in the group (GroupTitlesRequest).
    pub async fn group_titles(&self, group: Uuid) -> Result<Vec<GroupTitle>, ClientError> {
        self.check_group(group)?;
        self.group_ask(crate::REQUEST_TIMEOUT, |reply| GroupRequest::Titles {
            group,
            reply,
        })
        .await
    }

    /// The group's members: from the cache when younger than [`MEMBERS_MAX_AGE`], else fetched.
    pub async fn group_members(&self, group: Uuid) -> Result<Vec<GroupMember>, ClientError> {
        self.group_members_within(group, MEMBERS_MAX_AGE).await
    }

    /// The group's members, fetched unless the cache is younger than `max_age`.
    pub async fn group_members_within(
        &self,
        group: Uuid,
        max_age: Duration,
    ) -> Result<Vec<GroupMember>, ClientError> {
        self.check_group(group)?;
        if let Some((members, at)) = self.cached_members().get(&group)
            && at.elapsed() < max_age
        {
            return Ok(members.clone());
        }
        let members = self
            .group_ask(MEMBERS_TIMEOUT, |reply| GroupRequest::Members {
                group,
                reply,
            })
            .await?
            .map_err(ClientError::Refused)?;
        self.cached_members()
            .insert(group, (members.clone(), Instant::now()));
        Ok(members)
    }

    fn cached_members(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<Uuid, (Vec<GroupMember>, Instant)>>
    {
        self.shared
            .group_members
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Forget a group's cached member list: the next question fetches it again.
    pub fn forget_group_members(&self, group: Uuid) {
        self.cached_members().remove(&group);
    }

    /// Whether the avatar is in the group (its member list, as [`Client::group_members`]).
    pub async fn is_group_member(&self, group: Uuid, avatar: Uuid) -> Result<bool, ClientError> {
        Ok(self
            .group_members(group)
            .await?
            .iter()
            .any(|m| m.id == avatar))
    }

    /// Every (role, member) pair of the group (GroupRoleMembersRequest).
    pub async fn group_role_members(&self, group: Uuid) -> Result<Vec<(Uuid, Uuid)>, ClientError> {
        self.check_group(group)?;
        self.group_ask(MEMBERS_TIMEOUT, |reply| GroupRequest::RoleMembers {
            group,
            reply,
        })
        .await
    }

    /// Invite an avatar into the group with these roles (the nil role: "everyone"), one invitation
    /// block per role as LLGroupMgr::sendGroupMemberInvites. The grid does not answer: the
    /// invitation is sent.
    pub async fn group_invite(
        &self,
        group: Uuid,
        avatar: Uuid,
        roles: &[Uuid],
    ) -> Result<(), ClientError> {
        self.check_group(group)?;
        let invites = roles.iter().map(|role| (avatar, *role)).collect();
        self.group_ask(crate::REQUEST_TIMEOUT, |reply| GroupRequest::Invite {
            group,
            invites,
            reply,
        })
        .await
    }

    /// Eject an avatar from the group; refused when the grid says so (no power, not a member).
    pub async fn group_eject(&self, group: Uuid, avatar: Uuid) -> Result<(), ClientError> {
        self.check_group(group)?;
        let done = self
            .group_ask(crate::REQUEST_TIMEOUT, |reply| GroupRequest::Eject {
                group,
                members: vec![avatar],
                reply,
            })
            .await?;
        if !done {
            return Err(ClientError::Refused("the grid refused the ejection".into()));
        }
        if let Some((members, _)) = self.cached_members().get_mut(&group) {
            members.retain(|m| m.id != avatar);
        }
        Ok(())
    }

    async fn role_change(&self, group: Uuid, change: RoleChange) -> Result<(), ClientError> {
        self.check_group(group)?;
        self.group_ask(crate::REQUEST_TIMEOUT, |reply| GroupRequest::RoleChanges {
            group,
            changes: vec![change],
            reply,
        })
        .await
    }

    /// Give a member a role (GroupRoleChanges). The grid does not answer.
    pub async fn group_add_role(
        &self,
        group: Uuid,
        avatar: Uuid,
        role: Uuid,
    ) -> Result<(), ClientError> {
        self.role_change(
            group,
            RoleChange {
                role,
                member: avatar,
                add: true,
            },
        )
        .await
    }

    /// Take a role from a member (GroupRoleChanges). The grid does not answer.
    pub async fn group_remove_role(
        &self,
        group: Uuid,
        avatar: Uuid,
        role: Uuid,
    ) -> Result<(), ClientError> {
        self.role_change(
            group,
            RoleChange {
                role,
                member: avatar,
                add: false,
            },
        )
        .await
    }

    /// Answer a group invitation: `group` is the invitation's sender id, `transaction` its IM id.
    pub async fn answer_group_invite(
        &self,
        group: Uuid,
        transaction: Uuid,
        accept: bool,
    ) -> Result<(), ClientError> {
        self.ask(|reply| {
            Request::Group(GroupRequest::AnswerInvite {
                group,
                transaction,
                accept,
                reply,
            })
        })
        .await
    }

    /// Join a group's chat (IM_SESSION_GROUP_START); done at once when the bot is in it.
    pub async fn group_chat_join(&self, group: Uuid) -> Result<(), ClientError> {
        self.check_group(group)?;
        self.group_ask(crate::REQUEST_TIMEOUT, |reply| GroupRequest::ChatJoin {
            group,
            reply,
        })
        .await?
        .map_err(ClientError::Refused)
    }

    /// Say something in a group's chat, joining it first when needed (SecondBot's GroupChat).
    pub async fn group_chat_send(&self, group: Uuid, text: &str) -> Result<(), ClientError> {
        self.group_chat_join(group).await?;
        let text = text.to_owned();
        self.group_ask(crate::REQUEST_TIMEOUT, |reply| GroupRequest::ChatSend {
            group,
            text,
            reply,
        })
        .await
    }

    /// Leave a group's chat (IM_SESSION_LEAVE): its lines stop until it is joined again.
    pub async fn group_chat_leave(&self, group: Uuid) -> Result<(), ClientError> {
        self.check_group(group)?;
        self.group_ask(crate::REQUEST_TIMEOUT, |reply| GroupRequest::ChatLeave {
            group,
            reply,
        })
        .await
    }

    /// Send a group notice, an item of the bot's attached or not (needs the notices power).
    pub async fn group_notice(
        &self,
        group: Uuid,
        subject: &str,
        message: &str,
        item: Option<Uuid>,
    ) -> Result<(), ClientError> {
        self.check_group(group)?;
        let (subject, message) = (subject.to_owned(), message.to_owned());
        self.group_ask(crate::REQUEST_TIMEOUT, |reply| GroupRequest::Notice {
            group,
            subject,
            message,
            item,
            reply,
        })
        .await
    }

    /// Wear the title of one of the bot's roles in the group.
    pub async fn group_activate_title(&self, group: Uuid, role: Uuid) -> Result<(), ClientError> {
        self.check_group(group)?;
        self.group_ask(crate::REQUEST_TIMEOUT, |reply| {
            GroupRequest::ActivateTitle { group, role, reply }
        })
        .await
    }

    /// Make the group the bot's active group (the grid answers with AgentDataUpdate).
    pub async fn group_activate(&self, group: Uuid) -> Result<(), ClientError> {
        self.check_group(group)?;
        self.group_ask(crate::REQUEST_TIMEOUT, |reply| GroupRequest::Activate {
            group,
            reply,
        })
        .await
    }

    /// Join a group with open enrollment; false when the grid refused (invite only, a fee).
    pub async fn group_join(&self, group: Uuid) -> Result<bool, ClientError> {
        self.ask(|reply| Request::Group(GroupRequest::Join { group, reply }))
            .await
    }

    /// Leave a group; false when the grid refused.
    pub async fn group_leave(&self, group: Uuid) -> Result<bool, ClientError> {
        self.check_group(group)?;
        let left = self
            .group_ask(crate::REQUEST_TIMEOUT, |reply| GroupRequest::Leave {
                group,
                reply,
            })
            .await?;
        if left {
            self.forget_group_members(group);
        }
        Ok(left)
    }

    /// The avatars banned from the group (GroupAPIv1; needs the ban power).
    pub async fn group_bans(&self, group: Uuid) -> Result<Vec<Uuid>, ClientError> {
        self.check_group(group)?;
        self.group_ask(crate::REQUEST_TIMEOUT, |reply| GroupRequest::Bans {
            group,
            action: None,
            ids: Vec::new(),
            reply,
        })
        .await?
        .map_err(ClientError::Refused)
    }

    /// Ban (or unban) avatars from the group; answers the ban list after.
    pub async fn group_ban(
        &self,
        group: Uuid,
        ids: &[Uuid],
        ban: bool,
    ) -> Result<Vec<Uuid>, ClientError> {
        self.check_group(group)?;
        let action = if ban {
            crate::groups::ban::CREATE
        } else {
            crate::groups::ban::DELETE
        };
        let ids = ids.to_vec();
        self.group_ask(crate::REQUEST_TIMEOUT, |reply| GroupRequest::Bans {
            group,
            action: Some(action),
            ids,
            reply,
        })
        .await?
        .map_err(ClientError::Refused)
    }

    /// Ask the grid for the group list again (SecondBot's ForceLoadGroups).
    pub async fn reload_groups(&self) -> Result<(), ClientError> {
        self.ask(|reply| Request::Group(GroupRequest::Reload { reply }))
            .await
    }
}
