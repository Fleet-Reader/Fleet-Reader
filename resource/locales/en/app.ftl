language_name = English
app_title = Day News
nav_all_unread = All Unread
nav_all_articles = All Articles
nav_starred = Starred
timeline_empty = No articles
timeline_synchronizing = Articles synchronizing…
timeline_empty_unread = All caught up
reader_empty = Select an article
reader_no_content = This article has no content. Open it in your browser to read it.
search_placeholder = Search articles
subscribe_heading = Add a subscription
subscribe_placeholder = Feed or site URL
subscribe_action = Subscribe
refresh_action = Refresh
toggle_read = Toggle Read/Unread
refresh_progress = Refreshing { $done } of { $total }…
list_status_updated_now = Updated just now
list_status_updated_minutes = { $minutes ->
    [one] Updated { $minutes } minute ago
   *[other] Updated { $minutes } minutes ago
}
list_status_updated_time = Updated at { DATETIME($when, timeStyle: "short") }
list_status_updated_date = Updated { DATETIME($when, dateStyle: "medium") }
mark_all_read = Mark All as Read
mark_read = Mark as Read
mark_unread = Mark as Unread
star = Star
unstar = Unstar
unsubscribe = Unsubscribe
opml_heading = Subscriptions file
opml_import = Import OPML…
opml_export = Export OPML…
feeds_count =
    { $count ->
        [one] { $count } feed
       *[other] { $count } feeds
    }
nav_today = Today
nav_smart_feeds = Smart Feeds
nav_feeds_section = Feeds
# The feeds in no collection, and a collection: its name and how many feeds it holds.
nav_feeds_section_total = Feeds ({ $count })
nav_collection_section = { $name } ({ $count })
menu_file = File
menu_go = Go
menu_new_feed = New Feed…
menu_new_collection = New Collection…
menu_refresh = Refresh
menu_import = Import Subscriptions…
menu_export = Export Subscriptions…
menu_next_unread = Next Unread
menu_feed = Feed
menu_refresh_feed = Refresh Feed
new_feed_title = New Feed
new_collection_title = New Collection
new_collection_placeholder = Collection name
collection_menu = Collection
collection_none = None
collection_new = New Collection…
rename_collection_title = Rename Collection
rename_feed_title = Rename Feed
feed_name_placeholder = Feed name
menu_rename_feed = Rename Feed…
rename_feed = Rename…
feed_name_label = Name
delete_collection_title = Delete “{ $name }”?
delete_collection_message = Its feeds stay subscribed, outside any collection.
collection_name_taken = A collection named “{ $name }” already exists.
collection_ok = OK
unread_count = { $n ->
    [0] No unread
    [one] { $n } unread
   *[other] { $n } unread
}
untitled = Untitled
menu_article = Article
menu_mark_read = Mark as Read
menu_mark_unread = Mark as Unread
menu_star = Mark as Starred
menu_unstar = Mark as Unstarred
menu_open_in_browser = Open in Browser
nav_tags_section = Tags
nav_settings = Settings
tag_action = Tag…
tag_prompt_title = Tag Article
tag_prompt_placeholder = Tag name (again to remove)
menu_tag = Tag Article…
settings_articles = Articles
settings_reader = Reader
settings_browser = Browser
settings_feeds = Feeds
settings_retention_label = Keep articles
settings_retention_note = Limits Feedbin history downloads. Read articles older than this period are removed at launch and when this setting changes, using their publication date (arrival date if undated). Unread, locally starred, and tagged articles are always kept.
retention_30 = For one month
retention_90 = For three months
retention_180 = For six months
retention_365 = For one year
retention_forever = Forever
retention_pruned = Removed { $n } old articles

time_now = now
time_minutes = { $n }m
time_hours = { $n }h
time_days = { $n }d
timeline_date = { DATETIME($when, dateStyle: "medium", timeStyle: "none") }
article_date = { DATETIME($when, dateStyle: "long", timeStyle: "short") }

settings_browser_label = Open web links in
settings_browser_embedded = Embedded Browser
settings_browser_embedded_note = A site you sign in to in the embedded browser stays signed in for Load Full Article. Other links use their system application.
settings_browser_system = System Default
settings_browser_default = System Default ({ $name })
settings_browser_note = This choice applies to Day News. Other links use their system application.
settings_browser_unavailable = Links use your default browser. Change it in the device’s system settings.

