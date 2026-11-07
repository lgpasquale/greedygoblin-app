//! One sync: download the new scans of the configured auction house, fold them
//! into its local state and write the data add-on.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

use crate::api::{Api, AuctionHouse, SCAN_PAGE_SIZE, ScanList};
use crate::config::Config;
use crate::day::{day_of, day_start, first_kept_day};
use crate::error::SyncError;
use crate::game::Game;
use crate::log;
use crate::paths::write_atomic;
use crate::state::{Scan, State};

/// Where the scans come from: the API, or a fake in tests.
pub trait Source {
    fn auction_houses(&self) -> Result<Vec<AuctionHouse>, SyncError>;
    fn scans_url(&self, auction_house_id: u32, after: Option<u64>, from: i64) -> String;
    fn scans(&self, url: &str, etag: Option<&str>) -> Result<ScanList, SyncError>;
    fn scan(&self, auction_house_id: u32, scan_id: u64) -> Result<Option<Scan>, SyncError>;
}

impl Source for Api {
    fn auction_houses(&self) -> Result<Vec<AuctionHouse>, SyncError> {
        Api::auction_houses(self)
    }
    fn scans_url(&self, auction_house_id: u32, after: Option<u64>, from: i64) -> String {
        Api::scans_url(self, auction_house_id, after, from)
    }
    fn scans(&self, url: &str, etag: Option<&str>) -> Result<ScanList, SyncError> {
        Api::scans(self, url, etag)
    }
    fn scan(&self, auction_house_id: u32, scan_id: u64) -> Result<Option<Scan>, SyncError> {
        Api::scan(self, auction_house_id, scan_id)
    }
}

pub struct Syncer {
    state_dir: PathBuf,
    day_0: i64,
}

impl Syncer {
    pub fn new(state_dir: PathBuf, day_0: i64) -> Self {
        Self { state_dir, day_0 }
    }

    fn state_path(&self, config: &Config) -> PathBuf {
        self.state_dir
            .join(format!("{}.json.gz", config.auction_house_slug()))
    }

    /// Records what `Data.lua` was last written from.
    fn written_path(&self) -> PathBuf {
        self.state_dir.join("written.txt")
    }

    /// `ts` of the newest scan downloaded for the configured auction house.
    pub fn last_scan_ts(&self, config: &Config) -> Option<i64> {
        load_state(&self.state_path(config))?.last_scan_ts
    }

    /// Syncs; returns the `ts` of the newest scan downloaded.
    pub fn sync(
        &self,
        source: &impl Source,
        config: &Config,
        now: i64,
    ) -> Result<Option<i64>, SyncError> {
        let game = Game::open(config.wow_exe.as_deref())?;
        let interface = game.interface_version()?;
        let retention = game.retention_days();
        let first_day = first_kept_day(day_of(now, self.day_0), retention);
        let window_from = day_start(first_day, self.day_0);

        let state_path = self.state_path(config);
        let mut state = match load_state(&state_path) {
            Some(state) if state.from <= window_from => state,
            Some(_) => {
                log!("retention grew to {retention} days: downloading the whole window again");
                self.new_state(source, config, window_from)?
            }
            None => self.new_state(source, config, window_from)?,
        };

        let downloaded = self.download(source, &mut state, first_day);
        state.prune(first_day);
        save_state(&state_path, &state)
            .map_err(|e| SyncError::NeedsUser(format!("cannot save state: {e}")))?;
        downloaded?;

        game.write_toc(&interface)?;
        let written = format!(
            "{}\n{}\n{:?}\n{first_day}\n",
            game.dir().display(),
            config.auction_house_slug(),
            state.last_scan_id
        );
        let written_path = self.written_path();
        let up_to_date = game.data_lua_exists()
            && fs::read_to_string(&written_path).is_ok_and(|old| old == written);
        if !up_to_date {
            let cbor = state.encode(&config.addon_key(), config.region.api_name());
            game.write_data(&cbor, now)?;
            log!(
                "wrote Data.lua: {} items, {} days, {} bytes of CBOR",
                state.items.len(),
                state.days.len(),
                cbor.len()
            );
            write_atomic(&written_path, written.as_bytes())
                .map_err(|e| SyncError::NeedsUser(format!("cannot save state: {e}")))?;
        }
        Ok(state.last_scan_ts)
    }

    fn new_state(
        &self,
        source: &impl Source,
        config: &Config,
        window_from: i64,
    ) -> Result<State, SyncError> {
        let region = config.region.api_name();
        let ruleset = config.ruleset.api_name();
        let faction = config.faction.api_name();
        let id = source
            .auction_houses()?
            .into_iter()
            .find(|ah| {
                ah.region == region
                    && ah.ruleset.as_deref() == Some(ruleset)
                    && ah.faction == faction
            })
            .ok_or_else(|| {
                SyncError::NeedsUser(format!(
                    "auction house {region} {ruleset} {faction} not found on the server"
                ))
            })?
            .id;
        Ok(State::new(id, window_from))
    }

