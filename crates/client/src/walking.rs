//! Walking by keys and following an avatar.
//!
//! A held key is a control flag in every agent update until it is let go, as the viewer's
//! LLAgent::moveAt / moveLeft (Aurora's agent.rs `control_flags`: a held key always sends its
//! FAST_ flag too, running is SetAlwaysRun). The control flags are those of agent.h, from
//! Aurora's `aurora-net` types.rs (see NOTICE.md).
//!
//! Following: the viewer's startFollowPilot (Aurora's autopilot.rs) steers the keys itself each
//! frame, which a bot without frames cannot do well; like LibreMetaverse's FollowCommand, the bot
//! sends the region's autopilot toward the leader again as it moves, and stops near it. The stop
//! distance is bounded as Aurora's FollowSettings (0.5 m to 20 m).

use glam::Vec3;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// AGENT_CONTROL_* (indra/llcommon/indra_constants.h, agent.h), as Aurora's `control` module.
pub mod control {
    pub const AT_POS: u32 = 0x0000_0001;
    pub const AT_NEG: u32 = 0x0000_0002;
    pub const LEFT_POS: u32 = 0x0000_0004;
    pub const LEFT_NEG: u32 = 0x0000_0008;
    pub const FAST_AT: u32 = 0x0000_0400;
    pub const FAST_LEFT: u32 = 0x0000_0800;
    pub const FLY: u32 = 0x0000_2000;
    pub const STOP: u32 = 0x0000_4000;
    pub const STAND_UP: u32 = 0x0001_0000;
    pub const SIT_ON_GROUND: u32 = 0x0002_0000;
    pub const AWAY: u32 = 0x0800_0000;
}

/// A movement key: forward and back, or a step to the side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkKey {
    Forward,
    Back,
    Left,
    Right,
}

impl WalkKey {
    /// The flags the key holds in the agent updates.
    pub fn flags(self) -> u32 {
        use control::*;
        match self {
            WalkKey::Forward => AT_POS | FAST_AT,
            WalkKey::Back => AT_NEG | FAST_AT,
            WalkKey::Left => LEFT_POS | FAST_LEFT,
            WalkKey::Right => LEFT_NEG | FAST_LEFT,
        }
    }
}

/// A key held until a given time.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Walking {
    pub flags: u32,
    pub until: Instant,
}

impl Walking {
    pub fn new(key: WalkKey, hold: Duration, now: Instant) -> Self {
        Walking {
            flags: key.flags(),
            until: now + hold,
        }
    }

    pub fn over(&self, now: Instant) -> bool {
        now >= self.until
    }
}

/// The control flags of the last agent update, and whether one more must confirm them.
///
/// Port of the follow-up of `send_agent_update` (llviewermessage.cpp) in Aurora's
/// agent_update.rs: a change of the flags is sent at once, then sent again, as the simulator
/// stops some motions only when told twice.
#[derive(Debug, Default)]
pub(crate) struct SentFlags {
    last: u32,
    confirm: bool,
}

impl SentFlags {
    /// An update with these flags leaves now.
    pub fn sent(&mut self, flags: u32) {
        self.confirm = flags != self.last;
        self.last = flags;
    }

    /// The last update changed the flags: send one more.
    pub fn confirm(&self) -> bool {
        self.confirm
    }

    /// The flags of the last update.
    pub fn last(&self) -> u32 {
        self.last
    }
}

/// How long the leader may go unseen (no object update, no coarse location) before the follow
/// ends: it left the region, or logged out.
pub const LOST_AFTER: Duration = Duration::from_secs(5);
/// Aurora's FollowSettings: the distance kept from the leader, its bounds.
pub const MIN_DISTANCE: f32 = 0.5;
pub const MAX_DISTANCE: f32 = 20.0;
/// Farther than the distance by this much, the bot sets off again.
const SET_OFF: f32 = 1.0;
/// Within the distance plus this much, it is there.
const THERE: f32 = 0.5;
/// While on the way, the autopilot is sent again when the leader moved this far from where it
/// was aimed at...
const REAIM_MOVE: f32 = 1.0;
/// ...at most this often,
const REAIM_EVERY: Duration = Duration::from_secs(1);
/// ...and anyway after this long, should the region's autopilot have given up.
const RESEND_AFTER: Duration = Duration::from_secs(3);

/// What the follow wants done now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum FollowStep {
    /// Nothing.
    Wait,
    /// Autopilot to this place in the region.
    Go(Vec3),
    /// Arrived: stop the autopilot.
    Stop,
    /// The leader is gone: the follow ends.
    Lost,
}