menu_increase_text_size = Increase Text Size
menu_decrease_text_size = Decrease Text Size
settings_reader_size = Text size
settings_reader_percent = { $value }%
settings_reader_font = Font
settings_reader_default_font = System Font
settings_reader_font_note = Recommended fonts appear first, followed by all available fonts.
settings_reader_background = Background color
settings_reader_text = Text color
settings_reader_reset = Reset Styles
settings_reader_reset_note = Restore System appearance, two preview lines, and default list and reader sizes, reader font, and colors.

settings_preview_lines = Preview lines
settings_preview_none = No preview
settings_preview_lines_count = { $count ->
    [one] 1 line
   *[other] { $count } lines
    }

settings_list_size = Text size
settings_list_percent = { $value }%
menu_reader_view = Load Full Content
reader_view_failed = Couldn’t Load Full Article
reader_view_error_details = Show full article loading error
reader_view_loading = Loading full article…
reader_view_network_error = Couldn’t load the full article. Try Load Full Content again or open the original website.
reader_view_too_large = This page is too large for Load Full Content. Open the original website to read it.
reader_view_no_content = No readable article was found. The page may require JavaScript, sign-in, or a subscription. Open the original website to read it.
reader_view_timeout = Loading the full article took too long. Try Load Full Content again.
reader_view_unavailable = Load Full Content is unavailable. Open the original website to read the full article.
reader_browse = Open Page
reader_browse_failed = The full article didn’t load. Open the page to sign in or read it there.
reader_browse_short = This may be only part of the article. Open the page to sign in or read it there.
show_unread_feeds_only = Show Only Unread Feeds
show_all_feeds = Show All Feeds

menu_find_articles = Find Articles…
menu_view = View
menu_show_status_bar = Show Status Bar
menu_hide_status_bar = Hide Status Bar
status_bar_feeds = { $count ->
    [one] { NUMBER($count) } feed
   *[other] { NUMBER($count) } feeds
}
storage_error = { $error }

status_feed_refreshed = Refreshed { $title }
status_feed_failed = Could not refresh { $title }
status_feeds_refreshed = Refreshed { $done } of { $total } feeds — { $failed } failed
status_imported = Imported { $added } feeds ({ $existing } already subscribed)
subscriptions_export_title = Day News Subscriptions
menu_previous_article = Previous Article
menu_next_article = Next Article
menu_copy_article_link = Copy Article Link
copy_link_failed = The article link could not be copied. Please try again.
dismiss_alert = OK

settings_refresh_feeds = Refresh Feeds
settings_refresh_manual = Manually
settings_refresh_30 = Every 30 Minutes
settings_refresh_60 = Every Hour
settings_refresh_120 = Every 2 Hours
settings_refresh_240 = Every 4 Hours
settings_refresh_480 = Every 8 Hours

settings_refresh_automatic = Automatic
settings_refresh_automatic_note = Automatic checks each feed based on how often and how recently it publishes, from every 30 minutes to once a day.
group_by_feeds = Group by Feeds
move_feed_up = Move Feed Up
move_feed_down = Move Feed Down
settings_collections = Collections
settings_collections_note = Drag collections to set their order in the sidebar. Choose how each one sorts its feeds; with Manual, drag feeds in the sidebar to order them. The default collection holds feeds that are in no other collection: rename it or move it, but it can't be deleted. Deleting another collection moves its feeds there.
collection_default = Default
collection_add = New Collection…
collection_rename = Rename…
collection_delete = Delete
sort_feeds = Sort Feeds
sort_priority = Priority
sort_recently_updated = Recently Updated
sort_name = Name
sort_unread_count = Unread Count
sort_total_count = Total Count
sort_manual = Manual
feed_order_note = Reorder feeds to set refresh priority and article group order. Drag feeds or use Move Feed Up and Move Feed Down.

