//! SecondBot's Movement commands (Fly, SetAlwaysRun, AutoPilot, AutoPilotStop, RotateTo,
//! RotateToFace, RotateToFaceVector, HoverHeight) and its Self commands Sit and Stand.
//!
//! Deliberate difference: RotateTo works (SecondBot's refuses every number it can read, an
//! inverted check); the heading is in degrees clockwise from north.
//!
//! Ferrobot's own, as SecondBot has none: walking by keys (Walk, WalkStop, Turn) and following
//! an avatar (Follow, FollowStop), named and answered in SecondBot's manner. AutoPilotStop also
//! lets go of a key and ends a follow; any new movement (AutoPilot, Walk, Follow, Sit, a
//! teleport) ends the one under way.

use super::reply::{Answer, basic, failure};
use super::{Ctx, avatar, key, owned};
use ferro_client::walking::{MAX_DISTANCE, MIN_DISTANCE};
use ferro_client::{Move, WalkKey};
use glam::Vec3;
use std::time::Duration;

/// The longest a key is held by one Walk.
const MAX_WALK_SECONDS: f32 = 60.0;

/// The heading (degrees clockwise from north) from one place to another; none when the second is
/// right above or below the first.
pub fn heading_to(from: Vec3, to: Vec3) -> Option<f32> {
    let d = to - from;
    (d.x != 0.0 || d.y != 0.0).then(|| d.x.atan2(d.y).to_degrees())
}

/// LibreMetaverse's Vector3.TryParse: "<x, y, z>", the brackets optional.
fn vector(s: &str) -> Option<Vec3> {
    let inner = s.trim().trim_start_matches('<').trim_end_matches('>');
    let mut parts = inner.split(',').map(|p| p.trim().parse::<f32>());
    let v = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Ok(x)), Some(Ok(y)), Some(Ok(z)), None) => Vec3::new(x, y, z),
        _ => return None,
    };
    v.is_finite().then_some(v)
}

/// Turns toward a place; SecondBot replies with what TurnToward returned, a C# bool.
async fn turn_toward(ctx: &Ctx, c: &str, target: Vec3, args: &[String]) -> Answer {
    match heading_to(ctx.client.state().position, target) {
        Some(h) => go(ctx, c, Move::Face(h), "True", args).await,
        None => basic(c, "True", args),
    }
}

use super::cs_bool;

async fn go(ctx: &Ctx, c: &str, m: Move, ok: &str, args: &[String]) -> Answer {
    match ctx.client.move_bot(m).await {
        Ok(()) => basic(c, ok, args),
        Err(e) => failure(c, &e.to_string(), args),
    }
}

pub async fn fly(ctx: &Ctx, mode: &str) -> Answer {
    let args = owned(&[mode]);
    match cs_bool(mode) {
        Some(on) => go(ctx, "Fly", Move::Fly(on), "ok", &args).await,
        None => failure("Fly", "Invaild mode", &args),
    }
}

pub async fn always_run(ctx: &Ctx, mode: &str) -> Answer {
    let args = owned(&[mode]);
    match cs_bool(mode) {
        Some(on) => go(ctx, "SetAlwaysRun", Move::AlwaysRun(on), "ok", &args).await,
        None => failure("SetAlwaysRun", "Invaild mode", &args),
    }
}

/// Sit: on the ground ("ground", or the nil key), or on an object.
pub async fn sit(ctx: &Ctx, target: &str) -> Answer {
    let args = owned(&[target]);
    let m = if target == "ground" {
        Move::SitOnGround
    } else {
        match key(target) {
            Some(id) if id.is_nil() => Move::SitOnGround,
            Some(id) => Move::SitOn(id),
            None => return failure("Sit", "Invaild object uuid and not ground", &[]),
        }
    };
    go(ctx, "Sit", m, "ok", &args).await
}

/// Stand, as SecondBot's: homebound no longer sits the bot down (its MarkStandup), and the
/// animations are reset (its ResetAnimations).
pub async fn stand(ctx: &Ctx) -> Answer {
    ctx.homebound.stood_up();
    let answer = go(ctx, "Stand", Move::Stand, "ok", &[]).await;
    super::animation::stop_all(ctx).await;
    answer
}

