//! What an avatar was granted beyond the interaction settings, until the bot stops:
//! - one-time grants: SecondBot's "accept next" list (DataStoreService.AcceptNextStore, filled by
//!   AcceptNext), and SetPermFlag with sticky "false": the next request of that kind is accepted,
//!   then the grant is gone;
//! - lasting grants: SetPermFlag with sticky "true", and AddToAllowAnimations (a lasting
//!   animation grant).
//!
//! SecondBot's SetPermFlag only answers (a "@todo"); here it fills these lists (TASKS.md).

use crate::masters::canonical;
use ferro_client::Client;
use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, PoisonError};
use uuid::Uuid;

/// A kind of request an avatar may be granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Grant {
    Friendship,
    GroupInvite,
    Teleport,
    Inventory,
    /// A script of the avatar's objects may animate the bot (PERMISSION_TRIGGER_ANIMATION).
    Animation,
    /// The avatar's IMs are taken as commands without a signature (services/im_commands.rs).
    Command,
}

impl Grant {
    /// SecondBot's AcceptNext stores (DataStoreService.acceptedStores), their case as given.
    pub fn from_store(store: &str) -> Option<Grant> {
        Some(match store {
            "FriendRequest" => Grant::Friendship,
            "InventoryOffer" => Grant::Inventory,
            "GroupInvite" => Grant::GroupInvite,
            "Teleport" => Grant::Teleport,
            _ => return None,
        })
    }

    /// SetPermFlag's flags, their case as given (SecondBot's AcceptedFlags).
    pub fn from_flag(flag: &str) -> Option<Grant> {
        Some(match flag {
            "friend" => Grant::Friendship,
            "group" => Grant::GroupInvite,
            "animation" => Grant::Animation,
            "teleport" => Grant::Teleport,
            "command" => Grant::Command,
            "inventory" => Grant::Inventory,
            _ => return None,
        })
    }
}

#[derive(Default)]
struct Lists {
    once: HashSet<(Grant, Uuid)>,
    lasting: HashSet<(Grant, Uuid)>,
}

#[derive(Default)]
pub struct Grants(Mutex<Lists>);

impl Grants {
    fn lock(&self) -> MutexGuard<'_, Lists> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Grants `kind` to `id`, once or lasting. False when it was already granted so.
    pub fn add(&self, kind: Grant, id: Uuid, lasting: bool) -> bool {
        let mut l = self.lock();
        let list = if lasting { &mut l.lasting } else { &mut l.once };
        list.insert((kind, id))
    }

    /// Takes a grant back: a lasting one goes with any one-time grant of the same kind (nothing
    /// is left once "removed" lasting), a one-time one alone.
    pub fn remove(&self, kind: Grant, id: Uuid, lasting: bool) {
        let mut l = self.lock();
        l.once.remove(&(kind, id));
        if lasting {
            l.lasting.remove(&(kind, id));
        }
    }

    /// Whether a one-time grant waits (SecondBot's IsInAcceptNext): it is not used up.
    pub fn has_once(&self, kind: Grant, id: Uuid) -> bool {
        self.lock().once.contains(&(kind, id))
    }

    pub fn has_lasting(&self, kind: Grant, id: Uuid) -> bool {
        self.lock().lasting.contains(&(kind, id))
    }

    /// Uses up a one-time grant (SecondBot's GetNextAccept): true when there was one.
    pub fn take_once(&self, kind: Grant, id: Uuid) -> bool {
        self.lock().once.remove(&(kind, id))
    }