/// Following an avatar.
#[derive(Debug, Clone)]
pub(crate) struct Follow {
    pub leader: Uuid,
    distance: f32,
    /// On the way to it.
    moving: bool,
    /// Where the leader was when the autopilot was last sent, and when.
    aimed_at: Option<Vec3>,
    sent: Option<Instant>,
    /// Since when the leader is not seen.
    unseen: Option<Instant>,
}

impl Follow {
    pub fn new(leader: Uuid, distance: f32) -> Self {
        Follow {
            leader,
            distance: distance.clamp(MIN_DISTANCE, MAX_DISTANCE),
            moving: false,
            aimed_at: None,
            sent: None,
            unseen: None,
        }
    }

    /// One step: the bot is at `me`, the leader at `leader` when it is seen. On foot the
    /// distance is taken on the ground (a leader on a balcony above is reached below it);
    /// flying, in space.
    pub fn step(
        &mut self,
        now: Instant,
        me: Vec3,
        leader: Option<Vec3>,
        flying: bool,
    ) -> FollowStep {
        let Some(at) = leader else {
            let since = *self.unseen.get_or_insert(now);
            return if now.duration_since(since) >= LOST_AFTER {
                FollowStep::Lost
            } else {
                FollowStep::Wait
            };
        };
        self.unseen = None;
        let gap = gap(me, at, flying);
        if !self.moving {
            if gap <= self.distance + SET_OFF {
                return FollowStep::Wait;
            }
            self.moving = true;
            return self.go(now, me, at, flying);
        }
        if gap <= self.distance + THERE {
            self.moving = false;
            self.aimed_at = None;
            return FollowStep::Stop;
        }
        let since = self.sent.map_or(RESEND_AFTER, |t| now.duration_since(t));
        let moved = self.aimed_at.is_none_or(|a| a.distance(at) >= REAIM_MOVE);
        if since >= RESEND_AFTER || (moved && since >= REAIM_EVERY) {
            return self.go(now, me, at, flying);
        }
        FollowStep::Wait
    }

    fn go(&mut self, now: Instant, me: Vec3, at: Vec3, flying: bool) -> FollowStep {
        self.sent = Some(now);
        self.aimed_at = Some(at);
        FollowStep::Go(short_of(me, at, self.distance, flying))
    }
}

fn gap(me: Vec3, at: Vec3, flying: bool) -> f32 {
    if flying {
        me.distance(at)
    } else {
        me.truncate().distance(at.truncate())
    }
}