# Dashboards. Numbers, dates, chart axes and countdown digits follow the active locale.
dashboard_overview = FEED INTELLIGENCE
dashboard_today_eyebrow = YOUR DAY IN STORIES
dashboard_unread_eyebrow = YOUR READING HORIZON
dashboard_starred_eyebrow = YOUR PERSONAL COLLECTION
dashboard_library_eyebrow = THE BIG PICTURE
dashboard_feed_intro = A portrait of this publication, built from the articles in your library.
dashboard_today_intro = What arrived today, who published it, and how much there is to explore.
dashboard_unread_intro = See the shape of your backlog and find room for your next good read.
dashboard_starred_intro = The stories you kept: a map of your interests, publishers and reading time.
dashboard_library_intro = Your publications, their rhythms, and the stories they bring together.
dashboard_articles = Articles
dashboard_unread = Unread
dashboard_reading = Reading time
dashboard_publishers = Publishers
dashboard_read = Read
dashboard_number = { NUMBER($value, maximumFractionDigits: 0) }
dashboard_minutes = { NUMBER($value, maximumFractionDigits: 0) } min
dashboard_percent = { NUMBER($value, maximumFractionDigits: 0) }%
dashboard_authors = { $count ->
    [one] One contributing author
   *[other] { NUMBER($count) } contributing authors
    }
dashboard_activity = Publication rhythm
dashboard_activity_today = Arrivals by hour
dashboard_activity_unread = Age of your unread stories
dashboard_activity_starred = When your saved stories were published
dashboard_activity_note = { DATETIME($start, dateStyle: "medium", timeStyle: "none") } – { DATETIME($end, dateStyle: "medium", timeStyle: "none") } · articles by publication date
dashboard_today_note = Today, in your local time
dashboard_unread_note = Recent discoveries and stories waiting a little longer
dashboard_words_title = Common words
dashboard_words_note = Titles and summaries · choose a word to search for it
dashboard_words_empty = Not enough text yet
dashboard_sources = Publisher mix
dashboard_sources_note = Where these stories come from
dashboard_chart_date = { DATETIME($when, dateStyle: "medium", timeStyle: "none") }
dashboard_chart_hour = { NUMBER($hour, minimumIntegerDigits: 2, useGrouping: "false") }:00
dashboard_chart_count = Articles
dashboard_age_today = <1 day
dashboard_age_week = 1–6 days
dashboard_age_month = 1–4 weeks
dashboard_age_quarter = 1–3 months
dashboard_age_old = 3+ months
dashboard_other = Other publishers
dashboard_empty = No stories here yet
dashboard_empty_note = Your dashboard will take shape as articles arrive.
dashboard_loading = Gathering your feed insights…
dashboard_automatic = AUTOMATIC REFRESH
dashboard_every_minutes = Every { NUMBER($minutes) } minutes
dashboard_every_hours = Every { NUMBER($hours) } hours
dashboard_daily = Once a day
dashboard_interval_range = { $fast } – { $slow }
dashboard_next = Next check: { DATETIME($when, dateStyle: "medium", timeStyle: "medium") }
dashboard_countdown = { NUMBER($hours, minimumIntegerDigits: 2, useGrouping: "false") }:{ NUMBER($minutes, minimumIntegerDigits: 2, useGrouping: "false") }:{ NUMBER($seconds, minimumIntegerDigits: 2, useGrouping: "false") }
dashboard_due = Due now · waiting for the next scheduler tick
dashboard_refreshing = Checking feeds now…
dashboard_manual = Manual refresh is selected. Automatic would use this cadence.
dashboard_fixed = A fixed refresh interval is selected. Automatic would use this cadence.
dashboard_no_schedule = Subscribe to a feed to see its refresh forecast.
dashboard_backoff = Some feeds are backing off after an error or an origin-requested delay.
dashboard_policy = Based on recent publication cadence and recency; quiet feeds are checked less often.
dashboard_show = Feed overview
dashboard_checked = Last checked: { DATETIME($when, dateStyle: "medium", timeStyle: "short") }
dashboard_contributors = Named authors

dashboard_refresh_now = Refresh Now


subscribe_invalid = Enter a valid website or feed URL
subscribe_missing = No feed found
subscribe_missing_note = This website does not advertise an RSS, Atom, or JSON feed.
subscribe_choose = Choose a feed
subscribe_choose_note = This website offers several feeds. Select the one you want to follow.
subscribe_candidate = { $title } — { $url }
subscribe_duplicate_title = Already Subscribed
subscribe_duplicate = You already subscribe to “{ $title }” at this address.
subscribe_same_feed_title = Subscribe Again?
subscribe_same_feed = Most of this feed’s stories already arrive through “{ $title }”. It is probably the same feed at another address.
subscribe_anyway = Subscribe Anyway
subscribe_show_existing = Show Feed
subscribe_failed = Could not subscribe to this feed. Check the URL and try again.
cancel_action = Cancel
opml_filter = Subscription lists
opml_import_failed = Import failed: { $error }
opml_read_failed = Could not read the file: { $error }
opml_exported = Exported subscriptions

