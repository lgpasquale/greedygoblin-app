//! The game installation: the add-on's settings (read-only) and the data
//! add-on the app writes.

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::error::SyncError;
use crate::paths::write_atomic;

pub const DEFAULT_RETENTION_DAYS: i64 = 90;
pub const DATA_FORMAT_VERSION: i64 = 1;

const ADDON: &str = "GreedyGoblin";
const ADDON_TOC: &str = "GreedyGoblin_Camelot.toc";
const DATA_ADDON: &str = "GreedyGoblin_Data";

pub struct Game {
    /// The folder of `Wow.exe`.
    dir: PathBuf,
}

impl Game {
    /// Checks the installation: `Wow.exe` and the add-on must exist.
    pub fn open(wow_exe: Option<&Path>) -> Result<Self, SyncError> {
        let wow_exe =
            wow_exe.ok_or_else(|| SyncError::NeedsUser("WoW folder not set".to_owned()))?;
        if !wow_exe.is_file() {
            return Err(SyncError::NeedsUser(format!(
                "Wow.exe not found: {}",
                wow_exe.display()
            )));
        }
        let dir = wow_exe
            .parent()
            .ok_or_else(|| SyncError::NeedsUser("Wow.exe not found".to_owned()))?
            .to_owned();
        let game = Game { dir };
        if !game.addons_dir().join(ADDON).is_dir() {
            return Err(SyncError::NeedsUser(
                "GreedyGoblin add-on not found".to_owned(),
            ));
        }
        Ok(game)
    }

    fn addons_dir(&self) -> PathBuf {
        self.dir.join("Interface").join("AddOns")
    }

    pub fn data_addon_dir(&self) -> PathBuf {
        self.addons_dir().join(DATA_ADDON)
    }

    /// `## Interface` of the add-on's TOC.
    pub fn interface_version(&self) -> Result<String, SyncError> {
        let toc = self.addons_dir().join(ADDON).join(ADDON_TOC);
        let text = fs::read(&toc).map_err(|e| {
            SyncError::NeedsUser(format!(
                "GreedyGoblin add-on not found ({}: {e})",
                toc.display()
            ))
        })?;
        String::from_utf8_lossy(&text)
            .lines()
            .find_map(|line| line.strip_prefix("## Interface:"))
            .map(|v| v.trim().to_owned())
            .ok_or_else(|| {
                SyncError::NeedsUser(format!("no ## Interface line in {}", toc.display()))
            })
    }

    /// The add-on's `priceHistoryDays`: the largest of all accounts, or 90.
    pub fn retention_days(&self) -> i64 {
        let accounts = self.dir.join("WTF").join("Account");
        let Ok(entries) = fs::read_dir(&accounts) else {
            return DEFAULT_RETENTION_DAYS;
        };
        entries
            .flatten()
            .filter_map(|account| {
                let file = account
                    .path()
                    .join("SavedVariables")
                    .join("GreedyGoblin.lua");
                find_price_history_days(&fs::read(file).ok()?)
            })
            .max()
            .unwrap_or(DEFAULT_RETENTION_DAYS)
    }

    /// Writes `GreedyGoblin_Data.toc`, unless it is already up to date.
    pub fn write_toc(&self, interface: &str) -> Result<(), SyncError> {
        let path = self.data_addon_dir().join(format!("{DATA_ADDON}.toc"));
        let contents = toc(interface);
        if fs::read_to_string(&path).is_ok_and(|old| old == contents) {
            return Ok(());
        }
        write_atomic(&path, contents.as_bytes()).map_err(|e| self.cannot_write(e))
    }

    /// Writes `Data.lua`.
    pub fn write_data(&self, cbor: &[u8], generated: i64) -> Result<(), SyncError> {
        let path = self.data_addon_dir().join("Data.lua");
        write_atomic(&path, data_lua(cbor, generated).as_bytes()).map_err(|e| self.cannot_write(e))
    }

    fn cannot_write(&self, e: std::io::Error) -> SyncError {
        SyncError::NeedsUser(format!(
            "cannot write {}: {e}",
            self.data_addon_dir().display()
        ))
    }

