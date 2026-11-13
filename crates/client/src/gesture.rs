//! Port of aurora-assets/src/gesture.rs (Aurora Viewer, see NOTICE.md): a gesture asset,
//! LLMultiGesture v2 written by LLDataPackerAsciiBuffer (llcharacter, originally LGPL 2.1). Its
//! values are separated by newlines, without labels; a final NUL comes with older uploads.

use uuid::Uuid;

/// An animation step's flag: stop it (ANIM_FLAG_STOP).
pub const ANIM_STOP: u32 = 1;
/// A wait step's flags (WAIT_FLAG_TIME, WAIT_FLAG_ALL_ANIM).
pub const WAIT_TIME: u32 = 1;
pub const WAIT_ANIMATIONS: u32 = 2;
const MAX_STEPS: usize = 1024;
const MAX_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Gesture {
    pub key: u8,
    pub mask: u32,
    pub trigger: String,
    pub replacement: String,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Animation {
        name: String,
        asset: Uuid,
        flags: u32,
    },
    Sound {
        name: String,
        asset: Uuid,
        flags: u32,
    },
    Chat {
        text: String,
        flags: u32,
    },
    Wait {
        seconds: f32,
        flags: u32,
    },
}

impl Gesture {
    pub fn decode(data: &[u8]) -> Result<Gesture, String> {
        let bad = || "malformed gesture".to_owned();
        if data.len() > MAX_BYTES {
            return Err("gesture too large".into());
        }
        let data = data.strip_suffix(&[0]).unwrap_or(data);
        let text = std::str::from_utf8(data).map_err(|_| bad())?;
        if text.contains('\0') {
            return Err(bad());
        }
        let mut lines = text.lines();
        let mut next = || lines.next().ok_or_else(|| "truncated gesture".to_owned());
        if next()?.trim() != "2" {
            return Err("unsupported gesture version".into());
        }
        let key = next()?.trim().parse().map_err(|_| bad())?;
        let mask = next()?.trim().parse().map_err(|_| bad())?;
        let trigger = next()?.to_owned();
        let replacement = next()?.to_owned();
        let count: usize = next()?.trim().parse().map_err(|_| bad())?;
        if count > MAX_STEPS {
            return Err("too many gesture steps".into());
        }
        let mut steps = Vec::with_capacity(count);
        for _ in 0..count {
            let kind = next()?.trim().parse::<u32>().map_err(|_| bad())?;
            let step = match kind {
                0 | 1 => {
                    let name = next()?.to_owned();
                    let asset = Uuid::parse_str(next()?.trim()).map_err(|_| bad())?;
                    let flags = next()?.trim().parse().map_err(|_| bad())?;
                    if kind == 0 {
                        Step::Animation { name, asset, flags }
                    } else {
                        Step::Sound { name, asset, flags }
                    }
                }
                2 => Step::Chat {
                    text: next()?.to_owned(),
                    flags: next()?.trim().parse().map_err(|_| bad())?,
                },
                3 => {
                    let seconds: f32 = next()?.trim().parse().map_err(|_| bad())?;
                    if !seconds.is_finite() || seconds < 0.0 {
                        return Err(bad());
                    }
                    Step::Wait {
                        seconds,
                        flags: next()?.trim().parse().map_err(|_| bad())?,
                    }
                }
                _ => return Err("unsupported gesture step".into()),
            };
            steps.push(step);
        }
        Ok(Gesture {
            key,
            mask,
            trigger,
            replacement,
            steps,
        })
    }
}

/// A chat step's channel and text: "/5 hello" goes on channel 5, as the nearby chat bar sends it
/// (LLChatBar::stripChannelNumber).
pub fn chat_channel(text: &str) -> (i32, &str) {
    if let Some(rest) = text.strip_prefix('/') {
        let digits = rest
            .find(|c: char| !c.is_ascii_digit() && c != '-')
            .unwrap_or(rest.len());
        if digits > 0
            && let Ok(channel) = rest[..digits].parse::<i32>()
        {
            return (channel, rest[digits..].trim_start());
        }
    }
    (0, text)
}

