//! The grid's groups: their roles and members, and the replies a region sends about them
//! (GroupRoleDataReply, GroupMembersReply, GroupRoleMembersReply, GroupTitlesReply, the
//! GroupMemberData capability, AgentGroupDataUpdate). Lists go out a few entries per packet, so
//! that the bot gathers them as it must for a big group.

use crate::{GridGroup, GridMember, GridRole};
use ferro_llsd::{Llsd, Map};
use ferro_msg::msgs::*;
use ferro_msg::str_field;
use uuid::Uuid;

pub const TEST_GROUP: Uuid = Uuid::from_u128(0x6000_0001);
pub const SECOND_GROUP: Uuid = Uuid::from_u128(0x6000_0002);
/// A group the bot is not in, and can be invited to.
pub const THIRD_GROUP: Uuid = Uuid::from_u128(0x6000_0003);
pub const OWNERS: Uuid = Uuid::from_u128(0x7000_0001);
pub const OFFICERS: Uuid = Uuid::from_u128(0x7000_0002);
pub const BUILDERS: Uuid = Uuid::from_u128(0x7000_0003);

/// GP_* (roles_constants.h)
pub const MEMBER_INVITE: u64 = 1 << 1;
pub const MEMBER_EJECT: u64 = 1 << 2;
pub const ROLE_ASSIGN_MEMBER: u64 = 1 << 8;
pub const ROLE_REMOVE_MEMBER: u64 = 1 << 9;
pub const GROUP_BAN_ACCESS: u64 = 1 << 51;
const NOTICES_RECEIVE: u64 = 1 << 43;

/// Entries per reply packet.
const PAGE: usize = 2;

fn role(id: Uuid, name: &str, title: &str, powers: u64) -> GridRole {
    GridRole {
        id,
        name: name.into(),
        title: title.into(),
        powers,
    }
}

/// A group the bot just founded (CreateGroupRequest): it is its owner, alone in it, with the
/// roles a new group has (Everyone and Owners).
pub fn founded(id: Uuid, name: &str) -> GridGroup {
    let owners = Uuid::new_v4();
    GridGroup {
        id,
        name: name.into(),
        title: "Owner".into(),
        powers: u64::MAX >> 1,
        bot_member: true,
        bot_roles: vec![owners],
        roles: vec![
            role(Uuid::nil(), "Everyone", "Member", NOTICES_RECEIVE),
            role(owners, "Owners", "Owner", u64::MAX >> 1),
        ],
        members: Vec::new(),
    }
}

pub fn default_groups() -> Vec<GridGroup> {
    let everyone = role(Uuid::nil(), "Everyone", "Member", NOTICES_RECEIVE);
    vec![
        GridGroup {
            id: TEST_GROUP,
            name: "Test Group".into(),
            title: "Bot".into(),
            powers: MEMBER_INVITE
                | MEMBER_EJECT
                | ROLE_ASSIGN_MEMBER
                | ROLE_REMOVE_MEMBER
                | GROUP_BAN_ACCESS,
            bot_member: true,
            bot_roles: vec![OFFICERS],
            roles: vec![
                everyone.clone(),
                role(OWNERS, "Owners", "Owner", u64::MAX >> 1),
                role(
                    OFFICERS,
                    "Officers",
                    "Bot",
                    MEMBER_INVITE | MEMBER_EJECT | ROLE_ASSIGN_MEMBER | ROLE_REMOVE_MEMBER,
                ),
                role(BUILDERS, "Builders", "Builder", 0),
            ],
            members: vec![
                GridMember {
                    id: Uuid::from_u128(0x5eed_0001),
                    roles: vec![OWNERS, BUILDERS],
                    owner: true,
                    last_login: "Online".into(),
                },
                GridMember {
                    id: Uuid::from_u128(0x5eed_0002),
                    roles: vec![],
                    owner: false,
                    last_login: "09/12/2026".into(),
                },
            ],
        },
        GridGroup {
            id: SECOND_GROUP,
            name: "Second Group".into(),
            title: "Member".into(),
            powers: 0,
            bot_member: true,
            bot_roles: vec![],
            roles: vec![everyone.clone()],
            members: vec![],
        },
        GridGroup {
            id: THIRD_GROUP,
            name: "Third Group".into(),
            title: "Member".into(),
            powers: 0,
            bot_member: false,
            bot_roles: vec![],
            roles: vec![everyone],
            members: vec![GridMember {
                id: Uuid::from_u128(0x5eed_0001),
                roles: vec![],
                owner: true,
                last_login: "Online".into(),
            }],
        },
    ]
}

