# CLAUDE.md

## Project

`greedygoblin-app` is a small desktop companion of the **GreedyGoblin** WoW add-on
(`../GreedyGoblin`, for **WoW: Forever**). It downloads the auction house scans
made by `greedygoblin-data-server` (`../greedygoblin-data-server`, a public
read-only HTTP API) and hands them to the add-on through a **data add-on**,
`GreedyGoblin_Data`. The add-on then has a full price history without scanning in
game.

- Platforms: **Windows and Linux** (on Linux the game runs under Wine, e.g.
  Bottles).
- **Incremental**: the first run downloads the whole history the add-on keeps
  (its retention); later runs download only the new scans.
- It is configured for **one auction house**: one region, one ruleset, one
  faction.

The TradeSkillMaster desktop app works the same way, as far as we know: it
writes `TradeSkillMaster_AppHelper/AppData.lua`, not SavedVariables. The game never writes
files under `Interface/AddOns`. So the app can write at any time, even while the
game runs. The add-on loads the new data at the next login or `/reload`.

## Conventions

- All code, comments, identifiers, commit messages and docs are in **English**.
- Prices are integers in **copper**. Never use floating point for prices.
- Build only what is asked. Do not add features, settings or data that were not
  requested; propose them instead.
- **Keep UI code to a minimum.** No windows: a system tray icon with a menu,
  desktop notifications and the native file dialog. Anything that would need a
  real window must be proposed first.
- **Never write the add-on's SavedVariables.** The app only reads
  `priceHistoryDays` from them. It writes only inside its own folder
  `Interface/AddOns/GreedyGoblin_Data/` and its own config and data
  directories.
- The main branch is `master`.

## Stack

- **Rust** (stable), a single binary.
- Tray icon and menu: `tray-icon` + `muda` with a `tao` (or `winit`) event loop.
  On Linux they need GTK and libayatana-appindicator (or libappindicator) at
  runtime.
- Desktop notifications: `notify-rust`.
- File dialog: `rfd` (native dialog on Windows, GTK/portal on Linux).
- HTTP: a client with rustls and gzip (`ureq` or `reqwest`).
- Zlib: `flate2`. CBOR: `ciborium` or a small hand-written encoder (only maps,
  text strings and integers). Base64: `base64`.
- Settings and state in the OS directories (`dirs` crate): config in
  `%APPDATA%` / `$XDG_CONFIG_HOME`, cache and log in `%LOCALAPPDATA%` /
  `$XDG_DATA_HOME`.
- Config file format: **TOML** (`toml` + `serde`).

## Tray menu

- **WoW folder**: choose the game installation by selecting `Wow.exe` in a file
  dialog. The label shows the current path.
  - Default on Windows: `C:\Program Files (x86)\World of Warcraft\_forever_\Wow.exe`.
  - Linux has no default. An example Bottles path is
    `~/.local/share/bottles/bottles/<bottle>/drive_c/Program Files (x86)/World of Warcraft/_forever_/Wow.exe`.
- **Region**: `eu` (default), `us`, `kr`, `tw`.
- **Ruleset**: `pvp` (default), `normal`, `rp`, `rp-pvp`. KR and TW have only
  `pvp` and `normal`.
- **Faction**: `alliance` (default), `horde`.
- **Last scan downloaded**: date and time of the newest scan in the data file.
  This item is disabled and only shows the information.
- The last error, only while there is one (see Errors).
- **Open log** (see Errors).
- **Sync enabled** (check item, on by default): when off, the app downloads
  no scans and does not write the data add-on. The tooltip says
  `Sync disabled`. The Windows self-update keeps running.
- **Start at login** (check item): the app adds itself to the programs started
  at login, through the `auto-launch` crate (a `Run` registry value for the
  current user on Windows, an XDG autostart entry on Linux). On by default on
  Windows, off on Linux, where the app is used only in development. The entry
  is written again at each start, so it follows the executable.
- **Quit**.

Region, ruleset and faction are single-choice submenus (check items).

## Data source: the API

