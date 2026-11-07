//! Experimental DAV feed-state profile. Durable choices are independent of article caches.
use day_persistence::{DbError, Refresh, RowChange, SqliteConnection, Value};
pub use day_sync;
use day_sync::{Application, BookmarkValue, Change, Identity, Probe, Replica};
pub use day_sync_webdav;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
#[path = "webdav_layout.rs"]
mod layout;
pub const PROFILE: &str = "day-news:feed-state:1";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub kind: String,
    pub feed: String,
    pub guid: String,
    pub value: bool,
    pub label: String,
}
fn err() -> DbError {
    DbError::driver("news.webdav.invalid_state")
}
fn text(s: impl Into<String>) -> Value {
    Value::Text(s.into())
}
fn rows(c: &mut dyn SqliteConnection, sql: &str, p: &[Value]) -> Result<Vec<Vec<Value>>, DbError> {
    let mut out = vec![];
    c.query(sql, p, &mut |r| {
        out.push((0..r.len()).map(|i| r.get(i)).collect())
    })?;
    Ok(out)
}
impl State {
    pub fn id(&self) -> String {
        Sha256::digest(serde_json::to_vec(&(PROFILE, &self.kind, &self.feed, &self.guid)).unwrap())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
    pub fn change(&self) -> Result<Change, DbError> {
        let c = Change {
            id: self.id(),
            value: Some(BookmarkValue {
                url: PROFILE.into(),
                title: serde_json::to_string(self).map_err(|_| err())?,
            }),
        };
        NewsApplication.validate(&c)?;
        Ok(c)
    }
    fn decode(v: &BookmarkValue) -> Result<Self, DbError> {
        if v.url != PROFILE || v.title.len() > 8192 {
            return Err(err());
        }
        let s: Self = serde_json::from_str(&v.title).map_err(|_| err())?;
        let valid = if layout::is_layout(&s.kind) {
            layout::validate(&s)
        } else {
            matches!(s.kind.as_str(), "subscription" | "read" | "star")
                && (s.feed.starts_with("https://") || s.feed.starts_with("http://"))
                && if s.kind == "subscription" {
                    s.guid.is_empty()
                } else {
                    !s.guid.is_empty() && s.label.is_empty()
                }
        };
        if !valid
            || s.feed.len() > 4096
            || s.guid.len() > 4096
            || s.label.len() > 2048
            || serde_json::to_string(&s).map_err(|_| err())? != v.title
        {
            return Err(err());
        }
        Ok(s)
    }
}
#[derive(Clone, Copy)]
pub struct NewsApplication;
impl Application for NewsApplication {
    fn profile(&self) -> &'static str {
        PROFILE
    }
    fn install(&self, c: &mut dyn SqliteConnection) -> Result<(), DbError> {
        c.execute_batch(include_str!("webdav.sql"))?;
        c.execute_batch(include_str!("webdav_layout.sql"))
    }
    fn projected_tables(&self) -> Option<&'static [&'static str]> {
        Some(&["feeds", "articles", "folders"])
    }
    fn revision(&self) -> u32 {
        2
    }
    fn requires(&self, change: &Change) -> u32 {
        change
            .value
            .as_ref()
            .and_then(|v| State::decode(v).ok())
            .map_or(1, |s| if layout::is_layout(&s.kind) { 2 } else { 1 })
    }
    fn validate(&self, c: &Change) -> Result<(), DbError> {
        let s = State::decode(c.value.as_ref().ok_or_else(err)?)?;
        if s.id() != c.id {
            return Err(err());
        }
        Ok(())
    }
    fn resolve(&self, vs: &[Option<BookmarkValue>]) -> Result<usize, DbError> {
        // Causal successors already removed observed predecessors. Concurrent choices prefer
        // unread, starred, and unsubscribed. Order breaks ties deterministically.
        for (i, v) in vs.iter().enumerate() {
            let s = State::decode(v.as_ref().ok_or_else(err)?)?;
            if s.value == (s.kind == "star") {
                return Ok(i);
            }
        }
        Ok(0)
    }
    // Capture still reads the trigger-fed pending tables: the feed-state identity is the feed
    // URL, which a deleted row's key alone does not give back. The change set is unused until
    // deletes carry it (persistence-integration-plan.md, Phase 2).
    fn capture(
        &self,
        c: &mut dyn SqliteConnection,
        _: &[RowChange],
    ) -> Result<Vec<Change>, DbError> {
        c.execute("UPDATE _news_dav_guard SET projecting=1", &[])?;
        c.execute_batch("UPDATE articles SET is_read=COALESCE((SELECT v.value FROM _news_dav_values v JOIN feeds f ON f.feed_url=v.feed WHERE f.id=articles.feed AND f.sync_account IS NULL AND v.kind='read' AND v.guid=articles.guid),is_read),is_starred=COALESCE((SELECT v.value FROM _news_dav_values v JOIN feeds f ON f.feed_url=v.feed WHERE f.id=articles.feed AND f.sync_account IS NULL AND v.kind='star' AND v.guid=articles.guid),is_starred) WHERE id IN (SELECT id FROM _news_dav_arrivals); DELETE FROM _news_dav_arrivals;")?;
        c.execute("UPDATE _news_dav_guard SET projecting=0", &[])?;
        let mut changes = vec![];
        for r in rows(
            c,
            "SELECT kind,feed,guid,value,label FROM _news_dav_pending p WHERE NOT EXISTS(SELECT 1 FROM feeds f WHERE f.feed_url=p.feed AND f.sync_account IS NOT NULL) ORDER BY kind,feed,guid",
            &[],
        )? {
            let s = State {
                kind: r[0].as_text()?.into(),
                feed: r[1].as_text()?.into(),
                guid: r[2].as_text()?.into(),
                value: r[3].as_int()? != 0,
                label: r[4].as_text()?.into(),
            };
            changes.push(s.change()?);
            retain(c, &s)?;
        }
        c.execute("DELETE FROM _news_dav_pending", &[])?;
        changes.extend(layout::capture(c)?);
        Ok(changes)
    }
    fn project(
        &self,
        c: &mut dyn SqliteConnection,
        _id: &str,
        v: Option<&BookmarkValue>,
    ) -> Result<(), DbError> {
        let s = State::decode(v.ok_or_else(err)?)?;
        retain(c, &s)?;
        c.execute("UPDATE _news_dav_guard SET projecting=1", &[])?;
        if layout::is_layout(&s.kind) {
            layout::project(c, &s)?;
            c.execute("UPDATE _news_dav_guard SET projecting=0", &[])?;
            return Ok(());
        }
        let fid = super::feed_id(&s.feed);
        if let Some(r) = rows(
            c,
            "SELECT feed_url,sync_account FROM feeds WHERE id=?",
            &[Value::Int(fid as i64)],
        )?
        .first()
        {
            if r[0].as_text()? != s.feed {
                return Err(err());
            }
            if r[1] != Value::Null {
                c.execute("UPDATE _news_dav_guard SET projecting=0", &[])?;
                return Ok(());
            }
        }
        if s.kind == "subscription" {
            if s.value {
                c.execute("INSERT INTO feeds(id,feed_url,title,position,poll_failures,unsubscribed,collection_position) VALUES(?,?,?,0,0,0,0) ON CONFLICT(id) DO UPDATE SET unsubscribed=0 WHERE feeds.sync_account IS NULL",&[Value::Int(fid as i64),text(&s.feed),text(&s.label)])?;
            } else {
                c.execute(
                    "UPDATE feeds SET unsubscribed=1 WHERE id=? AND sync_account IS NULL",
                    &[Value::Int(fid as i64)],
                )?;
            }
        } else {
            let field = if s.kind == "read" {
                "is_read"
            } else {
                "is_starred"
            };
            c.execute(&format!("UPDATE articles SET {field}=? WHERE feed=? AND guid=? AND EXISTS(SELECT 1 FROM feeds WHERE id=? AND sync_account IS NULL)"),&[Value::Int(s.value as i64),Value::Int(fid as i64),text(&s.guid),Value::Int(fid as i64)])?;
        }
        if s.kind == "subscription" {
            layout::project_feed(c, &s.feed)?;
        }
        c.execute("UPDATE _news_dav_guard SET projecting=0", &[])?;
        Ok(())
    }
}
fn retain(c: &mut dyn SqliteConnection, s: &State) -> Result<(), DbError> {
    if layout::is_layout(&s.kind) {
        return layout::retain(c, s);
    }
    c.execute("INSERT INTO _news_dav_values(kind,feed,guid,value,label) VALUES(?,?,?,?,?) ON CONFLICT(kind,feed,guid) DO UPDATE SET value=excluded.value,label=excluded.label",&[text(&s.kind),text(&s.feed),text(&s.guid),Value::Int(s.value as i64),text(&s.label)])?;
    Ok(())
}
pub async fn open(
    path: impl AsRef<Path>,
    credentials: Arc<Identity>,
    probe: Probe,
) -> Result<Replica, day_sync::Error> {
    let replica = Replica::open_application(
        path,
        credentials,
        probe,
        Some(Arc::new(NewsApplication)),
        |p| super::Db::open(&p).map(|d| d.container),
    )
    .await?;
    // Release records held by an older build before deciding whether to import local layout.
    replica.apply_ready().await?;
    layout::seed(&replica).await?;
    Ok(replica)
}
/// Seed only the creating device's existing local feeds and non-default user choices once.
pub async fn seed(replica: &Replica) -> Result<(), day_sync::Error> {
    replica.worker().write(|db|db.try_with_connection(|c|{
 if !rows(c,"SELECT id FROM _news_dav_seeded",&[])?.is_empty(){return Ok(());}
 c.execute_batch("INSERT INTO _news_dav_pending SELECT 'subscription',feed_url,'',1,title FROM feeds WHERE sync_account IS NULL AND NOT unsubscribed AND (feed_url LIKE 'https://%' OR feed_url LIKE 'http://%'); INSERT INTO _news_dav_pending SELECT 'read',f.feed_url,a.guid,1,'' FROM articles a JOIN feeds f ON f.id=a.feed WHERE f.sync_account IS NULL AND (f.feed_url LIKE 'https://%' OR f.feed_url LIKE 'http://%') AND a.is_read; INSERT INTO _news_dav_pending SELECT 'star',f.feed_url,a.guid,1,'' FROM articles a JOIN feeds f ON f.id=a.feed WHERE f.sync_account IS NULL AND (f.feed_url LIKE 'https://%' OR f.feed_url LIKE 'http://%') AND a.is_starred; INSERT INTO _news_dav_seeded VALUES(1);")?;Ok(())
})).await?;
    Ok(())
}

