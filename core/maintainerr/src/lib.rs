// SPDX-License-Identifier: AGPL-3.0-or-later
//! Maintainerr API client, used only by alwywatch-server (apps never write to Maintainerr).
//!
//! Verified against Maintainerr 3.32.0 (server code and live read-only calls):
//! * `GET /api/collections` and `GET /api/collections/media/{id}/content/{page}?size=` (items carry
//!   `mediaServerId`, `tmdbId`, `tvdbId`, `addDate`, `image_path`); deletion date is
//!   `addDate + deleteAfterDays`.
//! * `GET /api/media-server/meta/{id}`: title, year, type, `providerIds` (lists).
//! * `POST /api/collections/media/postpone {collectionId, mediaId, days?}`: relative, moves
//!   `addDate` to `max(addDate, now - deleteAfterDays) + days`; without `days` resets it to today.
//! * `POST /api/rules/exclusion {mediaServerId, action: 0}` (global exclusion), lookup with
//!   `GET /api/rules/exclusion?mediaServerId=`, removal with `DELETE /api/rules/exclusion/{id}`.
//! * Writes return 409 while a rule/collection run holds the lock: callers retry later.

use net::reqwest::{Client, Method, RequestBuilder};
use net::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const DAY: i64 = 86_400;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Collection {
    pub id: i64,
    pub title: String,
    pub is_active: bool,
    /// "movie" | "show" | "season" | "episode"
    pub kind: String,
    pub delete_after_days: Option<i64>,
    pub media_count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectionItem {
    pub collection_id: i64,
    /// Media server id (Plex ratingKey when Maintainerr runs in Plex mode).
    pub media_server_id: String,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    /// Unix seconds.
    pub add_date: i64,
    pub image_path: Option<String>,
}

impl CollectionItem {
    /// Unix seconds of the scheduled deletion (may be in the past for inactive collections).
    pub fn deletion_date(&self, c: &Collection) -> Option<i64> {
        c.delete_after_days.map(|d| self.add_date + d * DAY)
    }

    /// TMDB poster URL from Maintainerr's `image_path` (a TMDB path or a full URL).
    pub fn poster_url(&self) -> Option<String> {
        let p = self.image_path.as_deref()?;
        Some(if p.starts_with("http") {
            p.to_owned()
        } else {
            format!("https://image.tmdb.org/t/p/w300{p}")
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub id: String,
    pub title: String,
    pub year: Option<i32>,
    pub kind: String,
    pub tmdb_id: Option<String>,
    pub tvdb_id: Option<String>,
    pub imdb_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Exclusion {
    pub id: i64,
    pub media_server_id: String,
    pub rule_group_id: Option<i64>,
}

/// Parses Maintainerr dates ("2026-04-08 22:00:00.000" or ISO) into unix seconds (UTC).
pub fn parse_date(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, time) = s.split_once(['T', ' ']).unwrap_or((s, "00:00:00"));
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.trim_end_matches('Z').split([':', '.']);
    let hh = t.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    let mm = t.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    let ss = t.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * DAY + hh * 3600 + mm * 60 + ss)
}

#[derive(Debug, Clone)]
pub struct Maintainerr {
    http: Client,
    base: String,
    api_key: Option<String>,
}

impl Maintainerr {
    pub fn new(http: Client, base_url: &str, api_key: Option<String>) -> Self {
        Self {
            http,
            base: base_url.trim_end_matches('/').to_owned(),
            api_key,
        }
    }

    fn req(&self, method: Method, path: &str) -> RequestBuilder {
        let mut r = self
            .http
            .request(method, format!("{}{path}", self.base))
            .header("Accept", "application/json");
        if let Some(k) = &self.api_key {
            r = r.header("X-Api-Key", k);
        }
        r
    }

    pub async fn version(&self) -> Result<String> {
        let res = net::send(self.req(Method::GET, "/api/settings/version")).await?;
        Ok(res
            .text()
            .await
            .map_err(Error::from)?
            .trim()
            .trim_matches('"')
            .to_owned())
    }

    /// "plex" | "jellyfin" | "emby"
    pub async fn media_server_type(&self) -> Result<String> {
        #[derive(Deserialize)]
        struct T {
            #[serde(rename = "type")]
            kind: String,
        }
        let t: T = net::send_json(self.req(Method::GET, "/api/media-server/type")).await?;
        Ok(t.kind)
    }

    pub async fn collections(&self) -> Result<Vec<Collection>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct C {
            id: i64,
            title: String,
            #[serde(default)]
            is_active: bool,
            #[serde(rename = "type", default)]
            kind: Option<serde_json::Value>,
            #[serde(default)]
            delete_after_days: Option<i64>,
            #[serde(default)]
            media_count: Option<i64>,
        }
        let list: Vec<C> = net::send_json(self.req(Method::GET, "/api/collections")).await?;
        Ok(list
            .into_iter()
            .map(|c| Collection {
                id: c.id,
                title: c.title,
                is_active: c.is_active,
                kind: match c.kind {
                    Some(serde_json::Value::String(s)) => s,
                    Some(other) => other.to_string(),
                    None => String::new(),
                },
                delete_after_days: c.delete_after_days,
                media_count: c.media_count.unwrap_or(0),
            })
            .collect())
    }

    /// Every item of a collection (pages of 500).
    pub async fn collection_items(&self, collection_id: i64) -> Result<Vec<CollectionItem>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Page {
            #[serde(default)]
            total_size: i64,
            #[serde(default)]
            items: Vec<Item>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Item {
            media_server_id: String,
            #[serde(default)]
            tmdb_id: Option<i64>,
            #[serde(default)]
            tvdb_id: Option<i64>,
            add_date: String,
            // The only snake_case field of this API.
            #[serde(default, rename = "image_path")]
            image_path: Option<String>,
        }
        let mut out = Vec::new();
        let mut page = 1;
        loop {
            let p: Page = net::send_json(self.req(
                Method::GET,
                &format!("/api/collections/media/{collection_id}/content/{page}?size=500"),
            ))
            .await?;
            let n = p.items.len();
            out.extend(p.items.into_iter().filter_map(|i| {
                Some(CollectionItem {
                    collection_id,
                    add_date: parse_date(&i.add_date)?,
                    media_server_id: i.media_server_id,
                    tmdb_id: i.tmdb_id,
                    tvdb_id: i.tvdb_id,
                    image_path: i.image_path,
                })
            }));
            if n == 0 || out.len() as i64 >= p.total_size || page > 200 {
                break;
            }
            page += 1;
        }
        Ok(out)
    }

    pub async fn meta(&self, media_server_id: &str) -> Result<Meta> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct M {
            id: serde_json::Value,
            #[serde(default)]
            title: String,
            #[serde(default)]
            year: Option<i32>,
            #[serde(rename = "type", default)]
            kind: String,
            #[serde(default)]
            provider_ids: std::collections::HashMap<String, Vec<String>>,
        }
        let m: M = net::send_json(self.req(
            Method::GET,
            &format!(
                "/api/media-server/meta/{}",
                net::percent_encode(media_server_id)
            ),
        ))
        .await?;
        let first = |k: &str| m.provider_ids.get(k).and_then(|v| v.first()).cloned();
        Ok(Meta {
            id: match &m.id {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            },
            tmdb_id: first("tmdb"),
            tvdb_id: first("tvdb"),
            imdb_id: first("imdb"),
            title: m.title,
            year: m.year,
            kind: m.kind,
        })
    }

    pub async fn exclusions_for(&self, media_server_id: &str) -> Result<Vec<Exclusion>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct E {
            id: i64,
            media_server_id: serde_json::Value,
            #[serde(default)]
            rule_group_id: Option<i64>,
        }
        let list: Vec<E> = net::send_json(
            self.req(Method::GET, "/api/rules/exclusion")
                .query(&[("mediaServerId", media_server_id)]),
        )
        .await?;
        Ok(list
            .into_iter()
            .map(|e| Exclusion {
                id: e.id,
                media_server_id: match e.media_server_id {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                },
                rule_group_id: e.rule_group_id,
            })
            .collect())
    }

    /// Excludes an item from every rule (global exclusion): it will not be deleted.
    pub async fn add_exclusion(&self, media_server_id: &str) -> Result<()> {
        net::send(
            self.req(Method::POST, "/api/rules/exclusion")
                .json(&json!({ "mediaServerId": media_server_id, "action": 0 })),
        )
        .await?;
        Ok(())
    }

    /// Removes our global exclusions of an item (rule-group scoped ones set by the admin stay).
    pub async fn remove_global_exclusions(&self, media_server_id: &str) -> Result<usize> {
        let mut n = 0;
        for e in self.exclusions_for(media_server_id).await? {
            if e.rule_group_id.is_none() && e.media_server_id == media_server_id {
                net::send(self.req(Method::DELETE, &format!("/api/rules/exclusion/{}", e.id)))
                    .await?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// Moves an item's deletion date later by `days` (relative, see module docs). `None` resets
    /// the countdown to start today.
    pub async fn postpone(
        &self,
        collection_id: i64,
        media_server_id: &str,
        days: Option<i64>,
    ) -> Result<Option<i64>> {
        let mut body = json!({ "collectionId": collection_id, "mediaId": media_server_id });
        if let Some(d) = days {
            body["days"] = json!(d.clamp(1, 3650));
        }
        let v: serde_json::Value = net::send_json(
            self.req(Method::POST, "/api/collections/media/postpone")
                .json(&body),
        )
        .await?;
        Ok(v.get("deletionDate")
            .and_then(|d| d.as_str())
            .and_then(parse_date))
    }
}

/// Days to postpone so that the deletion date reaches `target` (both unix seconds).
/// Mirrors Maintainerr: the new deletion date is `max(current_deletion, now) + days`.
pub fn postpone_days(current_deletion: i64, now: i64, target: i64) -> Option<i64> {
    let base = current_deletion.max(now);
    (target > base).then(|| (target - base + DAY - 1) / DAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(parse_date("1970-01-02 00:00:00.000"), Some(DAY));
        assert_eq!(parse_date("2026-04-08 22:00:00.000"), Some(1_775_685_600));
        assert_eq!(parse_date("2026-04-08T22:00:00.000Z"), Some(1_775_685_600));
        assert_eq!(parse_date("nope"), None);
    }

    #[test]
    fn postpone_math() {
        let now = 1000 * DAY;
        // Overdue item: counts from now.
        assert_eq!(postpone_days(now - 50 * DAY, now, now + 10 * DAY), Some(10));
        // Future deletion: counts from the current date, rounds up.
        assert_eq!(
            postpone_days(now + 5 * DAY, now, now + 7 * DAY + 1),
            Some(3)
        );
        assert_eq!(postpone_days(now + 9 * DAY, now, now + 7 * DAY), None);
    }
}