    /// Downloads and folds the new scans. On error, the scans folded so far
    /// stay in the state.
    fn download(
        &self,
        source: &impl Source,
        state: &mut State,
        first_day: i64,
    ) -> Result<(), SyncError> {
        let id = state.auction_house_id;
        loop {
            let url = source.scans_url(id, state.last_scan_id, state.from);
            let etag = state
                .scans_etag
                .as_ref()
                .filter(|(u, _)| *u == url)
                .map(|(_, e)| e.as_str());
            let (scans, etag) = match source.scans(&url, etag)? {
                ScanList::NotModified => return Ok(()),
                ScanList::Scans { scans, etag } => (scans, etag),
            };
            let full_page = scans.len() >= SCAN_PAGE_SIZE;
            if !scans.is_empty() {
                log!("auction house {id}: {} new scans", scans.len());
            }
            for info in scans {
                if info.has_data() {
                    match source.scan(id, info.id)? {
                        Some(scan) if day_of(scan.ts, self.day_0) >= first_day => {
                            state.apply_scan(&scan, self.day_0)
                        }
                        Some(_) => {}
                        None => log!("scan {} has no data", info.id),
                    }
                }
                state.last_scan_id = Some(info.id);
            }
            state.scans_etag = etag.map(|etag| (url, etag));
            if !full_page {
                return Ok(());
            }
        }
    }
}

fn load_state(path: &Path) -> Option<State> {
    let file = fs::File::open(path).ok()?;
    let mut json = Vec::new();
    if let Err(e) = GzDecoder::new(file).read_to_end(&mut json) {
        log!("cannot read {}: {e}", path.display());
        return None;
    }
    match serde_json::from_slice(&json) {
        Ok(state) => Some(state),
        Err(e) => {
            log!("cannot read {}: {e}", path.display());
            None
        }
    }
}