The base URL is configurable in the config file (no menu item). The default is
`https://wow.pestering-curiosity.com/greedygoblin`, so the endpoints are
`https://wow.pestering-curiosity.com/greedygoblin/v1/...`. A local server
(`docker compose` in the server repo) listens on `http://127.0.0.1:7008`.
The API is described in `../greedygoblin-data-server/README.md`;
that file is the reference. Summary:

- `GET /v1/auction-houses`: `[{id, region, ruleset, faction, blizzard_id}]`.
  - `ruleset` is `pvp`, `normal`, `rp`, `rp-pvp`, or null for the empty group
    (probably Hardcore).
  - `faction` is `alliance`, `horde` or `blackwater` (neutral).
  - Use this to find the `id` of the configured auction house.
- `GET /v1/auction-houses/{id}/scans?from=&after=&limit=`: the auction house's
  scans, oldest first: `{id, ts, status, last_modified}`.
  - `status` is `ok`, `unchanged` or `error`.
  - `limit` is 1–1000 (default 1000). If exactly `limit` scans come back,
    request again with `after` = the last id.
  - The response has an `ETag`; send it back as `If-None-Match` to get a 304.
- `GET /v1/auction-houses/{id}/scans/{scanId}`: `{scan_id, auction_house_id,
  ts, last_modified, items: [{item_id, min_price, p10_price, p50_price,
  p90_price, quantity, auctions}]}`.
  - An `unchanged` scan returns the data of the last `ok` scan, so treat `ok`
    and `unchanged` the same way.
  - An `error` scan returns 404: skip it.
  - The response is immutable: download each scan once.
- Prices are copper. `ts` is when the scan started (UTC). `last_modified` is
  when Blizzard produced the data.
- The server scans every region every 4 hours, so there are about 6 scans a day
  per auction house.
- Items are keyed by `item_id` only. Variants (random suffixes, item levels) are
  combined.
- **Battle pets are skipped.** The server combines all pet cages under item
  82800, while the add-on keys pets by species (`p:<speciesID>`). A plain
  `82800` entry would mix every pet, so the app drops item 82800.

## The add-on's data (what the app must match)

The add-on's source is the reference: `../GreedyGoblin/Prices.lua`, `Scan.lua`,
`Core.lua`, `Compat.lua` and its `CLAUDE.md`. Re-read them when this section
seems out of date.

### Auction house key

The add-on stores prices per auction house under `"<Ruleset> <Faction>"`, for
example `"PvP Alliance"`. It maps to the API as follows:

| Add-on ruleset | API `ruleset` |
|----------------|---------------|
| `PvP`          | `pvp`         |
| `PvE`          | `normal`      |
| `RP`           | `rp`          |
| `RP-PvP`       | `rp-pvp`      |
| `HC`           | `null`        |

Faction: `Alliance` = `alliance`, `Horde` = `horde`, `Neutral` = `blackwater`.

### Price model

Per item key (the item id as a text string, e.g. `"2988"`):

- `m`: last seen minimum price; `t`: unix time it was seen.
- `h[day]`: highest of the minimum prices seen that day.
- `l[day]`: lowest price seen that day, present only when below `h[day]`.
- `q[day]`: quantity on sale in the **last full scan** of the day.
  - A day with a `q` for any item is a **scan day**.
  - On a scan day, an item without `q` had quantity 0.

**Day**: `floor((unix_time - SCAN_DAY_0) / 86400)`, where `SCAN_DAY_0` is Lua
`time({year=2020, month=1, day=1, hour=0})`, the **local** midnight of
2020-01-01. Use this exact formula: across DST changes it differs from the local
calendar date. The game and the app run on the same machine, so they share the
local time zone.

### Retention

`GreedyGoblinDB.ui.priceHistoryDays` (default 90) is in
`<folder of Wow.exe>/WTF/Account/<ACCOUNT>/SavedVariables/GreedyGoblin.lua`.

- The app reads it **read-only**, by finding `["priceHistoryDays"] = <n>` in
  the file bytes. The file holds raw binary strings and is not valid UTF-8.
