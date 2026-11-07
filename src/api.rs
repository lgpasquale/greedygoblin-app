//! Client of the data server's API (`../greedygoblin-data-server/README.md`).

use std::time::Duration;

use chrono::DateTime;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::error::SyncError;
use crate::state::{Scan, ScanItem};

/// The server accepts at most this many scans per list request.
pub const SCAN_PAGE_SIZE: usize = 1000;
const MAX_BODY: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
pub struct AuctionHouse {
    pub id: u32,
    pub region: String,
    pub ruleset: Option<String>,
    pub faction: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ScanInfo {
    pub id: u64,
    pub status: String,
}

impl ScanInfo {
    /// An `error` scan has no data.
    pub fn has_data(&self) -> bool {
        self.status != "error"
    }
}

pub enum ScanList {
    NotModified,
    Scans {
        scans: Vec<ScanInfo>,
        etag: Option<String>,
    },
}

#[derive(Deserialize)]
struct ScanResponse {
    ts: String,
    last_modified: String,
    items: Vec<ScanItemResponse>,
}

#[derive(Deserialize)]
struct ScanItemResponse {
    item_id: u32,
    min_price: i64,
    quantity: i64,
}

pub struct Api {
    agent: ureq::Agent,
    base: String,
}

impl Api {
    pub fn new(base_url: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_global(Some(Duration::from_secs(300)))
            .user_agent(concat!("greedygoblin-app/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self {
            agent,
            base: format!("{}/v1", base_url.trim_end_matches('/')),
        }
    }

    pub fn auction_houses(&self) -> Result<Vec<AuctionHouse>, SyncError> {
        let url = format!("{}/auction-houses", self.base);
        let mut response = self
            .agent
            .get(&url)
            .call()
            .map_err(|e| transient(&url, e))?;
        read_json(&url, response.body_mut())
    }

    /// Lists scans. `url` (from [`Api::scans_url`]) identifies the request, so
    /// that a stored `ETag` is sent only for the same request.
    pub fn scans(&self, url: &str, etag: Option<&str>) -> Result<ScanList, SyncError> {
        let mut request = self.agent.get(url);
        if let Some(etag) = etag {
            request = request.header("If-None-Match", etag);
        }
        let mut response = request.call().map_err(|e| transient(url, e))?;
        if response.status() == 304 {
            return Ok(ScanList::NotModified);
        }
        let etag = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let scans = read_json(url, response.body_mut())?;
        Ok(ScanList::Scans { scans, etag })
    }

    /// The URL listing the scans after `after`, or else from `from` (unix time).
    pub fn scans_url(&self, auction_house_id: u32, after: Option<u64>, from: i64) -> String {
        let filter = match after {
            Some(after) => format!("after={after}"),
            None => {
                let from = DateTime::from_timestamp(from, 0)
                    .expect("valid unix time")
                    .format("%Y-%m-%dT%H:%M:%SZ");
                format!("from={from}")
            }
        };
        format!(
            "{}/auction-houses/{auction_house_id}/scans?{filter}&limit={SCAN_PAGE_SIZE}",
            self.base
        )
    }

    /// Downloads one scan; `None` if the server has no data for it (404).
    pub fn scan(&self, auction_house_id: u32, scan_id: u64) -> Result<Option<Scan>, SyncError> {
        let url = format!(
            "{}/auction-houses/{auction_house_id}/scans/{scan_id}",
            self.base
        );
        let mut response = match self.agent.get(&url).call() {
            Ok(response) => response,
            Err(ureq::Error::StatusCode(404)) => return Ok(None),
            Err(e) => return Err(transient(&url, e)),
        };
        let scan: ScanResponse = read_json(&url, response.body_mut())?;
        Ok(Some(Scan {
            ts: parse_time(&url, &scan.ts)?,
            last_modified: parse_time(&url, &scan.last_modified)?,
            items: scan
                .items
                .into_iter()
                .map(|i| ScanItem {
                    item_id: i.item_id,
                    min_price: i.min_price,
                    quantity: i.quantity,
                })
                .collect(),
        }))
    }
}

fn transient(url: &str, e: ureq::Error) -> SyncError {
    SyncError::Transient(format!("GET {url}: {e}"))
}

fn read_json<T: DeserializeOwned>(url: &str, body: &mut ureq::Body) -> Result<T, SyncError> {
    let bytes = body
        .with_config()
        .limit(MAX_BODY)
        .read_to_vec()
        .map_err(|e| transient(url, e))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| SyncError::Transient(format!("GET {url}: bad response: {e}")))
}

fn parse_time(url: &str, s: &str) -> Result<i64, SyncError> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.timestamp())
        .map_err(|e| SyncError::Transient(format!("GET {url}: bad time {s:?}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_url() {
        let api = Api::new("https://example.com/gg/");
        assert_eq!(
            api.scans_url(4, None, 1_793_923_200),
            "https://example.com/gg/v1/auction-houses/4/scans?from=2026-11-06T00:00:00Z&limit=1000"
        );
        assert_eq!(
            api.scans_url(4, Some(42), 0),
            "https://example.com/gg/v1/auction-houses/4/scans?after=42&limit=1000"
        );
    }
}
