//! The local price history of one auction house, built by folding the server's
//! scans into the add-on's daily price model, and its encoding for the data
//! add-on.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::cbor::Encoder;
use crate::day::day_of;

/// The server combines all battle pet cages under this item; the add-on keys
/// pets by species, so this item is dropped.
pub const PET_CAGE_ITEM_ID: u32 = 82800;

/// One scan of one auction house, as downloaded.
pub struct Scan {
    /// Unix time the scan started.
    pub ts: i64,
    /// Unix time Blizzard produced the data.
    pub last_modified: i64,
    pub items: Vec<ScanItem>,
}

pub struct ScanItem {
    pub item_id: u32,
    pub min_price: i64,
    pub quantity: i64,
}

/// Prices of one item on one day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayPrice {
    /// Highest of the day's minimum prices.
    pub h: i64,
    /// Lowest of the day's minimum prices (encoded only when below `h`).
    pub l: i64,
    /// Quantity in the day's last scan; `None` when the item was not in it.
    pub q: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// Newest minimum price.
    pub m: i64,
    /// `last_modified` of the scan `m` comes from.
    pub t: i64,
    /// `ts` of the scan `m` comes from, to keep the newest one.
    pub seen: i64,
    pub days: BTreeMap<i64, DayPrice>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// API id of the auction house.
    pub auction_house_id: u32,
    /// Start (unix time) of the window covered by the first download.
    pub from: i64,
    pub last_scan_id: Option<u64>,
    /// `ts` of the newest scan folded in.
    pub last_scan_ts: Option<i64>,
    /// URL and `ETag` of the last scan list, to ask for a 304.
    pub scans_etag: Option<(String, String)>,
    /// Per day: `ts` of its last scan.
    pub days: BTreeMap<i64, i64>,
    pub items: BTreeMap<u32, Item>,
}

impl State {
    pub fn new(auction_house_id: u32, from: i64) -> Self {
        Self {
            auction_house_id,
            from,
            ..Self::default()
        }
    }

    /// Folds one scan in. Scans must be applied oldest first.
    pub fn apply_scan(&mut self, scan: &Scan, day_0: i64) {
        let day = day_of(scan.ts, day_0);
        let is_last_of_day = self.days.get(&day).is_none_or(|&ts| scan.ts >= ts);
        if is_last_of_day {
            // The quantities of a day come from its last scan only: an item
            // missing from it had quantity 0.
            for item in self.items.values_mut() {
                if let Some(price) = item.days.get_mut(&day) {
                    price.q = None;
                }
            }
            self.days.insert(day, scan.ts);
        }

        for scan_item in &scan.items {
            if scan_item.item_id == PET_CAGE_ITEM_ID {
                continue;
            }
            let price = scan_item.min_price;
            let item = self.items.entry(scan_item.item_id).or_default();
            let day_price = item.days.entry(day).or_insert(DayPrice {
                h: price,
                l: price,
                q: None,
            });
            day_price.h = day_price.h.max(price);
            day_price.l = day_price.l.min(price);
            if is_last_of_day {
                day_price.q = Some(scan_item.quantity);
            }
            if scan.ts >= item.seen {
                item.m = price;
                item.t = scan.last_modified;
                item.seen = scan.ts;
            }
        }

        if self.last_scan_ts.is_none_or(|ts| scan.ts > ts) {
            self.last_scan_ts = Some(scan.ts);
        }
    }

    /// Drops the days before `first_day`, and the items left without days.
    /// Returns whether anything was dropped.
    pub fn prune(&mut self, first_day: i64) -> bool {
        let before = self.days.len();
        self.days = self.days.split_off(&first_day);
        let mut changed = self.days.len() != before;
        self.items.retain(|_, item| {
            let before = item.days.len();
            item.days = item.days.split_off(&first_day);
            changed |= item.days.len() != before;
            !item.days.is_empty()
        });
        changed
    }

