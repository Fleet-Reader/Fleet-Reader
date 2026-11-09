// SPDX-License-Identifier: Apache-2.0
//! Separate processes keep environment changes isolated from concurrent database tests.
use std::process::Command;

#[test]
fn sql_logging_fixture() {
    let db = daynews_db::Db::open_in_memory().unwrap();
    db.add_feed(
        "https://synthetic.example/sql-trace",
        "SQL trace fixture",
        None,
    );
    db.container.save().unwrap();
    assert_eq!(db.unread_count(daynews_db::Scope::All).get(), 0);
}

#[test]
fn sql_logging_is_opt_in_and_independent_of_log_level() {
    for enabled in [false, true] {
        let mut child = Command::new(std::env::current_exe().unwrap());
        child.args(["--exact", "sql_logging_fixture", "--nocapture"]);
        child.env("DAY_LOG", "off");
        child.env_remove("DAY_NEWS_LOG_SQL");
        if enabled {
            child.env("DAY_NEWS_LOG_SQL", "1");
        }
        let result = child.output().unwrap();
        let logs = String::from_utf8_lossy(&result.stderr);
        assert!(result.status.success(), "{logs}");
        assert_eq!(logs.contains("[Fleet-Reader SQL]"), enabled, "{logs}");
        if enabled {
            for expected in ["CREATE TABLE", "INSERT", "SELECT", "SQL trace fixture"] {
                assert!(logs.contains(expected), "missing {expected}: {logs}");
            }
        }
    }
}