/// Whether local subscription commands must preserve the sync archive. Query failures choose
/// the non-destructive path; the subsequent transactional write still reports the real error.
pub fn is_installed(db: &day_persistence::ModelContainer) -> bool {
    db.try_with_connection(|c| {
        Ok(!rows(
            c,
            "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('_news_dav_guard','_day_sync_config')",
            &[],
        )?
        .is_empty())
    })
    .unwrap_or(true)
}

/// Recovery mode preserves reading while rejecting every worker write origin. Never silently
/// detach journaling to make a failed identity appear writable.
pub struct RecoveryReadOnly;
impl day_persistence::TransactionParticipant for RecoveryReadOnly {
    fn begin(
        &self,
        _: &mut dyn SqliteConnection,
        _: day_persistence::WriteOrigin,
    ) -> Result<(), DbError> {
        Err(DbError::new(
            day_persistence::DbErrorKind::Deny,
            "news.webdav.read_only_recovery",
        ))
    }
    fn before_commit(
        &self,
        _: &mut dyn SqliteConnection,
        _: day_persistence::WriteOrigin,
        _: &[RowChange],
    ) -> Result<Refresh, DbError> {
        Err(DbError::new(
            day_persistence::DbErrorKind::Deny,
            "news.webdav.read_only_recovery",
        ))
    }
}