reader_inline_mode = Inline Reader Mode
reader_inline_mode_note = Show the full article below the feed preview in the same page. Turn this off to replace the preview with the full article.
reader_auto_mode = Auto-load Reader Mode
reader_auto_mode_note = Automatically load full articles when their load button becomes visible. You can override this for each feed. Hiding an article keeps auto-loading enabled when this default is on.
reader_load_full = Load Full Article
reader_hide_full = Hide Full Article


site_preview = Preview or Log In…
site_forget = Forget Site Data
site_label = Site
incognito_mode = Incognito Mode
site_browser_back = Back
site_browser_forward = Forward
site_browser_close = Close
site_browser_done = Done
site_browser_open_external = Open in Browser
site_data_cleared = Site information forgotten.
site_data_failed = Could not clear site information. Please try again.
site_private_unsupported = Private browsing is unavailable on this platform. Site browsing is disabled in Incognito Mode.

subscribe_verify = Verify
subscribe_preview_note = Verify a feed or website address to preview its articles before subscribing.
subscribe_verifying = Checking feed…
subscribe_preview_ready =
    { $count ->
        [one] One article available
       *[other] { $count } articles available
    }

reader_script_app = App Reader Script…
reader_script_site = Site Reader Script
reader_script_edit = Edit…
reader_script_label = Reader script
reader_script_sites = Manage Site Scripts…
reader_script_new_site = New site…
reader_script_address = Site URL
reader_script_invalid_site = Enter an http:// or https:// site URL without credentials.
reader_script_help = Define function extract(document, url, Readability). Return an object with HTML content and optional title and byline, or null to use Mozilla Readability. Site scripts override the app script; sites match the final article URL’s scheme, host, and port.
reader_script_trust = Install only scripts you trust: they execute JavaScript in the reader. Input and returned HTML are sanitized. Close discards unsaved edits.
reader_script_template = Mozilla Template
reader_script_import = Import…
reader_script_export = Export…
reader_script_filter = JavaScript files
reader_script_import_failed = Could not import the script. Choose a UTF-8 JavaScript file, at most 256 KiB, without NUL characters.
reader_script_remove = Remove Override
reader_script_removed = Override removed. This scope now inherits the app script or Mozilla Readability.
reader_script_save = Save Script
reader_script_saved = Script saved. It will be used the next time full content is loaded.
reader_script_invalid_source = Enter a nonempty script, at most 256 KiB, without NUL characters.
reader_script_failed = The custom reader script failed. Check its extract function and return value, or remove the override in Reader Script settings to restore the default.

# Account synchronization
settings_accounts = Accounts
sync_feedbin = Feedbin
sync_description = Synchronize subscriptions and read/unread status between your devices using Feedbin.
sync_username = Email address
sync_password = Password
sync_sign_in = Sign In
sync_preview = Subscriptions in Feedbin: { $remote }. Eligible local subscriptions: { $local }.
sync_read_state_import = Feedbin’s reading state will be used for synchronized articles. Existing unmatched local history stays on this device.
sync_merge = Also upload my local subscriptions to Feedbin
sync_connect = Connect and Synchronize
sync_cancel = Cancel
sync_account = Connected as { $username }
sync_pending = Changes waiting to synchronize: { $count }
sync_last = Last synchronized: { $time }
sync_never = Not synchronized yet
sync_now = Sync Now
sync_disconnect = Disconnect
sync_disconnect_note = Disconnecting keeps downloaded articles on this device and leaves your Feedbin account unchanged.
sync_auth_error = Sign-in failed or your credentials have expired. Please sign in again.
sync_storage_error = Secure credentials or account data could not be saved. Check that your device’s credential store is available and unlocked.
sync_network_error = Unable to reach Feedbin. Your changes are saved and will be retried.
sync_failed = Some changes could not be synchronized. Your pending changes have been kept. Try again.
sync_working = Synchronizing…
sync_ready = Synchronization is up to date

sync_waiting = Changes are waiting to synchronize…

