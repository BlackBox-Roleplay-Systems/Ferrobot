//! SecondBot's Services commands (Commands/Services.cs): EnableAndStartService, StopService,
//! RestartService and ChangeConfigValue, off unless FERRO_ALLOW_SERVICE_CONTROL (its
//! AllowServiceControl). The services and what they refuse are in services/control.rs. As
//! SecondBot's, the answers carry no arguments: ChangeConfigValue's value is never echoed back.
//!
//! Deliberate differences: the bot's core services (BotClientService, HttpService) answer
//! "Unable to manage selected service" (SecondBot stops them, locking the operator out);
//! ChangeConfigValue refuses settings that hold secrets ("Secret values can not be changed by
//! command"), settings the service does not read ("Unknown config name") and values it could not
//! read ("Invalid config value"), where SecondBot keeps anything; service and setting names are
//! taken in any case.

use super::Ctx;
use super::reply::{Answer, basic, failure};
use crate::services::control::Refused;

fn answer(command: &str, done: Result<(), Refused>) -> Answer {
    let why = match done {
        Ok(()) => return basic(command, "ok", &[]),
        Err(Refused::NotAllowed) => "Service control is not enabled by config",
        Err(Refused::Unknown) => "Unknown service",
        Err(Refused::Protected) => "Unable to manage selected service",
        Err(Refused::Secret) => "Secret values can not be changed by command",
        Err(Refused::UnknownSetting) => "Unknown config name",
        Err(Refused::Invalid) => "Invalid config value",
    };
    failure(command, why, &[])
}

/// EnableAndStartService: its Enabled setting to true, then (re)started.
pub async fn enable_and_start(ctx: &Ctx, service: &str) -> Answer {
    answer(
        "EnableAndStartService",
        ctx.services.enable_and_start(service).await,
    )
}

pub async fn stop(ctx: &Ctx, service: &str) -> Answer {
    answer("StopService", ctx.services.stop(service).await)
}

pub async fn restart(ctx: &Ctx, service: &str) -> Answer {
    answer("RestartService", ctx.services.restart(service).await)
}

/// ChangeConfigValue: kept for the service's next start (RestartService applies it).
pub async fn change_config_value(ctx: &Ctx, service: &str, name: &str, value: &str) -> Answer {
    answer(
        "ChangeConfigValue",
        ctx.services.change(service, name, value).await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_secondbots_without_arguments() {
        assert_eq!(
            answer("StopService", Ok(())),
            r#"{"command":"StopService","args":[],"reply":"ok","status":true}"#
        );
        assert_eq!(
            answer("ChangeConfigValue", Err(Refused::NotAllowed)),
            r#"{"command":"ChangeConfigValue","args":[],"reply":"Service control is not enabled by config","status":false,"whyfailed":null}"#
        );
    }
}
