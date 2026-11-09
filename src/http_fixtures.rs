// SPDX-License-Identifier: Apache-2.0
//! Explicit dayscript-only synthetic publication. Never contacts a publisher.
//! Enable with DAY_NEWS_HTTP_TEST=1 before launch; ordinary app launches do not install it.
pub(crate) fn install() {
    let sync = day::env("DAY_NEWS_SYNC_TEST").as_deref() == Some("1");
    let http = day::env("DAY_NEWS_HTTP_TEST").as_deref() == Some("1");
    if !sync && !http {
        return;
    }
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let simulation = if sync {daynews_sync::fixture::Server::new().simulation} else {day_part_http::simulation::Simulation::new()};
        simulation.set_strict(false);
        if sync && let Some(ms) = day::env("DAY_NEWS_SYNC_TEST_LATENCY_MS").and_then(|s| s.parse::<u64>().ok()) {
            simulation.set_conditions(day_part_http::simulation::Conditions {
                latency: std::time::Duration::from_millis(ms.min(10_000)),
                ..Default::default()
            });
        }
        if http { simulation.handle(|_, url| url.starts_with("https://dayscript.invalid/"), |request| {
            use day_part_http::simulation::Reply;
            if request.url != "https://dayscript.invalid/feed.json" { return Ok(Reply::new(404, Vec::new())); }
            if request.headers.iter().any(|(k,v)| k.eq_ignore_ascii_case("if-none-match") && v == "\"fixture-1\"") {
                return Ok(Reply::new(304, Vec::new()).header("ETag", "\"fixture-1\""));
            }
            // Synthetic unit/dayscript data, not shipped application UI translations.
            Ok(Reply::new(200, br#"{"version":"https://jsonfeed.org/version/1.1","title":"HTTP interception fixture","feed_url":"https://dayscript.invalid/feed.json","items":[{"id":"fixture-one","title":"Intercepted article","date_published":"2026-10-01T12:00:00Z","content_text":"This synthetic article travelled through the ordinary HTTP and feed parsing path."}]}"#.to_vec()).header("Content-Type", "application/feed+json").header("ETag", "\"fixture-1\""))
        }); }
        day_part_http::Session::global().set_simulation(Some(simulation));
    });
}