    /// Encodes the state as the CBOR value the data add-on carries.
    pub fn encode(&self, key: &str, region: &str) -> Vec<u8> {
        let mut e = Encoder::new();
        e.map(5);
        e.text("key");
        e.text(key);
        e.text("region");
        e.text(region);
        e.text("lastScan");
        e.int(self.last_scan_ts.unwrap_or(0));

        e.text("days");
        e.map(self.days.len());
        for (day, ts) in &self.days {
            e.text(&day.to_string());
            e.int(*ts);
        }

        e.text("items");
        e.map(self.items.len());
        for (item_id, item) in &self.items {
            e.text(&item_id.to_string());
            e.map(5);
            e.text("m");
            e.int(item.m);
            e.text("t");
            e.int(item.t);

            e.text("h");
            e.map(item.days.len());
            for (day, price) in &item.days {
                e.text(&day.to_string());
                e.int(price.h);
            }

            let lows: Vec<_> = item.days.iter().filter(|(_, p)| p.l < p.h).collect();
            e.text("l");
            e.map(lows.len());
            for (day, price) in lows {
                e.text(&day.to_string());
                e.int(price.l);
            }

            let quantities: Vec<_> = item
                .days
                .iter()
                .filter_map(|(day, p)| p.q.map(|q| (day, q)))
                .collect();
            e.text("q");
            e.map(quantities.len());
            for (day, q) in quantities {
                e.text(&day.to_string());
                e.int(q);
            }
        }
        e.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::day::SECONDS_PER_DAY;

    const DAY_0: i64 = 1_577_836_800;
    const DAY: i64 = 2500;

    fn at(day: i64, hour: i64) -> i64 {
        DAY_0 + day * SECONDS_PER_DAY + hour * 3600
    }

    fn scan(ts: i64, items: &[(u32, i64, i64)]) -> Scan {
        Scan {
            ts,
            last_modified: ts - 60,
            items: items
                .iter()
                .map(|&(item_id, min_price, quantity)| ScanItem {
                    item_id,
                    min_price,
                    quantity,
                })
                .collect(),
        }
    }

    #[test]
    fn folds_a_day_of_scans() {
        let mut s = State::new(1, 0);
        s.apply_scan(&scan(at(DAY, 1), &[(10, 500, 3), (20, 70, 1)]), DAY_0);
        s.apply_scan(&scan(at(DAY, 5), &[(10, 800, 2)]), DAY_0);
        s.apply_scan(&scan(at(DAY, 9), &[(10, 600, 4)]), DAY_0);

        let a = &s.items[&10];
        assert_eq!(
            a.days[&DAY],
            DayPrice {
                h: 800,
                l: 500,
                q: Some(4)
            }
        );
        assert_eq!((a.m, a.t), (600, at(DAY, 9) - 60));

        // Item 20 was not in the day's last scan: quantity 0, so no q.
        let b = &s.items[&20];
        assert_eq!(
            b.days[&DAY],
            DayPrice {
                h: 70,
                l: 70,
                q: None
            }
        );
        assert_eq!((b.m, b.t), (70, at(DAY, 1) - 60));

        assert_eq!(s.days[&DAY], at(DAY, 9));
        assert_eq!(s.last_scan_ts, Some(at(DAY, 9)));
    }

    #[test]
    fn days_are_separate() {
        let mut s = State::new(1, 0);
        s.apply_scan(&scan(at(DAY, 23), &[(10, 500, 3)]), DAY_0);
        s.apply_scan(&scan(at(DAY + 1, 1), &[(10, 900, 1)]), DAY_0);
        let a = &s.items[&10];
        assert_eq!(a.days[&DAY].h, 500);
        assert_eq!(a.days[&DAY].q, Some(3));
        assert_eq!(a.days[&(DAY + 1)].h, 900);
        assert_eq!(s.days.len(), 2);
    }

    #[test]
    fn drops_pet_cages() {
        let mut s = State::new(1, 0);
        s.apply_scan(
            &scan(at(DAY, 1), &[(PET_CAGE_ITEM_ID, 100, 1), (10, 5, 1)]),
            DAY_0,
        );
        assert!(!s.items.contains_key(&PET_CAGE_ITEM_ID));
        assert!(s.items.contains_key(&10));
    }

    #[test]
    fn prune_drops_old_days_and_empty_items() {
        let mut s = State::new(1, 0);
        s.apply_scan(&scan(at(DAY, 1), &[(10, 500, 3), (20, 70, 1)]), DAY_0);
        s.apply_scan(&scan(at(DAY + 1, 1), &[(10, 600, 3)]), DAY_0);
        assert!(s.prune(DAY + 1));
        assert!(!s.items.contains_key(&20));
        assert_eq!(s.items[&10].days.len(), 1);
        assert_eq!(s.days.keys().copied().collect::<Vec<_>>(), [DAY + 1]);
        assert!(!s.prune(DAY + 1));
    }

    #[test]
    fn encode_omits_l_not_below_h_and_missing_q() {
        let mut s = State::new(1, 0);
        s.apply_scan(&scan(at(DAY, 1), &[(10, 500, 3), (20, 7, 1)]), DAY_0);
        s.apply_scan(&scan(at(DAY, 5), &[(10, 800, 2)]), DAY_0);
        let bytes = s.encode("PvP Alliance", "eu");

        let mut e = Encoder::new();
        e.map(5);
        e.text("key");
        e.text("PvP Alliance");
        e.text("region");
        e.text("eu");
        e.text("lastScan");
        e.int(at(DAY, 5));
        e.text("days");
        e.map(1);
        e.text("2500");
        e.int(at(DAY, 5));
        e.text("items");
        e.map(2);
        // Item 10: l below h, q from the last scan.
        e.text("10");
        e.map(5);
        e.text("m");
        e.int(800);
        e.text("t");
        e.int(at(DAY, 5) - 60);
        e.text("h");
        e.map(1);
        e.text("2500");
        e.int(800);
        e.text("l");
        e.map(1);
        e.text("2500");
        e.int(500);
        e.text("q");
        e.map(1);
        e.text("2500");
        e.int(2);
        // Item 20: l equals h, absent from the last scan.
        e.text("20");
        e.map(5);
        e.text("m");
        e.int(7);
        e.text("t");
        e.int(at(DAY, 1) - 60);
        e.text("h");
        e.map(1);
        e.text("2500");
        e.int(7);
        e.text("l");
        e.map(0);
        e.text("q");
        e.map(0);

        assert_eq!(bytes, e.into_bytes());
    }
}
