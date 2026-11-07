//! User settings, saved as TOML in the OS config directory.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const DEFAULT_API_URL: &str = "https://wow.pestering-curiosity.com/greedygoblin";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Region {
    Eu,
    Us,
    Kr,
    Tw,
}

impl Region {
    pub const ALL: [Region; 4] = [Region::Eu, Region::Us, Region::Kr, Region::Tw];

    pub fn api_name(self) -> &'static str {
        match self {
            Region::Eu => "eu",
            Region::Us => "us",
            Region::Kr => "kr",
            Region::Tw => "tw",
        }
    }

    /// KR and TW have only PvP and Normal realms.
    pub fn has_ruleset(self, ruleset: Ruleset) -> bool {
        match self {
            Region::Eu | Region::Us => true,
            Region::Kr | Region::Tw => matches!(ruleset, Ruleset::Pvp | Ruleset::Normal),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ruleset {
    #[serde(rename = "pvp")]
    Pvp,
    #[serde(rename = "normal")]
    Normal,
    #[serde(rename = "rp")]
    Rp,
    #[serde(rename = "rp-pvp")]
    RpPvp,
}

impl Ruleset {
    pub const ALL: [Ruleset; 4] = [Ruleset::Pvp, Ruleset::Normal, Ruleset::Rp, Ruleset::RpPvp];

    pub fn api_name(self) -> &'static str {
        match self {
            Ruleset::Pvp => "pvp",
            Ruleset::Normal => "normal",
            Ruleset::Rp => "rp",
            Ruleset::RpPvp => "rp-pvp",
        }
    }

    /// The ruleset as the add-on names it in its auction house key.
    pub fn addon_name(self) -> &'static str {
        match self {
            Ruleset::Pvp => "PvP",
            Ruleset::Normal => "PvE",
            Ruleset::Rp => "RP",
            Ruleset::RpPvp => "RP-PvP",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Faction {
    Alliance,
    Horde,
}

impl Faction {
    pub const ALL: [Faction; 2] = [Faction::Alliance, Faction::Horde];

    pub fn api_name(self) -> &'static str {
        match self {
            Faction::Alliance => "alliance",
            Faction::Horde => "horde",
        }
    }

    pub fn addon_name(self) -> &'static str {
        match self {
            Faction::Alliance => "Alliance",
            Faction::Horde => "Horde",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Path of `Wow.exe`.
    pub wow_exe: Option<PathBuf>,
    pub region: Region,
    pub ruleset: Ruleset,
    pub faction: Faction,
    /// Base URL of the API, without `/v1`.
    pub api_url: String,
    /// Whether the app downloads scans and writes the data add-on.
    pub enabled: bool,
    /// Whether the app starts at login. On by default on Windows only: on
    /// Linux the app is used in development.
    pub start_at_login: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            wow_exe: default_wow_exe(),
            region: Region::Eu,
            ruleset: Ruleset::Pvp,
            faction: Faction::Alliance,
            api_url: DEFAULT_API_URL.to_owned(),
            enabled: true,
            start_at_login: cfg!(windows),
        }
    }
}

fn default_wow_exe() -> Option<PathBuf> {
    if cfg!(windows) {
        Some(PathBuf::from(
            r"C:\Program Files (x86)\World of Warcraft\_forever_\Wow.exe",
        ))
    } else {
        None
    }
}

impl Config {
    /// The add-on's auction house key, e.g. `"PvP Alliance"`.
    pub fn addon_key(&self) -> String {
        format!(
            "{} {}",
            self.ruleset.addon_name(),
            self.faction.addon_name()
        )
    }

    /// A file name that identifies the auction house, for its local state.
    pub fn auction_house_slug(&self) -> String {
        format!(
            "{}-{}-{}",
            self.region.api_name(),
            self.ruleset.api_name(),
            self.faction.api_name()
        )
    }

    /// Whether a sync with `other` could give a different result.
    pub fn same_sync(&self, other: &Config) -> bool {
        self.wow_exe == other.wow_exe
            && self.region == other.region
            && self.ruleset == other.ruleset
            && self.faction == other.faction
            && self.api_url == other.api_url
            && self.enabled == other.enabled
    }

    /// Makes the ruleset valid for the region.
    pub fn normalize(&mut self) {
        if !self.region.has_ruleset(self.ruleset) {
            self.ruleset = Ruleset::Pvp;
        }
    }

    /// Loads the config; a missing file gives the defaults.
    pub fn load(path: &Path) -> Result<Self, String> {
        let mut config: Config = match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        config.normalize();
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string(self).map_err(|e| e.to_string())?;
        crate::paths::write_atomic(path, text.as_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let config = Config {
            wow_exe: Some(PathBuf::from("/games/wow/Wow.exe")),
            region: Region::Us,
            ruleset: Ruleset::RpPvp,
            faction: Faction::Horde,
            api_url: "http://127.0.0.1:7008".to_owned(),
            enabled: false,
            start_at_login: true,
        };
        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("ruleset = \"rp-pvp\""));
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), config);
    }

    #[test]
    fn missing_fields_take_defaults() {
        let config: Config = toml::from_str("faction = \"horde\"").unwrap();
        assert_eq!(config.faction, Faction::Horde);
        assert_eq!(config.region, Region::Eu);
        assert_eq!(config.api_url, DEFAULT_API_URL);
        assert!(config.enabled);
    }

    #[test]
    fn keys() {
        let mut config = Config::default();
        assert_eq!(config.addon_key(), "PvP Alliance");
        config.ruleset = Ruleset::Normal;
        config.faction = Faction::Horde;
        assert_eq!(config.addon_key(), "PvE Horde");
        assert_eq!(config.auction_house_slug(), "eu-normal-horde");
    }

    #[test]
    fn normalize_fixes_ruleset_for_kr_and_tw() {
        let mut config = Config {
            region: Region::Kr,
            ruleset: Ruleset::Rp,
            ..Config::default()
        };
        config.normalize();
        assert_eq!(config.ruleset, Ruleset::Pvp);
    }
}
