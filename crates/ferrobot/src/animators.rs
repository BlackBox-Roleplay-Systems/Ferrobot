//! The avatars whose objects may animate the bot (SecondBot's AddToAllowAnimations), until the
//! bot stops.

use std::collections::HashSet;
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Default)]
pub struct Animators(Mutex<HashSet<Uuid>>);

impl Animators {
    /// False when the avatar was already on the list.
    pub fn add(&self, id: Uuid) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id)
    }

    pub fn list(&self) -> Vec<Uuid> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .copied()
            .collect()
    }
}
