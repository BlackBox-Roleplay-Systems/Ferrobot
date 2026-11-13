//! SecondBot's TriggerOnEventService (Services/TriggerOnEventService.cs, Config/OnEventConfig.cs):
//! commands the bot runs by itself when an event matches the conditions its settings give. This
//! is the pure part: the events read from the settings, their conditions, the values an event
//! gives, and the actions with those values put in. crate::services::on_event runs them.
//!
//! An event has a source (`AvatarIm`, `SimAlert`, `BalanceUpdate`, `GroupMemberJoin`,
//! `GroupMemberLeave`, `ChangeSim`, `GuestJoins`, `GuestLeaves`, `Clock`), what it watches
//! (`Monitor`: a group's key, `Region=#=Parcel`, an avatar or `Any`), conditions (`Where`:
//! `left{OP}right`, the values in square brackets put in first) and actions: a command in
//! SignedCommand's text or JSON form, or `lockout=<key>+<minutes>`.
//!
//! The values come from the grid: an IM's text, an avatar's name. They are put into each
//! argument of a command once the action is cut up, so that none can add a command, an argument
//! or a reply target (`~#~`, `|||`, `#|#` in a chat message stay text), and they are put in one
//! pass, so that a value holding `[avataruuid]` stays as it is. SecondBot replaces in the whole
//! action text, one value after the other, then reads it: an IM could add arguments to the
//! command it triggers. The command's name never takes a value.
//!
//! Other deliberate differences, each a SecondBot slip: `IS_UUID` (SecondBot's list spells it
//! `IS_UUD`, both are read), `IS_EVEN` and `DIVISIBLE` are read (SecondBot evaluates them but its
//! list leaves them out, so a condition naming them is dropped); a condition with no operator it
//! knows makes the event invalid (SecondBot drops the condition and runs the event without it);
//! an `AvatarIm` event's Monitor that is an avatar's key watches that avatar only (SecondBot
//! refuses a key and watches every avatar whatever it says); `botx`, `boty`, `botz` are the bot's
//! place (SecondBot's are the avatar's).

use crate::api::signing::{Unpacked, unpack};
use std::collections::HashMap;
use std::sync::Mutex;
use uuid::Uuid;

/// Where an event comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    AvatarIm,
    SimAlert,
    BalanceUpdate,
    GroupMemberJoin,
    GroupMemberLeave,
    ChangeSim,
    GuestJoins,
    GuestLeaves,
    Clock,
}

impl Source {
    /// SecondBot's names, as they are written (case counts).
    pub fn parse(s: &str) -> Option<Source> {
        Some(match s.trim() {
            "AvatarIm" => Source::AvatarIm,
            "SimAlert" => Source::SimAlert,
            "BalanceUpdate" => Source::BalanceUpdate,
            "GroupMemberJoin" => Source::GroupMemberJoin,
            "GroupMemberLeave" => Source::GroupMemberLeave,
            "ChangeSim" => Source::ChangeSim,
            "GuestJoins" => Source::GuestJoins,
            "GuestLeaves" => Source::GuestLeaves,
            "Clock" => Source::Clock,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Source::AvatarIm => "AvatarIm",
            Source::SimAlert => "SimAlert",
            Source::BalanceUpdate => "BalanceUpdate",
            Source::GroupMemberJoin => "GroupMemberJoin",
            Source::GroupMemberLeave => "GroupMemberLeave",
            Source::ChangeSim => "ChangeSim",
            Source::GuestJoins => "GuestJoins",
            Source::GuestLeaves => "GuestLeaves",
            Source::Clock => "Clock",
        }
    }
}

/// A condition's operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Is,
    Not,
    IsUuid,
    Missing,
    Contains,
    InGroup,
    NotInGroup,
    LessThan,
    MoreThan,
    Lockout,
    IsEven,
    Divisible,
}

