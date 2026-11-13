//! SecondBot's Animation commands (AddToAllowAnimations, PlayGesture, PlayGestureDirect,
//! ResetAnimations) and its Self command PointAt.
//!
//! Deliberate differences: AddToAllowAnimations works (SecondBot's answers "@todo"): that
//! avatar's objects may animate the bot; PointAt looks the avatar up before using it (SecondBot's
//! reads the key first, so it never finds it).

use super::reply::{Answer, basic, failure};
use super::{Ctx, avatar, key, owned};
use uuid::Uuid;

/// ANIM_AGENT_POINT_YOU (llanimationstates.cpp)
const POINT_YOU: Uuid = Uuid::from_u128(0xb1709c8d_ecd3_54a1_4f28_d55ac0840782);

pub async fn allow_animations(ctx: &Ctx, who: &str) -> Answer {
    const C: &str = "AddToAllowAnimations";
    let args = owned(&[who]);
    let Some(id) = avatar(&ctx.client, who).await else {
        return basic(C, "avatar lookup", &args);
    };
    ctx.animators.add(id);
    basic(C, "ok", &args)
}

pub async fn play_gesture(ctx: &Ctx, item: &str) -> Answer {
    const C: &str = "PlayGesture";
    let args = owned(&[item]);
    let Some(id) = key(item) else {
        return basic(C, "Error with gesture inventory uuid", &args);
    };
    let Ok(Some(found)) = ctx.client.inventory_item(id).await else {
        return basic(C, "unable to get getsture from inventory", &args);
    };
    play(ctx, C, found.asset_id, &args).await
}

pub async fn play_gesture_direct(ctx: &Ctx, asset: &str) -> Answer {
    const C: &str = "PlayGestureDirect";
    let args = owned(&[asset]);
    let Some(id) = key(asset) else {
        return basic(C, "Error with gesture", &args);
    };
    play(ctx, C, id, &args).await
}

async fn play(ctx: &Ctx, c: &str, asset: Uuid, args: &[String]) -> Answer {
    match ctx.client.play_gesture(asset).await {
        Ok(()) => basic(c, "Accepted", args),
        Err(e) => failure(c, &e.to_string(), args),
    }
}

/// Stop what the region says the bot plays; how many.
pub async fn stop_all(ctx: &Ctx) -> usize {
    let playing = ctx.client.state().animations.clone();
    if !playing.is_empty() {
        let stop = playing.iter().map(|a| (*a, false)).collect();
        let _ = ctx.client.animate(stop).await;
    }
    playing.len()
}

pub async fn reset_animations(ctx: &Ctx) -> Answer {
    let n = stop_all(ctx).await;
    basic(
        "ResetAnimations",
        &format!("Accepted - {n} stopped animations"),
        &[],
    )
}

/// PointAt: stand, face the avatar, the point animation, the point-at effect and a beam.
pub async fn point_at(ctx: &Ctx, who: &str) -> Answer {
    const C: &str = "PointAt";
    let args = owned(&[who]);
    let Some(id) = avatar(&ctx.client, who).await else {
        return failure(C, "Cant find UUID in sim", &args);
    };
    let at = {
        let st = ctx.client.state();
        st.avatars
            .get(&id)
            .or(st.nearby_positions.get(&id))
            .copied()
    };
    let Some(at) = at else {
        return failure(C, "Cant find UUID in sim", &args);
    };
    let _ = ctx.client.move_bot(ferro_client::Move::Stand).await;
    if let Some(h) = super::movement::heading_to(ctx.client.state().position, at) {
        let _ = ctx.client.move_bot(ferro_client::Move::Face(h)).await;
    }
    let _ = ctx.client.animate(vec![(POINT_YOU, true)]).await;
    match ctx.client.point_at(id).await {
        Ok(()) => basic(C, "ok", &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}
