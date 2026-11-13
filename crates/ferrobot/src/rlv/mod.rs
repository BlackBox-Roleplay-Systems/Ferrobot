//! RLV (RestrainedLove): objects drive the bot with "@" commands said to their owner
//! (llOwnerSay), as SecondBot's RLVService (Services/RLVService.cs, after Radegast's RLV
//! manager). Here: the commands read from a line, the restrictions kept per object, and what each
//! other command asks the bot to do ([`Action`]); services/rlv.rs does it, folders.rs reads the
//! shared folder (#RLV).
//!
//! A line is "@" then commands separated by commas; each is `behaviour[:option]=param`, read as
//! SecondBot's regular expression `(?<behaviour>[^:=]+)(:(?<option>[^=]*))?=(?<param>\w+)` reads
//! it (from the start of the command), the behaviour and the param in lower case, the option as
//! sent. `=n` (or `=add`) keeps a restriction, `=y` (or `=rem`) lifts it (all of that behaviour's
//! from that object when no option is given), `=force` acts, a channel number asks for an answer
//! on that channel. "@clear" lifts every restriction of the object.
//!
//! Deliberate differences, each where it is done:
//! - the version answers name Ferrobot, not "SecondBot Events";
//! - the bounds of Aurora's RLVa (a line of 4096 bytes, 256 commands, 256 restrictions an object,
//!   1024 objects), and no answer on DEBUG_CHANNEL (2147483647), as the viewer;
//! - names of attachment points, clothing layers and groups compare without case, as the viewer
//!   (it folds the whole line): SecondBot compares the option as sent with lower-case names;
//! - @getpath and @getpathnew answer the folder's path under #RLV, as the viewer, without
//!   "getpathnew: " before it and without the item's name after it;
//! - @findfolder and @findfolders find folders whose name holds every word, as the viewer
//!   (Aurora's RLVa), and answer their paths (SecondBot compares the words with whole folder names
//!   and answers a bare name);
//! - the restrictions of an object gone from the region are dropped every 2 minutes, as
//!   SecondBot's cleanup timer meant to (it is never started there).

pub mod folders;

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};
use uuid::Uuid;

/// What @version and @versionnew answer: SecondBot's texts, Ferrobot's name.
pub const VERSION: &str = "RestrainedLife viewer v1.23 (Ferrobot)";
pub const VERSION_NEW: &str = "RestrainedLove viewer v1.23 (Ferrobot)";
/// What @versionnum answers (SecondBot's).
pub const VERSION_NUM: &str = "1230100";

/// Aurora's RLVa bounds (world/rlva.rs, Rlva::receive and Rlva::process): a line, the commands
/// read from it, the restrictions of one object, the objects that keep some.
pub const MAX_LINE: usize = 4096;
const MAX_COMMANDS: usize = 256;
const MAX_RULES_PER_OBJECT: usize = 256;
const MAX_OBJECTS: usize = 1024;
/// The viewer never answers on DEBUG_CHANNEL (Aurora's Rlva::reply).
const DEBUG_CHANNEL: i32 = i32::MAX;
/// Cleanups an object never seen in the region keeps its restrictions through (Aurora's
/// RlvHandler::onGC gives such an object twenty lookups, 30 s apart; at SecondBot's 2 minutes,
/// five make ten minutes). One seen once loses them the first time it is gone.
const UNSEEN_ROUNDS: u32 = 5;

/// LibreMetaverse's WearableType texts in lower case, by LLWearableType number: SecondBot's
/// RLVWearables, one digit each in @getoutfit's answer.
pub const LAYERS: [&str; 17] = [
    "shape",
    "skin",
    "hair",
    "eyes",
    "shirt",
    "pants",
    "shoes",
    "socks",
    "jacket",
    "gloves",
    "undershirt",
    "underpants",
    "skirt",
    "alpha",
    "tattoo",
    "physics",
    "universal",
];