/// The longest a gesture waits for its animations to end (WAIT_ANIMATIONS, and before it stops
/// them at its end).
const ANIMATIONS_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

impl crate::Client {
    /// Play a gesture asset as LLGestureMgr does: download it (ViewerAsset), then its steps in
    /// turn; at its end the animations it started are stopped once they have played
    /// (stopGesture). Answers once it is downloaded; the steps run on.
    pub async fn play_gesture(&self, asset: Uuid) -> Result<(), crate::ClientError> {
        let data = self.viewer_asset("gesture_id", asset).await?;
        let gesture = Gesture::decode(&data).map_err(crate::ClientError::Refused)?;
        let client = self.clone();
        tokio::spawn(async move { client.run_gesture(gesture).await });
        Ok(())
    }

    async fn run_gesture(&self, g: Gesture) {
        let mut started: Vec<Uuid> = Vec::new();
        for step in g.steps {
            match step {
                Step::Animation { asset, flags, .. } => {
                    let start = flags & ANIM_STOP == 0;
                    let _ = self.animate(vec![(asset, start)]).await;
                    if start {
                        started.push(asset);
                    } else {
                        started.retain(|a| *a != asset);
                    }
                }
                Step::Sound { asset, .. } => {
                    let _ = self.trigger_sound(asset).await;
                }
                Step::Chat { text, .. } => {
                    let (channel, said) = chat_channel(&text);
                    if !said.is_empty() {
                        let _ = self
                            .chat(said, channel, crate::events::chat_type::SAY)
                            .await;
                    }
                }
                Step::Wait { seconds, flags } => {
                    if flags & WAIT_TIME != 0 {
                        tokio::time::sleep(std::time::Duration::from_secs_f32(seconds.min(3600.0)))
                            .await;
                    }
                    if flags & WAIT_ANIMATIONS != 0 {
                        self.animations_end(&started).await;
                    }
                }
            }
        }
        if started.is_empty() {
            return;
        }
        self.animations_end(&started).await;
        let still: Vec<(Uuid, bool)> = {
            let playing = &self.state().animations;
            started
                .iter()
                .filter(|a| playing.contains(a))
                .map(|a| (*a, false))
                .collect()
        };
        if !still.is_empty() {
            let _ = self.animate(still).await;
        }
    }

    /// Until none of `anims` plays any more (the region's AvatarAnimation), or the wait is over.
    async fn animations_end(&self, anims: &[Uuid]) {
        // the region tells of a new animation a moment after it starts
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let deadline = tokio::time::Instant::now() + ANIMATIONS_WAIT;
        while tokio::time::Instant::now() < deadline {
            if !self.state().animations.iter().any(|a| anims.contains(a)) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gesture_as_firestorm_writes_it() {
        let data = b"2\n255\n0\n/wave\n\n4\n0\nhello\n0a9fb970-8b44-9114-d3a9-bf69cfe804d6\n0\n1\nding\n00000000-0000-0000-0000-000000000002\n0\n2\nHi there\n0\n3\n1.500000\n1\n\0";
        let g = Gesture::decode(data).expect("a gesture");
        assert_eq!(g.trigger, "/wave");
        assert_eq!(g.steps.len(), 4);
        assert_eq!(
            g.steps[3],
            Step::Wait {
                seconds: 1.5,
                flags: WAIT_TIME
            }
        );
        assert!(Gesture::decode(b"1\n").is_err());
        assert!(Gesture::decode(b"2\n255\n0\n").is_err());
    }

    #[test]
    fn chat_steps_on_their_channel() {
        assert_eq!(chat_channel("/28574 T4"), (28574, "T4"));
        assert_eq!(chat_channel("hello"), (0, "hello"));
        assert_eq!(chat_channel("/me waves"), (0, "/me waves"));
    }
}
