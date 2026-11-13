//! The shared folder: "#RLV" in the inventory's root, whose folders RLV commands name by path
//! ("Gear/Hats"), as SecondBot's RLVService finds and reads them (FindFolder, GetAllItems,
//! AllSubfolderWearables, GetWornIndicator). SecondBot reads the inventory it keeps; Ferrobot,
//! which keeps none, fetches the folder's tree when a command needs it.

use ferro_client::inventory::{FolderContents, InvItem};
use ferro_client::outfit::{BODYPART, CLOTHING, OBJECT};
use ferro_client::{Client, ClientError};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// The shared folder's name.
pub const SHARED: &str = "#RLV";
/// Folders fetched at most for one tree, and per request.
const MAX_FOLDERS: usize = 2000;
const PER_FETCH: usize = 10;

/// A folder of the tree.
#[derive(Debug, Clone)]
pub struct Node {
    pub id: Uuid,
    pub name: String,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub items: Vec<InvItem>,
}

/// A folder and every folder under it, depth first in the order the grid gave them (the order
/// SecondBot walks its inventory's nodes in).
#[derive(Debug, Clone)]
pub struct Tree {
    pub nodes: Vec<Node>,
}

/// What can be worn: clothing, a body part, an object (CanBeWorn).
fn wearable(i: &InvItem) -> bool {
    matches!(i.asset_type, CLOTHING | BODYPART | OBJECT)
}

fn hidden(name: &str) -> bool {
    name.starts_with('.')
}

impl Tree {
    /// The tree from the folders' contents, `root` first.
    pub fn build(root: Uuid, name: &str, contents: &[FolderContents]) -> Tree {
        let by_id: HashMap<Uuid, &FolderContents> =
            contents.iter().map(|c| (c.folder_id, c)).collect();
        let mut tree = Tree { nodes: Vec::new() };
        tree.add(root, name, None, &by_id);
        tree
    }

    fn add(
        &mut self,
        id: Uuid,
        name: &str,
        parent: Option<usize>,
        by_id: &HashMap<Uuid, &FolderContents>,
    ) {
        let at = self.nodes.len();
        let contents = by_id.get(&id);
        self.nodes.push(Node {
            id,
            name: name.to_owned(),
            parent,
            children: Vec::new(),
            items: contents.map(|c| c.items.clone()).unwrap_or_default(),
        });
        if let Some(parent) = parent {
            self.nodes[parent].children.push(at);
        }
        for f in contents.map(|c| c.folders.as_slice()).unwrap_or_default() {
            // a folder met twice (a loop the grid should never send) is not walked again
            if self.nodes.len() < MAX_FOLDERS && !self.nodes.iter().any(|n| n.id == f.id) {
                self.add(f.id, &f.name, Some(at), by_id);
            }
        }
    }

    /// A folder and all under it, fetched a few at a time.
    pub async fn fetch(client: &Client, root: Uuid, name: &str) -> Result<Tree, ClientError> {
        let mut contents = Vec::new();
        let mut todo = vec![root];
        let mut asked = HashSet::from([root]);
        while !todo.is_empty() {
            let batch: Vec<Uuid> = todo.drain(..todo.len().min(PER_FETCH)).collect();
            for c in client.fetch_folders(&batch).await? {
                for f in &c.folders {
                    if asked.len() < MAX_FOLDERS && asked.insert(f.id) {
                        todo.push(f.id);
                    }
                }
                contents.push(c);
            }
        }
        Ok(Tree::build(root, name, &contents))
    }

    /// The shared folder's tree; `None` when the inventory's root has no #RLV.
    pub async fn shared(client: &Client) -> Result<Option<Tree>, ClientError> {
        let root = client.folder_contents(client.inventory_root()).await?;
        match root.folders.iter().find(|f| f.name == SHARED) {
            Some(f) => Ok(Some(Tree::fetch(client, f.id, SHARED).await?)),
            None => Ok(None),
        }
    }

    /// A folder's path under the root ("" for the root), as RLV names it.
    pub fn path(&self, node: usize) -> String {
        let mut parts = Vec::new();
        let mut at = Some(node);
        while let Some(n) = at {
            let Some(parent) = self.nodes[n].parent else {
                break;
            };
            parts.push(self.nodes[n].name.as_str());
            at = Some(parent);
        }
        parts.reverse();
        parts.join("/")
    }