/// Startup-only detach. The caller must have stopped the replica and secured a consistent
/// backup before calling. Removes protocol state atomically; leaves feeds/articles/tags intact.
pub fn detach(db: &day_persistence::ModelContainer) -> Result<(), DbError> {
    db.try_with_connection(|c| {
        c.begin()?;
        let result = (|| {
            c.execute_batch("PRAGMA defer_foreign_keys=ON")?;
            let objects = rows(c, "SELECT type,name FROM sqlite_master WHERE type IN ('trigger','table','index') AND (substr(name,1,10)='_day_sync_' OR substr(name,1,10)='_news_dav_') ORDER BY type DESC", &[])?;
            for r in objects {
                let kind = match r[0].as_text()? { "trigger" => "TRIGGER", "table" => "TABLE", "index" => "INDEX", _ => return Err(err()) };
                let name = r[1].as_text()?.replace('"', "\"\"");
                c.execute_batch(&format!("DROP {kind} IF EXISTS \"{name}\""))?;
            }
            // This is a declared model table: retain its schema/fingerprint, clear its mirror.
            if !rows(c, "SELECT name FROM sqlite_master WHERE type='table' AND name='sync_bookmarks'", &[])?.is_empty() {
                c.execute("DELETE FROM sync_bookmarks", &[])?;
            }
            c.commit()
        })();
        if result.is_err() { c.rollback()?; }
        result
    })
}

/// Collection identities are never recycled after deletion in a synchronized library.
pub fn collection_id_used(db: &day_persistence::ModelContainer, id: u64) -> bool {
    db.try_with_connection(|c| {
        if rows(
            c,
            "SELECT name FROM sqlite_master WHERE name='_news_dav_layout_values'",
            &[],
        )?
        .is_empty()
        {
            return Ok(false);
        }
        Ok(!rows(
            c,
            "SELECT feed FROM _news_dav_layout_values WHERE kind='collection' AND feed=?",
            &[text(id.to_string())],
        )?
        .is_empty())
    })
    .unwrap_or(false)
}
