// SPDX-License-Identifier: Apache-2.0
//! Feedbin v2 adapter. All real and simulated requests use day-part-http's Client.
use crate::*;
use day_part_http::{Client, Method, Redirects, Request, Response};
use futures_util::FutureExt;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::time::Duration;

const BASE: &str = "https://api.feedbin.com/v2/";

pub struct Feedbin {
    client: Client,
    credentials: Credentials,
    checkpoint: std::cell::RefCell<FeedbinCheckpoint>,
    subscriptions_changed: std::cell::Cell<bool>,
    last_backdate: std::cell::Cell<Option<i64>>,
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct FeedbinCheckpoint {
    entries_etag: Option<String>,
    /// Missing on older checkpoints, which covered unrestricted history.
    #[serde(default)]
    entries_since: Option<i64>,
    subscriptions: Vec<String>,
    #[serde(default)]
    history_since: Option<i64>,
    /// Server Date from the first page of the last fully committed scan.
    #[serde(default)]
    last_fetch: Option<i64>,
}
impl Feedbin {
    pub fn new(credentials: Credentials) -> Self {
        Self::with_client(
            credentials,
            Client::builder()
                .cache(day_part_http::Cache::Off)
                .redirects(Redirects::Never)
                .build(),
        )
    }
    pub fn with_client(credentials: Credentials, client: Client) -> Self {
        Self {
            client,
            credentials,
            checkpoint: Default::default(),
            subscriptions_changed: Default::default(),
            last_backdate: Default::default(),
        }
    }
    async fn request(&self, method: Method, path: &str, body: Option<Value>) -> Result<Response> {
        self.request_validated(method, path, body, None).await
    }
    async fn request_validated(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        etag: Option<String>,
    ) -> Result<Response> {
        // Cursors never become arbitrary credential-bearing URLs.
        if path.starts_with('/') || path.contains("://") || path.contains("..") {
            return Err(Error::InvalidResponse);
        }
        let mut request = Request::new(method, format!("{BASE}{path}"))
            .basic_auth(&self.credentials.username, &self.credentials.password)
            .cache(day_part_http::CachePolicy::Reload)
            .header("Cache-Control", "no-cache")
            .header("Accept", "application/json")
            .timeout_total(Duration::from_secs(60));
        if let Some(etag) = etag {
            request = request.header("If-None-Match", &etag);
        }
        if let Some(body) = body {
            request = request
                .header("Content-Type", "application/json; charset=utf-8")
                .body(serde_json::to_vec(&body).map_err(|_| Error::InvalidResponse)?);
        }
        let response = self
            .client
            .fetch_limited_future(request, 64 * 1024 * 1024)
            .await
            .map_err(|e| match e {
                day_part_http::HttpError::BodyTooLarge { .. } => Error::InvalidResponse,
                _ => Error::Network,
            })?;
        match response.status {
            200..=299 | 300 | 302 | 304 => Ok(response),
            401 => Err(Error::Authentication),
            429 | 500..=599 => {
                let delay = response
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
                    .and_then(|(_, v)| {
                        v.parse::<u64>().ok().or_else(|| {
                            httpdate::parse_http_date(v).ok().map(|at| {
                                at.duration_since(std::time::SystemTime::now())
                                    .unwrap_or_default()
                                    .as_secs()
                            })
                        })
                    })
                    .unwrap_or(60);
                Err(Error::Retry {
                    after_seconds: delay,
                })
            }
            _ => Err(Error::Rejected),
        }
    }
    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self.request(Method::Get, path, None).await?;
        if response.status != 200 {
            return Err(Error::InvalidResponse);
        }
        decode(&response)
    }
    async fn subscription_write(&self, mutation: &Mutation) -> Result<Option<Subscription>> {
        match mutation {
            Mutation::Subscribe { url } => {
                let response = self
                    .request(
                        Method::Post,
                        "subscriptions.json",
                        Some(json!({"feed_url":url})),
                    )
                    .await?;
                if response.status == 300 {
                    return Err(Error::Rejected);
                }
                Ok(Some(decode::<RemoteSubscription>(&response)?.into()))
            }
            Mutation::Unsubscribe { subscription } => {
                numeric(subscription)?;
                // A retry after an accepted deletion is successful only if a fresh full
                // snapshot confirms absence; don't treat every 403 as a missing record.
                let all: Vec<RemoteSubscription> = self.get("subscriptions.json").await?;
                if all.iter().any(|s| s.id.to_string() == *subscription) {
                    self.request(
                        Method::Delete,
                        &format!("subscriptions/{subscription}.json"),
                        None,
                    )
                    .await?;
                }
                Ok(None)
            }
            Mutation::Rename {
                subscription,
                title,
            } => {
                numeric(subscription)?;
                let response = self
                    .request(
                        Method::Post,
                        &format!("subscriptions/{subscription}/update.json"),
                        Some(json!({"title":title})),
                    )
                    .await?;
                Ok(Some(decode::<RemoteSubscription>(&response)?.into()))
            }
            _ => Err(Error::Unsupported),
        }
    }
    /// `known` caches the account's taggings across one upload batch; a write that adds a
    /// tagging or renames a tag drops it, so the next folder write reads a fresh list.
    async fn folder_write(
        &self,
        mutation: &Mutation,
        known: &mut Option<Vec<RemoteTagging>>,
    ) -> Result<()> {
        if let Mutation::SetFolder { feed, .. } = mutation {
            numeric(feed)?;
        }
        let taggings = match known.take() {
            Some(taggings) => taggings,
            None => self.get("taggings.json").await?,
        };
        match mutation {
            Mutation::SetFolder { feed, folder } => {
                let feed = numeric(feed)?;
                if let Some(name) = folder
                    && !taggings
                        .iter()
                        .any(|t| t.feed_id == feed && t.name == *name)
                {
                    // 201 creates the tagging; 302 names one that already exists.
                    self.request(
                        Method::Post,
                        "taggings.json",
                        Some(json!({"feed_id":feed,"name":name})),
                    )
                    .await?;
                    return self
                        .drop_other_taggings(feed, name, taggings)
                        .await
                        .map(|_| ());
                }
                let kept = self
                    .drop_other_taggings(feed, folder.as_deref().unwrap_or_default(), taggings)
                    .await?;
                *known = Some(kept);
                Ok(())
            }
            Mutation::RenameFolder { old, new } => {
                // A collection no feed uses on the service has no tag to rename.
                if taggings.iter().any(|t| t.name == *old) {
                    self.request(
                        Method::Post,
                        "tags.json",
                        Some(json!({"old_name":old,"new_name":new})),
                    )
                    .await?;
                } else {
                    *known = Some(taggings);
                }
                Ok(())
            }
            Mutation::DeleteFolder { name } => {
                if taggings.iter().any(|t| t.name == *name) {
                    self.request(Method::Delete, "tags.json", Some(json!({"name":name})))
                        .await?;
                    *known = Some(taggings.into_iter().filter(|t| t.name != *name).collect());
                } else {
                    *known = Some(taggings);
                }
                Ok(())
            }
            _ => Err(Error::Unsupported),
        }
    }
    /// Remove the feed's taggings other than `keep`; answer the account's remaining ones.
    async fn drop_other_taggings(
        &self,
        feed: u64,
        keep: &str,
        taggings: Vec<RemoteTagging>,
    ) -> Result<Vec<RemoteTagging>> {
        let mut kept = Vec::with_capacity(taggings.len());
        for tagging in taggings {
            if tagging.feed_id == feed && tagging.name != keep {
                self.request(
                    Method::Delete,
                    &format!("taggings/{}.json", tagging.id),
                    None,
                )
                .await?;
            } else {
                kept.push(tagging);
            }
        }
        Ok(kept)
    }
}
fn numeric(id: &str) -> Result<u64> {
    id.parse().map_err(|_| Error::InvalidResponse)
}
fn decode<T: DeserializeOwned>(r: &Response) -> Result<T> {
    serde_json::from_slice(&r.body).map_err(|_| Error::InvalidResponse)
}