/// The operators in the order SecondBot looks for them (the first one found in the text is the
/// condition's), then the ones its list leaves out.
const OPS: &[(&str, Op)] = &[
    ("{IS}", Op::Is),
    ("{NOT}", Op::Not),
    ("{IS_UUD}", Op::IsUuid),
    ("{MISSING}", Op::Missing),
    ("{CONTAINS}", Op::Contains),
    ("{IN_GROUP}", Op::InGroup),
    ("{NOT_IN_GROUP}", Op::NotInGroup),
    ("{LESSTHAN}", Op::LessThan),
    ("{MORETHAN}", Op::MoreThan),
    ("{LOCKOUT}", Op::Lockout),
    ("{IS_UUID}", Op::IsUuid),
    ("{IS_EVEN}", Op::IsEven),
    ("{DIVISIBLE}", Op::Divisible),
];

/// A condition: `left{OP}right`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub left: String,
    pub op: Op,
    pub right: String,
}

impl Rule {
    /// SecondBot's reading: the first operator in the text; split on it, the empty parts left
    /// out, two parts must remain; each trimmed.
    pub fn parse(text: &str) -> Option<Rule> {
        let (sep, op) = OPS.iter().find(|(sep, _)| text.contains(sep))?;
        let parts: Vec<&str> = text.split(sep).filter(|p| !p.is_empty()).collect();
        let [left, right] = parts[..] else {
            return None;
        };
        Some(Rule {
            left: left.trim().to_owned(),
            op: *op,
            right: right.trim().to_owned(),
        })
    }
}

/// C#'s int.TryParse, 0 when it fails.
fn int(s: &str) -> i64 {
    s.trim().parse::<i32>().map_or(0, i64::from)
}

/// LibreMetaverse's UUID.TryParse.
pub fn key(s: &str) -> Option<Uuid> {
    Uuid::parse_str(s.trim()).ok()
}

impl Op {
    /// Whether the condition holds, the values put in (SecondBot's EvaluateRule). `in_group`:
    /// the left key is a member of the right key's group; `locked`: the left text is locked out.
    pub fn holds(self, left: &str, right: &str, in_group: bool, locked: bool) -> bool {
        let (l, r) = (int(left), int(right));
        let right_true = right == "true";
        let both_keys = key(left).is_some() && key(right).is_some();
        match self {
            Op::Is => left == right,
            Op::Not => left != right,
            Op::IsUuid => key(left).is_some() == right_true,
            Op::Missing => !left.contains(right),
            Op::Contains => left.contains(right),
            Op::InGroup => both_keys && in_group,
            Op::NotInGroup => !(both_keys && in_group),
            Op::LessThan => l < r,
            Op::MoreThan => l > r,
            Op::Lockout => locked == right_true,
            Op::IsEven => (l != 0 && l % 2 == 0) == right_true,
            Op::Divisible => l != 0 && r != 0 && l % r == 0,
        }
    }
}

/// An action: a command (SignedCommand's form, read before any value is put in), or a lockout.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Command(Unpacked),
    /// `lockout=<key>+<minutes>`: the key locked out for that long.
    Lockout {
        key: String,
        minutes: String,
    },
}

impl Action {
    pub fn parse(text: &str) -> Option<Action> {
        // SecondBot: two parts around "=", the first "lockout", then two around "+"
        let bits: Vec<&str> = text.split('=').collect();
        if let ["lockout", rest] = bits[..] {
            let parts: Vec<&str> = rest.split('+').collect();
            return match parts[..] {
                [key, minutes] => Some(Action::Lockout {
                    key: key.to_owned(),
                    minutes: minutes.to_owned(),
                }),
                // SecondBot skips a lockout it cannot read
                _ => None,
            };
        }
        unpack(text).map(Action::Command)
    }
}

/// What an `AvatarIm` event watches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Watch {
    /// A group's key (GroupMemberJoin, GroupMemberLeave).
    Group(Uuid),
    /// The bot's region and parcel, by name, where the avatar must be too (GuestJoins,
    /// GuestLeaves).
    Place {
        region: String,
        parcel: String,
    },
    /// One avatar's IMs, by key.
    Avatar(Uuid),
    Anything,
}

/// An event, read from the settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Trigger {
    /// Its number in the settings, for the log.
    pub n: usize,
    pub source: Source,
    pub watch: Watch,
    pub rules: Vec<Rule>,
    pub actions: Vec<Action>,
}

