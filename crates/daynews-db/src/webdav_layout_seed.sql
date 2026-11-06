INSERT INTO _news_dav_layout_pending SELECT 'collection',CAST(id AS TEXT),1,name FROM folders;
INSERT INTO _news_dav_layout_pending SELECT 'collection_order',CAST(id AS TEXT),0,CAST(position AS TEXT) FROM folders;
INSERT INTO _news_dav_layout_pending SELECT 'collection_sort',CAST(id AS TEXT),0,sort FROM folders;
INSERT INTO _news_dav_layout_pending SELECT 'membership',feed_url,0,COALESCE(CAST(folder AS TEXT),'') FROM feeds WHERE sync_account IS NULL AND (feed_url LIKE 'https://%' OR feed_url LIKE 'http://%') AND EXISTS(SELECT 1 FROM folders);
INSERT INTO _news_dav_layout_pending SELECT 'feed_order',feed_url,0,CAST(collection_position AS TEXT) FROM feeds WHERE sync_account IS NULL AND (feed_url LIKE 'https://%' OR feed_url LIKE 'http://%') AND EXISTS(SELECT 1 FROM folders);
INSERT INTO _news_dav_layout_pending SELECT 'priority',feed_url,0,CAST(position AS TEXT) FROM feeds WHERE sync_account IS NULL AND (feed_url LIKE 'https://%' OR feed_url LIKE 'http://%') AND EXISTS(SELECT 1 FROM folders);