#[derive(Deserialize)]
struct RemoteSubscription {
    id: u64,
    feed_id: u64,
    feed_url: String,
    title: String,
    site_url: Option<String>,
}
impl From<RemoteSubscription> for Subscription {
    fn from(s: RemoteSubscription) -> Self {
        Self {
            id: s.id.to_string(),
            feed_id: s.feed_id.to_string(),
            url: s.feed_url,
            title: s.title,
            site_url: s.site_url,
        }
    }
}
#[derive(Deserialize)]
struct RemoteTagging {
    id: u64,
    feed_id: u64,
    name: String,
}
#[derive(Deserialize)]
struct RemoteEntry {
    id: u64,
    feed_id: u64,
    title: Option<String>,
    url: Option<String>,
    author: Option<String>,
    content: Option<String>,
    summary: Option<String>,
    published: Option<String>,
}
impl From<RemoteEntry> for Entry {
    fn from(e: RemoteEntry) -> Self {
        Self {
            id: e.id.to_string(),
            feed_id: e.feed_id.to_string(),
            title: e.title,
            url: e.url,
            author: e.author,
            content: e.content,
            summary: e.summary,
            published: e
                .published
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.timestamp())
                .unwrap_or(0),
        }
    }
}
impl Provider for Feedbin {
    fn kind(&self) -> &'static str {
        "feedbin"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            subscriptions: true,
            read_state: true,
            entries: true,
        }
    }
    fn restore_checkpoint(&self, checkpoint: Option<String>) -> Result<()> {
        self.subscriptions_changed.set(false);
        *self.checkpoint.borrow_mut() = match checkpoint {
            Some(value) => serde_json::from_str(&value).map_err(|_| Error::InvalidResponse)?,
            None => FeedbinCheckpoint::default(),
        };
        Ok(())
    }
    fn checkpoint(&self) -> Option<String> {
        serde_json::to_string(&*self.checkpoint.borrow()).ok()
    }
    fn authenticate(&self) -> Call<'_, ()> {
        async {
            let r = self
                .request(Method::Get, "authentication.json", None)
                .await?;
            if r.status == 200 {
                Ok(())
            } else {
                Err(Error::Authentication)
            }
        }
        .boxed_local()
    }
    fn subscriptions(&self) -> Call<'_, Changes<Subscription>> {
        async {
            let rows: Vec<Subscription> = self
                .get::<Vec<RemoteSubscription>>("subscriptions.json")
                .await?
                .into_iter()
                .map(Into::into)
                .collect();
            let mut ids: Vec<_> = rows.iter().map(|s| s.id.clone()).collect();
            ids.sort();
            let mut checkpoint = self.checkpoint.borrow_mut();
            if checkpoint.subscriptions != ids {
                // A new feed may add history behind the unchanged first page.
                checkpoint.entries_etag = None;
                checkpoint.subscriptions = ids;
                self.subscriptions_changed.set(true);
            }
            Ok(Changes::Snapshot(rows))
        }
        .boxed_local()
    }
    fn prepare_history(&self, since: Option<i64>) -> Result<bool> {
        if since.is_some_and(|at| chrono::DateTime::from_timestamp(at, 0).is_none()) {
            return Err(Error::InvalidResponse);
        }
        let mut checkpoint = self.checkpoint.borrow_mut();
        let expanded = checkpoint
            .history_since
            .is_some_and(|old| since.is_none_or(|new| new < old));
        let backfill = expanded || self.subscriptions_changed.get();
        let query_since = match checkpoint.last_fetch.filter(|_| !backfill) {
            Some(last) => {
                // NetNewsWire's policy: one day of overlap on the first refresh after launch,
                // then once per day. Use server time, never a device clock as the watermark.
                let backdate = self
                    .last_backdate
                    .get()
                    .is_none_or(|at| last.saturating_sub(at) >= 86_400);
                let from = if backdate {
                    self.last_backdate.set(Some(last));
                    last.saturating_sub(86_400)
                } else {
                    last
                };
                Some(since.map_or(from, |floor| from.max(floor)))
            }
            None => since,
        };
        if checkpoint.entries_since != query_since {
            // Validators belong to a specific query, including its time window.
            checkpoint.entries_etag = None;
            checkpoint.entries_since = query_since;
        }
        checkpoint.history_since = since;
        // Scan the whole small delta/lookback, even through known pages. A wider history
        // or changed subscription set must likewise reach older, previously excluded items.
        Ok(backfill || checkpoint.last_fetch.is_some())
    }

    fn entries(&self, cursor: Option<String>) -> Call<'_, Page<Entry>> {
        async move {
            let page = cursor.as_deref().map(numeric).transpose()?.unwrap_or(1);
            let etag = if page == 1 {
                self.checkpoint.borrow().entries_etag.clone()
            } else {
                None
            };
            let mut path = format!("entries.json?page={page}&per_page=100");
            if let Some(since) = self.checkpoint.borrow().entries_since {
                let date = chrono::DateTime::from_timestamp(since, 0)
                    .ok_or(Error::InvalidResponse)?
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                let query = url::form_urlencoded::Serializer::new(String::new())
                    .append_pair("since", &date)
                    .finish();
                path.push('&');
                path.push_str(&query);
            }
            let r = self
                .request_validated(Method::Get, &path, None, etag)
                .await?;
            if page == 1
                && matches!(r.status, 200 | 304)
                && let Some(at) = r
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("date"))
                    .and_then(|(_, value)| httpdate::parse_http_date(value).ok())
                    .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                    .and_then(|d| i64::try_from(d.as_secs()).ok())
            {
                self.checkpoint.borrow_mut().last_fetch = Some(at);
            }
            if r.status == 304 {
                return Ok(Page {
                    values: vec![],
                    next: None,
                    total_pages: Some(1),
                });
            }
            if r.status != 200 {
                return Err(Error::InvalidResponse);
            }
            if page == 1 {
                self.checkpoint.borrow_mut().entries_etag = r
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("etag"))
                    .map(|(_, v)| v.clone());
            }
            let rows: Vec<RemoteEntry> = decode(&r)?;
            let next = r
                .headers
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case("link") || k.eq_ignore_ascii_case("links"))
                .any(|(_, v)| v.contains("rel=\"next\"") || v.contains("rel=next"));
            let total_pages = last_page(&r).or_else(|| (!next).then_some(page as usize));
            Ok(Page {
                total_pages,
                values: rows.into_iter().map(Into::into).collect(),
                next: if next {
                    Some(
                        page.checked_add(1)
                            .ok_or(Error::InvalidResponse)?
                            .to_string(),
                    )
                } else {
                    None
                },
            })
        }
        .boxed_local()
    }
    fn entries_by_id(&self, ids: Vec<String>) -> Call<'_, Vec<Entry>> {
        async move {
            let mut entries = Vec::new();
            for chunk in ids.chunks(100) {
                for id in chunk {
                    numeric(id)?;
                }
                let rows: Vec<RemoteEntry> = self
                    .get(&format!("entries.json?ids={}", chunk.join(",")))
                    .await?;
                entries.extend(rows.into_iter().map(Into::into));
            }
            Ok(entries)
        }
        .boxed_local()
    }
    fn read_state(&self) -> Call<'_, ReadChanges> {
        async {
            Ok(ReadChanges::UnreadSnapshot(
                self.get::<Vec<u64>>("unread_entries.json")
                    .await?
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
            ))
        }
        .boxed_local()
    }
    fn taggings(&self) -> Call<'_, Option<Vec<Tagging>>> {
        async {
            Ok(Some(
                self.get::<Vec<RemoteTagging>>("taggings.json")
                    .await?
                    .into_iter()
                    .map(|t| Tagging {
                        feed_id: t.feed_id.to_string(),
                        name: t.name,
                    })
                    .collect(),
            ))
        }
        .boxed_local()
    }
    fn apply(&self, operations: Vec<Operation>) -> Call<'_, Vec<Outcome>> {
        async move {
            let mut outcomes = Vec::new();
            for read in [true, false] {
                let group: Vec<_> = operations
                    .iter()
                    .filter(|o| matches!(&o.mutation,Mutation::SetRead(s) if s.read==read))
                    .collect();
                for chunk in group.chunks(1000) {
                    let ids: Result<Vec<_>> = chunk
                        .iter()
                        .map(|o| match &o.mutation {
                            Mutation::SetRead(s) => numeric(&s.id),
                            _ => unreachable!(),
                        })
                        .collect();
                    let result = match ids {
                        Ok(ids) => self
                            .request(
                                Method::Post,
                                if read {
                                    "unread_entries/delete.json"
                                } else {
                                    "unread_entries.json"
                                },
                                Some(json!({"unread_entries":ids})),
                            )
                            .await
                            .and_then(|r| decode::<Vec<u64>>(&r)),
                        Err(e) => Err(e),
                    };
                    for op in chunk {
                        let accepted = match (&result, &op.mutation) {
                            (Ok(ids), Mutation::SetRead(s)) => {
                                ids.iter().any(|id| id.to_string() == s.id)
                            }
                            _ => false,
                        };
                        outcomes.push(if accepted {
                            Outcome::Applied {
                                revision: op.revision,
                                subscription: None,
                            }
                        } else {
                            Outcome::Failed {
                                revision: op.revision,
                                error: result.as_ref().err().cloned().unwrap_or(Error::Rejected),
                            }
                        });
                    }
                }
            }
            let mut taggings = None;
            for op in operations
                .iter()
                .filter(|o| !matches!(o.mutation, Mutation::SetRead(_)))
            {
                let result = match &op.mutation {
                    Mutation::SetFolder { .. }
                    | Mutation::RenameFolder { .. }
                    | Mutation::DeleteFolder { .. } => self
                        .folder_write(&op.mutation, &mut taggings)
                        .await
                        .map(|()| None),
                    _ => {
                        // Unsubscribing can take a feed's taggings with it.
                        taggings = None;
                        self.subscription_write(&op.mutation).await
                    }
                };
                outcomes.push(match result {
                    Ok(subscription) => Outcome::Applied {
                        revision: op.revision,
                        subscription,
                    },
                    Err(error) => Outcome::Failed {
                        revision: op.revision,
                        error,
                    },
                });
            }
            Ok(outcomes)
        }
        .boxed_local()
    }
}

/// Only consume pagination metadata, never follow a server-provided authenticated URL.
fn last_page(response: &Response) -> Option<usize> {
    response
        .headers
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case("link") || key.eq_ignore_ascii_case("links"))
        .flat_map(|(_, value)| value.split(','))
        .find_map(|link| {
            let (address, attributes) = link.split_once('>')?;
            let last = attributes.split(';').any(|attr| {
                attr.trim().split_once('=').is_some_and(|(name, value)| {
                    name.trim() == "rel"
                        && value
                            .trim()
                            .trim_matches('"')
                            .split_whitespace()
                            .any(|v| v == "last")
                })
            });
            if !last {
                return None;
            }
            let url = url::Url::parse(address.trim().strip_prefix('<')?).ok()?;
            url.query_pairs()
                .find(|(k, _)| k == "page")?
                .1
                .parse::<usize>()
                .ok()
                .filter(|n| *n > 0)
        })
}
