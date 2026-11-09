// SPDX-License-Identifier: Apache-2.0
//! Synthetic fixtures implementing Feedbin's observed wire format, installed only by tests.
//! Based on the trial round trip on 2026-10-18: 200 auth, subscription 201, paged entries,
//! unread ID array and per-ID 200 acknowledgements. Taggings and tags follow the published
//! feedbin-api documents (taggings.md, tags.md) rather than a recorded exchange: 201/302
//! creation, 204/403 deletion, and tag rename/delete answering the remaining taggings. All
//! content and credentials are synthetic.
use day_part_http::{
    Client, Request,
    simulation::{Reply, Simulation},
    transport::PreparedBody,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct Server {
    pub simulation: Simulation,
    state: Arc<Mutex<State>>,
}
struct State {
    subscriptions: Vec<Value>,
    unread: BTreeSet<u64>,
    reject: BTreeSet<u64>,
    next_id: u64,
    entries: Vec<Value>,
    entries_version: u64,
    response_date: Option<String>,
    taggings: Vec<Value>,
    next_tagging: u64,
}
impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}
impl Server {
    pub fn new() -> Self {
        let simulation = Simulation::new();
        let state = Arc::new(Mutex::new(State {
            subscriptions: vec![subscription(
                101,
                11,
                "https://fixture.invalid/feed.xml",
                "Sync fixture",
            )],
            unread: [1001, 1002].into_iter().collect(),
            reject: BTreeSet::new(),
            next_id: 102,
            entries: vec![entry(1001), entry(1002)],
            entries_version: 1,
            response_date: None,
            taggings: vec![],
            next_tagging: 1,
        }));
        let captured = state.clone();
        let auth = Request::get("https://api.feedbin.com/v2/authentication.json")
            .basic_auth("fixture@example.invalid", "fixture-password")
            .headers()[0]
            .1
            .clone();
        simulation.handle(
            |_, u| u.starts_with("https://api.feedbin.com/v2/"),
            move |request| {
                if !request
                    .headers
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("authorization") && *v == auth)
                {
                    return Ok(Reply::new(401, vec![]));
                }
                let url = url::Url::parse(&request.url).expect("fixture URL");
                let path = url.path();
                let method = request.method.as_str();
                let body = match &request.body {
                    PreparedBody::Bytes(b) => {
                        serde_json::from_slice::<Value>(b).unwrap_or(Value::Null)
                    }
                    _ => Value::Null,
                };
                let mut s = captured.lock().unwrap();
                let (status, value) = match (method, path) {
                    ("GET", "/v2/authentication.json") => return Ok(Reply::new(200, vec![])),
                    ("GET", "/v2/subscriptions.json") => {
                        (200, Value::Array(s.subscriptions.clone()))
                    }
                    ("POST", "/v2/subscriptions.json") => {
                        let Some(url) = body["feed_url"].as_str() else {
                            return Ok(Reply::new(422, vec![]));
                        };
                        // Synthetic permanent rejection used to exercise independent upload
                        // retries and download checkpoints through the production HTTP client.
                        if url == "https://fixture.invalid/rejected" {
                            return Ok(Reply::new(422, vec![]));
                        }
                        if let Some(old) = s.subscriptions.iter().find(|r| r["feed_url"] == url) {
                            (302, old.clone())
                        } else {
                            let id = s.next_id;
                            s.next_id += 1;
                            let sub = subscription(id, id + 100, url, "Added fixture");
                            s.subscriptions.push(sub.clone());
                            (201, sub)
                        }
                    }
                    ("GET", "/v2/unread_entries.json") => (200, json!(s.unread)),
                    ("POST", "/v2/unread_entries.json" | "/v2/unread_entries/delete.json") => {
                        let ids: Vec<u64> = body["unread_entries"]
                            .as_array()
                            .map(|a| a.iter().filter_map(Value::as_u64).collect())
                            .unwrap_or_default();
                        if ids.len() > 1000 {
                            return Ok(Reply::new(422, vec![]));
                        }
                        let mut accepted = vec![];
                        for id in ids {
                            if s.reject.contains(&id) {
                                continue;
                            }
                            if path.ends_with("delete.json") {
                                s.unread.remove(&id);
                            } else {
                                s.unread.insert(id);
                            }
                            accepted.push(id);
                        }
                        (200, json!(accepted))
                    }
                    ("GET", "/v2/entries.json") => {
                        let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
                        let ids = query.get("ids").map(|v| {
                            v.split(',')
                                .filter_map(|i| i.parse::<u64>().ok())
                                .collect::<Vec<_>>()
                        });
                        let page = query.get("page").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1);
                        let etag = format!("\"fixture-entries-v{}\"", s.entries_version);
                        if ids.is_none() && page == 1 && request.headers.iter().any(|(k, v)| {
                            k.eq_ignore_ascii_case("if-none-match") && v == &etag
                        }) {
                            let mut reply = Reply::new(304, vec![]);
                            if let Some(date) = &s.response_date { reply = reply.header("Date", date); }
                            return Ok(reply);
                        }
                        let since = query.get("since").map(|v| {
                            chrono::DateTime::parse_from_rfc3339(v).expect("fixture since timestamp")
                        });
                        let filtered: Vec<_> = s.entries.iter().filter(|row| {
                            if let Some(ids) = &ids {
                                return row["id"].as_u64().is_some_and(|id| ids.contains(&id));
                            }
                            since.is_none_or(|since| {
                                chrono::DateTime::parse_from_rfc3339(row["created_at"].as_str().unwrap()).unwrap() > since
                            })
                        }).cloned().collect();
                        // Deliberately one article per fixture page to exercise pagination.
                        let total = filtered.len();
                        let rows = if ids.is_some() { filtered } else {
                            filtered.into_iter().skip(page.saturating_sub(1)).take(1).collect()
                        };
                        let mut reply = Reply::new(200, serde_json::to_vec(&rows).unwrap())
                            .header("Content-Type", "application/json")
                            .header("ETag", etag);
                        if ids.is_none() && page < total {
                            reply = reply.header("Link", format!(
                                "<https://api.feedbin.com/v2/entries.json?page={}>; rel=\"next\", <https://api.feedbin.com/v2/entries.json?page={total}>; rel=\"last\"", page + 1));
                        }
                        if let Some(date) = &s.response_date { reply = reply.header("Date", date); }
                        return Ok(reply);
                    }
                    ("GET", "/v2/taggings.json") => (200, Value::Array(s.taggings.clone())),
                    ("POST", "/v2/taggings.json") => {
                        let (Some(feed), Some(name)) =
                            (body["feed_id"].as_u64(), body["name"].as_str())
                        else {
                            return Ok(Reply::new(422, vec![]));
                        };
                        let (status, row) = match s
                            .taggings
                            .iter()
                            .find(|t| t["feed_id"] == feed && t["name"] == name)
                        {
                            Some(old) => (302, old.clone()),
                            None => {
                                let row = json!({"id":s.next_tagging,"feed_id":feed,"name":name});
                                s.next_tagging += 1;
                                s.taggings.push(row.clone());
                                (201, row)
                            }
                        };
                        let location =
                            format!("https://api.feedbin.com/v2/taggings/{}.json", row["id"]);
                        return Ok(Reply::new(status, serde_json::to_vec(&row).unwrap())
                            .header("Content-Type", "application/json")
                            .header("Location", location));
                    }
                    ("POST", "/v2/tags.json") => {
                        let (Some(old), Some(new)) =
                            (body["old_name"].as_str(), body["new_name"].as_str())
                        else {
                            return Ok(Reply::new(422, vec![]));
                        };
                        for row in s.taggings.iter_mut().filter(|t| t["name"] == old) {
                            row["name"] = json!(new);
                        }
                        (200, Value::Array(s.taggings.clone()))
                    }
                    ("DELETE", "/v2/tags.json") => {
                        let Some(name) = body["name"].as_str() else {
                            return Ok(Reply::new(422, vec![]));
                        };
                        s.taggings.retain(|t| t["name"] != name);
                        (200, Value::Array(s.taggings.clone()))
                    }
                    ("DELETE", _) if path.starts_with("/v2/taggings/") => {
                        let id = path
                            .trim_start_matches("/v2/taggings/")
                            .trim_end_matches(".json")
                            .parse::<u64>()
                            .ok();
                        match s.taggings.iter().position(|t| t["id"].as_u64() == id) {
                            Some(index) => {
                                s.taggings.remove(index);
                                return Ok(Reply::new(204, vec![]));
                            }
                            None => (403, Value::Null),
                        }
                    }
                    _ if path.starts_with("/v2/subscriptions/") => {
                        let id = path
                            .trim_start_matches("/v2/subscriptions/")
                            .split(['.', '/'])
                            .next()
                            .and_then(|s| s.parse::<u64>().ok());
                        if let Some(index) =
                            s.subscriptions.iter().position(|r| r["id"].as_u64() == id)
                        {
                            if method == "DELETE" {
                                s.subscriptions.remove(index);
                                return Ok(Reply::new(204, vec![]));
                            }
                            if method == "POST" {
                                s.subscriptions[index]["title"] = body["title"].clone();
                                (200, s.subscriptions[index].clone())
                            } else {
                                (404, Value::Null)
                            }
                        } else {
                            (403, Value::Null)
                        }
                    }
                    _ => (404, Value::Null),
                };
                Ok(Reply::new(status, serde_json::to_vec(&value).unwrap())
                    .header("Content-Type", "application/json"))
            },
        );
        Self { simulation, state }
    }
    pub fn client(&self) -> Client {
        Client::builder()
            .transport(self.simulation.clone())
            .redirects(day_part_http::Redirects::Never)
            .build()
    }
    pub fn provider(&self) -> crate::feedbin::Feedbin {
        crate::feedbin::Feedbin::with_client(
            crate::Credentials {
                username: "fixture@example.invalid".into(),
                password: "fixture-password".into(),
            },
            self.client(),
        )
    }
    /// Synthetic wire records, newest arrival first; lets tests model historical backfills.
    pub fn replace_entries(&self, entries: Vec<Value>) {
        let mut state = self.state.lock().unwrap();
        state.entries = entries;
        state.entries_version += 1;
    }
    pub fn set_response_date(&self, timestamp: u64) {
        self.state.lock().unwrap().response_date = Some(httpdate::fmt_http_date(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(timestamp),
        ));
    }
    pub fn unread(&self) -> BTreeSet<u64> {
        self.state.lock().unwrap().unread.clone()
    }
    pub fn set_read(&self, id: u64, read: bool) {
        let mut s = self.state.lock().unwrap();
        if read {
            s.unread.remove(&id);
        } else {
            s.unread.insert(id);
        }
    }
    /// The service's taggings as (feed ID, tag name), in creation order.
    pub fn taggings(&self) -> Vec<(u64, String)> {
        self.state
            .lock()
            .unwrap()
            .taggings
            .iter()
            .filter_map(|t| Some((t["feed_id"].as_u64()?, t["name"].as_str()?.to_string())))
            .collect()
    }
    /// File a feed under a tag, as another client would.
    pub fn tag(&self, feed: u64, name: &str) {
        let mut s = self.state.lock().unwrap();
        let id = s.next_tagging;
        s.next_tagging += 1;
        s.taggings.push(json!({"id":id,"feed_id":feed,"name":name}));
    }
    /// Remove every tagging of a feed, as another client would.
    pub fn untag(&self, feed: u64) {
        self.state
            .lock()
            .unwrap()
            .taggings
            .retain(|t| t["feed_id"] != feed);
    }
    pub fn reject(&self, id: u64) {
        self.state.lock().unwrap().reject.insert(id);
    }
}
fn subscription(id: u64, feed: u64, url: &str, title: &str) -> Value {
    json!({"id":id,"feed_id":feed,"feed_url":url,"title":title,"site_url":"https://fixture.invalid/","created_at":"2026-10-18T12:00:00.000000Z"})
}
fn entry(id: u64) -> Value {
    json!({"id":id,"feed_id":11,"title":format!("Synchronized article {id}"),"url":format!("https://fixture.invalid/articles/{id}"),"author":"Fixture Author","content":"<p>Synthetic synchronized content.</p>","summary":"Synthetic sync fixture","published":"2026-10-18T12:00:00.000000Z","created_at":"2026-10-18T12:00:00.000000Z"})
}
