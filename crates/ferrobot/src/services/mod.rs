//! The bot's services: each subscribes to the client's events and does one thing.

pub mod chat_log;
pub mod chatgpt;
pub mod control;
pub mod events;
pub mod health;
pub mod homebound;
pub mod im_commands;
pub mod interactions;
pub mod on_event;
pub mod relays;
pub mod restart;
pub mod rlv;
pub mod script_permissions;
pub mod smtp;