    /// A folder by its path under the root, whatever the case, as SecondBot's FindFolder: the
    /// slashes and spaces before it ignored, one slash after it allowed; "" is the root.
    pub fn find(&self, path: &str) -> Option<usize> {
        let wanted = format!(
            "/{}",
            path.trim_start_matches(|c: char| c == '/' || c.is_whitespace())
                .to_lowercase()
        );
        (0..self.nodes.len()).find(|n| {
            let have = format!("/{}", self.path(*n).to_lowercase());
            wanted == have || wanted == format!("{have}/")
        })
    }

    /// The folder an item is in.
    pub fn folder_of(&self, item: Uuid) -> Option<usize> {
        (0..self.nodes.len()).find(|n| self.nodes[*n].items.iter().any(|i| i.id == item))
    }

    /// The items of a folder that can be worn, with those of its subfolders (`deep`); folders
    /// and items whose name starts with "." left out or not (`skip_hidden`): SecondBot's
    /// GetAllItems keeps them, its AllSubfolderWearables does not.
    pub fn items(&self, node: usize, deep: bool, skip_hidden: bool) -> Vec<InvItem> {
        let mut out: Vec<InvItem> = self.nodes[node]
            .items
            .iter()
            .filter(|i| wearable(i) && !(skip_hidden && hidden(&i.name)))
            .cloned()
            .collect();
        if deep {
            for c in &self.nodes[node].children {
                if !(skip_hidden && hidden(&self.nodes[*c].name)) {
                    out.extend(self.items(*c, true, skip_hidden));
                }
            }
        }
        out
    }

