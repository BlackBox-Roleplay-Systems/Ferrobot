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
| `crates/client/src/region_restart.rs` | `crates/aurora-net/src/region_restart.rs`, without the channel announcement | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |

Aurora's code in turn follows the Second Life viewer's sources (Linden Lab,
LGPL 2.1), as the comments in each file say.

## Code following SecondBot

[SecondBot](https://github.com/Madpeterz/SecondBot) (C#, GPL-3.0) is rewritten in Rust, not copied:
these files follow its behaviour closely, its commands' names, replies and checks.

| Ferrobot | Follows SecondBot | SecondBot commit |
|---|---|---|
| `crates/ferrobot/src/api/` | `SecondBotEvents/Services/HttpService.cs`, `CommandsService.cs` (SignedCommand, RunCommand) | `a2377e5` (branch `update-libremetaverse`) |
| `crates/ferrobot/src/commands/` | `SecondBotEvents/Commands/Group.cs`, `Info.cs`, `Avatars.cs`, `Chat.cs` | `a2377e5` (branch `update-libremetaverse`) |
| `crates/ferrobot/src/dialogs.rs` | `SecondBotEvents/Services/DialogService.cs`, `Commands/Dialogs.cs` | `a2377e5` (branch `update-libremetaverse`) |
| `crates/ferrobot/src/history.rs` | the chat history of `SecondBotEvents/Services/DataStoreService.cs` | `a2377e5` (branch `update-libremetaverse`) |
| `crates/ferrobot/src/services/interactions.rs`, `im_commands.rs` | `SecondBotEvents/Services/InteractionService.cs`, `CommandsService.cs` (BotImMessage, SmartCommandReply) | `a2377e5` (branch `update-libremetaverse`) |
