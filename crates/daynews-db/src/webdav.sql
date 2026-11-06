-- Protocol-owned SQL. No secrets or article-cache foreign keys.
CREATE TABLE IF NOT EXISTS _news_dav_guard(id INTEGER PRIMARY KEY CHECK(id=1),projecting INTEGER NOT NULL CHECK(projecting IN (0,1)));
INSERT OR IGNORE INTO _news_dav_guard VALUES(1,0);
CREATE INDEX IF NOT EXISTS _news_dav_article_identity ON articles(feed,guid);
CREATE TABLE IF NOT EXISTS _news_dav_seeded(id INTEGER PRIMARY KEY CHECK(id=1));
CREATE TABLE IF NOT EXISTS _news_dav_values(kind TEXT NOT NULL CHECK(kind IN ('subscription','read','star')),feed TEXT NOT NULL,guid TEXT NOT NULL,value INTEGER NOT NULL CHECK(value IN (0,1)),label TEXT NOT NULL,PRIMARY KEY(kind,feed,guid));
CREATE TABLE IF NOT EXISTS _news_dav_pending(kind TEXT NOT NULL CHECK(kind IN ('subscription','read','star')),feed TEXT NOT NULL,guid TEXT NOT NULL,value INTEGER NOT NULL CHECK(value IN (0,1)),label TEXT NOT NULL,PRIMARY KEY(kind,feed,guid));
CREATE TABLE IF NOT EXISTS _news_dav_arrivals(id INTEGER PRIMARY KEY);
CREATE TRIGGER IF NOT EXISTS _news_dav_feed_add AFTER INSERT ON feeds WHEN NEW.sync_account IS NULL AND (NEW.feed_url LIKE 'https://%' OR NEW.feed_url LIKE 'http://%') AND (SELECT projecting FROM _news_dav_guard)=0 BEGIN
 INSERT INTO _news_dav_pending VALUES('subscription',NEW.feed_url,'',1,NEW.title) ON CONFLICT(kind,feed,guid) DO UPDATE SET value=1,label=excluded.label;
END;
CREATE TRIGGER IF NOT EXISTS _news_dav_feed_delete BEFORE DELETE ON feeds WHEN OLD.sync_account IS NULL AND (OLD.feed_url LIKE 'https://%' OR OLD.feed_url LIKE 'http://%') AND (SELECT projecting FROM _news_dav_guard)=0 BEGIN
 INSERT INTO _news_dav_pending VALUES('subscription',OLD.feed_url,'',0,OLD.title) ON CONFLICT(kind,feed,guid) DO UPDATE SET value=0,label=excluded.label;
END;
CREATE TRIGGER IF NOT EXISTS _news_dav_arrival AFTER INSERT ON articles BEGIN
 INSERT OR IGNORE INTO _news_dav_arrivals VALUES(NEW.id);
END;
CREATE TRIGGER IF NOT EXISTS _news_dav_read AFTER UPDATE OF is_read ON articles WHEN NEW.is_read IS NOT OLD.is_read AND (SELECT projecting FROM _news_dav_guard)=0 AND EXISTS(SELECT 1 FROM feeds WHERE id=NEW.feed AND sync_account IS NULL AND (feed_url LIKE 'https://%' OR feed_url LIKE 'http://%')) BEGIN
 INSERT INTO _news_dav_pending SELECT 'read',feed_url,NEW.guid,NEW.is_read,'' FROM feeds WHERE id=NEW.feed ON CONFLICT(kind,feed,guid) DO UPDATE SET value=excluded.value;
END;
CREATE TRIGGER IF NOT EXISTS _news_dav_star AFTER UPDATE OF is_starred ON articles WHEN NEW.is_starred IS NOT OLD.is_starred AND (SELECT projecting FROM _news_dav_guard)=0 AND EXISTS(SELECT 1 FROM feeds WHERE id=NEW.feed AND sync_account IS NULL AND (feed_url LIKE 'https://%' OR feed_url LIKE 'http://%')) BEGIN
 INSERT INTO _news_dav_pending SELECT 'star',feed_url,NEW.guid,NEW.is_starred,'' FROM feeds WHERE id=NEW.feed ON CONFLICT(kind,feed,guid) DO UPDATE SET value=excluded.value;
END;

CREATE TRIGGER IF NOT EXISTS _news_dav_feed_visibility AFTER UPDATE OF unsubscribed ON feeds WHEN NEW.unsubscribed IS NOT OLD.unsubscribed AND NEW.sync_account IS NULL AND (NEW.feed_url LIKE 'https://%' OR NEW.feed_url LIKE 'http://%') AND (SELECT projecting FROM _news_dav_guard)=0 BEGIN
 INSERT INTO _news_dav_pending VALUES('subscription',NEW.feed_url,'',1-NEW.unsubscribed,NEW.title) ON CONFLICT(kind,feed,guid) DO UPDATE SET value=excluded.value,label=excluded.label;
END;