    pub fn data_lua_exists(&self) -> bool {
        self.data_addon_dir().join("Data.lua").is_file()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// Finds `["priceHistoryDays"] = <n>` in the SavedVariables bytes, which are
/// not valid UTF-8.
fn find_price_history_days(bytes: &[u8]) -> Option<i64> {
    const NEEDLE: &[u8] = b"[\"priceHistoryDays\"] = ";
    let start = bytes.windows(NEEDLE.len()).position(|w| w == NEEDLE)? + NEEDLE.len();
    let digits: Vec<u8> = bytes[start..]
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .copied()
        .collect();
    std::str::from_utf8(&digits)
        .ok()?
        .parse()
        .ok()
        .filter(|&n| n > 0)
}

fn toc(interface: &str) -> String {
    format!(
        "## Interface: {interface}\n\
         ## Title: GreedyGoblin Data\n\
         ## Notes: Price history downloaded by the GreedyGoblin app\n\
         ## Dependencies: GreedyGoblin\n\
         \n\
         Data.lua\n"
    )
}

fn data_lua(cbor: &[u8], generated: i64) -> String {
    let mut zlib = ZlibEncoder::new(Vec::new(), Compression::best());
    std::io::Write::write_all(&mut zlib, cbor).expect("writing to a Vec cannot fail");
    let compressed = zlib.finish().expect("writing to a Vec cannot fail");
    let data = base64::engine::general_purpose::STANDARD.encode(compressed);
    format!(
        "GreedyGoblinData = {{\n\
         \tversion = {DATA_FORMAT_VERSION},\n\
         \tgenerated = {generated},\n\
         \tdata = \"{data}\",\n\
         }}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn finds_price_history_days_in_binary_file() {
        let mut bytes = b"GreedyGoblinDB = {\n[\"x\"] = \"\xff\xfe\",\n".to_vec();
        bytes.extend_from_slice(b"[\"ui\"] = {\n[\"priceHistoryDays\"] = 120,\n},\n}");
        assert_eq!(find_price_history_days(&bytes), Some(120));
        assert_eq!(find_price_history_days(b"[\"ui\"] = {}"), None);
    }

    #[test]
    fn data_lua_round_trips() {
        let lua = data_lua(b"hello", 1_793_950_887);
        assert!(
            lua.starts_with("GreedyGoblinData = {\n\tversion = 1,\n\tgenerated = 1793950887,\n")
        );
        let data = lua.split('"').nth(1).unwrap();
        let compressed = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap();
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(&compressed[..])
            .read_to_end(&mut out)
            .unwrap();
        assert_eq!(out, b"hello");
    }

    #[test]
    fn open_checks_wow_exe_and_addon() {
        let root = std::env::temp_dir().join(format!("gg-game-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let exe = root.join("Wow.exe");
        assert!(matches!(
            Game::open(Some(&exe)),
            Err(SyncError::NeedsUser(m)) if m.starts_with("Wow.exe not found")
        ));

        fs::create_dir_all(&root).unwrap();
        fs::write(&exe, b"").unwrap();
        assert!(matches!(
            Game::open(Some(&exe)),
            Err(SyncError::NeedsUser(m)) if m == "GreedyGoblin add-on not found"
        ));

        let addon = root.join("Interface/AddOns/GreedyGoblin");
        fs::create_dir_all(&addon).unwrap();
        fs::write(addon.join(ADDON_TOC), "## Interface: 16001\n## Title: x\n").unwrap();
        let sv = root.join("WTF/Account/A/SavedVariables");
        fs::create_dir_all(&sv).unwrap();
        fs::write(sv.join("GreedyGoblin.lua"), "[\"priceHistoryDays\"] = 30,").unwrap();
        let sv = root.join("WTF/Account/B/SavedVariables");
        fs::create_dir_all(&sv).unwrap();
        fs::write(sv.join("GreedyGoblin.lua"), "[\"priceHistoryDays\"] = 45,").unwrap();

        let game = Game::open(Some(&exe)).unwrap();
        assert_eq!(game.interface_version().unwrap(), "16001");
        assert_eq!(game.retention_days(), 45);

        game.write_toc("16001").unwrap();
        game.write_data(b"x", 1).unwrap();
        let toc = fs::read_to_string(game.data_addon_dir().join("GreedyGoblin_Data.toc")).unwrap();
        assert!(toc.starts_with("## Interface: 16001\n"));
        assert!(game.data_lua_exists());

        fs::remove_dir_all(&root).unwrap();
    }
}