/// The place `distance` short of the leader, on the way from the bot; at the leader's height.
fn short_of(me: Vec3, at: Vec3, distance: f32, flying: bool) -> Vec3 {
    let way = if flying {
        at - me
    } else {
        (at - me).with_z(0.0)
    };
    let len = way.length();
    if len <= distance || len == 0.0 {
        return me.with_z(at.z);
    }
    let p = at - way / len * distance;
    p.with_z(if flying { p.z } else { at.z })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEADER: Uuid = Uuid::from_u128(0x5eed_0001);

    #[test]
    fn keys_hold_their_flags_and_their_fast_flag() {
        use control::*;
        assert_eq!(WalkKey::Forward.flags(), AT_POS | FAST_AT);
        assert_eq!(WalkKey::Back.flags(), AT_NEG | FAST_AT);
        assert_eq!(WalkKey::Left.flags(), LEFT_POS | FAST_LEFT);
        assert_eq!(WalkKey::Right.flags(), LEFT_NEG | FAST_LEFT);
        let now = Instant::now();
        let w = Walking::new(WalkKey::Forward, Duration::from_secs(2), now);
        assert!(!w.over(now + Duration::from_secs(1)));
        assert!(w.over(now + Duration::from_secs(2)));
    }

    #[test]
    fn a_change_of_the_flags_is_sent_twice() {
        let mut f = SentFlags::default();
        f.sent(0);
        assert!(!f.confirm());
        f.sent(control::AT_POS);
        assert!(f.confirm());
        f.sent(control::AT_POS);
        assert!(!f.confirm());
        f.sent(0);
        assert!(f.confirm(), "letting go is a change too");
        f.sent(0);
        assert!(!f.confirm());
    }

    #[test]
    fn the_stop_distance_is_bounded() {
        assert_eq!(Follow::new(LEADER, 0.0).distance, MIN_DISTANCE);
        assert_eq!(Follow::new(LEADER, 100.0).distance, MAX_DISTANCE);
        assert_eq!(Follow::new(LEADER, 3.0).distance, 3.0);
    }

    #[test]
    fn it_goes_to_the_leader_and_stops_short_of_it() {
        let t = Instant::now();
        let mut f = Follow::new(LEADER, 2.0);
        let me = Vec3::new(128.0, 128.0, 25.0);
        // close enough already
        let near = Some(Vec3::new(126.5, 128.0, 25.0));
        assert_eq!(f.step(t, me, near, false), FollowStep::Wait);
        // 30 m to the west: two metres short of it
        let leader = Vec3::new(98.0, 128.0, 26.0);
        assert_eq!(
            f.step(t, me, Some(leader), false),
            FollowStep::Go(Vec3::new(100.0, 128.0, 26.0))
        );
        // on the way, the leader still there: nothing more for a while
        let halfway = Vec3::new(115.0, 128.0, 25.0);
        let s = Duration::from_millis(500);
        assert_eq!(
            f.step(t + s, halfway, Some(leader), false),
            FollowStep::Wait
        );
        // there: stop, then wait
        let there = Vec3::new(100.0, 128.0, 25.0);
        assert_eq!(
            f.step(t + 2 * s, there, Some(leader), false),
            FollowStep::Stop
        );
        assert_eq!(
            f.step(t + 3 * s, there, Some(leader), false),
            FollowStep::Wait
        );
    }

    #[test]
    fn it_aims_again_as_the_leader_walks_away() {
        let t = Instant::now();
        let sec = Duration::from_secs(1);
        let mut f = Follow::new(LEADER, 2.0);
        let me = Vec3::new(128.0, 128.0, 25.0);
        let at = |x: f32| Some(Vec3::new(x, 128.0, 25.0));
        assert!(matches!(f.step(t, me, at(98.0), false), FollowStep::Go(_)));
        // moved, but too soon
        assert_eq!(f.step(t + sec / 2, me, at(90.0), false), FollowStep::Wait);
        // a second later: aimed at again
        assert_eq!(
            f.step(t + sec, me, at(90.0), false),
            FollowStep::Go(Vec3::new(92.0, 128.0, 25.0))
        );
        // still, but the region's autopilot may have given up: sent again after a while
        assert_eq!(f.step(t + 2 * sec, me, at(90.0), false), FollowStep::Wait);
        assert!(matches!(
            f.step(t + 4 * sec, me, at(90.0), false),
            FollowStep::Go(_)
        ));
    }

    #[test]
    fn on_foot_the_height_does_not_count() {
        let t = Instant::now();
        let mut f = Follow::new(LEADER, 2.0);
        let me = Vec3::new(128.0, 128.0, 25.0);
        // right above: there already on foot, far when flying
        let above = Some(Vec3::new(128.0, 129.0, 45.0));
        assert_eq!(f.step(t, me, above, false), FollowStep::Wait);
        assert!(matches!(f.step(t, me, above, true), FollowStep::Go(_)));
    }

    #[test]
    fn flying_it_stops_short_of_the_leader_in_space() {
        let me = Vec3::new(0.0, 0.0, 0.0);
        let p = short_of(me, Vec3::new(0.0, 0.0, 10.0), 2.0, true);
        assert!((p - Vec3::new(0.0, 0.0, 8.0)).length() < 1e-4, "{p}");
        // nearer than the distance: where the bot is
        assert_eq!(
            short_of(me, Vec3::new(1.0, 0.0, 3.0), 2.0, false),
            Vec3::new(0.0, 0.0, 3.0)
        );
    }

    #[test]
    fn a_leader_unseen_for_a_while_ends_the_follow() {
        let t = Instant::now();
        let mut f = Follow::new(LEADER, 2.0);
        let me = Vec3::new(128.0, 128.0, 25.0);
        assert_eq!(f.step(t, me, None, false), FollowStep::Wait);
        // seen again in between: the wait starts over
        let back = t + Duration::from_secs(4);
        assert_eq!(f.step(back, me, Some(me), false), FollowStep::Wait);
        assert_eq!(
            f.step(back + Duration::from_secs(1), me, None, false),
            FollowStep::Wait
        );
        assert_eq!(
            f.step(
                back + Duration::from_secs(1) + LOST_AFTER - Duration::from_millis(1),
                me,
                None,
                false
            ),
            FollowStep::Wait
        );
        assert_eq!(
            f.step(back + Duration::from_secs(1) + LOST_AFTER, me, None, false),
            FollowStep::Lost
        );
    }
}
