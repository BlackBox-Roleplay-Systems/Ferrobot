//! What the probe tells: one JSON line per step, as it happens, then a table of the steps by kind
//! (how many went right, the median and the longest time). The lines keep the Tranquillity rig's
//! fields, so that its results and the probe's compare side by side.

use glam::Vec3;
use std::collections::BTreeMap;

/// One step: a login, a teleport, a crossing.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// The bot's name, when several run at once.
    pub bot: String,
    /// The step's kind and round: `near-tp-to-Beta#2`.
    pub what: String,
    pub ok: bool,
    pub ms: u64,
    /// Where the bot is afterwards.
    pub region: String,
    pub pos: Vec3,
    /// The neighbour regions it keeps a circuit to.
    pub neighbours: Vec<String>,
    /// Why it went wrong, or what else is worth knowing.
    pub note: String,
}

impl Record {
    pub fn json(&self) -> String {
        serde_json::json!({
            "bot": self.bot,
            "what": self.what,
            "ok": self.ok,
            "ms": self.ms,
            "region": self.region,
            "pos": format!("<{:.2}, {:.2}, {:.2}>", self.pos.x, self.pos.y, self.pos.z),
            "sims": 1 + self.neighbours.len(),
            "neighbours": self.neighbours,
            "note": self.note,
        })
        .to_string()
    }

    /// The step's kind, without its round.
    pub fn kind(&self) -> &str {
        self.what.split('#').next().unwrap_or(&self.what)
    }
}

/// The table: one row per kind of step, in the order they first came.
pub fn summary(records: &[Record]) -> String {
    let mut order: Vec<&str> = Vec::new();
    let mut by_kind: BTreeMap<&str, Vec<&Record>> = BTreeMap::new();
    for r in records {
        let kind = r.kind();
        if !by_kind.contains_key(kind) {
            order.push(kind);
        }
        by_kind.entry(kind).or_default().push(r);
    }
    let mut out = String::new();
    for kind in order {
        let rs = &by_kind[kind];
        let ok = rs.iter().filter(|r| r.ok).count();
        let mut ms: Vec<u64> = rs.iter().map(|r| r.ms).collect();
        ms.sort_unstable();
        // the upper median, as the rig's summ.py
        let median = ms[ms.len() / 2];
        let max = ms[ms.len() - 1];
        out.push_str(&format!(
            "{kind:<28} {ok}/{} ok  median {median:>6} ms  max {max:>6} ms\n",
            rs.len()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(what: &str, ok: bool, ms: u64) -> Record {
        Record {
            bot: "Test User".into(),
            what: what.into(),
            ok,
            ms,
            region: "Alpha".into(),
            pos: Vec3::new(1.0, 2.0, 3.0),
            neighbours: vec!["Beta".into()],
            note: String::new(),
        }
    }

    #[test]
    fn a_line_has_the_rigs_fields() {
        let v: serde_json::Value =
            serde_json::from_str(&rec("local-tp#1", true, 14).json()).expect("json");
        assert_eq!(v["what"], "local-tp#1");
        assert_eq!(v["bot"], "Test User");
        assert_eq!(v["ok"], true);
        assert_eq!(v["ms"], 14);
        assert_eq!(v["region"], "Alpha");
        assert_eq!(v["pos"], "<1.00, 2.00, 3.00>");
        assert_eq!(v["sims"], 2);
    }

    #[test]
    fn the_table_counts_by_kind_in_order() {
        let t = summary(&[
            rec("login", true, 700),
            rec("local-tp#1", true, 10),
            rec("local-tp#2", false, 30),
            rec("local-tp#3", true, 20),
        ]);
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("login "), "{t}");
        assert!(lines[1].starts_with("local-tp "), "{t}");
        assert!(lines[1].contains("2/3 ok"), "{t}");
        assert!(lines[1].contains("median     20 ms"), "{t}");
        assert!(lines[1].contains("max     30 ms"), "{t}");
    }
}