sync_cancel_transfer = Cancel Synchronization
sync_paused = Synchronization cancelled. Downloaded articles and pending changes are kept. Choose Sync Now to resume.
sync_reset = Clear Local Data and Re-synchronize…
sync_reset_title = Clear local data and re-synchronize?
sync_reset_message = This deletes all local subscriptions, articles, saved reader content, stars, folders, tags, pending changes, and undo history on this device. Unsynchronized changes will be lost. Your Feedbin account, saved login, and app preferences are kept. Nothing is deleted from Feedbin; subscriptions, articles, and read/unread status will be downloaded again.
sync_reset_confirm = Clear and Re-synchronize
sync_progress_preparing = Preparing synchronization…
sync_progress_upload = Uploading local changes: { $done }/{ $total }
sync_progress_subscriptions = Synchronizing subscriptions: { $count } feeds received
sync_progress_read = Synchronizing read/unread status…
sync_progress_articles = Downloading articles: page { $page }/{ $total } · { $articles ->
    [one] 1 new article saved
   *[other] { $articles } new articles saved
    }
sync_progress_articles_unknown = Downloading articles · Pages: { $pages } · New articles saved: { $articles }
sync_progress_feed = Current page includes: { $title }
sync_progress_missing = Downloading missing unread articles: { $done }/{ $total }
sync_progress_finalizing = Updating reading state and saving synchronization checkpoint…
sync_progress_complete = Synchronization complete
sync_progress_resetting = Clearing local library…
sync_progress_duration = { $minutes }m { $seconds }s
sync_progress_eta = Elapsed { $elapsed } · About { $remaining } remaining in this stage
sync_progress_estimating = Elapsed { $elapsed } · Estimating time remaining…

reader_page_loading = Loading article…
reader_page_unavailable = This article is no longer available.
reader_page_failed = Unable to load this article. Return to the article list to try again.

settings_show_sync_status = Show synchronization status
settings_show_status_bar = Show status bar
settings_word_cloud_layout = Word cloud layout
settings_word_cloud_2d = Across and up
settings_word_cloud_1d = Across only
settings_word_cloud_font = Word cloud font
settings_word_cloud_system_font = System

# Experimental WebDAV enrollment. Private credentials are never shown in these controls.
dav_title = WebDAV account
accounts_webdav = WebDAV
accounts_provider = Synchronization provider
dav_description = WebDAV keeps your subscriptions, collections, feed order, read/unread choices, and stars in a folder on your server. Each device downloads article content separately. Tags stay on each device, and Feedbin accounts sync separately.
    Use a server you trust: sync files are not encrypted on the server. To connect another device, copy the library ID from Advanced and enter it there before connecting. Changing the server requires disconnecting first.
dav_endpoint = Server URL
dav_password = Password
dav_reqwest = Alternative HTTP client
dav_create = Create sync space
dav_join = Join existing space
dav_disabled = Enter your server and account details to connect.
dav_restart = Saved. Restart Day-News to connect.
dav_failed = Could not sync. Check your password and connection. Your changes are saved on this device.
dav_attention = Sync needs attention. Check your account and server, then try Sync now.


dav_username = Username

dav_update_credentials = Save password


# Experimental recovery keeps the library visible without silently bypassing journaling.
dav_read_only = Sync could not start. Check your account details and restart.
dav_pause = Pause synchronization
dav_resume = Resume synchronization
dav_paused = Sync is paused. Changes are saved on this device.
dav_disconnect = Disconnect account
dav_disconnect_confirm = Back up and disconnect
dav_disconnect_message = After restart, Day-News will back up this library, including unsent records, then remove its configuration and any saved sync credentials it can identify. Local feeds and articles stay. Unsent changes will not reach other devices. This does not revoke the device on the server.
dav_disconnect_restart = Restart Day-News to finish backing up and disconnecting WebDAV. Your local library will be preserved.
dav_space_id = Library ID
dav_current_space = Current space
dav_copy_space = Copy library ID

dav_information = About WebDAV sync
dav_connect = Connect
dav_advanced = Advanced
dav_advanced_help = To join an existing library, enter its ID before connecting. Leave it empty to create a new library. Save your password to apply a networking change.

accounts_sync_unavailable = Account synchronization is not available in this build.

refresh_preparing = Preparing refresh…
status_feeds_sync_pending = Refreshed { $done } of { $total } feeds — { $failed } failed; library sync pending
library_sync_pending = Library sync pending
