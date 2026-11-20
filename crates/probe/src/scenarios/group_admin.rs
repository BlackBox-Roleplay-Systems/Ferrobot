//! Running a group with a second avatar. The bot owns a closed group; its partner (the bot's name
//! with "Partner" after the last name, created through RemoteAdmin when missing) is refused a
//! join, accepts an invitation, cannot eject the owner or make itself an owner, is given a role and
//! has it taken back, then is ejected. These are the rules a rebuild of the grid's groups has to
//! keep, the permission refusals most of all.

use super::Probe;
use ferro_client::groups::{GroupSettings, NewGroup, RoleUpdate};
use ferro_client::{Client, Event};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// IM_GROUP_INVITATION (llinstantmessage.h): the sender is the group, the id the invitation.
const IM_GROUP_INVITATION: u8 = 3;
/// GP_MEMBER_INVITE: officers have it, a plain member does not.
const POWER_INVITE: u64 = 1 << 1;
/// GP_ROLE_CHANGE_ACTIONS: owners have it, the default officers do not.
const POWER_CHANGE_ACTIONS: u64 = 1 << 10;
/// How long the region may take to answer, or to pass an invitation or an ejection on.
const ANSWER_WAIT: Duration = Duration::from_secs(15);
/// How long a change the grid does not answer (a role given) gets before it is checked.
const SETTLE: Duration = Duration::from_secs(3);
/// How soon the partner's own region must show a change made through the owner's: on a grid,
/// a region that is not told of a change keeps its cached answer up to 30 s.
const SEEN_ELSEWHERE: Duration = Duration::from_secs(5);
/// How long the probe watches for it, to tell "late" from "never".
const WATCH_ELSEWHERE: Duration = Duration::from_secs(40);

impl Probe {
    pub(super) async fn group_admin(&mut self, owner: &Client) {
        let Some(partner_name) = self.partner_name() else {
            self.emit(
                owner,
                "group-admin-partner",
                false,
                Instant::now(),
                "--user as \"First Last\"".into(),
            );
            return;
        };
        if let Err(e) = self.ensure_account(&partner_name).await {
            self.emit(owner, "group-admin-partner", false, Instant::now(), e);
            return;
        }
        let region = self
            .opts
            .partner_home
            .clone()
            .unwrap_or_else(|| self.opts.home.clone());
        let Some(partner) = self
            .login_as("group-admin-partner", &partner_name, &region)
            .await
        else {
            return;
        };
        self.group_admin_with(owner, &partner.client).await;
        self.logout(partner).await;
    }

    /// The partner's account: the bot's first name, its last name with "Partner" after it.
    fn partner_name(&self) -> Option<String> {
        let (first, last) = self.user.split_once(' ')?;
        Some(format!("{first} {last}Partner"))
    }

    /// Make the account through RemoteAdmin when it is missing; without RemoteAdmin, assume it is there.
    async fn ensure_account(&self, name: &str) -> Result<(), String> {
        let Some(admin) = self.admin.as_ref() else {
            return Ok(());
        };
        let (first, last) = name.split_once(' ').ok_or("an account as \"First Last\"")?;
        if !admin.exists_user(first, last).await? {
            admin.create_user(first, last, &self.password).await?;
            eprintln!("account {first} {last} created");
        }
        Ok(())
    }