/// An event as the settings write it.
#[derive(Debug, Clone, Default)]
pub struct Raw {
    pub source: String,
    pub monitor: String,
    pub wheres: Vec<String>,
    pub actions: Vec<String>,
}

impl Trigger {
    /// SecondBot's checks when it loads an event; why it is refused otherwise.
    pub fn parse(n: usize, raw: &Raw) -> Result<Trigger, String> {
        let source = Source::parse(&raw.source)
            .ok_or_else(|| format!("unknown source \"{}\"", raw.source))?;
        let mut rules = Vec::new();
        for w in &raw.wheres {
            rules.push(Rule::parse(w).ok_or_else(|| format!("condition \"{w}\" not valid"))?);
        }
        let flags: Vec<&str> = raw.monitor.split("=#=").collect();
        let watch = match source {
            Source::GroupMemberJoin | Source::GroupMemberLeave => match flags[..] {
                [group] => Watch::Group(key(group).ok_or("monitor flags not valid: a group key")?),
                _ => return Err("monitor flags not valid: a group key".into()),
            },
            Source::GuestJoins | Source::GuestLeaves => match flags[..] {
                [region, parcel] => Watch::Place {
                    region: region.to_owned(),
                    parcel: parcel.to_owned(),
                },
                _ => return Err("monitor flags not valid: Region=#=Parcel".into()),
            },
            Source::AvatarIm => match flags[..] {
                [who] => key(who)
                    .filter(|id| !id.is_nil())
                    .map_or(Watch::Anything, Watch::Avatar),
                _ => return Err("monitor flags not valid: Any or an avatar".into()),
            },
            _ => Watch::Anything,
        };
        let mut actions = Vec::new();
        for a in &raw.actions {
            match Action::parse(a) {
                Some(action) => actions.push(action),
                None => log::warn!("on event {n}: action \"{a}\" skipped: not a command"),
            }
        }
        if actions.is_empty() {
            return Err("no actions".into());
        }
        Ok(Trigger {
            n,
            source,
            watch,
            rules,
            actions,
        })
    }

    /// SecondBot's EventPreChecks, and the avatar an `AvatarIm` event watches.
    pub fn applies(&self, v: &Values) -> bool {
        match &self.watch {
            Watch::Group(g) => v.get("groupuuid") == g.to_string(),
            Watch::Place { region, parcel } => {
                v.get("botsim") == region
                    && v.get("botparcel") == parcel
                    && v.get("avatarparcel") == parcel
            }
            Watch::Avatar(a) => v.get("avataruuid") == a.to_string(),
            Watch::Anything => true,
        }
    }
}

/// The values of an event, by name (SecondBot's BuildEventValues).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Values(HashMap<&'static str, String>);

impl Values {
    pub fn set(&mut self, name: &'static str, value: impl Into<String>) {
        self.0.insert(name, value.into());
    }

    pub fn get(&self, name: &str) -> &str {
        self.0.get(name).map_or("", String::as_str)
    }