/// LibreMetaverse's AttachmentPoint texts in lower case, points 1 to 55: SecondBot's
/// RLVAttachments are "none" (point 0) then these, the HUD points left out.
const POINTS: [&str; 55] = [
    "chest",
    "head",
    "left shoulder",
    "right shoulder",
    "left hand",
    "right hand",
    "left foot",
    "right foot",
    "back",
    "pelvis",
    "mouth",
    "chin",
    "left ear",
    "right ear",
    "left eye",
    "right eye",
    "nose",
    "right upper arm",
    "right lower arm",
    "left upper arm",
    "left lower arm",
    "right hip",
    "right upper leg",
    "right lower leg",
    "left hip",
    "left upper leg",
    "left lower leg",
    "belly",
    "left pec",
    "right pec",
    "hud center 2",
    "hud top right",
    "hud top center",
    "hud top left",
    "hud center 1",
    "hud bottom left",
    "hud bottom",
    "hud bottom right",
    "neck",
    "avatar center",
    "left ring finger",
    "right ring finger",
    "tail base",
    "tail tip",
    "left wing",
    "right wing",
    "jaw",
    "alt left ear",
    "alt right ear",
    "alt left eye",
    "alt right eye",
    "tongue",
    "groin",
    "left hind foot",
    "right hind foot",
];

/// SecondBot's RLVAttachments, in order: "none" (point 0), then the points that are not a HUD's.
pub fn points() -> impl Iterator<Item = (u8, &'static str)> {
    std::iter::once((0, "none")).chain(
        (1u8..)
            .zip(POINTS)
            .filter(|(_, name)| !name.starts_with("hud")),
    )
}

/// An attachment point by its RLV name (AttachmentPointFromString), whatever its case.
pub fn point(name: &str) -> Option<u8> {
    points()
        .find(|(_, n)| n.eq_ignore_ascii_case(name))
        .map(|(p, _)| p)
}

/// A clothing layer's LLWearableType by its RLV name (WearableFromString), whatever its case.
pub fn layer(name: &str) -> Option<u32> {
    (0u32..)
        .zip(LAYERS)
        .find(|(_, n)| n.eq_ignore_ascii_case(name))
        .map(|(t, _)| t)
}

/// @getoutfit's answer: whether a layer is worn ("1" or "0", "0" for a name it does not know),
/// else a digit for each layer of [`LAYERS`].
pub fn outfit_status(worn: &HashSet<u32>, name: &str) -> String {
    let on = |t: u32| if worn.contains(&t) { "1" } else { "0" };
    if name.is_empty() {
        (0u32..).take(LAYERS.len()).map(on).collect()
    } else {
        layer(name).map_or("0", on).to_owned()
    }
}

/// @getattach's answer: nothing when nothing is worn; else whether a point is worn on ("1" or
/// "0", a name it does not know being point 0), else a digit for each of [`points`].
pub fn attach_status(worn: &HashSet<u8>, name: &str) -> String {
    if worn.is_empty() {
        return String::new();
    }
    let on = |p: u8| if worn.contains(&p) { "1" } else { "0" };
    if name.is_empty() {
        points().map(|(p, _)| on(p)).collect()
    } else {
        on(point(name).unwrap_or(0)).to_owned()
    }
}

/// One command of a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub behaviour: String,
    pub option: String,
    pub param: String,
}

/// A command as SecondBot's regular expression reads it: the behaviour up to ':' or '=', the
/// option up to '=', the param the word after it; `None` without a behaviour or a param.
pub fn parse(text: &str) -> Option<Command> {
    let (left, right) = text.split_once('=')?;
    let (behaviour, option) = left.split_once(':').unwrap_or((left, ""));
    // .NET's \w: letters, digits, the underscore
    let param: String = right
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if behaviour.is_empty() || param.is_empty() {
        return None;
    }
    Some(Command {
        behaviour: behaviour.to_lowercase(),
        option: option.to_owned(),
        param: param.to_lowercase(),
    })
}