pub async fn autopilot(ctx: &Ctx, x: &str, y: &str, z: &str) -> Answer {
    const C: &str = "AutoPilot";
    let args = owned(&[x, y, z]);
    let (Ok(px), Ok(py), Ok(pz)) = (
        x.trim().parse::<f32>(),
        y.trim().parse::<f32>(),
        z.trim().parse::<f32>(),
    ) else {
        return failure(C, "Convert to vector has failed", &args);
    };
    if !(0.0..=255.0).contains(&px) {
        return failure(C, "x value out of range 0-255", &args);
    }
    if !(0.0..=255.0).contains(&py) {
        return failure(C, "y value out of range 0-255", &args);
    }
    if !(0.0..=5000.0).contains(&pz) {
        return failure(C, "z value out of range 0-5000", &args);
    }
    let target = Vec3::new(px, py, pz);
    // face the way first, as SecondBot's TurnToward
    if let Some(h) = heading_to(ctx.client.state().position, target) {
        let _ = ctx.client.move_bot(Move::Face(h)).await;
    }
    go(ctx, C, Move::AutoPilot(target), "ok", &args).await
}

pub async fn autopilot_stop(ctx: &Ctx) -> Answer {
    go(ctx, "AutoPilotStop", Move::Stop, "ok", &[]).await
}

pub async fn rotate_to(ctx: &Ctx, deg: &str) -> Answer {
    let args = owned(&[deg]);
    match deg.trim().parse::<f32>() {
        Ok(h) if h.is_finite() => go(ctx, "RotateTo", Move::Face(h), "ok", &args).await,
        _ => failure("RotateTo", "Unable to process rotation", &args),
    }
}

pub async fn rotate_to_face_vector(ctx: &Ctx, v: &str) -> Answer {
    const C: &str = "RotateToFaceVector";
    let args = owned(&[v]);
    let Some(pos) = vector(v) else {
        return failure(C, "Invaild vector", &args);
    };
    if !(0.0..=255.0).contains(&pos.x) {
        return failure(C, "Vector x value is out of range 0-255", &args);
    }
    if !(0.0..=255.0).contains(&pos.y) {
        return failure(C, "Vector y value is out of range 0-255", &args);
    }
    if !(0.0..=5000.0).contains(&pos.z) {
        return failure(C, "Vector z value is out of range 0-5000", &args);
    }
    turn_toward(ctx, C, pos, &args).await
}

/// Where the avatar stands: its object update, as SecondBot's AvatarPositions, else the region's
/// coarse location, close enough to turn toward or to follow.
fn seen_at(ctx: &Ctx, id: uuid::Uuid) -> Option<Vec3> {
    let st = ctx.client.state();
    st.avatars
        .get(&id)
        .or(st.nearby_positions.get(&id))
        .copied()
}

pub async fn rotate_to_face(ctx: &Ctx, who: &str) -> Answer {
    const C: &str = "RotateToFace";
    let args = owned(&[who]);
    let Some(id) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar UUID", &args);
    };
    let Some(pos) = seen_at(ctx, id) else {
        return failure(C, "Unable to see avatar", &args);
    };
    turn_toward(ctx, C, pos, &args).await
}