    /// @getinv: the names of a folder's subfolders, those starting with "." left out.
    pub fn subfolders(&self, node: usize) -> String {
        self.nodes[node]
            .children
            .iter()
            .map(|c| self.nodes[*c].name.as_str())
            .filter(|n| !hidden(n))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// @getinvworn: "|" and the folder's own two digits, then each subfolder's name, "|" and its
    /// digits, separated by commas.
    pub fn worn_status(&self, node: usize, worn: &HashSet<Uuid>) -> String {
        let mut parts = vec![format!("|{}", self.worn_indicator(node, worn))];
        for c in &self.nodes[node].children {
            let name = &self.nodes[*c].name;
            if !hidden(name) {
                parts.push(format!("{name}|{}", self.worn_indicator(*c, worn)));
            }
        }
        parts.join(",")
    }

    /// SecondBot's GetWornIndicator: a digit for the folder's own items, one for those of its
    /// subfolders: 0 none, 1 none worn, 2 some worn, 3 all worn.
    fn worn_indicator(&self, node: usize, worn: &HashSet<Uuid>) -> String {
        let digit = |items: &[InvItem]| {
            let on = items.iter().filter(|i| worn.contains(&i.id)).count();
            match (items.len(), on) {
                (0, _) => '0',
                (n, on) if n == on => '3',
                (_, 0) => '1',
                _ => '2',
            }
        };
        let own = self.items(node, false, true);
        let below: Vec<InvItem> = self.nodes[node]
            .children
            .iter()
            .filter(|c| !hidden(&self.nodes[**c].name))
            .flat_map(|c| self.items(*c, true, true))
            .collect();
        format!("{}{}", digit(&own), digit(&below))
    }

    /// @findfolder and @findfolders, as the viewer (Aurora's RLVa): the folders under the root
    /// whose name holds every word, those starting with "." or "~" left out; their paths, or the
    /// deepest one's (the first of those as deep).
    pub fn find_words(&self, words: &[String], all: bool) -> String {
        if words.is_empty() {
            return String::new();
        }
        let found = (1..self.nodes.len()).filter(|n| {
            let name = self.nodes[*n].name.to_lowercase();
            !hidden(&name) && !name.starts_with('~') && words.iter().all(|w| name.contains(w))
        });
        let paths: Vec<String> = found.map(|n| self.path(n)).collect();
        if all {
            return paths.join(",");
        }
        let mut best: Option<&String> = None;
        for p in &paths {
            if best.is_none_or(|b| p.matches('/').count() > b.matches('/').count()) {
                best = Some(p);
            }
        }
        best.cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferro_client::inventory::InvFolder;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn folder(n: u128, parent: u128, name: &str) -> InvFolder {
        InvFolder {
            id: id(n),
            parent: id(parent),
            name: name.into(),
            type_default: -1,
            ..Default::default()
        }
    }

    fn item(n: u128, name: &str, asset_type: i32) -> InvItem {
        InvItem {
            id: id(n),
            name: name.into(),
            asset_type,
            ..Default::default()
        }
    }

    fn contents(n: u128, folders: Vec<InvFolder>, items: Vec<InvItem>) -> FolderContents {
        FolderContents {
            folder_id: id(n),
            owner_id: Uuid::nil(),
            version: 1,
            folders,
            items,
        }
    }

    /// #RLV (1): Gear (2) with a collar and a notecard, Gear/Red Hats (3) with a hat,
    /// Gear/.hidden (4) with a ring, Spare (5) empty, ~Shared (6), Gear/Red Hats/Red (7).
    fn tree() -> Tree {
        Tree::build(
            id(1),
            SHARED,
            &[
                contents(
                    1,
                    vec![
                        folder(2, 1, "Gear"),
                        folder(5, 1, "Spare"),
                        folder(6, 1, "~Shared"),
                    ],
                    vec![],
                ),
                contents(
                    2,
                    vec![folder(3, 2, "Red Hats"), folder(4, 2, ".hidden")],
                    vec![
                        item(20, "Collar", OBJECT),
                        item(21, "Note", 7),
                        item(22, ".spare socks", CLOTHING),
                    ],
                ),
                contents(3, vec![folder(7, 3, "Red")], vec![item(30, "Hat", OBJECT)]),
                contents(4, vec![], vec![item(40, "Ring", OBJECT)]),
                contents(5, vec![], vec![]),
                contents(6, vec![], vec![]),
                contents(7, vec![], vec![item(70, "Shirt", CLOTHING)]),
            ],
        )
    }

    #[test]
    fn folders_by_path() {
        let t = tree();
        let at = |p: &str| t.find(p).map(|n| t.nodes[n].id);
        assert_eq!(at(""), Some(id(1)));
        assert_eq!(at("gear"), Some(id(2)));
        assert_eq!(at("/ Gear/red hats/"), Some(id(3)));
        assert_eq!(at("Gear/Red Hats/Red"), Some(id(7)));
        assert_eq!(at("Gear/Red"), None);
        assert_eq!(
            t.path(t.find("gear/red hats/red").expect("found")),
            "Gear/Red Hats/Red"
        );
        assert_eq!(t.path(0), "");
        assert_eq!(t.folder_of(id(30)), t.find("Gear/Red Hats"));
        assert_eq!(t.folder_of(id(99)), None);
    }

    #[test]
    fn items_and_subfolders() {
        let t = tree();
        let gear = t.find("Gear").expect("Gear");
        let names = |items: Vec<InvItem>| items.into_iter().map(|i| i.name).collect::<Vec<_>>();
        // the notecard cannot be worn
        assert_eq!(
            names(t.items(gear, false, false)),
            ["Collar", ".spare socks"]
        );
        assert_eq!(
            names(t.items(gear, true, false)),
            ["Collar", ".spare socks", "Hat", "Shirt", "Ring"]
        );
        assert_eq!(names(t.items(gear, true, true)), ["Collar", "Hat", "Shirt"]);
        assert_eq!(t.subfolders(0), "Gear,Spare,~Shared");
        assert_eq!(t.subfolders(gear), "Red Hats");
    }

    #[test]
    fn what_is_worn_of_each_folder() {
        let t = tree();
        let gear = t.find("Gear").expect("Gear");
        // the collar and the hat worn, not the shirt
        let worn = HashSet::from([id(20), id(30)]);
        assert_eq!(t.worn_status(gear, &worn), "|32,Red Hats|31");
        assert_eq!(t.worn_status(0, &worn), "|02,Gear|32,Spare|00,~Shared|00");
        assert_eq!(t.worn_status(gear, &HashSet::new()), "|11,Red Hats|11");
    }

    #[test]
    fn folders_by_words() {
        let t = tree();
        let words = |w: &[&str]| w.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            t.find_words(&words(&["red"]), true),
            "Gear/Red Hats,Gear/Red Hats/Red"
        );
        // the deepest
        assert_eq!(t.find_words(&words(&["red"]), false), "Gear/Red Hats/Red");
        assert_eq!(
            t.find_words(&words(&["red", "hat"]), false),
            "Gear/Red Hats"
        );
        // hidden and "~" folders are not found
        assert_eq!(t.find_words(&words(&["hidden"]), true), "");
        assert_eq!(t.find_words(&words(&["shared"]), true), "");
        assert_eq!(t.find_words(&[], true), "");
    }
}