/// A restriction an object keeps on the bot (SecondBot's RLVRule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub behaviour: String,
    pub option: String,
    pub sender: Uuid,
    pub sender_name: String,
}

/// What a command asks the bot to do, beyond its restrictions.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Say this on a channel (SecondBot's Respond: normal chat).
    Reply { channel: i32, text: String },
    /// @getgroup: the active group's name.
    GetGroup { channel: i32 },
    /// @setgroup:<name>=force.
    SetGroup { name: String },
    /// @getpath (the asking attachment) and @getpathnew:<attachment's key>.
    GetPath {
        channel: i32,
        object: Uuid,
        new: bool,
    },
    /// @getsitid: the seat's key, or the nil key.
    GetSitId { channel: i32 },
    /// @sit[:<key>]=force, through the Sit command (the nil key: the ground).
    Sit { target: Uuid },
    /// @unsit=force, through the Stand command.
    Unsit,
    /// @setrot:<radians>=force: face that way, clockwise from north.
    SetRot { radians: f64 },
    /// @tpto:<x>/<y>/<z>=force, global coordinates.
    TeleportGlobal { x: f64, y: f64, z: f32 },
    /// @tpto:<region>/<x>/<y>/<z>[;lookat]=force.
    TeleportRegion {
        region: String,
        x: f32,
        y: f32,
        z: f32,
    },
    /// @getoutfit[:<layer>].
    GetOutfit { channel: i32, layer: String },
    /// @getattach[:<point>].
    GetAttach { channel: i32, point: String },
    /// @detach[:<point or folder>]=force and @remattach: one attachment at a point, the items of
    /// a folder under #RLV, or every attachment (`what` empty).
    Detach { what: String },
    /// @detachall:<folder>=force: the items of a folder and its subfolders.
    DetachAll { folder: String },
    /// @detachallthis=force: the items of the folder the asking attachment came from.
    DetachThis { object: Uuid },
    /// @remoutfit:<layer>=force.
    RemOutfit { layer: String },
    /// @attach…:<folder>=force: its items worn, with those of its subfolders (`recursive`), in
    /// the place of what is worn (`replace`) or added to it.
    Attach {
        folder: String,
        recursive: bool,
        replace: bool,
    },
    /// @getinv[:<folder>]: the subfolders' names.
    GetInv { channel: i32, folder: String },
    /// @getinvworn[:<folder>]: what of each subfolder is worn.
    GetInvWorn { channel: i32, folder: String },
    /// @findfolder and @findfolders:<word>&&<word>…
    FindFolder {
        channel: i32,
        terms: Vec<String>,
        all: bool,
    },
}

/// The restrictions objects keep on the bot, and which objects the cleanups have looked for.
#[derive(Default)]
pub struct Rules {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// In the order they came, as SecondBot's list (@getstatus answers in that order).
    rules: Vec<Rule>,
    /// By object: seen in the region once, and the cleanups it was missing from since.
    lookups: HashMap<Uuid, (bool, u32)>,
}