    /// The avatars that hold a grant of this kind, lasting or not, for the requests that name
    /// their sender instead of giving its key (ScriptQuestion, group invitations).
    pub fn holders(&self, kind: Grant) -> Vec<Uuid> {
        let l = self.lock();
        let mut ids: Vec<Uuid> = l
            .once
            .iter()
            .chain(l.lasting.iter())
            .filter(|(k, _)| *k == kind)
            .map(|(_, id)| *id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Whether `id` may have this: a lasting grant, else a one-time grant, used up.
    pub fn use_key(&self, kind: Grant, id: Uuid) -> Option<Used> {
        if id.is_nil() {
            return None;
        }
        if self.has_lasting(kind, id) {
            Some(Used::Lasting)
        } else {
            self.take_once(kind, id).then_some(Used::Once)
        }
    }

    /// The same for a request that names its sender instead of giving a key (ScriptQuestion
    /// names the object's owner, a group invitation its inviter): the holders' names are looked
    /// up and compared as the masters' are ("Some" is "Some Resident").
    pub async fn use_name(&self, client: &Client, kind: Grant, name: &str) -> Option<Used> {
        let holders = self.holders(kind);
        let wanted = canonical(name);
        if holders.is_empty() || wanted.is_empty() {
            return None;
        }
        let names = match client.names(&holders).await {
            Ok(names) => names,
            Err(e) => {
                log::debug!("grants: names not looked up ({e})");
                return None;
            }
        };
        let named: Vec<Uuid> = holders
            .into_iter()
            .filter(|id| names.get(id).is_some_and(|n| canonical(n) == wanted))
            .collect();
        if named.iter().any(|id| self.has_lasting(kind, *id)) {
            return Some(Used::Lasting);
        }
        named
            .into_iter()
            .any(|id| self.take_once(kind, id))
            .then_some(Used::Once)
    }
}

/// Which grant let a request through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Used {
    Lasting,
    Once,
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Uuid = Uuid::from_u128(1);
    const B: Uuid = Uuid::from_u128(2);

    #[test]
    fn names_as_secondbot_spells_them() {
        assert_eq!(Grant::from_store("GroupInvite"), Some(Grant::GroupInvite));
        assert_eq!(Grant::from_store("groupinvite"), None);
        assert_eq!(Grant::from_flag("command"), Some(Grant::Command));
        assert_eq!(Grant::from_flag("Command"), None);
        assert_eq!(Grant::from_store("animation"), None);
    }

    #[test]
    fn a_one_time_grant_is_used_once() {
        let g = Grants::default();
        assert!(g.add(Grant::Teleport, A, false));
        assert!(!g.add(Grant::Teleport, A, false), "already there");
        assert!(g.has_once(Grant::Teleport, A));
        assert!(!g.has_once(Grant::Teleport, B));
        assert!(!g.has_once(Grant::Inventory, A));
        assert!(g.take_once(Grant::Teleport, A));
        assert!(!g.take_once(Grant::Teleport, A));
        assert!(!g.has_once(Grant::Teleport, A));
    }

    #[test]
    fn a_lasting_grant_stays_until_removed() {
        let g = Grants::default();
        g.add(Grant::Command, A, true);
        g.add(Grant::Command, A, false);
        assert!(g.has_lasting(Grant::Command, A));
        assert!(!g.has_lasting(Grant::Command, B));
        g.remove(Grant::Command, A, false);
        assert!(!g.has_once(Grant::Command, A));
        assert!(
            g.has_lasting(Grant::Command, A),
            "a one-time removal leaves it"
        );
        g.add(Grant::Command, A, false);
        g.remove(Grant::Command, A, true);
        assert!(!g.has_lasting(Grant::Command, A));
        assert!(
            !g.has_once(Grant::Command, A),
            "a lasting removal takes both"
        );
    }

    #[test]
    fn holders_of_a_kind() {
        let g = Grants::default();
        g.add(Grant::Animation, B, true);
        g.add(Grant::Animation, A, false);
        g.add(Grant::Animation, B, false);
        g.add(Grant::Friendship, A, true);
        assert_eq!(g.holders(Grant::Animation), vec![A, B]);
        assert!(g.holders(Grant::Command).is_empty());
    }

    #[test]
    fn by_key_lasting_first_then_once() {
        let g = Grants::default();
        g.add(Grant::Inventory, A, true);
        g.add(Grant::Inventory, A, false);
        assert_eq!(g.use_key(Grant::Inventory, A), Some(Used::Lasting));
        assert!(
            g.has_once(Grant::Inventory, A),
            "the one-time grant is kept"
        );
        g.add(Grant::Inventory, B, false);
        assert_eq!(g.use_key(Grant::Inventory, B), Some(Used::Once));
        assert_eq!(g.use_key(Grant::Inventory, B), None);
        assert_eq!(g.use_key(Grant::Inventory, Uuid::nil()), None);
    }
}
