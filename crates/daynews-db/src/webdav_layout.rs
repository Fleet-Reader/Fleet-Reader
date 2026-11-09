// SPDX-License-Identifier: Apache-2.0
//! Revision-two collection registers. Projection is independent of delivery order.
use super::{State, err, rows, text};
use day_persistence::{DbError, SqliteConnection, Value};
use day_sync::{Change, Replica};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn is_layout(kind: &str) -> bool {
    matches!(
        kind,
        "collection"
            | "collection_order"
            | "collection_sort"
            | "membership"
            | "feed_order"
            | "priority"
    )
}
fn collection_id(s: &str) -> Option<i64> {
    s.parse::<i64>()
        .ok()
        .filter(|n| *n > 0 && n.to_string() == s)
}
fn position(s: &str) -> Option<f64> {
    s.parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && n.abs() <= 1e12)
}
fn canonical_position(s: &str) -> Option<String> {
    position(s).and_then(|n| serde_json::to_string(&n).ok())
}
pub(super) fn validate(s: &State) -> bool {
    if !s.guid.is_empty() {
        return false;
    }
    if s.kind.starts_with("collection") {
        if collection_id(&s.feed).is_none() {
            return false;
        }
    } else if !(s.feed.starts_with("https://") || s.feed.starts_with("http://")) {
        return false;
    }
    match s.kind.as_str() {
        "collection" => !s.label.trim().is_empty() && s.label.trim() == s.label,
        "collection_sort" => !s.value && crate::FeedSort::ALL.iter().any(|v| v.key() == s.label),
        "collection_order" | "feed_order" | "priority" => {
            !s.value && canonical_position(&s.label).as_ref() == Some(&s.label)
        }
        "membership" => !s.value && (s.label.is_empty() || collection_id(&s.label).is_some()),
        _ => false,
    }
}
fn state(r: &[Value]) -> Result<State, DbError> {
    Ok(State {
        kind: r[0].as_text()?.into(),
        feed: r[1].as_text()?.into(),
        guid: String::new(),
        value: r[2].as_int()? != 0,
        label: r[3].as_text()?.into(),
    })
}
pub(super) fn retain(c: &mut dyn SqliteConnection, s: &State) -> Result<(), DbError> {
    c.execute("INSERT INTO _news_dav_layout_values(kind,feed,value,label) VALUES(?,?,?,?) ON CONFLICT(kind,feed) DO UPDATE SET value=excluded.value,label=excluded.label", &[text(&s.kind),text(&s.feed),Value::Int(s.value as i64),text(&s.label)])?;
    Ok(())
}
pub(super) fn capture(c: &mut dyn SqliteConnection) -> Result<Vec<Change>, DbError> {
    let mut changes = vec![];
    for r in rows(
        c,
        "SELECT kind,feed,value,label FROM _news_dav_layout_pending p WHERE NOT EXISTS(SELECT 1 FROM feeds f WHERE f.feed_url=p.feed AND f.sync_account IS NOT NULL) ORDER BY kind,feed",
        &[],
    )? {
        let mut s = state(&r)?;
        if matches!(
            s.kind.as_str(),
            "collection_order" | "feed_order" | "priority"
        ) {
            s.label = canonical_position(&s.label).ok_or_else(err)?;
        }
        changes.push(s.change()?);
        retain(c, &s)?;
    }
    c.execute("DELETE FROM _news_dav_layout_pending", &[])?;
    Ok(changes)
}
/// Import pre-existing local organization once. Empty joining libraries publish no defaults
/// that could clear the creator's membership. Held remote records take precedence at upgrade.
pub(super) async fn seed(replica: &Replica) -> Result<(), day_sync::Error> {
    replica
        .worker()
        .write(|db| {
            db.try_with_connection(|c| {
                if !rows(c, "SELECT id FROM _news_dav_layout_seeded", &[])?.is_empty() {
                    return Ok(());
                }
                if rows(c, "SELECT feed FROM _news_dav_layout_values LIMIT 1", &[])?.is_empty() {
                    c.execute_batch(include_str!("webdav_layout_seed.sql"))?;
                }
                c.execute("INSERT INTO _news_dav_layout_seeded VALUES(1)", &[])?;
                Ok(())
            })
        })
        .await?;
    Ok(())
}

pub(super) fn project(c: &mut dyn SqliteConnection, s: &State) -> Result<(), DbError> {
    if s.kind.starts_with("collection") {
        project_collections(c)
    } else {
        project_feed(c, &s.feed)
    }
}