fn save_state(path: &Path, state: &State) -> std::io::Result<()> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    serde_json::to_writer(&mut gz, state)?;
    gz.flush()?;
    write_atomic(path, &gz.finish()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ScanInfo;
    use crate::day::SECONDS_PER_DAY;
    use crate::state::ScanItem;
    use std::cell::RefCell;

    const DAY_0: i64 = 1_577_836_800;
    const TODAY: i64 = 2500;

    fn at(day: i64, hour: i64) -> i64 {
        DAY_0 + day * SECONDS_PER_DAY + hour * 3600
    }

    /// id, status, ts, items as (item_id, min_price, quantity).
    type FakeScan = (u64, &'static str, i64, Vec<(u32, i64, i64)>);

    /// A fake server: scans of auction house 7, by id.
    struct Fake {
        scans: Vec<FakeScan>,
        requests: RefCell<Vec<String>>,
        downloads: RefCell<Vec<u64>>,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                scans: Vec::new(),
                requests: RefCell::new(Vec::new()),
                downloads: RefCell::new(Vec::new()),
            }
        }
    }

    impl Source for Fake {
        fn auction_houses(&self) -> Result<Vec<AuctionHouse>, SyncError> {
            Ok(vec![
                AuctionHouse {
                    id: 3,
                    region: "eu".into(),
                    ruleset: Some("pvp".into()),
                    faction: "horde".into(),
                },
                AuctionHouse {
                    id: 7,
                    region: "eu".into(),
                    ruleset: Some("pvp".into()),
                    faction: "alliance".into(),
                },
            ])
        }

        fn scans_url(&self, id: u32, after: Option<u64>, from: i64) -> String {
            format!("{id}?after={after:?}&from={from}")
        }

        fn scans(&self, url: &str, etag: Option<&str>) -> Result<ScanList, SyncError> {
            self.requests.borrow_mut().push(url.to_owned());
            let tag = format!("n{}", self.scans.len());
            if etag == Some(tag.as_str()) {
                return Ok(ScanList::NotModified);
            }
            let after: Option<u64> = url
                .split("after=Some(")
                .nth(1)
                .map(|s| s.split(')').next().unwrap().parse().unwrap());
            let from: i64 = url.split("from=").nth(1).unwrap().parse().unwrap();
            let scans = self
                .scans
                .iter()
                .filter(|s| match after {
                    Some(after) => s.0 > after,
                    None => s.2 >= from,
                })
                .map(|s| ScanInfo {
                    id: s.0,
                    status: s.1.to_owned(),
                })
                .collect();
            Ok(ScanList::Scans {
                scans,
                etag: Some(tag),
            })
        }

        fn scan(&self, id: u32, scan_id: u64) -> Result<Option<Scan>, SyncError> {
            assert_eq!(id, 7);
            self.downloads.borrow_mut().push(scan_id);
            let s = self.scans.iter().find(|s| s.0 == scan_id).unwrap();
            if s.1 == "error" {
                return Ok(None);
            }
            Ok(Some(Scan {
                ts: s.2,
                last_modified: s.2 - 60,
                items: s
                    .3
                    .iter()
                    .map(|&(item_id, min_price, quantity)| ScanItem {
                        item_id,
                        min_price,
                        quantity,
                    })
                    .collect(),
            }))
        }
    }

    struct Setup {
        root: PathBuf,
        config: Config,
        syncer: Syncer,
    }

    impl Setup {
        fn new(name: &str, retention: i64) -> Self {
            let root = std::env::temp_dir().join(format!("gg-sync-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let game = root.join("game");
            let addon = game.join("Interface/AddOns/GreedyGoblin");
            fs::create_dir_all(&addon).unwrap();
            fs::write(game.join("Wow.exe"), b"").unwrap();
            fs::write(
                addon.join("GreedyGoblin_Camelot.toc"),
                "## Interface: 16001\n",
            )
            .unwrap();
            let sv = game.join("WTF/Account/A/SavedVariables");
            fs::create_dir_all(&sv).unwrap();
            fs::write(
                sv.join("GreedyGoblin.lua"),
                format!("[\"priceHistoryDays\"] = {retention},"),
            )
            .unwrap();
            let config = Config {
                wow_exe: Some(game.join("Wow.exe")),
                ..Config::default()
            };
            let syncer = Syncer::new(root.join("state"), DAY_0);
            Self {
                root,
                config,
                syncer,
            }
        }

        fn data_lua(&self) -> PathBuf {
            self.root
                .join("game/Interface/AddOns/GreedyGoblin_Data/Data.lua")
        }

        fn set_retention(&self, retention: i64) {
            fs::write(
                self.root
                    .join("game/WTF/Account/A/SavedVariables/GreedyGoblin.lua"),
                format!("[\"priceHistoryDays\"] = {retention},"),
            )
            .unwrap();
        }
    }

    impl Drop for Setup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn first_sync_then_incremental() {
        let setup = Setup::new("incremental", 2);
        let mut fake = Fake::new();
        fake.scans = vec![
            (1, "ok", at(TODAY - 2, 10), vec![(10, 100, 1)]), // outside the window
            (2, "ok", at(TODAY - 1, 10), vec![(10, 200, 1)]),
            (3, "error", at(TODAY - 1, 14), vec![]),
            (4, "unchanged", at(TODAY - 1, 18), vec![(10, 200, 1)]),
        ];
        let now = at(TODAY, 12);

        let last = setup.syncer.sync(&fake, &setup.config, now).unwrap();
        assert_eq!(last, Some(at(TODAY - 1, 18)));
        // The first list starts at the window (2 days: yesterday and today).
        assert_eq!(
            fake.requests.borrow()[0],
            format!("7?after=None&from={}", at(TODAY - 1, 0))
        );
        assert_eq!(*fake.downloads.borrow(), [2, 4]);
        let data = fs::read_to_string(setup.data_lua()).unwrap();
        assert!(data.contains(&format!("generated = {now},")));
        let toc = setup.data_lua().with_file_name("GreedyGoblin_Data.toc");
        assert!(
            fs::read_to_string(toc)
                .unwrap()
                .starts_with("## Interface: 16001\n")
        );

        // Nothing new: Data.lua is not rewritten.
        let later = now + 3600;
        setup.syncer.sync(&fake, &setup.config, later).unwrap();
        assert_eq!(
            *fake.requests.borrow().last().unwrap(),
            format!("7?after=Some(4)&from={}", at(TODAY - 1, 0))
        );
        assert_eq!(fs::read_to_string(setup.data_lua()).unwrap(), data);

        // A new scan: only that one is downloaded.
        fake.scans
            .push((5, "ok", at(TODAY, 13), vec![(10, 300, 2)]));
        let last = setup.syncer.sync(&fake, &setup.config, later).unwrap();
        assert_eq!(last, Some(at(TODAY, 13)));
        assert_eq!(*fake.downloads.borrow(), [2, 4, 5]);
        assert!(
            fs::read_to_string(setup.data_lua())
                .unwrap()
                .contains(&format!("generated = {later},"))
        );
        assert_eq!(
            setup.syncer.last_scan_ts(&setup.config),
            Some(at(TODAY, 13))
        );
    }

    #[test]
    fn grown_retention_downloads_the_window_again() {
        let setup = Setup::new("retention", 1);
        let mut fake = Fake::new();
        fake.scans = vec![
            (1, "ok", at(TODAY - 1, 10), vec![(10, 100, 1)]),
            (2, "ok", at(TODAY, 10), vec![(10, 200, 1)]),
        ];
        let now = at(TODAY, 12);
        setup.syncer.sync(&fake, &setup.config, now).unwrap();
        assert_eq!(*fake.downloads.borrow(), [2]);

        setup.set_retention(2);
        setup.syncer.sync(&fake, &setup.config, now).unwrap();
        assert_eq!(*fake.downloads.borrow(), [2, 1, 2]);
    }

    #[test]
    fn missing_addon_needs_the_user() {
        let setup = Setup::new("missing", 90);
        fs::remove_dir_all(setup.root.join("game/Interface/AddOns/GreedyGoblin")).unwrap();
        let err = setup
            .syncer
            .sync(&Fake::new(), &setup.config, at(TODAY, 0))
            .unwrap_err();
        assert_eq!(
            err,
            SyncError::NeedsUser("GreedyGoblin add-on not found".to_owned())
        );
    }
}