/// The members, the bot first when it is in the group.
pub fn all_members(g: &GridGroup, bot: Uuid) -> Vec<GridMember> {
    let me = g.bot_member.then(|| GridMember {
        id: bot,
        roles: g.bot_roles.clone(),
        owner: g.bot_roles.contains(&OWNERS),
        last_login: "Online".into(),
    });
    me.into_iter().chain(g.members.iter().cloned()).collect()
}

/// A member's powers: those of "everyone" and of each of its roles.
fn powers_of(g: &GridGroup, m: &GridMember) -> u64 {
    g.roles
        .iter()
        .filter(|r| r.id.is_nil() || m.roles.contains(&r.id))
        .fold(0, |p, r| p | r.powers)
}

fn title_of(g: &GridGroup, m: &GridMember) -> String {
    g.roles
        .iter()
        .find(|r| m.roles.first() == Some(&r.id))
        .or_else(|| g.roles.first())
        .map(|r| r.title.clone())
        .unwrap_or_default()
}

pub fn role_data_replies(g: &GridGroup, bot: Uuid, request: Uuid) -> Vec<GroupRoleDataReply> {
    let members = all_members(g, bot);
    g.roles
        .chunks(PAGE)
        .map(|chunk| {
            let mut m = GroupRoleDataReply::default();
            m.agent_data.agent_id = bot;
            m.group_data.group_id = g.id;
            m.group_data.request_id = request;
            m.group_data.role_count = g.roles.len() as i32;
            m.role_data = chunk
                .iter()
                .map(|r| group_role_data_reply::RoleData {
                    role_id: r.id,
                    name: str_field(&r.name),
                    title: str_field(&r.title),
                    description: str_field(""),
                    powers: r.powers,
                    members: members
                        .iter()
                        .filter(|m| r.id.is_nil() || m.roles.contains(&r.id))
                        .count() as u32,
                })
                .collect();
            m
        })
        .collect()
}

pub fn members_replies(g: &GridGroup, bot: Uuid, request: Uuid) -> Vec<GroupMembersReply> {
    let members = all_members(g, bot);
    members
        .chunks(PAGE)
        .map(|chunk| {
            let mut m = GroupMembersReply::default();
            m.agent_data.agent_id = bot;
            m.group_data.group_id = g.id;
            m.group_data.request_id = request;
            m.group_data.member_count = members.len() as i32;
            m.member_data = chunk
                .iter()
                .map(|member| group_members_reply::MemberData {
                    agent_id: member.id,
                    contribution: 0,
                    online_status: str_field(&member.last_login),
                    agent_powers: powers_of(g, member),
                    title: str_field(&title_of(g, member)),
                    is_owner: member.owner,
                })
                .collect();
            m
        })
        .collect()
}

