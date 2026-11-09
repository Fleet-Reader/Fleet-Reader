// SPDX-License-Identifier: Apache-2.0
//! Durable synchronization records. No credential material is stored in this database.
//! Call mutations inside the same worker transaction as the user's edit.
use crate::*;
use daynews_sync::{
    Changes, Entry, Mutation, Operation, Outcome, ReadChanges, ReadState, Subscription, Tagging,
};
use std::collections::{HashMap, HashSet};

#[derive(Model, Clone, Default, PartialEq, Debug)]
#[model(table = "sync_accounts")]
pub struct Account {
    #[model(id)]
    pub id: u64,
    pub provider: String,
    pub username: String,
    pub connected: bool,
    pub revision: u64,
    pub last_sync: Option<i64>,
    pub checkpoint: Option<String>,
    pub importing: bool,
}
#[derive(Model, Clone, Default, PartialEq, Debug)]
#[model(table = "sync_subscriptions")]
pub struct SubscriptionLink {
    #[model(id)]
    pub id: u64,
    #[model(index)]
    pub account: u64,
    pub remote: String,
    pub remote_feed: String,
    pub local_feed: u64,
    pub title: String,
    pub removed: bool,
}
#[derive(Model, Clone, Default, PartialEq, Debug)]
#[model(table = "sync_entries")]
pub struct EntryLink {
    #[model(id)]
    pub id: u64,
    #[model(index)]
    pub account: u64,
    pub remote: String,
    pub remote_feed: String,
    #[model(index)]
    pub article: u64,
    pub confirmed_read: bool,
}
#[derive(Model, Clone, Default, PartialEq, Debug)]
#[model(table = "sync_intents")]
pub struct Intent {
    #[model(id)]
    pub id: u64,
    #[model(index)]
    pub account: u64,
    pub revision: u64,
    pub mutation: String,
    pub blocked: bool,
}

pub fn account_id(provider: &str, username: &str) -> u64 {
    ident(&["account", provider, username.trim()])
}
fn entry_id(account: u64, remote: &str) -> u64 {
    ident(&["remote-entry", &account.to_string(), remote])
}
fn link_id(account: u64, remote: &str) -> u64 {
    ident(&["remote-subscription", &account.to_string(), remote])
}

impl Db {
    /// Discard the entire local library and upload journal, retaining account identity only.
    /// Must run in a worker transaction that also clears undo. No remote mutation is queued.
    pub fn sync_reset_local(&self, account: u64) -> Result<(), DbError> {
        let active = self
            .container
            .try_get::<Account>(account)?
            .ok_or_else(|| DbError::driver("sync account missing"))?;
        if !active.connected().peek() {
            return Err(DbError::driver("sync account disconnected"));
        }
        macro_rules! clear {
            ($model:ty) => {
                for row in self.container.query::<$model>().live().try_collect()? {
                    self.container.delete::<$model>(row.id)?;
                }
            };
        }
        clear!(Intent);
        clear!(EntryLink);
        clear!(SubscriptionLink);
        clear!(ArticleBody);
        clear!(Article);
        clear!(Feed);
        clear!(Folder);
        clear!(Tag);
        for account in self.container.query::<Account>().live().try_collect()? {
            let record = self.container.try_get::<Account>(account.id)?.unwrap();
            record.last_sync().write(None);
            record.checkpoint().write(None);
            record.importing().write(false);
        }
        Ok(())
    }

    pub fn sync_begin_entries(&self, account: u64) -> Result<bool, DbError> {
        let a = self
            .container
            .try_get::<Account>(account)?
            .ok_or_else(|| DbError::driver("sync account missing"))?;
        let interrupted = a.importing().peek();
        a.importing().write(true);
        Ok(interrupted)
    }

