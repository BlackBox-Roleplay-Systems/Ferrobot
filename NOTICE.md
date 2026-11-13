# Notice

Ferrobot is free software under the GNU General Public License, version 3 or
(at your option) any later version. See `LICENSE`.

## Code from Aurora Viewer

Parts of Ferrobot come from [Aurora Viewer](https://github.com/Aurora-Viewer/Aurora-Viewer),
a Second Life viewer in Rust, GPL-3.0-or-later. They may have been changed since.

| Ferrobot | From Aurora Viewer | Aurora commit |
|---|---|---|
| `crates/llsd` (`ferro-llsd`) | `crates/aurora-llsd` | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |
| `crates/msg` (`ferro-msg`) | `crates/aurora-msg` | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |
| `crates/client/src/xmlrpc.rs` | `crates/aurora-net/src/xmlrpc.rs` | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |
| `crates/client/src/login.rs` | `crates/aurora-net/src/login.rs`, trimmed for a bot | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |
| `crates/client/src/circuit.rs` | `crates/aurora-net/src/circuit.rs`, trimmed | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |
| `crates/client/src/caps.rs` | parts of `crates/aurora-net/src/caps.rs` | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |
| `crates/client/src/network/mod.rs` | the connect sequence of `crates/aurora-net/src/session.rs` | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |
| `crates/client/src/groups.rs` | `crates/aurora-net/src/groups.rs`, without notices, bans and group edits | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |
| `crates/client/src/network/groups.rs` | the group requests of `crates/aurora-net/src/session_groups.rs` | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |
| `crates/client/src/inventory.rs` | `crates/aurora-net/src/inventory.rs`, without AIS, uploads and rezzing | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |
| `crates/client/src/objects.rs` | `crates/aurora-net/src/objects.rs`: the place in full, compressed and terse updates only (`Dp`, `u16_to_f32`, the head of `parse_full`, `parse_compressed`, `parse_terse`); `parse_properties` and `parse_family` of `crates/aurora-net/src/session/build_cmds.rs`; the pace of the property requests of `crates/aurora-viewer/src/app/area_search.rs` (`PACKET`, `IN_FLIGHT`, `TIMEOUT`, `Feed::requests`) | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |
| `crates/client/src/transfer.rs` | `crates/aurora-net/src/session/inventory_preview.rs` (the request and the assembly of the packets) and `notecard_text` of `crates/aurora-net/src/land.rs` | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |
| `crates/client/src/mutes.rs` | the mute list xfer of `crates/aurora-net/src/session_social.rs`, and `parse_line` of `crates/aurora-viewer/src/world/mutes.rs` | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |
| `crates/client/src/parcels.rs` | `parse_parcel_properties`, `ParcelUpdate::to_llsd`, `access_list_chunks` of `crates/aurora-net/src/land.rs`, and `ParcelInfo` / `ParcelMedia` of `crates/aurora-net/src/types.rs` | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |
| `crates/client/src/estate.rs` | `crates/aurora-net/src/estate.rs`: the estate and access flags, `EstateInfo` and `parse_estate_update_info`, `estate_change_body` / `estate_change_strings`, `parse_estate_access`, `RegionRules`, `access_delta_strings`, `region_message_strings` | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |
| `crates/client/src/terrain.rs` | the RAW file's upload of `crates/aurora-net/src/session/terrain_raw.rs`, and `packet` of `crates/aurora-net/src/session/inventory_upload.rs` | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |
| `crates/client/src/gesture.rs` | `crates/aurora-assets/src/gesture.rs` (the decoding) | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |
| `crates/client/src/walking.rs` | the control flags of `crates/aurora-net/src/types.rs`, the follow-up update of `crates/aurora-net/src/agent_update.rs`, the keys' flags of `control_flags` in `crates/aurora-viewer/src/agent.rs`, the follow distance bounds of `FollowSettings` in `crates/aurora-viewer/src/autopilot.rs` | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |
| `crates/client/src/region_restart.rs` | `crates/aurora-net/src/region_restart.rs`, without the channel announcement | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |

Aurora's code in turn follows the Second Life viewer's sources (Linden Lab,
LGPL 2.1), as the comments in each file say.

## Code following SecondBot

[SecondBot](https://github.com/Madpeterz/SecondBot) (C#, GPL-3.0) is rewritten in Rust, not copied:
these files follow its behaviour closely, its commands' names, replies and checks.

| Ferrobot | Follows SecondBot | SecondBot commit |
|---|---|---|
| `crates/ferrobot/src/api/` | `SecondBotEvents/Services/HttpService.cs`, `CommandsService.cs` (SignedCommand, RunCommand) | `a2377e5` (branch `update-libremetaverse`) |
| `crates/ferrobot/src/commands/` | `SecondBotEvents/Commands/` (Group, Info, Avatars, Chat, Self, Core, Inventory, Notecard, Movement, Friends, Funds, Discord, StreamAdmin, Estate) | `a2377e5` (branch `update-libremetaverse`) |
| `crates/ferrobot/src/discord/` | `SecondBotEvents/Services/DiscordService.cs`, `Config/DiscordConfig.cs` (no voice) | `a2377e5` (branch `update-libremetaverse`) |
| `crates/ferrobot/src/rabbit/`, `commands/rabbit.rs` | `SecondBotEvents/Services/RabbitService.cs`, `Config/RabbitConfig.cs`, `Commands/RabbitMQ.cs` | `a2377e5` (branch `update-libremetaverse`) |
| `crates/ferrobot/src/inventory_updates.rs` | `SecondBotEvents/Services/EventsService.cs` (InventoryUpdateEvent, AddInventoryEvent), `InteractionService.cs` (BotInventoryAdd) | `a2377e5` (branch `update-libremetaverse`) |
| `crates/ferrobot/src/dialogs.rs` | `SecondBotEvents/Services/DialogService.cs`, `Commands/Dialogs.cs` | `a2377e5` (branch `update-libremetaverse`) |
| `crates/ferrobot/src/history.rs` | the chat history of `SecondBotEvents/Services/DataStoreService.cs` | `a2377e5` (branch `update-libremetaverse`) |
| `crates/ferrobot/src/services/relays.rs` | `SecondBotEvents/Services/RelayService.cs`, `Config/RelayConfig.cs` | `a2377e5` (branch `update-libremetaverse`) |
| `crates/ferrobot/src/services/homebound.rs` | `SecondBotEvents/Services/HomeboundService.cs` | `a2377e5` (branch `update-libremetaverse`) |
| `crates/ferrobot/src/services/interactions.rs`, `im_commands.rs`, `crates/ferrobot/src/grants.rs` | `SecondBotEvents/Services/InteractionService.cs`, `Config/InteractionConfig.cs`, `CommandsService.cs` (BotImMessage, SmartCommandReply), the accept-next list of `DataStoreService.cs` | `a2377e5` (branch `update-libremetaverse`) |
