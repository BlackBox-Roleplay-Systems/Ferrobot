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
| `crates/client/src/network.rs` | the connect sequence of `crates/aurora-net/src/session.rs` | `b8cf1f047dd111f71981b500ae8825ef0c1321ed` (2026-11-12) |

Aurora's code in turn follows the Second Life viewer's sources (Linden Lab,
LGPL 2.1), as the comments in each file say.