    pub fn sync_accounts(&self) -> Result<Vec<Account>, DbError> {
        self.container.query::<Account>().live().try_collect()
    }
    pub fn sync_connect(&self, provider: &str, username: &str) -> Result<u64, DbError> {
        let id = account_id(provider, username);
        for old in self.container.query::<Account>().live().try_collect()? {
            if let Some(a) = self.container.get::<Account>(old.id) {
                a.connected().write(old.id == id);
            }
        }
        if let Some(a) = self.container.get::<Account>(id) {
            a.connected().write(true);
        } else {
            self.container.insert(Account {
                id,
                provider: provider.into(),
                username: username.trim().into(),
                connected: true,
                ..Default::default()
            });
        }
        Ok(id)
    }
    pub fn sync_disconnect(&self, account: u64) {
        if let Some(a) = self.container.get::<Account>(account) {
            a.connected().write(false);
        }
    }
    pub fn sync_pending(&self, account: u64) -> Result<Vec<Operation>, DbError> {
        self.container
            .query::<Intent>()
            .filter(Intent::account().eq(account) & Intent::blocked().eq(false))
            .sort(Intent::revision().asc())
            .live()
            .try_collect()?
            .into_iter()
            .map(|r| {
                Ok(Operation {
                    revision: r.revision,
                    mutation: serde_json::from_str(&r.mutation)
                        .map_err(|_| DbError::driver("invalid sync intent"))?,
                })
            })
            .collect()
    }
    pub fn sync_pending_count(&self, account: u64) -> usize {
        self.container
            .query::<Intent>()
            .filter(Intent::account().eq(account))
            .live()
            .ids()
            .len()
    }
    fn sync_queue(&self, account: u64, key: &str, mutation: Mutation) -> Result<(), DbError> {
        day_model::with_author("sync-journal", || {
            let Some(a) = self.container.try_get::<Account>(account)? else {
                return Err(DbError::driver("sync account missing"));
            };
            let revision = a.revision().peek().saturating_add(1);
            a.revision().write(revision);
            let id = ident(&["intent", &account.to_string(), key]);
            let json = serde_json::to_string(&mutation)
                .map_err(|_| DbError::driver("invalid sync intent"))?;
            self.container.insert(Intent {
                id,
                account,
                revision,
                mutation: json,
                blocked: false,
            });
            Ok(())
        })
    }
    pub fn sync_queue_read(&self, article: u64, read: bool) -> Result<(), DbError> {
        let links = self
            .container
            .query::<EntryLink>()
            .filter(EntryLink::article().eq(article))
            .live()
            .try_collect()?;
        for link in links {
            let owner = self
                .container
                .try_get::<Article>(article)?
                .and_then(|a| a.feed().peek().id())
                .and_then(|id| self.container.get::<Feed>(id.handle()))
                .and_then(|f| f.sync_account().peek());
            if owner != Some(link.account) {
                continue;
            }
            self.sync_queue(
                link.account,
                &format!("read:{}", link.remote),
                Mutation::SetRead(ReadState {
                    id: link.remote,
                    read,
                }),
            )?;
        }
        Ok(())
    }
    pub fn sync_queue_rename(&self, feed: u64, title: &str) -> Result<(), DbError> {
        for link in self
            .container
            .query::<SubscriptionLink>()
            .filter(SubscriptionLink::local_feed().eq(feed) & SubscriptionLink::removed().eq(false))
            .live()
            .try_collect()?
        {
            if self
                .container
                .try_get::<Feed>(feed)?
                .and_then(|f| f.sync_account().peek())
                != Some(link.account)
            {
                continue;
            }
            self.sync_queue(
                link.account,
                &format!("subscription:{}", link.remote),
                Mutation::Rename {
                    subscription: link.remote,
                    title: title.into(),
                },
            )?;
        }
        Ok(())
    }
    /// Queue a feed's folder change for the connected account: `None` leaves every folder.
    /// One intent per feed: a later move replaces an earlier one not yet uploaded.
    pub fn sync_queue_feed_folder(&self, feed: u64, folder: Option<&str>) -> Result<(), DbError> {
        for link in self
            .container
            .query::<SubscriptionLink>()
            .filter(SubscriptionLink::local_feed().eq(feed) & SubscriptionLink::removed().eq(false))
            .live()
            .try_collect()?
        {
            if self
                .container
                .try_get::<Feed>(feed)?
                .and_then(|f| f.sync_account().peek())
                != Some(link.account)
            {
                continue;
            }
            self.sync_queue(
                link.account,
                &format!("folder:{}", link.remote_feed),
                Mutation::SetFolder {
                    feed: link.remote_feed,
                    folder: folder.map(Into::into),
                },
            )?;
        }
        Ok(())
    }
    /// Queue a folder rename for the connected account. Folder edits never coalesce: each
    /// applies to the names the previous ones left behind.
    pub fn sync_queue_folder_rename(&self, old: &str, new: &str) -> Result<(), DbError> {
        let Some(account) = self.sync_connected()? else {
            return Ok(());
        };
        self.sync_queue_folder_edit(
            account,
            Mutation::RenameFolder {
                old: old.into(),
                new: new.into(),
            },
        )?;
        // A move into the old name not yet uploaded now means the new one.
        self.sync_requeue_folder_moves(account, old, Some(new))
    }
    /// Queue a folder deletion (its feeds stay subscribed) for the connected account.
    pub fn sync_queue_folder_delete(&self, name: &str) -> Result<(), DbError> {
        let Some(account) = self.sync_connected()? else {
            return Ok(());
        };
        self.sync_queue_folder_edit(account, Mutation::DeleteFolder { name: name.into() })?;
        // Its feeds left the folder locally; a pending move into it must not recreate it.
        self.sync_requeue_folder_moves(account, name, None)
    }
    fn sync_connected(&self) -> Result<Option<u64>, DbError> {
        Ok(self
            .container
            .query::<Account>()
            .filter(Account::connected().eq(true))
            .live()
            .try_collect()?
            .first()
            .map(|a| a.id))
    }
    fn sync_queue_folder_edit(&self, account: u64, mutation: Mutation) -> Result<(), DbError> {
        let Some(a) = self.container.try_get::<Account>(account)? else {
            return Err(DbError::driver("sync account missing"));
        };
        // Keyed by the revision it is about to take, so no later edit replaces it.
        let key = format!("folder-edit:{}", a.revision().peek().saturating_add(1));
        self.sync_queue(account, &key, mutation)
    }
    fn sync_requeue_folder_moves(
        &self,
        account: u64,
        from: &str,
        to: Option<&str>,
    ) -> Result<(), DbError> {
        for op in self.sync_pending(account)? {
            if let Mutation::SetFolder {
                feed,
                folder: Some(name),
            } = op.mutation
                && name == from
            {
                self.sync_queue(
                    account,
                    &format!("folder:{feed}"),
                    Mutation::SetFolder {
                        feed,
                        folder: to.map(Into::into),
                    },
                )?;
            }
        }
        Ok(())
    }
    /// A newly linked feed Fleet Reader subscribed itself keeps the folder it was filed under.
    fn sync_queue_linked_folder(
        &self,
        account: u64,
        local_feed: u64,
        remote_feed: &str,
    ) -> Result<(), DbError> {
        if let Some(name) = self.sync_folder_name(local_feed)? {
            self.sync_queue(
                account,
                &format!("folder:{remote_feed}"),
                Mutation::SetFolder {
                    feed: remote_feed.into(),
                    folder: Some(name),
                },
            )?;
        }
        Ok(())
    }
    fn sync_folder_name(&self, feed: u64) -> Result<Option<String>, DbError> {
        let Some(folder) = self
            .container
            .try_get::<Feed>(feed)?
            .and_then(|f| f.folder().peek())
            .and_then(|f| f.id())
        else {
            return Ok(None);
        };
        Ok(self
            .container
            .try_get::<Folder>(folder.handle())?
            .map(|f| f.name().peek()))
    }
    /// File the account's linked feeds by a complete tagging list. A feed with a pending move
    /// keeps its local folder until that move is acknowledged, and pending folder renames and
    /// deletions apply on top of the service's names. Folders are created by name and never
    /// deleted here: one no feed uses may be local-only. Unlinked feeds keep their folders.
    pub fn sync_apply_taggings(&self, account: u64, taggings: Vec<Tagging>) -> Result<(), DbError> {
        let pending = self.sync_pending(account)?;
        let mut moving = HashSet::new();
        let mut edits = Vec::new();
        for op in &pending {
            match &op.mutation {
                Mutation::SetFolder { feed, .. } => {
                    moving.insert(feed.clone());
                }
                Mutation::RenameFolder { .. } | Mutation::DeleteFolder { .. } => {
                    edits.push(&op.mutation)
                }
                _ => {}
            }
        }
        let mut names: HashMap<String, Vec<String>> = HashMap::new();
        for tagging in taggings {
            let mut name = Some(tagging.name).filter(|n| !n.trim().is_empty());
            for edit in &edits {
                match edit {
                    Mutation::RenameFolder { old, new } if name.as_ref() == Some(old) => {
                        name = Some(new.clone())
                    }
                    Mutation::DeleteFolder { name: gone } if name.as_ref() == Some(gone) => {
                        name = None
                    }
                    _ => {}
                }
            }
            if let Some(name) = name {
                names.entry(tagging.feed_id).or_default().push(name);
            }
        }
        for link in self
            .container
            .query::<SubscriptionLink>()
            .filter(SubscriptionLink::account().eq(account) & SubscriptionLink::removed().eq(false))
            .live()
            .try_collect()?
        {
            if moving.contains(&link.remote_feed) {
                continue;
            }
            let Some(feed) = self.container.try_get::<Feed>(link.local_feed)? else {
                continue;
            };
            if feed.sync_account().peek() != Some(account) {
                continue;
            }
            // Fleet Reader files a feed once; several tags resolve to the alphabetically first.
            let name = names
                .get(&link.remote_feed)
                .and_then(|n| n.iter().min_by_key(|n| (n.to_lowercase(), n.as_str())));
            let target = name.map(|n| self.add_folder(n));
            let current = feed
                .folder()
                .peek()
                .and_then(|f| f.id())
                .map(|id| id.handle());
            if current == target {
                continue;
            }
            feed.folder().write(target.map(One::to));
            if let Some(folder) = target {
                // Arrive last in the folder's manual order.
                let last = self
                    .container
                    .query::<Feed>()
                    .filter(Feed::folder().is(folder))
                    .live()
                    .try_collect()?
                    .into_iter()
                    .filter(|f| f.id != link.local_feed)
                    .map(|f| f.collection_position)
                    .fold(None, |max: Option<f64>, p| {
                        Some(max.map_or(p, |m| m.max(p)))
                    });
                let position = last.map_or(0.0, |p| p + 1.0);
                if feed.collection_position().peek() != position {
                    feed.collection_position().write(position);
                }
            }
        }
        Ok(())
    }
    pub fn sync_adopt_local(&self, feed: u64) -> Result<(), DbError> {
        let Some(f) = self.container.try_get::<Feed>(feed)? else {
            return Ok(());
        };
        let url = f.feed_url().peek();
        if let Some(account) = f.sync_account().peek() {
            if f.unsubscribed().peek() {
                f.unsubscribed().write(false);
                for link in self
                    .container
                    .query::<SubscriptionLink>()
                    .filter(
                        SubscriptionLink::account().eq(account)
                            & SubscriptionLink::local_feed().eq(feed),
                    )
                    .live()
                    .try_collect()?
                {
                    for op in self.sync_pending(account)? {
                        if matches!(&op.mutation,Mutation::Unsubscribe{subscription} if *subscription==link.remote)
                        {
                            self.sync_ack(account, op.revision)?;
                        }
                    }
                }
                self.sync_queue_subscribe(account, &url)?;
            }
            return Ok(());
        }
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Ok(());
        }
        if let Some(account) = self
            .container
            .query::<Account>()
            .filter(Account::connected().eq(true))
            .live()
            .try_collect()?
            .first()
        {
            f.sync_account().write(Some(account.id));
            self.sync_queue_subscribe(account.id, &url)?;
        }
        Ok(())
    }
    pub fn sync_queue_subscribe(&self, account: u64, url: &str) -> Result<(), DbError> {
        self.sync_queue(
            account,
            &format!("subscribe:{url}"),
            Mutation::Subscribe { url: url.into() },
        )?;
        Ok(())
    }
    pub fn sync_unsubscribe(&self, feed: u64) -> Result<bool, DbError> {
        let links = self
            .container
            .query::<SubscriptionLink>()
            .filter(SubscriptionLink::local_feed().eq(feed) & SubscriptionLink::removed().eq(false))
            .live()
            .try_collect()?;
        if links.is_empty() {
            if let Some(f) = self.container.try_get::<Feed>(feed)?
                && f.sync_account().peek().is_some()
            {
                f.unsubscribed().write(true);
                return Ok(true);
            }
            return Ok(false);
        }
        for link in links {
            if self
                .container
                .try_get::<Feed>(feed)?
                .and_then(|f| f.sync_account().peek())
                != Some(link.account)
            {
                continue;
            }
            self.sync_queue(
                link.account,
                &format!("subscription:{}", link.remote),
                Mutation::Unsubscribe {
                    subscription: link.remote,
                },
            )?;
        }
        if let Some(f) = self.container.get::<Feed>(feed) {
            f.unsubscribed().write(true);
        }
        Ok(true)
    }
    pub fn sync_apply_subscriptions(
        &self,
        account: u64,
        changes: Changes<Subscription>,
    ) -> Result<(), DbError> {
        let existing = self
            .container
            .query::<SubscriptionLink>()
            .filter(SubscriptionLink::account().eq(account))
            .live()
            .try_collect()?;
        let (rows, removed) = match changes {
            Changes::Unchanged => return Ok(()),
            Changes::Delta { upserts, removed } => (upserts, removed),
            Changes::Snapshot(rows) => {
                let present: HashSet<_> = rows.iter().map(|s| s.id.as_str()).collect();
                let removed = existing
                    .iter()
                    .filter(|l| !present.contains(l.remote.as_str()))
                    .map(|l| l.remote.clone())
                    .collect();
                (rows, removed)
            }
        };
        let pending = self.sync_pending(account)?;
        for remote in removed {
            if let Some(link) = existing.iter().find(|l| l.remote == remote) {
                if let Some(f) = self.container.get::<Feed>(link.local_feed) {
                    f.unsubscribed().write(true);
                }
                if let Some(l) = self.container.get::<SubscriptionLink>(link.id) {
                    l.removed().write(true);
                }
                // Removal wins over a stale rename or move. A later explicit subscribe is a new
                // intent.
                for op in &pending {
                    if matches!(&op.mutation,Mutation::Rename{subscription,..}|Mutation::Unsubscribe{subscription} if *subscription==remote)
                        || matches!(&op.mutation,Mutation::SetFolder{feed,..} if *feed==link.remote_feed)
                    {
                        self.sync_ack(account, op.revision)?;
                    }
                }
            }
        }
        for s in rows {
            let old = existing.iter().find(|l| l.remote == s.id);
            if old.is_none_or(|l| l.removed || l.remote_feed != s.feed_id)
                && let Some(a) = self.container.try_get::<Account>(account)?
            {
                // A newly followed feed may contain only old, already-read entries. Do not
                // stop at a known first page before reaching this subscription's history.
                a.last_sync().write(None);
            }
            let local = old
                .map(|l| l.local_feed)
                .unwrap_or_else(|| self.add_feed(&s.url, &s.title, None));
            let rename = pending.iter().find_map(|o| match &o.mutation {
                Mutation::Rename {
                    subscription,
                    title,
                } if *subscription == s.id => Some(title.clone()),
                _ => None,
            });
            let mut deleting=pending.iter().any(|o|matches!(&o.mutation,Mutation::Unsubscribe{subscription} if *subscription==s.id));
            if let Some(f) = self.container.get::<Feed>(local) {
                // New subscriptions can be edited/deleted while their POST is in flight.
                if old.is_none() && f.sync_account().peek() == Some(account) {
                    deleting |= f.unsubscribed().peek();
                    if deleting {
                        self.sync_queue(
                            account,
                            &format!("subscription:{}", s.id),
                            Mutation::Unsubscribe {
                                subscription: s.id.clone(),
                            },
                        )?;
                    } else if let Some(title) = f.custom_title().peek()
                        && title != s.title
                    {
                        self.sync_queue(
                            account,
                            &format!("subscription:{}", s.id),
                            Mutation::Rename {
                                subscription: s.id.clone(),
                                title,
                            },
                        )?;
                    }
                    if !deleting {
                        self.sync_queue_linked_folder(account, local, &s.feed_id)?;
                    }
                }
                f.sync_account().write(Some(account));
                f.unsubscribed().write(deleting);
                let title = rename
                    .or_else(|| {
                        if old.is_none() {
                            f.custom_title().peek()
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| s.title.clone());
                f.title().write(title.clone());
                f.custom_title().write(Some(title));
                f.site_url().write(s.site_url);
            }
            self.container.insert(SubscriptionLink {
                id: link_id(account, &s.id),
                account,
                remote: s.id,
                remote_feed: s.feed_id,
                local_feed: local,
                title: s.title,
                removed: false,
            });
        }
        Ok(())
    }
    pub fn sync_missing(&self, account: u64, ids: Vec<String>) -> Vec<String> {
        ids.into_iter()
            .filter(|id| {
                self.container
                    .get::<EntryLink>(entry_id(account, id))
                    .is_none_or(|l| self.container.get::<Article>(l.article().peek()).is_none())
            })
            .collect()
    }
    pub fn sync_import_entries(
        &self,
        account: u64,
        entries: Vec<Entry>,
        read: &ReadChanges,
    ) -> Result<daynews_sync::engine::Imported, DbError> {
        if entries.is_empty() {
            return Ok(Default::default());
        }
        let mut added = 0;
        let links = self
            .container
            .query::<SubscriptionLink>()
            .filter(SubscriptionLink::account().eq(account) & SubscriptionLink::removed().eq(false))
            .live()
            .try_collect()?;
        let feeds: HashMap<_, _> = links
            .iter()
            .map(|l| (l.remote_feed.clone(), l.local_feed))
            .collect();
        let pending: HashMap<_, _> = self
            .sync_pending(account)?
            .into_iter()
            .filter_map(|o| match o.mutation {
                Mutation::SetRead(s) => Some((s.id, s.read)),
                _ => None,
            })
            .collect();
        let unread: HashSet<_> = match read {
            ReadChanges::UnreadSnapshot(ids) => ids.iter().cloned().collect(),
            _ => HashSet::new(),
        };
        let mut all_known = !entries.is_empty()
            && self
                .container
                .try_get::<Account>(account)?
                .is_some_and(|a| a.last_sync().peek().is_some());
        for e in entries {
            let Some(&feed) = feeds.get(&e.feed_id) else {
                continue;
            };
            let identity = entry_id(account, &e.id);
            let old = self.container.get::<EntryLink>(identity);
            all_known &= old.is_some();
            let local = if let Some(old) = old {
                old.article().peek()
            } else {
                // Adopt a unique same-feed URL match, never guess from a title or date.
                let matches = e
                    .url
                    .as_ref()
                    .filter(|u| !u.is_empty())
                    .map(|url| {
                        self.container
                            .query::<Article>()
                            .filter(Article::feed().is(feed) & Article::url().eq(Some(url.clone())))
                            .limit(2)
                            .live()
                            .ids()
                    })
                    .unwrap_or_default();
                if matches.len() == 1 {
                    matches[0].handle()
                } else {
                    identity
                }
            };
            let confirmed = match read {
                ReadChanges::UnreadSnapshot(_) => !unread.contains(&e.id),
                ReadChanges::Delta(rows) => {
                    rows.iter().find(|r| r.id == e.id).is_some_and(|r| r.read)
                }
            };
            let current = pending.get(&e.id).copied().unwrap_or(confirmed);
            if let Some(article) = self.container.get::<Article>(local) {
                // A bounded lookback can deliver publisher corrections. Update content without
                // disturbing read/star/tag intent or the original local arrival date. Unchanged
                // values must not dirty models or wake the timeline on every sync.
                macro_rules! update {
                    ($field:ident, $value:expr) => {
                        if article.$field().peek() != $value {
                            article.$field().write($value);
                        }
                    };
                }
                update!(title, e.title);
                update!(url, e.url);
                update!(author, e.author);
                update!(summary, e.summary);
                update!(published_at, e.published);
                if let Some(html) = e.content {
                    if let Some(body) = self.container.get::<ArticleBody>(local) {
                        if body.content_html().peek() != html {
                            update!(word_count, Some(dashboard::word_count(&html)));
                            body.content_html().write(html);
                        }
                    } else {
                        update!(word_count, Some(dashboard::word_count(&html)));
                        self.container.insert(ArticleBody {
                            id: local,
                            article: One::to(local),
                            content_html: html,
                        });
                    }
                }
            } else {
                all_known = false;
                added += 1;
                self.container.insert(Article {
                    id: local,
                    feed: One::to(feed),
                    guid: format!("remote:{account}:{}", e.id),
                    title: e.title,
                    url: e.url,
                    author: e.author,
                    published_at: e.published,
                    first_seen_at: now(),
                    summary: e.summary,
                    is_read: current,
                    word_count: Some(dashboard::word_count(e.content.as_deref().unwrap_or(""))),
                    ..Default::default()
                });
                if let Some(html) = e.content {
                    self.container.insert(ArticleBody {
                        id: local,
                        article: One::to(local),
                        content_html: html,
                    });
                }
            }
            let link = EntryLink {
                id: identity,
                account,
                remote: e.id,
                remote_feed: e.feed_id,
                article: local,
                confirmed_read: confirmed,
            };
            // Replayed history and incremental boundary pages must not dirty unchanged
            // links, flush them to SQLite, or invalidate subscribed UI queries.
            if self
                .container
                .try_get::<EntryLink>(identity)?
                .is_none_or(|old| {
                    old.account().peek() != link.account
                        || old.remote().peek() != link.remote
                        || old.remote_feed().peek() != link.remote_feed
                        || old.article().peek() != link.article
                        || old.confirmed_read().peek() != link.confirmed_read
                })
            {
                self.container.insert(link);
            }
        }
        Ok(daynews_sync::engine::Imported { all_known, added })
    }
    pub fn sync_apply_read(&self, account: u64, changes: ReadChanges) -> Result<(), DbError> {
        let pending: HashMap<_, _> = self
            .sync_pending(account)?
            .into_iter()
            .filter_map(|o| match o.mutation {
                Mutation::SetRead(s) => Some((s.id, s.read)),
                _ => None,
            })
            .collect();
        let links = self
            .container
            .query::<EntryLink>()
            .filter(EntryLink::account().eq(account))
            .live()
            .try_collect()?;
        let active: HashSet<_> = self
            .container
            .query::<SubscriptionLink>()
            .filter(SubscriptionLink::account().eq(account) & SubscriptionLink::removed().eq(false))
            .live()
            .try_collect()?
            .into_iter()
            .map(|l| l.remote_feed)
            .collect();
        let unread: HashSet<_> = match &changes {
            ReadChanges::UnreadSnapshot(ids) => ids.iter().cloned().collect(),
            _ => HashSet::new(),
        };
        for l in links {
            if !active.contains(&l.remote_feed) {
                continue;
            }
            let read = match &changes {
                ReadChanges::UnreadSnapshot(_) => !unread.contains(&l.remote),
                ReadChanges::Delta(rows) => {
                    let Some(r) = rows.iter().find(|r| r.id == l.remote) else {
                        continue;
                    };
                    r.read
                }
            };
            // Remote snapshots repeat unchanged values on every poll. Do not dirty the
            // entire library: dirty rows cannot leave the persistence working-set cache.
            if l.confirmed_read != read
                && let Some(link) = self.container.try_get::<EntryLink>(l.id)?
            {
                link.confirmed_read().write(read);
            }
            let current = pending.get(&l.remote).copied().unwrap_or(read);
            if let Some(article) = self.container.try_get::<Article>(l.article)?
                && article.is_read().peek() != current
            {
                article.is_read().write(current);
            }
        }
        Ok(())
    }
    pub fn sync_ack(&self, account: u64, revision: u64) -> Result<(), DbError> {
        let ids = self
            .container
            .query::<Intent>()
            .filter(Intent::account().eq(account) & Intent::revision().eq(revision))
            .live()
            .try_ids()?;
        for id in ids {
            self.container.delete::<Intent>(id.handle())?;
        }
        Ok(())
    }
    pub fn sync_outcomes(&self, account: u64, outcomes: Vec<Outcome>) -> Result<(), DbError> {
        for result in outcomes {
            if let Outcome::Applied {
                revision,
                subscription,
            } = result
            {
                // Acknowledge only the revision sent. A newer edit retains its pending row.
                if let Some(s) = &subscription {
                    // Feedbin can canonicalize a submitted URL. Bind its reply to the
                    // submitted local feed before applying the remote snapshot.
                    if let Some(op) = self
                        .sync_pending(account)?
                        .into_iter()
                        .find(|o| o.revision == revision)
                        && let Mutation::Subscribe { url } = op.mutation
                        && url != s.url
                        && let Some(f) = self.container.try_get::<Feed>(feed_id(&url))?
                    {
                        self.container.insert(SubscriptionLink {
                            id: link_id(account, &s.id),
                            account,
                            remote: s.id.clone(),
                            remote_feed: s.feed_id.clone(),
                            local_feed: f.id().peek(),
                            title: s.title.clone(),
                            removed: false,
                        });
                        if f.unsubscribed().peek() {
                            self.sync_queue(
                                account,
                                &format!("subscription:{}", s.id),
                                Mutation::Unsubscribe {
                                    subscription: s.id.clone(),
                                },
                            )?;
                        } else {
                            if let Some(title) = f.custom_title().peek()
                                && title != s.title
                            {
                                self.sync_queue(
                                    account,
                                    &format!("subscription:{}", s.id),
                                    Mutation::Rename {
                                        subscription: s.id.clone(),
                                        title,
                                    },
                                )?;
                            }
                            self.sync_queue_linked_folder(account, f.id().peek(), &s.feed_id)?;
                        }
                    }
                }
                self.sync_ack(account, revision)?;
                if let Some(s) = subscription {
                    self.sync_apply_subscriptions(
                        account,
                        Changes::Delta {
                            upserts: vec![s],
                            removed: vec![],
                        },
                    )?;
                }
            }
        }
        Ok(())
    }
    /// Undo touches user-visible rows, never rewinds network acknowledgement/checkpoints.
    /// Reconcile changed rows into fresh operations after a local undo/redo transaction.
    pub fn sync_after_undo(&self) -> Result<(), DbError> {
        for l in self.container.query::<EntryLink>().live().try_collect()? {
            if let Some(a) = self.container.get::<Article>(l.article) {
                let read = a.is_read().peek();
                let pending =
                    self.sync_pending(l.account)?
                        .into_iter()
                        .find_map(|o| match o.mutation {
                            Mutation::SetRead(s) if s.id == l.remote => Some(s.read),
                            _ => None,
                        });
                if read != pending.unwrap_or(l.confirmed_read) {
                    self.sync_queue_read(l.article, read)?;
                }
            }
        }
        for l in self
            .container
            .query::<SubscriptionLink>()
            .live()
            .try_collect()?
        {
            let Some(f) = self.container.try_get::<Feed>(l.local_feed)? else {
                continue;
            };
            if f.sync_account().peek() != Some(l.account) {
                continue;
            }
            let pending = self.sync_pending(l.account)?;
            let deleting=pending.iter().any(|o|matches!(&o.mutation,Mutation::Unsubscribe{subscription} if *subscription==l.remote));
            if !f.unsubscribed().peek() && l.removed {
                self.sync_queue_subscribe(l.account, &f.feed_url().peek())?;
            } else if f.unsubscribed().peek() && !l.removed && !deleting {
                self.sync_unsubscribe(l.local_feed)?;
            } else if !f.unsubscribed().peek() && !l.removed {
                let title = f.title().peek();
                let remote_title = pending
                    .iter()
                    .find_map(|o| match &o.mutation {
                        Mutation::Rename {
                            subscription,
                            title,
                        } if *subscription == l.remote => Some(title.as_str()),
                        _ => None,
                    })
                    .unwrap_or(&l.title);
                if deleting || title != remote_title {
                    self.sync_queue_rename(l.local_feed, &title)?;
                }
                // Only a move still pending is known to differ from the service: after its
                // acknowledgement the next download files the feed as the service does.
                let folder = self.sync_folder_name(l.local_feed)?;
                if pending.iter().any(|o| {
                    matches!(&o.mutation, Mutation::SetFolder { feed, folder: queued }
                        if *feed == l.remote_feed && *queued != folder)
                }) {
                    self.sync_queue_feed_folder(l.local_feed, folder.as_deref())?;
                }
            }
        }
        Ok(())
    }
}