- If there is more than one account folder, the app uses the largest value.
- If the value is not found, it uses 90.
- The add-on keeps days `> today - priceHistoryDays`.

## The data add-on: `GreedyGoblin_Data`

The app owns the folder `<folder of Wow.exe>/Interface/AddOns/GreedyGoblin_Data/`
and rewrites it at each sync. The folder contains two files.

### `GreedyGoblin_Data.toc`

```
## Interface: <same value as GreedyGoblin's TOC>
## Title: GreedyGoblin Data
## Notes: Price history downloaded by the GreedyGoblin app
## Dependencies: GreedyGoblin

Data.lua
```

Copy the `## Interface` value from `GreedyGoblin/GreedyGoblin_Camelot.toc` at
each write, so the data add-on is never marked out of date when the game
updates.

### `Data.lua`

```lua
GreedyGoblinData = {
	version = 1,             -- format version
	generated = 1793950887,  -- unix time the file was written
	data = "<base64>",
}
```

`data` is **Base64** (standard) of **zlib** of **CBOR**, decoded in game with
`C_EncodingUtil.DecodeBase64`, `DecompressString(..., Enum.CompressionMethod.Zlib)`
and `DeserializeCBOR`. All three exist on Forever. The data is a string, not a
Lua table: tables this large overflow the constant limit of a Lua chunk, which is
also why the add-on packs its own prices.

The decoded value:

```
{
  key = "PvP Alliance",          -- add-on auction house key
  region = "eu",
  lastScan = 1793950887,         -- ts of the newest scan included
  days = { ["2501"] = 1793950200, ... },   -- per day: ts of its last scan
  items = {
    ["2988"] = {
      m = 1000, t = 1793950887,  -- newest min_price, with its scan's last_modified
      h = { ["2501"] = 2400, ... },
      l = { ["2501"] = 1800, ... },   -- only where below h
      q = { ["2501"] = 23, ... },     -- from the day's last scan
    },
    ...
  },
}
```

Day keys and item keys are text strings, as in the add-on. The file always holds
the **whole retention window**: the add-on's merge is idempotent, so a file that
is read twice, or a missed `/reload`, causes no harm.

## Sync

The app syncs at start, then **once an hour**. Most hourly checks find nothing
new: the scan list then answers `304 Not Modified` (ETag). A "Sync now" menu
item has not been requested.

1. Find the API `id` of the configured auction house from `/v1/auction-houses`.
2. List its scans after the last scan id in the local state. On the first run,
   list them from the start of the retention window (`from`). Page through
   them with `limit`.
3. Download each new scan whose status is not `error`.
4. Fold the scans into the local state, which has the same shape as the decoded
   data above:
   - **`h[day]`**: the maximum `min_price` of the day's scans.
   - **`l[day]`**: the minimum `min_price` of the day's scans. It is kept only
     where it is below `h[day]`.
   - **`q[day]`**: the `quantity` of the day's last scan.
   - **`days[day]`**: the `ts` of the day's last scan.
   - **`m` and `t`**: from the newest scan that lists the item.
   - A scan's day is computed from its `ts` with the `SCAN_DAY_0` formula.
5. Drop days outside the retention window.
6. Write `Data.lua` and the TOC: write temporary files, then rename them over
   the old ones.
7. Save the local state with the last scan id.

Changing region, ruleset or faction selects another auction house, with its own
local state; its first sync fills the whole window.

The local state also records the `from` of its first download (the start of
the window it covers). If the retention grows (`priceHistoryDays` raised) and
the new window starts before that `from`, the app discards the state and
downloads the whole window again.

## Add-on side (to build in `../GreedyGoblin`)

At `PLAYER_LOGIN`, if the global `GreedyGoblinData` exists, `version` is
known, and `generated` differs from the last one merged (saved in the add-on's
SavedVariables), the add-on:

1. decodes `data`;
2. merges it into `prices[key]`, the auction house of the data, for every day
   inside its retention:
   - `h[day]`: the maximum of the existing and incoming values;
   - `l[day]`: the minimum of the existing `l` (or `h`) and the incoming `l`
     (or `h`). It is kept only where it is below the new `h[day]`;
   - `q[day]`: replace the day's quantities with the incoming ones when
     `days[day]` is later than the add-on's own last full scan of that day
     (`scans[key]`). As `ApplyScan` does, remove the `q` of that day from
     items absent from the incoming day;
   - `m` and `t`: take the incoming values if `t` is newer;
3. drops the `marketDev[key]` days whose data changed, so they are computed
   again;
4. clears its caches, saves `generated`, and sets `GreedyGoblinData` to nil to
   free the memory.

`g:` (gear by item level) and `p:` (pets) keys are never in the data and stay as
they are.

## Errors

There are two kinds of error:

- **Transient**: no network, API 5xx, timeouts.
  - Retry silently at the next sync.
  - Show the error only if syncing keeps failing for a long time (a few
    hours).
- **Needs the user**:
  - `Wow.exe` not found;
  - no `Interface/AddOns/GreedyGoblin` folder;
  - the data add-on folder cannot be written.

  Stop until the problem is fixed.

How the app shows them, without windows:

1. **Icon state**: normal, busy (syncing), or error. Ship one icon per state.
   The sources are 900×900 PNGs with the background in the state's colour:
   `../GreedyGoblin-cropped.png` (normal), `../GreedyGoblin-blue.png` (busy),
   `../GreedyGoblin-red.png` (error). They are scaled down for the tray
   (`assets/icon-<state>.png`, 64×64, made with
   `magick <source> -resize 64x64 -strip <target>`) and embedded in the binary.
   The Windows executable's own icon is `assets/icon.ico` (made with
   `magick ../GreedyGoblin-cropped.png -strip -define icon:auto-resize=256,128,64,48,32,24,16 assets/icon.ico`),
   embedded by `build.rs`. The Windows build links the C runtime statically
   (`.cargo/config.toml`), so it needs no Visual C++ redistributable.
2. **Tooltip**: one line with the state, e.g. `Last scan: 7 Nov 16:30` or
   `Error: GreedyGoblin add-on not found`.
3. **Menu line**: a disabled item under "Last scan downloaded" with the last
   error, in short. Present only while there is an error.
4. **Log file** in the app's data directory, with full details. An **Open log**
   menu item opens it with the system's default program.
5. **Desktop notification** (`notify-rust`), only for errors that need the user.
   Send it the first time an error appears, not at every retry. This matters
   because the tray icon is often hidden in the overflow area on Windows.

## Self-update (Windows only)

The repository is public, and a GitHub Action builds the Windows binary and
publishes it as a GitHub Release (tag `v<version>`, asset
`greedygoblin-app.exe`).

- At start and then once a day, the app asks
  `https://api.github.com/repos/lgpasquale/greedygoblin-app/releases/latest`
  for the newest release (with a generic `User-Agent`, no token).
- If its version is newer than the running one, the app downloads the asset,
  replaces its own executable, starts the new one and quits. The update is
  applied at once, but never during a sync: it waits for the sync to end.
- Errors while updating are transient: log them and try again the next day.
- **Linux has no self-update and no packaging**: there the app is used only
  in development, built from source.

To release: bump `version` in `Cargo.toml`, commit, then push a tag
`v<version>`. The workflow `.github/workflows/release.yml` checks that the tag
matches, runs the tests, builds and publishes the release.

## Open questions

- **Neutral (Blackwater) auction house**: the faction menu has Alliance and
  Horde only. Should Neutral be added (key `"<Ruleset> Neutral"`)?
- **Add-on Scan tab**: should merged data appear in the add-on's list of scans
  (`scans[key]`)? Today the merge does not touch that list.
- **Size of `Data.lua`**: the whole window (90 days, ~6 scans/day folded into
  days) is estimated at under 1 MB. Measure it on real data. If it is too large,
  the file could carry only the days changed since the last merge.
- **Windows installer**: not requested yet.

## Commands

```sh
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```