impl Rules {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A line an object said to the bot (SecondBot's TryProcessCMD): its restrictions kept or
    /// lifted at once, and what else it asks for, in order. A line that does not start with "@",
    /// or is too long, does nothing.
    pub fn handle(&self, sender: Uuid, sender_name: &str, message: &str) -> Vec<Action> {
        let Some(body) = message.strip_prefix('@') else {
            return Vec::new();
        };
        if message.len() > MAX_LINE {
            return Vec::new();
        }
        if message == "@clear" {
            self.clear(sender);
            return Vec::new();
        }
        let mut out = Vec::new();
        for text in body.split(',').take(MAX_COMMANDS) {
            let Some(c) = parse(text) else {
                continue;
            };
            log::debug!(
                "rlv: {sender_name}: {}:{}={} [{sender}]",
                c.behaviour,
                c.option,
                c.param
            );
            match c.param.as_str() {
                "n" | "add" => {
                    self.add(Rule {
                        behaviour: c.behaviour,
                        option: c.option,
                        sender,
                        sender_name: sender_name.to_owned(),
                    });
                }
                "y" | "rem" => self.remove(sender, &c.behaviour, &c.option),
                _ => out.extend(self.action(sender, &c)),
            }
        }
        out
    }

    /// A restriction kept: the same one from the same object again goes to the end.
    fn add(&self, rule: Rule) {
        let mut inner = self.lock();
        let rules = &mut inner.rules;
        rules.retain(|r| {
            !(r.behaviour == rule.behaviour && r.sender == rule.sender && r.option == rule.option)
        });
        let own = rules.iter().filter(|r| r.sender == rule.sender).count();
        let objects: HashSet<Uuid> = rules.iter().map(|r| r.sender).collect();
        if own >= MAX_RULES_PER_OBJECT || (own == 0 && objects.len() >= MAX_OBJECTS) {
            log::warn!("rlv: too many restrictions, {} ignored", rule.behaviour);
            return;
        }
        rules.push(rule);
    }

    /// A restriction lifted; without an option, every one of that behaviour from that object
    /// (SecondBot's).
    fn remove(&self, sender: Uuid, behaviour: &str, option: &str) {
        self.lock().rules.retain(|r| {
            !(r.behaviour == behaviour
                && r.sender == sender
                && (option.is_empty() || r.option == option))
        });
    }

    /// Every restriction of an object lifted (@clear, SecondBot's Clear).
    pub fn clear(&self, sender: Uuid) {
        let mut inner = self.lock();
        inner.rules.retain(|r| r.sender != sender);
        inner.lookups.remove(&sender);
    }

    /// @getstatus (one object's restrictions, `sender`) and @getstatusall: the option is
    /// "<part of a behaviour>;<separator>", the separator "/" by default; each restriction is
    /// the separator, the behaviour, and ":<option>" when it has one.
    pub fn status(&self, sender: Option<Uuid>, option: &str) -> String {
        let mut separator = "/".to_owned();
        let mut filter = String::new();
        if !option.is_empty() {
            let parts: Vec<&str> = option.split(';').collect();
            if let Some(c) = parts.get(1).and_then(|p| p.chars().next()) {
                separator = c.to_string();
            }
            filter = parts[0].to_lowercase();
        }
        let mut out = String::new();
        for r in self
            .lock()
            .rules
            .iter()
            .filter(|r| sender.is_none_or(|s| r.sender == s) && r.behaviour.contains(&filter))
        {
            out.push_str(&separator);
            out.push_str(&r.behaviour);
            if !r.option.is_empty() {
                out.push(':');
                out.push_str(&r.option);
            }
        }
        out
    }

    /// Whether an attachment keeps itself on (@detach=n from it): RLV's own detaches leave it
    /// (SecondBot's AllowDetach).
    pub fn locked(&self, object: Uuid) -> bool {
        self.lock()
            .rules
            .iter()
            .any(|r| r.behaviour == "detach" && r.sender == object)
    }

    /// The objects that keep restrictions.
    pub fn senders(&self) -> Vec<Uuid> {
        let mut out: Vec<Uuid> = Vec::new();
        for r in &self.lock().rules {
            if !out.contains(&r.sender) {
                out.push(r.sender);
            }
        }
        out
    }

    /// A cleanup (SecondBot's CleanupTimer_Elapsed, Aurora's RlvHandler::onGC): `present` says
    /// which objects the region shows. An object seen once and now gone, or never seen in
    /// [`UNSEEN_ROUNDS`] cleanups, loses its restrictions.
    pub fn collect(&self, present: &HashMap<Uuid, bool>) {
        let mut gone = Vec::new();
        {
            let mut inner = self.lock();
            for (id, here) in present {
                let entry = inner.lookups.entry(*id).or_default();
                if *here {
                    *entry = (true, 0);
                } else {
                    entry.1 = entry.1.saturating_add(1);
                    if entry.0 || entry.1 > UNSEEN_ROUNDS {
                        gone.push(*id);
                    }
                }
            }
        }
        for id in gone {
            log::debug!("rlv: {id} is gone, its restrictions with it");
            self.clear(id);
        }
        let mut inner = self.lock();
        let senders: HashSet<Uuid> = inner.rules.iter().map(|r| r.sender).collect();
        inner.lookups.retain(|id, _| senders.contains(id));
    }

    /// What a command that is not a restriction asks for (the switch of SecondBot's
    /// TryProcessCMD); `None` for what it does not know or a param it does not take.
    fn action(&self, sender: Uuid, c: &Command) -> Option<Action> {
        // int.TryParse(param) and above 0
        let channel = || {
            c.param
                .parse::<i32>()
                .ok()
                .filter(|ch| *ch > 0 && *ch != DEBUG_CHANNEL)
        };
        let force = c.param == "force";
        let option = c.option.clone();
        let reply = |text: &str| {
            channel().map(|channel| Action::Reply {
                channel,
                text: text.to_owned(),
            })
        };
        match c.behaviour.as_str() {
            "version" => reply(VERSION),
            "versionnew" => reply(VERSION_NEW),
            "versionnum" => reply(VERSION_NUM),
            "getgroup" => channel().map(|channel| Action::GetGroup { channel }),
            "setgroup" if force => Some(Action::SetGroup { name: option }),
            "getpath" => channel().map(|channel| Action::GetPath {
                channel,
                object: sender,
                new: false,
            }),
            "getpathnew" => {
                let channel = channel()?;
                let object = Uuid::parse_str(option.trim())
                    .ok()
                    .filter(|id| !id.is_nil())?;
                Some(Action::GetPath {
                    channel,
                    object,
                    new: true,
                })
            }
            "getsitid" => channel().map(|channel| Action::GetSitId { channel }),
            "getstatus" | "getstatusall" => {
                let channel = channel()?;
                let of = (c.behaviour == "getstatus").then_some(sender);
                Some(Action::Reply {
                    channel,
                    text: self.status(of, &option),
                })
            }
            // a key that does not read is the nil key: the ground (SecondBot's)
            "sit" if force => Some(Action::Sit {
                target: Uuid::parse_str(option.trim()).unwrap_or_default(),
            }),
            "unsit" if force => Some(Action::Unsit),
            "setrot" if force => option
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|r| r.is_finite())
                .map(|radians| Action::SetRot { radians }),
            "tpto" if force => teleport(&option),
            "getoutfit" => channel().map(|channel| Action::GetOutfit {
                channel,
                layer: option,
            }),
            "getattach" => channel().map(|channel| Action::GetAttach {
                channel,
                point: option,
            }),
            "remattach" | "detach" if force => Some(Action::Detach { what: option }),
            "detachall" if force && !option.is_empty() => {
                Some(Action::DetachAll { folder: option })
            }
            "detachallthis" if force => Some(Action::DetachThis { object: sender }),
            "remoutfit" if force && !option.is_empty() => Some(Action::RemOutfit { layer: option }),
            "attach" | "attachoverorreplace" | "attachover" | "attachall" | "attachallover"
                if force && !option.is_empty() =>
            {
                Some(Action::Attach {
                    folder: option,
                    recursive: c.behaviour.starts_with("attachall"),
                    replace: !c.behaviour.ends_with("over"),
                })
            }
            "getinv" => channel().map(|channel| Action::GetInv {
                channel,
                folder: option,
            }),
            "getinvworn" => channel().map(|channel| Action::GetInvWorn {
                channel,
                folder: option,
            }),
            "findfolder" | "findfolders" => channel().map(|channel| Action::FindFolder {
                channel,
                terms: option
                    .split("&&")
                    .filter(|t| !t.is_empty())
                    .map(str::to_lowercase)
                    .collect(),
                all: c.behaviour == "findfolders",
            }),
            _ => None,
        }
    }
}