    /// Every `[name]` of the text replaced with its value, in one pass: a value is never read
    /// again, and an unknown name stays as written.
    pub fn put_in(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(open) = rest.find('[') {
            out.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            match after.find(']').map(|close| (&after[..close], close)) {
                Some((name, close)) if self.0.contains_key(name) => {
                    out.push_str(self.get(name));
                    rest = &after[close + 1..];
                }
                _ => {
                    out.push('[');
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }
}

/// What the world says when an event comes, for its values.
#[derive(Debug, Clone, Default)]
pub struct World {
    pub bot_sim: String,
    /// The bot's parcel's name, "-" when unknown.
    pub bot_parcel: String,
    pub bot_at: [i64; 3],
    /// The avatar the event names: its key, name, parcel ("?" when unknown) and place.
    pub avatar: Option<Uuid>,
    pub avatar_name: String,
    pub avatar_parcel: String,
    /// x, y, z and the distance to the bot, rounded; -1 when it is not seen.
    pub avatar_at: [i64; 4],
    /// The clock: hour, minute, day of the week (0 Sunday).
    pub clock: (u32, u32, u32),
}

/// The values of an event: what it says (`args`), with SecondBot's defaults for what it does not,
/// and the world around it.
pub fn values(source: Source, args: &[(&'static str, String)], w: &World) -> Values {
    let arg = |name: &str, default: &str| {
        args.iter()
            .find(|(k, _)| *k == name)
            .map_or_else(|| default.to_owned(), |(_, v)| v.clone())
    };
    let mut v = Values::default();
    v.set("alertmessage", arg("alertmessage", "none"));
    v.set("message", arg("message", "none"));
    v.set("funds", arg("funds", "-1"));
    v.set("eventype", source.name());
    v.set("groupuuid", arg("groupuuid", "none"));
    v.set("groupname", arg("groupname", "none"));
    v.set("oldsimname", arg("oldsimname", "none"));
    v.set("newsimname", arg("newsimname", "none"));
    // C#'s Replace with a null value takes the placeholder out
    v.set(
        "avataruuid",
        w.avatar.map(|a| a.to_string()).unwrap_or_default(),
    );
    v.set("avatarname", w.avatar_name.clone());
    v.set("avatarparcel", w.avatar_parcel.clone());
    v.set("botsim", w.bot_sim.clone());
    v.set("botparcel", w.bot_parcel.clone());
    for (name, value) in ["avatarx", "avatary", "avatarz", "avatardistance"]
        .into_iter()
        .zip(w.avatar_at)
    {
        v.set(name, value.to_string());
    }
    for (name, value) in ["botx", "boty", "botz"].into_iter().zip(w.bot_at) {
        v.set(name, value.to_string());
    }
    v.set("clockhour", format!("{:02}", w.clock.0));
    v.set("clockmin", format!("{:02}", w.clock.1));
    v.set("dayofweek", w.clock.2.to_string());
    v
}

/// A command ready to run: the action's parts with the values put in.
#[derive(Debug, Clone, PartialEq)]
pub struct Ready {
    pub command: String,
    pub args: Vec<String>,
    pub reply: Option<String>,
}

/// The command of an action, the values put into each of its arguments and its reply target.
pub fn ready(u: &Unpacked, v: &Values) -> Ready {
    Ready {
        command: u.command.clone(),
        args: u.args.iter().map(|a| v.put_in(a)).collect(),
        reply: u.reply.as_deref().map(|r| v.put_in(r)),
    }
}

/// The keys locked out, until when (unix seconds).
#[derive(Debug, Default)]
pub struct Lockouts(Mutex<HashMap<String, i64>>);

impl Lockouts {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, i64>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// SecondBot's IsLockedout: until the time is reached.
    pub fn locked(&self, key: &str, now: i64) -> bool {
        let mut map = self.map();
        match map.get(key) {
            Some(until) if now < *until => true,
            Some(_) => {
                map.remove(key);
                false
            }
            None => false,
        }
    }

    /// SecondBot's AddToLockout: a later end wins.
    pub fn add(&self, key: &str, minutes: i64, now: i64) {
        let until = now + minutes * 60;
        let mut map = self.map();
        let end = map.entry(key.to_owned()).or_insert(until);
        *end = (*end).max(until);
    }

    /// The ended ones forgotten (SecondBot's CleanLockouts).
    pub fn clean(&self, now: i64) {
        self.map().retain(|_, until| *until > now);
    }
}

/// The clock as SecondBot reads it (DateTime.Now: hour, minute, day of the week, 0 Sunday), on
/// the container's clock: UTC.
pub fn clock(unix: i64) -> (u32, u32, u32) {
    let secs = unix.rem_euclid(86_400);
    let days = unix.div_euclid(86_400);
    // 1970-01-01 was a Thursday
    let dow = (days + 4).rem_euclid(7);
    (
        u32::try_from(secs / 3600).unwrap_or(0),
        u32::try_from(secs % 3600 / 60).unwrap_or(0),
        u32::try_from(dow).unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions_are_read_as_secondbot_reads_them() {
        let r = Rule::parse(" [avatarname] {IS} Some Resident ").expect("a rule");
        assert_eq!(
            r,
            Rule {
                left: "[avatarname]".into(),
                op: Op::Is,
                right: "Some Resident".into()
            }
        );
        // NOT_IN_GROUP is not NOT, IN_GROUP is not IS
        assert_eq!(
            Rule::parse("a{NOT_IN_GROUP}b").map(|r| r.op),
            Some(Op::NotInGroup)
        );
        assert_eq!(Rule::parse("a{IN_GROUP}b").map(|r| r.op), Some(Op::InGroup));
        assert_eq!(Rule::parse("a{IS_UUD}true").map(|r| r.op), Some(Op::IsUuid));
        assert_eq!(
            Rule::parse("a{IS_UUID}true").map(|r| r.op),
            Some(Op::IsUuid)
        );
        assert_eq!(
            Rule::parse("4{IS_EVEN}true").map(|r| r.op),
            Some(Op::IsEven)
        );
        // an empty side, a separator twice, no operator
        assert_eq!(Rule::parse("{IS}b"), None);
        assert_eq!(Rule::parse("a{IS}b{IS}c"), None);
        assert_eq!(Rule::parse("a == b"), None);
    }

    #[test]
    fn operators_hold_as_secondbots() {
        let k = "00000000-0000-0000-0000-000000000001";
        assert!(Op::Is.holds("a", "a", false, false));
        assert!(Op::Not.holds("a", "b", false, false));
        assert!(Op::IsUuid.holds(k, "true", false, false));
        assert!(Op::IsUuid.holds("x", "false", false, false));
        assert!(Op::Contains.holds("hello there", "there", false, false));
        assert!(Op::Missing.holds("hello", "there", false, false));
        assert!(Op::InGroup.holds(k, k, true, false));
        assert!(!Op::InGroup.holds("x", k, true, false));
        assert!(Op::NotInGroup.holds(k, k, false, false));
        assert!(Op::LessThan.holds("3", "10", false, false));
        assert!(Op::MoreThan.holds(" 11", "10", false, false));
        // not a number: 0
        assert!(Op::LessThan.holds("x", "1", false, false));
        assert!(Op::Lockout.holds(k, "false", false, false));
        assert!(Op::Lockout.holds(k, "true", false, true));
        assert!(Op::IsEven.holds("4", "true", false, false));
        assert!(Op::IsEven.holds("0", "false", false, false));
        assert!(Op::Divisible.holds("15", "5", false, false));
        assert!(!Op::Divisible.holds("15", "0", false, false));
    }

    #[test]
    fn values_go_in_once_and_never_build_a_command() {
        let mut v = Values::default();
        v.set("message", "hi~#~[avataruuid]#|#http://x|||Logoff");
        v.set("avataruuid", "K");
        v.set("avatarname", "A [b] c");
        assert_eq!(
            v.put_in("[avatarname]: [message] [nothing] [x"),
            "A [b] c: hi~#~[avataruuid]#|#http://x|||Logoff [nothing] [x"
        );

        let Some(Action::Command(u)) = Action::parse("IM|||[avataruuid]~#~You said [message]")
        else {
            panic!("a command");
        };
        let r = ready(&u, &v);
        assert_eq!(r.command, "IM");
        assert_eq!(
            r.args,
            vec!["K", "You said hi~#~[avataruuid]#|#http://x|||Logoff"],
            "one argument still, no reply target"
        );
        assert_eq!(r.reply, None);

        // the JSON form, its reply target given by the settings
        let Some(Action::Command(u)) =
            Action::parse(r#"{"cmd":"Say","args":["0","[avatarname]"],"reply":"[avataruuid]"}"#)
        else {
            panic!("a command");
        };
        let r = ready(&u, &v);
        assert_eq!(
            (r.args[1].as_str(), r.reply.as_deref()),
            ("A [b] c", Some("K"))
        );

        // the command's name never takes a value
        let Some(Action::Command(u)) = Action::parse("[message]|||x") else {
            panic!("a command");
        };
        assert_eq!(ready(&u, &v).command, "[message]");
    }

    #[test]
    fn lockouts() {
        assert_eq!(
            Action::parse("lockout=[avataruuid]+5"),
            Some(Action::Lockout {
                key: "[avataruuid]".into(),
                minutes: "5".into()
            })
        );
        assert_eq!(Action::parse("lockout=a+b+c"), None);
        assert!(matches!(
            Action::parse("Say|||0~#~a=b"),
            Some(Action::Command(_))
        ));
        let l = Lockouts::default();
        l.add("k", 5, 1000);
        l.add("k", 1, 1000);
        assert!(l.locked("k", 1299));
        assert!(!l.locked("k", 1300));
        l.add("k", 1, 1000);
        l.clean(2000);
        assert!(!l.locked("k", 1001));
    }

    #[test]
    fn events_are_checked_when_loaded() {
        let raw = |source: &str, monitor: &str, wheres: &[&str]| Raw {
            source: source.into(),
            monitor: monitor.into(),
            wheres: wheres.iter().map(|w| (*w).to_owned()).collect(),
            actions: vec!["Say|||0~#~hi".into()],
        };
        let g = "00000000-0000-0000-0000-000060000001";
        let t = Trigger::parse(1, &raw("GroupMemberJoin", g, &[])).expect("valid");
        assert_eq!(t.watch, Watch::Group(Uuid::from_u128(0x6000_0001)));
        assert!(Trigger::parse(1, &raw("GroupMemberJoin", "x", &[])).is_err());
        assert!(Trigger::parse(1, &raw("GuestJoins", "Region", &[])).is_err());
        let t = Trigger::parse(1, &raw("GuestJoins", "Region=#=Parcel", &[])).expect("valid");
        assert_eq!(
            t.watch,
            Watch::Place {
                region: "Region".into(),
                parcel: "Parcel".into()
            }
        );
        assert_eq!(
            Trigger::parse(1, &raw("AvatarIm", "Any", &[])).map(|t| t.watch),
            Ok(Watch::Anything)
        );
        assert_eq!(
            Trigger::parse(1, &raw("AvatarIm", g, &[])).map(|t| t.watch),
            Ok(Watch::Avatar(Uuid::from_u128(0x6000_0001)))
        );
        assert!(Trigger::parse(1, &raw("Clock", "", &["no operator"])).is_err());
        assert!(Trigger::parse(1, &raw("Nothing", "", &[])).is_err());
        let mut no_action = raw("Clock", "", &[]);
        no_action.actions.clear();
        assert!(Trigger::parse(1, &no_action).is_err());
    }

    #[test]
    fn the_values_of_an_event() {
        let w = World {
            bot_sim: "Region".into(),
            bot_parcel: "Plaza".into(),
            bot_at: [1, 2, 3],
            avatar: Some(Uuid::from_u128(5)),
            avatar_name: "Some Resident".into(),
            avatar_parcel: "Plaza".into(),
            avatar_at: [10, 20, 30, 4],
            clock: (7, 5, 0),
        };
        let v = values(Source::AvatarIm, &[("message", "hello".into())], &w);
        assert_eq!(v.get("message"), "hello");
        assert_eq!(v.get("alertmessage"), "none");
        assert_eq!(v.get("funds"), "-1");
        assert_eq!(v.get("eventype"), "AvatarIm");
        assert_eq!(v.get("avataruuid"), "00000000-0000-0000-0000-000000000005");
        assert_eq!((v.get("avatarx"), v.get("avatardistance")), ("10", "4"));
        assert_eq!(v.get("botz"), "3");
        assert_eq!((v.get("clockhour"), v.get("clockmin")), ("07", "05"));
        assert_eq!(v.get("dayofweek"), "0");

        let t = Trigger::parse(
            1,
            &Raw {
                source: "GuestJoins".into(),
                monitor: "Region=#=Plaza".into(),
                actions: vec!["Hello".into()],
                ..Raw::default()
            },
        )
        .expect("valid");
        assert!(t.applies(&v));
        let mut away = v.clone();
        away.set("avatarparcel", "?");
        assert!(!t.applies(&away));
    }

    #[test]
    fn the_clock_in_utc() {
        // 2026-11-13 12:34 UTC was a Friday
        assert_eq!(clock(1_794_573_240), (12, 34, 5));
        assert_eq!(clock(0), (0, 0, 4));
    }
}