fn project_collections(c: &mut dyn SqliteConnection) -> Result<(), DbError> {
    let records = rows(
        c,
        "SELECT kind,feed,value,label FROM _news_dav_layout_values WHERE kind IN ('collection','collection_order','collection_sort') ORDER BY feed,kind",
        &[],
    )?;
    let mut attrs = BTreeMap::new();
    let mut collections = BTreeMap::new();
    for r in records {
        let s = state(&r)?;
        if s.kind == "collection" {
            collections.insert(collection_id(&s.feed).ok_or_else(err)?, s.clone());
        }
        attrs.insert((s.feed, s.kind), s.label);
    }
    let existing = rows(c, "SELECT id,name FROM folders ORDER BY id", &[])?;
    let mut used: BTreeSet<String> = existing
        .iter()
        .filter(|r| !collections.contains_key(&r[0].as_int().unwrap_or(0)))
        .map(|r| r[1].as_text().unwrap_or_default().to_string())
        .collect();
    // Reserve all canonical names before assigning stable disambiguators to conflicting
    // renames. No replica's arrival order decides who gets a name.
    let mut counts = BTreeMap::<String, usize>::new();
    for s in collections.values().filter(|s| s.value) {
        *counts.entry(s.label.clone()).or_default() += 1;
    }
    let reserved: BTreeSet<_> = counts.keys().cloned().collect();
    let mut names = BTreeMap::new();
    for (&id, s) in collections.iter().filter(|(_, s)| s.value) {
        let mut name = s.label.clone();
        if counts[&name] > 1 || used.contains(&name) {
            name = format!("{} [{id}]", s.label);
            while used.contains(&name) || reserved.contains(&name) {
                name.push_str(&format!(" [{id}]"));
            }
        }
        used.insert(name.clone());
        names.insert(id, name);
    }
    // Rename in two phases to handle swaps under the model's UNIQUE(name) constraint.
    // Temporary labels exist only inside this transaction and never reach the UI.
    let mut occupied: BTreeSet<_> = existing
        .iter()
        .map(|r| r[1].as_text().unwrap_or_default().to_owned())
        .chain(names.values().cloned())
        .collect();
    for r in &existing {
        let id = r[0].as_int()?;
        if collections.contains_key(&id)
            && names.get(&id).map(String::as_str) != Some(r[1].as_text()?)
        {
            let mut temp = format!("\u{1f}day-sync:{id}");
            while occupied.contains(&temp) {
                temp.push(':');
            }
            occupied.insert(temp.clone());
            c.execute(
                "UPDATE folders SET name=? WHERE id=?",
                &[text(temp), Value::Int(id)],
            )?;
        }
    }
    for (&id, s) in &collections {
        if !s.value {
            // Release every feed, including Feedbin-owned feeds, before the schema's cascade.
            c.execute(
                "UPDATE feeds SET folder=NULL WHERE folder=?",
                &[Value::Int(id)],
            )?;
            c.execute("DELETE FROM folders WHERE id=?", &[Value::Int(id)])?;
            continue;
        }
        let order = attrs
            .get(&(s.feed.clone(), "collection_order".into()))
            .and_then(|v| position(v))
            .unwrap_or(0.0);
        let sort = attrs
            .get(&(s.feed.clone(), "collection_sort".into()))
            .cloned()
            .unwrap_or_default();
        c.execute("INSERT INTO folders(id,name,position,sort) VALUES(?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,position=excluded.position,sort=excluded.sort WHERE name IS NOT excluded.name OR position IS NOT excluded.position OR sort IS NOT excluded.sort",&[Value::Int(id),text(&names[&id]),Value::Real(order),text(sort)])?;
    }
    for r in rows(
        c,
        "SELECT feed FROM _news_dav_layout_values WHERE kind='membership' ORDER BY feed",
        &[],
    )? {
        project_feed(c, r[0].as_text()?)?;
    }
    Ok(())
}

pub(super) fn project_feed(c: &mut dyn SqliteConnection, feed: &str) -> Result<(), DbError> {
    for r in rows(
        c,
        "SELECT kind,feed,value,label FROM _news_dav_layout_values WHERE feed=? AND kind IN ('membership','feed_order','priority') ORDER BY kind",
        &[text(feed)],
    )? {
        let s = state(&r)?;
        if s.kind == "membership" {
            let target = if let Some(id) = collection_id(&s.label) {
                if rows(c, "SELECT id FROM folders WHERE id=?", &[Value::Int(id)])?.is_empty() {
                    Value::Null
                } else {
                    Value::Int(id)
                }
            } else {
                Value::Null
            };
            c.execute("UPDATE feeds SET folder=? WHERE feed_url=? AND sync_account IS NULL AND folder IS NOT ?",&[target.clone(),text(feed),target])?;
        } else {
            let col = if s.kind == "priority" {
                "position"
            } else {
                "collection_position"
            };
            let v = Value::Real(position(&s.label).ok_or_else(err)?);
            c.execute(&format!("UPDATE feeds SET {col}=? WHERE feed_url=? AND sync_account IS NULL AND {col} IS NOT ?"),&[v.clone(),text(feed),v])?;
        }
    }
    Ok(())
}