    async fn group_admin_with(&mut self, owner: &Client, partner: &Client) {
        let me = owner.state().agent_id;
        let them = partner.state().agent_id;
        let name = format!("Probe admin group {} {:04x}", self.round, rand_suffix());

        // a closed group, the bot its founder
        let t = Instant::now();
        let group = match owner
            .create_group(NewGroup {
                name,
                charter: "Made by ferro-probe.".into(),
                show_in_list: true,
                insignia: Uuid::nil(),
                membership_fee: 0,
                open_enrollment: false,
                allow_publish: false,
                mature_publish: false,
            })
            .await
        {
            Ok(c) if c.success && !c.id.is_nil() => c.id,
            Ok(c) => {
                self.emit(
                    owner,
                    "group-admin-create",
                    false,
                    t,
                    format!("refused: {}", c.message),
                );
                return;
            }
            Err(e) => {
                self.emit(owner, "group-admin-create", false, t, e.to_string());
                return;
            }
        };
        let listed = wait_until(ANSWER_WAIT, || {
            owner.state().groups.iter().any(|g| g.id == group)
        })
        .await;
        self.emit(owner, "group-admin-create", listed, t, String::new());
        if !listed {
            return;
        }
        let roles = match owner.group_roles(group).await {
            Ok(r) => r,
            Err(e) => {
                self.emit(
                    owner,
                    "group-admin-create",
                    false,
                    t,
                    format!("its roles: {e}"),
                );
                return;
            }
        };
        let role = |n: &str| roles.iter().find(|r| r.name == n).map(|r| r.id);
        let (Some(officers), Some(owners)) = (role("Officers"), role("Owners")) else {
            self.emit(
                owner,
                "group-admin-create",
                false,
                t,
                "no Officers or Owners role".into(),
            );
            return;
        };

        // a closed group: the partner cannot just join
        let t = Instant::now();
        let joined = partner.group_join(group).await.unwrap_or(false);
        let member = self.is_member(owner, group, them).await;
        let (ok, note) = match (joined, member) {
            (false, Some(false)) => (true, String::new()),
            (_, None) => (false, "the member list did not come".to_owned()),
            (j, Some(m)) => (false, format!("join answered {j}, a member {m}")),
        };
        self.emit(owner, "group-admin-closed", ok, t, note);

        // an invitation, accepted
        let mut events = partner.subscribe();
        let t = Instant::now();
        let invited = owner.group_invite(group, them, &[Uuid::nil()]).await;
        let invitation = match invited {
            Ok(()) => {
                next_event(&mut events, ANSWER_WAIT, |e| match e {
                    Event::InstantMessage(im)
                        if im.dialog == IM_GROUP_INVITATION && im.from_agent_id == group =>
                    {
                        Some(im.id)
                    }
                    _ => None,
                })
                .await
            }
            Err(_) => None,
        };
        let accepted = match invitation {
            Some(transaction) => {
                partner
                    .answer_group_invite(group, transaction, true)
                    .await
                    .is_ok()
                    && wait_until(ANSWER_WAIT, || {
                        partner.state().groups.iter().any(|g| g.id == group)
                    })
                    .await
            }
            None => false,
        };
        // the owner's region shows the new member: at once on one region server, or once Robust
        // has told the owner's server of the change (the partner accepted on its own region)
        let since = Instant::now();
        let mut member = self.is_member(owner, group, them).await;
        while accepted && member != Some(true) && since.elapsed() < SEEN_ELSEWHERE {
            tokio::time::sleep(Duration::from_millis(100)).await;
            member = self.is_member(owner, group, them).await;
        }
        let shown = since.elapsed().as_millis();
        let (ok, note) = match (invited, invitation, accepted, member) {
            (Err(e), ..) => (false, format!("invite: {e}")),
            (_, None, ..) => (false, "no invitation came".to_owned()),
            (_, _, false, _) => (
                false,
                "accepted, but the group is not in the partner's list".to_owned(),
            ),
            (_, _, true, Some(true)) => (true, format!("in the owner's list after {shown} ms")),
            (_, _, true, m) => (
                false,
                format!("not in the owner's member list after {shown} ms ({m:?})"),
            ),
        };
        self.emit(owner, "group-admin-invite", ok, t, note);
        if !ok {
            return;
        }
        // the partner's region now has the member list cached (the role step checks it changes)
        let _ = partner.group_members_within(group, Duration::ZERO).await;

        // a plain member can neither eject the owner nor make itself an owner
        let t = Instant::now();
        let ejected = partner.group_eject(group, me).await;
        // the owner's viewer drops the group if the region (wrongly) tells it so
        tokio::time::sleep(SETTLE).await;
        let kept = owner.state().groups.iter().any(|g| g.id == group);
        let still = self.is_member(owner, group, me).await;
        let (ok, note) = match (&ejected, still, kept) {
            (Err(_), Some(true), true) => (true, String::new()),
            (_, Some(false), _) => (false, "the owner was ejected by a plain member".to_owned()),
            (Ok(()), Some(true), kept) => (
                false,
                format!(
                    "the grid answered the ejection as done; the owner is still a member{}",
                    if kept {
                        ""
                    } else {
                        ", but their viewer was told to drop the group"
                    }
                ),
            ),
            (Err(_), Some(true), false) => (
                false,
                "refused, but the owner's viewer was told to drop the group".to_owned(),
            ),
            (_, None, _) => (false, "the member list did not come".to_owned()),
        };
        self.emit(owner, "group-admin-member-cannot-eject", ok, t, note);

        let t = Instant::now();
        let _ = partner.group_add_role(group, them, owners).await;
        tokio::time::sleep(SETTLE).await;
        let (ok, note) = match owner.group_role_members(group).await {
            Ok(pairs) if pairs.contains(&(owners, them)) => {
                (false, "the partner made itself an owner".to_owned())
            }
            Ok(_) => (true, String::new()),
            Err(e) => (false, e.to_string()),
        };
        self.emit(owner, "group-admin-member-cannot-promote", ok, t, note);

        // a role given, then taken back
        let t = Instant::now();
        let given = owner.group_add_role(group, them, officers).await.is_ok()
            && self.holds(owner, group, officers, them, true).await;
        let powers = self.powers_of(owner, group, them).await;
        let (ok, note) = match (given, powers) {
            (true, Some(p)) if p & POWER_INVITE != 0 => (true, String::new()),
            (true, p) => (false, format!("an officer, powers {p:x?} lack Invite")),
            (false, _) => (false, "not in the Officers role".to_owned()),
        };
        self.emit(owner, "group-admin-role", ok, t, note);

        // the partner's own region shows the role given through the owner's (another region
        // server on a grid: there it takes Robust telling that server of the change)
        let t = Instant::now();
        let mut seen = None;
        while t.elapsed() < WATCH_ELSEWHERE {
            if self
                .powers_seen_by(partner, group, them)
                .await
                .is_some_and(|p| p & POWER_INVITE != 0)
            {
                seen = Some(t.elapsed());
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let (ok, note) = match seen {
            Some(d) if d <= SEEN_ELSEWHERE => (true, format!("seen after {} ms", d.as_millis())),
            Some(d) => (false, format!("seen only after {} ms", d.as_millis())),
            None => (
                false,
                format!("not seen in {} s", WATCH_ELSEWHERE.as_secs()),
            ),
        };
        self.emit(owner, "group-admin-role-seen-by-member", ok, t, note);

        // an officer may change the charter (Change Identity) and a role's title (Role Properties),
        // but not a role's abilities (no Change Actions)
        let t = Instant::now();
        let charter = format!("Charter by the officer, round {}", self.round);
        let (ok, note) = match self.change_charter(owner, partner, group, &charter).await {
            Ok(now) if now == charter => (true, String::new()),
            Ok(now) => (false, format!("the charter stayed {now:?}")),
            Err(e) => (false, e),
        };
        self.emit(owner, "group-admin-officer-charter", ok, t, note);

        let t = Instant::now();
        let title = format!("Officer, round {}", self.round);
        let (ok, note) = match roles.iter().find(|r| r.id == officers) {
            Some(before) => {
                let sent = partner
                    .group_role_update(
                        group,
                        RoleUpdate {
                            role: officers,
                            name: before.name.clone(),
                            description: before.description.clone(),
                            title: title.clone(),
                            powers: before.powers | POWER_CHANGE_ACTIONS,
                        },
                    )
                    .await;
                tokio::time::sleep(SETTLE).await;
                match (sent, owner.group_roles(group).await) {
                    (Err(e), _) | (_, Err(e)) => (false, e.to_string()),
                    (Ok(()), Ok(now)) => match now.iter().find(|r| r.id == officers) {
                        Some(r) if r.title == title && r.powers == before.powers => {
                            (true, String::new())
                        }
                        Some(r) if r.title != title => {
                            (false, format!("the title stayed {:?}", r.title))
                        }
                        Some(r) => (
                            false,
                            format!(
                                "the abilities changed: {:x} to {:x}",
                                before.powers, r.powers
                            ),
                        ),
                        None => (false, "the Officers role went".to_owned()),
                    },
                }
            }
            None => (false, "no Officers role".to_owned()),
        };
        self.emit(owner, "group-admin-officer-role", ok, t, note);

        let t = Instant::now();
        let taken = owner.group_remove_role(group, them, officers).await.is_ok()
            && self.holds(owner, group, officers, them, false).await;
        let powers = self.powers_of(owner, group, them).await;
        let (ok, note) = match (taken, powers) {
            (true, Some(p)) if p & POWER_INVITE == 0 => (true, String::new()),
            (true, p) => (false, format!("powers {p:x?} still have Invite")),
            (false, _) => (false, "still in the Officers role".to_owned()),
        };
        self.emit(owner, "group-admin-unrole", ok, t, note);

        // a plain member again: the charter is not theirs to change
        let t = Instant::now();
        let before = owner.group_profile(group).await.map(|p| p.charter);
        let (ok, note) = match (
            before,
            self.change_charter(owner, partner, group, "Charter by a plain member")
                .await,
        ) {
            (Ok(b), Ok(now)) if now == b => (true, String::new()),
            (Ok(_), Ok(now)) => (false, format!("the charter became {now:?}")),
            (Err(e), _) => (false, e.to_string()),
            (_, Err(e)) => (false, e),
        };
        self.emit(owner, "group-admin-member-cannot-edit", ok, t, note);

        // ejected: gone from the member list and from the partner's own groups
        let t = Instant::now();
        let ejected = owner.group_eject(group, them).await;
        let gone = ejected.is_ok()
            && wait_until(ANSWER_WAIT, || {
                !partner.state().groups.iter().any(|g| g.id == group)
            })
            .await;
        let member = self.is_member(owner, group, them).await;
        let (ok, note) = match (ejected, gone, member) {
            (Err(e), ..) => (false, e.to_string()),
            (_, true, Some(false)) => (true, String::new()),
            (_, false, _) => (false, "the group stayed in the partner's list".to_owned()),
            (_, _, m) => (false, format!("still in the member list ({m:?})")),
        };
        self.emit(owner, "group-admin-eject", ok, t, note);

        // banned (GroupAPIv1): on the list, and no invitation reaches them; then the ban lifted
        let t = Instant::now();
        let (ok, note) = match owner.group_ban(group, &[them], true).await {
            Ok(list) if list.contains(&them) => (true, String::new()),
            Ok(list) => (
                false,
                format!("not on the ban list ({} banned)", list.len()),
            ),
            Err(e) => (false, e.to_string()),
        };
        self.emit(owner, "group-admin-ban", ok, t, note);

        let mut events = partner.subscribe();
        let t = Instant::now();
        let invited = owner.group_invite(group, them, &[Uuid::nil()]).await;
        let came = next_event(&mut events, SEEN_ELSEWHERE, |e| match e {
            Event::InstantMessage(im)
                if im.dialog == IM_GROUP_INVITATION && im.from_agent_id == group =>
            {
                Some(())
            }
            _ => None,
        })
        .await;
        let (ok, note) = match (invited, came) {
            (Err(e), _) => (false, format!("invite: {e}")),
            (Ok(()), None) => (true, String::new()),
            (Ok(()), Some(())) => (false, "a banned avatar was invited".to_owned()),
        };
        self.emit(owner, "group-admin-banned-not-invited", ok, t, note);

        let t = Instant::now();
        let (ok, note) = match owner.group_ban(group, &[them], false).await {
            Ok(list) if !list.contains(&them) => (true, String::new()),
            Ok(_) => (false, "still on the ban list".to_owned()),
            Err(e) => (false, e.to_string()),
        };
        self.emit(owner, "group-admin-unban", ok, t, note);
    }

    /// The partner sends the group's settings with a new charter (the rest as they are); the
    /// owner then reads the charter the group has.
    async fn change_charter(
        &self,
        owner: &Client,
        partner: &Client,
        group: Uuid,
        charter: &str,
    ) -> Result<String, String> {
        let current = partner
            .group_profile(group)
            .await
            .map_err(|e| format!("profile: {e}"))?;
        let mut settings = GroupSettings::from(&current);
        settings.charter = charter.to_owned();
        partner
            .group_update(group, settings)
            .await
            .map_err(|e| format!("update: {e}"))?;
        tokio::time::sleep(SETTLE).await;
        owner
            .group_profile(group)
            .await
            .map(|p| p.charter)
            .map_err(|e| format!("the owner's profile: {e}"))
    }

    /// Whether the avatar is in the group's member list, fetched afresh; None when it did not come.
    async fn is_member(&self, owner: &Client, group: Uuid, avatar: Uuid) -> Option<bool> {
        owner
            .group_members_within(group, Duration::ZERO)
            .await
            .ok()
            .map(|m| m.iter().any(|x| x.id == avatar))
    }

    /// The avatar's powers in the group, as `viewer`'s region gives them in the member list.
    async fn powers_seen_by(&self, viewer: &Client, group: Uuid, avatar: Uuid) -> Option<u64> {
        viewer
            .group_members_within(group, Duration::ZERO)
            .await
            .ok()?
            .iter()
            .find(|x| x.id == avatar)
            .map(|x| x.powers)
    }

    /// The avatar's powers in the group, as its member list gives them.
    async fn powers_of(&self, owner: &Client, group: Uuid, avatar: Uuid) -> Option<u64> {
        owner
            .group_members_within(group, Duration::ZERO)
            .await
            .ok()?
            .iter()
            .find(|x| x.id == avatar)
            .map(|x| x.powers)
    }

    /// Wait until the avatar holds (or no longer holds) the role.
    async fn holds(
        &self,
        owner: &Client,
        group: Uuid,
        role: Uuid,
        avatar: Uuid,
        held: bool,
    ) -> bool {
        let t = Instant::now();
        while t.elapsed() < ANSWER_WAIT {
            if let Ok(pairs) = owner.group_role_members(group).await
                && pairs.contains(&(role, avatar)) == held
            {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        false
    }
}

fn rand_suffix() -> u16 {
    (Uuid::new_v4().as_u128() & 0xffff) as u16
}

async fn wait_until(wait: Duration, done: impl Fn() -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < wait {
        if done() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    done()
}

async fn next_event<T>(
    events: &mut tokio::sync::broadcast::Receiver<Event>,
    wait: Duration,
    pick: impl Fn(&Event) -> Option<T>,
) -> Option<T> {
    tokio::time::timeout(wait, async {
        loop {
            match events.recv().await {
                Ok(e) => {
                    if let Some(v) = pick(&e) {
                        return Some(v);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                Err(_) => {}
            }
        }
    })
    .await
    .ok()
    .flatten()
}