/// The (role, member) pairs of the roles other than "everyone", as the grid lists them.
pub fn role_members_replies(g: &GridGroup, bot: Uuid, request: Uuid) -> Vec<GroupRoleMembersReply> {
    let pairs: Vec<(Uuid, Uuid)> = all_members(g, bot)
        .iter()
        .flat_map(|m| m.roles.iter().map(|r| (*r, m.id)))
        .collect();
    let page = |chunk: &[(Uuid, Uuid)]| {
        let mut m = GroupRoleMembersReply::default();
        m.agent_data.agent_id = bot;
        m.agent_data.group_id = g.id;
        m.agent_data.request_id = request;
        m.agent_data.total_pairs = pairs.len() as u32;
        m.member_data = chunk
            .iter()
            .map(|(role, member)| group_role_members_reply::MemberData {
                role_id: *role,
                member_id: *member,
            })
            .collect();
        m
    };
    if pairs.is_empty() {
        return vec![page(&[])];
    }
    pairs.chunks(PAGE).map(page).collect()
}

pub fn titles_reply(g: &GridGroup, bot: Uuid, request: Uuid) -> GroupTitlesReply {
    let mut m = GroupTitlesReply::default();
    m.agent_data.agent_id = bot;
    m.agent_data.group_id = g.id;
    m.agent_data.request_id = request;
    m.group_data = g
        .roles
        .iter()
        .filter(|r| r.id.is_nil() || g.bot_roles.contains(&r.id))
        .map(|r| group_titles_reply::GroupData {
            title: str_field(&r.title),
            role_id: r.id,
            selected: g.bot_roles.first() == Some(&r.id),
        })
        .collect();
    m
}

/// The GroupMemberData capability's reply (LLGroupMgr::processCapGroupMembersResponse reads it):
/// titles by index, powers in hexadecimal when not the default ones.
pub fn member_data(g: &GridGroup, bot: Uuid) -> Llsd {
    let titles: Vec<String> = g.roles.iter().map(|r| r.title.clone()).collect();
    let default_powers = g.roles.first().map_or(0, |r| r.powers);
    let mut members = Map::new();
    for m in all_members(g, bot) {
        let mut info = Map::new();
        info.insert("last_login".into(), m.last_login.clone().into());
        let title = title_of(g, &m);
        let index = titles.iter().position(|t| *t == title).unwrap_or(0);
        if index != 0 {
            info.insert("title".into(), (index as i32).into());
        }
        let powers = powers_of(g, &m);
        if powers != default_powers {
            info.insert("powers".into(), format!("{powers:x}").into());
        }
        if m.owner {
            info.insert("owner".into(), "Y".into());
        }
        members.insert(m.id.to_string(), Llsd::Map(info));
    }
    let mut defaults = Map::new();
    defaults.insert(
        "default_powers".into(),
        format!("{default_powers:x}").into(),
    );
    let mut v = Map::new();
    v.insert("group_id".into(), g.id.into());
    v.insert("members".into(), Llsd::Map(members));
    v.insert(
        "titles".into(),
        Llsd::Array(titles.into_iter().map(Llsd::from).collect()),
    );
    v.insert("defaults".into(), Llsd::Map(defaults));
    Llsd::Map(v)
}

/// AgentGroupDataUpdate's body, on the event queue: the groups the bot is in.
pub fn agent_group_data(groups: &[GridGroup], bot: Uuid) -> Llsd {
    let list: Vec<Llsd> = groups
        .iter()
        .filter(|g| g.bot_member)
        .map(|g| {
            let mut e = Map::new();
            e.insert("GroupID".into(), g.id.into());
            e.insert("GroupName".into(), g.name.clone().into());
            e.insert("GroupInsigniaID".into(), Uuid::nil().into());
            // U64 travels as 8 big-endian bytes in LLSD
            e.insert("GroupPowers".into(), g.powers.to_be_bytes().to_vec().into());
            e.insert("AcceptNotices".into(), true.into());
            e.insert("Contribution".into(), 0.into());
            Llsd::Map(e)
        })
        .collect();
    let mut agent = Map::new();
    agent.insert("AgentID".into(), bot.into());
    let mut body = Map::new();
    body.insert("AgentData".into(), Llsd::Array(vec![Llsd::Map(agent)]));
    body.insert("GroupData".into(), Llsd::Array(list));
    Llsd::Map(body)
}