pub async fn hover_height(ctx: &Ctx, height: &str) -> Answer {
    const C: &str = "HoverHeight";
    let args = owned(&[height]);
    let Ok(h) = height.trim().parse::<f32>() else {
        return failure(C, "Convert to float has failed", &args);
    };
    if !(-2.0..=2.0).contains(&h) {
        return failure(C, "height value out of range(-2.0 to 2.0)", &args);
    }
    match ctx.client.set_hover_height(h).await {
        Ok(()) => basic(C, "ok", &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

/// A number as C#'s float.TryParse reads it, finite.
fn number(s: &str) -> Option<f32> {
    s.trim().parse::<f32>().ok().filter(|n| n.is_finite())
}

/// The movement key of a direction, whatever its case.
fn walk_key(direction: &str) -> Option<WalkKey> {
    match direction.trim().to_ascii_lowercase().as_str() {
        "forward" => Some(WalkKey::Forward),
        "back" | "backward" => Some(WalkKey::Back),
        "left" => Some(WalkKey::Left),
        "right" => Some(WalkKey::Right),
        _ => None,
    }
}

/// Walk: hold a movement key (forward, back, or a step to the left or right) for a few seconds,
/// as the viewer's arrow keys; answered at once, the bot walks on.
pub async fn walk(ctx: &Ctx, direction: &str, seconds: &str) -> Answer {
    const C: &str = "Walk";
    let args = owned(&[direction, seconds]);
    let Some(key) = walk_key(direction) else {
        return failure(C, "Invaild direction", &args);
    };
    let Some(secs) = number(seconds) else {
        return failure(C, "Convert to float has failed", &args);
    };
    if secs <= 0.0 || secs > MAX_WALK_SECONDS {
        return failure(C, "seconds value out of range 0-60", &args);
    }
    go(
        ctx,
        C,
        Move::Walk(key, Duration::from_secs_f32(secs)),
        "ok",
        &args,
    )
    .await
}

/// WalkStop: let go of the key.
pub async fn walk_stop(ctx: &Ctx) -> Answer {
    go(ctx, "WalkStop", Move::WalkStop, "ok", &[]).await
}

/// Turn: left or right by some degrees, from the way the bot faces.
pub async fn turn(ctx: &Ctx, direction: &str, degrees: &str) -> Answer {
    const C: &str = "Turn";
    let args = owned(&[direction, degrees]);
    let sign = match direction.trim().to_ascii_lowercase().as_str() {
        "left" => -1.0,
        "right" => 1.0,
        _ => return failure(C, "Invaild direction", &args),
    };
    let Some(deg) = number(degrees) else {
        return failure(C, "Unable to process rotation", &args);
    };
    if !(0.0..=360.0).contains(&deg) {
        return failure(C, "degrees value out of range 0-360", &args);
    }
    go(ctx, C, Move::Turn(sign * deg), "ok", &args).await
}

/// Follow: walk after an avatar of the region (key or name), keeping some metres from it, until
/// it leaves the region or FollowStop.
pub async fn follow(ctx: &Ctx, who: &str, distance: &str) -> Answer {
    const C: &str = "Follow";
    let args = owned(&[who, distance]);
    let Some(d) = number(distance) else {
        return failure(C, "Convert to float has failed", &args);
    };
    if !(MIN_DISTANCE..=MAX_DISTANCE).contains(&d) {
        return failure(C, "distance value out of range 0.5-20", &args);
    }
    let Some(id) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar UUID", &args);
    };
    if seen_at(ctx, id).is_none() {
        return failure(C, "Unable to see avatar", &args);
    }
    go(
        ctx,
        C,
        Move::Follow {
            avatar: id,
            distance: d,
        },
        "ok",
        &args,
    )
    .await
}

/// FollowStop: stop following.
pub async fn follow_stop(ctx: &Ctx) -> Answer {
    go(ctx, "FollowStop", Move::FollowStop, "ok", &[]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directions_whatever_their_case() {
        assert_eq!(walk_key("Forward"), Some(WalkKey::Forward));
        assert_eq!(walk_key(" BACK "), Some(WalkKey::Back));
        assert_eq!(walk_key("backward"), Some(WalkKey::Back));
        assert_eq!(walk_key("left"), Some(WalkKey::Left));
        assert_eq!(walk_key("right"), Some(WalkKey::Right));
        assert_eq!(walk_key("up"), None);
    }

    #[test]
    fn numbers_are_finite() {
        assert_eq!(number(" 2.5 "), Some(2.5));
        assert_eq!(number("NaN"), None);
        assert_eq!(number("inf"), None);
        assert_eq!(number("two"), None);
    }

    #[test]
    fn vectors_as_libremetaverse_reads_them() {
        assert_eq!(vector("<1, 2.5, 3>"), Some(Vec3::new(1.0, 2.5, 3.0)));
        assert_eq!(vector("4,5,6"), Some(Vec3::new(4.0, 5.0, 6.0)));
        assert_eq!(vector("<1,2>"), None);
        assert_eq!(vector("<1,2,3,4>"), None);
        assert_eq!(vector("<a,b,c>"), None);
        assert_eq!(vector("<NaN,1,1>"), None);
    }

    #[test]
    fn headings_are_clockwise_from_north() {
        let here = Vec3::new(128.0, 128.0, 25.0);
        let h = |x, y| heading_to(here, Vec3::new(x, y, 25.0)).map(f32::round);
        assert_eq!(h(128.0, 200.0), Some(0.0));
        assert_eq!(h(200.0, 128.0), Some(90.0));
        assert_eq!(h(128.0, 0.0), Some(180.0));
        assert_eq!(h(0.0, 128.0), Some(-90.0));
        assert_eq!(heading_to(here, here + Vec3::Z), None);
    }

    #[test]
    fn booleans_as_csharp_reads_them() {
        assert_eq!(cs_bool("True"), Some(true));
        assert_eq!(cs_bool(" false "), Some(false));
        assert_eq!(cs_bool("1"), None);
    }
}