/// @tpto's option, as SecondBot splits it at '/' and ';': three numbers are global coordinates,
/// four or five a region's name and a place in it (the look-at is not used). A number that does
/// not read drops the teleport.
fn teleport(option: &str) -> Option<Action> {
    let parts: Vec<&str> = option.split(['/', ';']).collect();
    let n = |s: &str| s.trim().parse::<f32>().ok().filter(|v| v.is_finite());
    match parts.as_slice() {
        [x, y, z] => {
            let wide = |s: &str| s.trim().parse::<f64>().ok().filter(|v| v.is_finite());
            Some(Action::TeleportGlobal {
                x: wide(x)?,
                y: wide(y)?,
                z: n(z)?,
            })
        }
        [region, x, y, z] | [region, x, y, z, _] => Some(Action::TeleportRegion {
            region: (*region).to_owned(),
            x: n(x)?,
            y: n(y)?,
            z: n(z)?,
        }),
        _ => None,
    }
}

/// Global coordinates as a region's handle and a place in it (LibreMetaverse's
/// Helpers.GlobalPosToRegionHandle: the region's corner on a 256 m grid); `None` off the grid.
pub fn global_to_region(x: f64, y: f64) -> Option<(u64, f32, f32)> {
    let limit = f64::from(u32::MAX);
    if !(0.0..limit).contains(&x) || !(0.0..limit).contains(&y) {
        return None;
    }
    // in range: whole metres fit a u32
    let corner = |v: f64| ((v as u32) / 256) * 256;
    let (cx, cy) = (corner(x), corner(y));
    let handle = (u64::from(cx) << 32) | u64::from(cy);
    Some((
        handle,
        (x - f64::from(cx)) as f32,
        (y - f64::from(cy)) as f32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OBJECT: Uuid = Uuid::from_u128(0x0b1e_c701);
    const OTHER: Uuid = Uuid::from_u128(0x0b1e_c702);

    #[test]
    fn commands_read_as_secondbots_expression() {
        assert_eq!(
            parse("Detach:Left Hand=N"),
            Some(Command {
                behaviour: "detach".into(),
                option: "Left Hand".into(),
                param: "n".into()
            })
        );
        let tp = parse("tpto:Some Region/1/2/3;0=force").expect("a command");
        assert_eq!(tp.option, "Some Region/1/2/3;0");
        assert_eq!(tp.param, "force");
        // the param is the word after '=': "123" of "123)", nothing of "-5"
        assert_eq!(parse("version=123)").map(|c| c.param), Some("123".into()));
        assert_eq!(parse("version=-5"), None);
        assert_eq!(parse("version"), None);
        assert_eq!(parse("=n"), None);
        assert_eq!(parse(":x=n"), None);
        // the option may hold ':' but not '='
        assert_eq!(parse("a:b:c=y").map(|c| c.option), Some("b:c".into()));
    }

    #[test]
    fn restrictions_are_kept_lifted_and_cleared() {
        let rules = Rules::default();
        assert!(
            rules
                .handle(OBJECT, "Collar", "@detach=n,sendchat=n,tplure:a=add")
                .is_empty()
        );
        rules.handle(OTHER, "Cuffs", "@fly=n");
        assert_eq!(rules.status(Some(OBJECT), ""), "/detach/sendchat/tplure:a");
        assert_eq!(rules.status(None, ""), "/detach/sendchat/tplure:a/fly");
        // a filter and a separator
        assert_eq!(rules.status(None, "tp;|"), "|tplure:a");
        assert_eq!(rules.status(None, ";,"), ",detach,sendchat,tplure:a,fly");
        // the same restriction again goes to the end
        rules.handle(OBJECT, "Collar", "@detach=n");
        assert_eq!(rules.status(Some(OBJECT), ""), "/sendchat/tplure:a/detach");
        // =y without an option lifts all of that behaviour, with one only that one
        rules.handle(OBJECT, "Collar", "@tplure:b=n,tplure:a=rem");
        assert_eq!(rules.status(Some(OBJECT), ""), "/sendchat/detach/tplure:b");
        rules.handle(OBJECT, "Collar", "@tplure=y");
        assert_eq!(rules.status(Some(OBJECT), ""), "/sendchat/detach");
        assert!(rules.locked(OBJECT));
        assert!(!rules.locked(OTHER));
        // @clear: all of that object's
        rules.handle(OBJECT, "Collar", "@clear");
        assert_eq!(rules.status(None, ""), "/fly");
        assert!(!rules.locked(OBJECT));
        assert_eq!(rules.senders(), vec![OTHER]);
        // not a command line
        rules.handle(OTHER, "Cuffs", "fly=y");
        assert_eq!(rules.status(None, ""), "/fly");
    }

    #[test]
    fn queries_answer_on_their_channel() {
        let rules = Rules::default();
        rules.handle(OBJECT, "Collar", "@detach=n");
        let reply = |channel, text: &str| Action::Reply {
            channel,
            text: text.into(),
        };
        assert_eq!(
            rules.handle(
                OBJECT,
                "Collar",
                "@version=11,versionnew=12,versionnum=13,getstatus=14,getstatusall:det=15"
            ),
            vec![
                reply(11, VERSION),
                reply(12, VERSION_NEW),
                reply(13, VERSION_NUM),
                reply(14, "/detach"),
                reply(15, "/detach"),
            ]
        );
        // channel 0, a negative one, DEBUG_CHANNEL and words: no answer
        assert!(
            rules
                .handle(
                    OBJECT,
                    "Collar",
                    "@version=0,version=abc,version=2147483647,version=-3"
                )
                .is_empty()
        );
        // a restriction then a query in one line: the query sees it
        assert_eq!(
            rules.handle(OTHER, "Cuffs", "@fly=n,getstatus=5"),
            vec![reply(5, "/fly")]
        );
    }

    #[test]
    fn forced_actions() {
        let rules = Rules::default();
        let one = |line: &str| {
            let mut a = rules.handle(OBJECT, "Collar", line);
            assert_eq!(a.len(), 1, "{line}: {a:?}");
            a.remove(0)
        };
        assert_eq!(
            one("@sit=force"),
            Action::Sit {
                target: Uuid::nil()
            }
        );
        assert_eq!(
            one(&format!("@sit:{OTHER}=force")),
            Action::Sit { target: OTHER }
        );
        assert_eq!(one("@unsit=force"), Action::Unsit);
        assert_eq!(one("@setrot:1.5=force"), Action::SetRot { radians: 1.5 });
        assert_eq!(
            one("@tpto:256100.5/256200/30=force"),
            Action::TeleportGlobal {
                x: 256_100.5,
                y: 256_200.0,
                z: 30.0
            }
        );
        assert_eq!(
            one("@tpto:Some Region/10/20/30;1.5=force"),
            Action::TeleportRegion {
                region: "Some Region".into(),
                x: 10.0,
                y: 20.0,
                z: 30.0
            }
        );
        assert_eq!(
            one("@attachallover:Gear/Hats=force"),
            Action::Attach {
                folder: "Gear/Hats".into(),
                recursive: true,
                replace: false
            }
        );
        assert_eq!(
            one("@attachoverorreplace:Gear=force"),
            Action::Attach {
                folder: "Gear".into(),
                recursive: false,
                replace: true
            }
        );
        assert_eq!(
            one("@detachallthis=force"),
            Action::DetachThis { object: OBJECT }
        );
        assert_eq!(
            one("@findfolders:Red&&HAT&&=9"),
            Action::FindFolder {
                channel: 9,
                terms: vec!["red".into(), "hat".into()],
                all: true
            }
        );
        assert_eq!(
            one(&format!("@getpathnew:{OTHER}=3")),
            Action::GetPath {
                channel: 3,
                object: OTHER,
                new: true
            }
        );
        // nothing of: a param they do not take, numbers that do not read, a missing folder,
        // a nil or missing key, an unknown behaviour
        for line in [
            "@sit=1",
            "@unsit=n",
            "@setrot:north=force",
            "@tpto:1/2=force",
            "@tpto:Region/a/2/3=force",
            "@detachall=force",
            "@remoutfit=force",
            "@attach=force",
            "@getpathnew=3",
            "@getpathnew:00000000-0000-0000-0000-000000000000=3",
            "@dance=force",
        ] {
            assert!(rules.handle(OBJECT, "Collar", line).is_empty(), "{line}");
        }
        // none of those kept a restriction
        assert_eq!(rules.status(None, ""), "/unsit");
    }

    #[test]
    fn bounds() {
        let rules = Rules::default();
        let long = format!("@{}", "a=n,".repeat(MAX_LINE));
        rules.handle(OBJECT, "Collar", &long);
        assert!(rules.senders().is_empty());
        let many: Vec<String> = (0..300).map(|i| format!("b{i}=n")).collect();
        rules.handle(OBJECT, "Collar", &format!("@{}", many.join(",")));
        assert_eq!(
            rules.status(None, "").matches('/').count(),
            MAX_RULES_PER_OBJECT
        );
    }

    #[test]
    fn cleanups_drop_the_objects_gone() {
        let rules = Rules::default();
        rules.handle(OBJECT, "Collar", "@detach=n");
        rules.handle(OTHER, "Cuffs", "@fly=n");
        // the collar is seen, the cuffs are not
        rules.collect(&HashMap::from([(OBJECT, true), (OTHER, false)]));
        assert_eq!(rules.senders(), vec![OBJECT, OTHER]);
        // the collar gone: dropped at once; the cuffs, never seen, after their rounds
        rules.collect(&HashMap::from([(OBJECT, false), (OTHER, false)]));
        assert_eq!(rules.senders(), vec![OTHER]);
        for _ in 0..UNSEEN_ROUNDS {
            rules.collect(&HashMap::from([(OTHER, false)]));
        }
        assert!(rules.senders().is_empty());
    }

    #[test]
    fn layers_and_points() {
        assert_eq!(layer("Socks"), Some(7));
        assert_eq!(layer("universal"), Some(16));
        assert_eq!(layer("hat"), None);
        assert_eq!(point("head"), Some(2));
        assert_eq!(point("Left Hand"), Some(5));
        assert_eq!(point("none"), Some(0));
        // no HUD point
        assert_eq!(point("hud center 1"), None);
        assert_eq!(points().count(), 48);
        let worn = HashSet::from([0, 4, 7]);
        assert_eq!(outfit_status(&worn, ""), "10001001000000000");
        assert_eq!(outfit_status(&worn, "socks"), "1");
        assert_eq!(outfit_status(&worn, "pants"), "0");
        assert_eq!(outfit_status(&worn, "hat"), "0");
        assert_eq!(attach_status(&HashSet::new(), ""), "");
        let on = HashSet::from([2, 39]);
        let all = attach_status(&on, "");
        assert_eq!(all.len(), 48);
        assert_eq!(&all[..3], "001");
        // the neck (39) comes after the 30 body points, the HUDs left out
        assert_eq!(all.char_indices().filter(|(_, c)| *c == '1').count(), 2);
        assert_eq!(all.as_bytes()[31], b'1');
        assert_eq!(attach_status(&on, "Head"), "1");
        assert_eq!(attach_status(&on, "chest"), "0");
    }

    #[test]
    fn global_coordinates() {
        assert_eq!(
            global_to_region(256_100.5, 256_200.0),
            Some(((256_000u64 << 32) | 256_000, 100.5, 200.0))
        );
        assert_eq!(global_to_region(-1.0, 5.0), None);
    }
}
