//! SecondBot's Dialogs commands (dialogs.rs keeps the dialogs and the relay targets).

use super::reply::{Answer, basic, failure};
use super::{Ctx, avatar, owned};

pub async fn relay_avatar(ctx: &Ctx, who: &str) -> Answer {
    const C: &str = "DialogRelayAvatarTarget";
    match avatar(&ctx.client, who).await {
        Some(id) => basic(C, ctx.dialogs.toggle_avatar(id), &[]),
        None => failure(C, "unable to find avatar", &[]),
    }
}

pub fn relay_chat(ctx: &Ctx, channel: &str) -> Answer {
    const C: &str = "DialogRelayChatTarget";
    let Ok(channel) = channel.trim().parse::<i32>() else {
        return failure(C, "channel is not vaild", &[]);
    };
    if channel < 0 {
        return failure(C, "channel must be zero or more", &[]);
    }
    basic(C, ctx.dialogs.toggle_channel(channel), &[])
}

pub fn relay_http(ctx: &Ctx, url: &str) -> Answer {
    const C: &str = "DialogRelayHttpTarget";
    if !url.starts_with("http") {
        return failure(C, "url must start with http", &[]);
    }
    basic(C, ctx.dialogs.toggle_url(url), &[])
}

/// DialogResponce (SecondBot's spelling): press a button. SecondBot answers why it could not as a
/// reply, not a failure; so does this.
pub async fn respond(ctx: &Ctx, dialog: &str, button: &str) -> Answer {
    const C: &str = "DialogResponce";
    let Ok(id) = dialog.trim().parse::<i32>() else {
        return failure(C, "bad dialog id", &owned(&[dialog, button]));
    };
    match ctx.dialogs.take(id, button) {
        Err(why) => basic(C, why, &[]),
        Ok((d, index)) => match ctx
            .client
            .reply_dialog(d.object_id, d.channel, index, button)
            .await
        {
            Ok(()) => basic(C, "action", &[]),
            Err(e) => failure(C, &e.to_string(), &owned(&[dialog, button])),
        },
    }
}
