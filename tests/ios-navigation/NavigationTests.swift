import XCTest

// Real UIKit gestures: injected route changes cannot catch an offstage split column.
// Run only against the isolated English fixtures documented in native-interactions.md.
final class NavigationTests: XCTestCase {
    let app = XCUIApplication(bundleIdentifier: "org.appfair.app.FleetReader")

    func testNewFeedRemainsUsableWhenPasteIsDenied() {
        continueAfterFailure = false
        app.launchEnvironment["DAY_DATA_DIR"] = "/tmp/day-news-new-feed-clipboard-test"
        app.launchArguments = ["-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()
        let newFeed = app.buttons["New Feed…"].firstMatch
        if !newFeed.waitForExistence(timeout: 2) || !newFeed.isHittable {
            let overflow = app.buttons["day-toolbar-overflow"].firstMatch
            visible(overflow)
            overflow.tap()
        }
        visible(newFeed)
        newFeed.tap()
        // UIKit's external-paste prompt may arrive while the optional suggestion is
        // pending. Denying it must leave an editable, dismissible subscription form.
        let alert = XCUIApplication(bundleIdentifier: "com.apple.springboard").alerts.firstMatch
        if alert.waitForExistence(timeout: 3) {
            let deny = alert.buttons.matching(NSPredicate(
                format: "label IN %@", ["Don't Allow Paste", "Don’t Allow Paste", "Ne pas autoriser le collage"]
            )).firstMatch
            visible(deny)
            deny.tap()
        }
        let address = app.textFields["feed-url"]
        visible(address)
        address.tap()
        address.press(forDuration: 1)
        let selectAll = app.menuItems["Select All"]
        if selectAll.exists { selectAll.tap(); address.typeText(XCUIKeyboardKey.delete.rawValue) }
        address.typeText("https://fixture.example/manual.xml")
        XCTAssertTrue((address.value as? String)?.contains("https://fixture.example/manual.xml") == true)
        // Return dismisses the software keyboard on compact phones before the footer tap.
        address.typeText("\n")
        photograph("New Feed after clipboard permission handling")
        let cancel = app.buttons["feed-cancel"].firstMatch
        visible(cancel)
        cancel.tap()
        XCTAssertTrue(address.waitForNonExistence(timeout: 5))
    }

    func testScrollingFeedChromeAndAdaptiveSettings() {
        continueAfterFailure = false
        app.launchEnvironment["DAY_DATA_DIR"] = "/tmp/day-news-mobile-chrome"
        app.launchEnvironment["DAY_NEWS_SYNC_TEST"] = "1"
        app.launch()
        let back = app.navigationBars.buttons.matching(
            NSPredicate(format: "label == 'Back' OR label == 'Fleet Reader'")).firstMatch
        if back.waitForExistence(timeout: 2) && back.isHittable { back.tap() }
        let search = app.searchFields.firstMatch
        // Pull down first: the previous session may have left the list scrolled.
        let feeds = app.collectionViews.firstMatch
        visible(feeds)
        feeds.swipeDown()
        visible(search)
        let bar = app.navigationBars.firstMatch
        let expanded = bar.frame.height
        photograph("Expanded feeds search and title")
        feeds.swipeUp()
        let collapsed = XCTNSPredicateExpectation(
            predicate: NSPredicate { _, _ in bar.frame.height < expanded - 20 }, object: nil)
        XCTAssertEqual(XCTWaiter.wait(for: [collapsed], timeout: 5), .completed)
        XCTAssertFalse(search.isHittable)
        photograph("Collapsed feeds search and title")
        feeds.swipeDown()
        feeds.swipeDown()
        visible(search)
        let settings = app.buttons["Settings"].firstMatch
        visible(settings)
        settings.tap()
        let settingsBar = app.navigationBars["Settings"]
        visible(settingsBar)
        let width = app.frame.width
        if width >= 600 {
            XCTAssertLessThan(settingsBar.frame.width, width - 80)
            XCTAssertGreaterThan(settingsBar.frame.minX, 20)
        } else {
            XCTAssertEqual(settingsBar.frame.width, width, accuracy: 2)
        }
        photograph("Adaptive Settings presentation")
        let done = app.buttons["Done"].firstMatch
        visible(done)
        done.tap()
        visible(search)
    }

    func testTabletToolbarColumns() {
        continueAfterFailure = false
        app.launchEnvironment["DAY_DATA_DIR"] = "/tmp/day-news-mobile-chrome"
        app.launchEnvironment["DAY_NEWS_SYNC_TEST"] = "1"
        app.launch()
        XCTAssertGreaterThan(app.frame.width, 600)
        let all = app.staticTexts["All Articles"].firstMatch
        visible(all)
        all.tap()
        let refresh = app.navigationBars.buttons["Refresh"].firstMatch
        let filter = app.navigationBars.buttons["Show Only Unread Feeds"].firstMatch
        visible(refresh)
        visible(filter)
        // The commands belong to the middle column, and its title must not repeat Fleet Reader.
        let bars = app.navigationBars.allElementsBoundByIndex.filter { $0.isHittable }
            .sorted { $0.frame.minX < $1.frame.minX }
        XCTAssertEqual(bars.count, 3)
        XCTAssertTrue(bars[1].frame.contains(refresh.frame))
        XCTAssertTrue(bars[1].frame.contains(filter.frame))
        XCTAssertFalse(bars[1].staticTexts["Fleet Reader"].exists)
        XCTAssertEqual(app.navigationBars.buttons.matching(identifier: "Refresh").count, 1)
        for button in app.navigationBars.buttons.allElementsBoundByIndex where button.isHittable {
            XCTAssertGreaterThanOrEqual(button.frame.width, 28, "Compressed toolbar: \(button.label)")
        }
        photograph("Toolbar ownership across iPad columns")
        let count = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Feeds · '")).firstMatch
        visible(count)
        let before = count.label
        filter.tap()
        XCTAssertEqual(count.label, before, "Filtering must not change subscription totals")
        filter.tap()
    }

    func testRootReachableFromSmartFeeds() {
        continueAfterFailure = false
        app.launchEnvironment["DAY_DATA_DIR"] = "/tmp/day-news-navigation-up-empty"
        app.launch()
        // A compact initial selection may show the list or the root. Either way,
        // a visible list must provide UIKit's Back, not just a scripted nav_back.
        let back = app.navigationBars.buttons.matching(
            NSPredicate(format: "label == 'Back' OR label == 'Fleet Reader'")).firstMatch
        if app.staticTexts["No articles"].waitForExistence(timeout: 3) {
            visible(back)
            back.tap()
        }
        for title in ["Today", "All Unread", "Starred", "All Articles", "Today"] {
            let row = app.staticTexts[title].firstMatch
            visible(row)
            row.tap()
            visible(app.staticTexts[title == "All Unread" ? "All caught up" : "No articles"].firstMatch)
            visible(back)
            photograph("Back from \(title)")
            back.tap()
            visible(app.staticTexts["All Articles"].firstMatch)
        }
    }

    func visible(_ element: XCUIElement) {
        let expectation = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "hittable == true"), object: element)
        XCTAssertEqual(XCTWaiter.wait(for: [expectation], timeout: 8), .completed)
    }

    func photograph(_ name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    func testArticlePaging() {
        // XCTest may relaunch its target. Explicitly retain the isolated fixture store
        // and navigate natively; never let an offscreen timeline label pass as an article.
        continueAfterFailure = false
        app.launchEnvironment["DAY_DATA_DIR"] = "/tmp/day-news-native-pager"
        app.launchEnvironment["DAY_NEWS_HTTP_TEST"] = "1"
        app.launch()
        let feed = app.staticTexts["Field Notes"].firstMatch
        visible(feed)
        feed.tap()
        let first = "How monarch butterflies find their way to Mexico"
        let second = "What a honeybee's dance tells the hive"
        let row = app.tables.staticTexts[first].firstMatch
        visible(row)
        row.tap()
        visible(app.webViews.staticTexts[first].firstMatch)
        func point(_ x: CGFloat) -> XCUICoordinate {
            app.coordinate(withNormalizedOffset: CGVector(dx: x, dy: 0.55))
        }
        // A short, deliberately slow drag must cancel, preserving the current article.
        point(0.65).press(forDuration: 0.05, thenDragTo: point(0.58),
                         withVelocity: XCUIGestureVelocity(rawValue: 60), thenHoldForDuration: 0.5)
        visible(app.webViews.staticTexts[first].firstMatch)
        photograph("Cancelled article page")
        // A long slow drag exercises tracking rather than a completion-only flick.
        point(0.85).press(forDuration: 0.05, thenDragTo: point(0.15),
                         withVelocity: XCUIGestureVelocity(rawValue: 180), thenHoldForDuration: 0.1)
        visible(app.webViews.staticTexts[second].firstMatch)
        photograph("Next article page")
        point(0.15).press(forDuration: 0.05, thenDragTo: point(0.85))
        visible(app.webViews.staticTexts[first].firstMatch)
        // Rotate every cached slot, then reverse, exercising UIKit's cached neighbors.
        let titles = [first, second,
                      "Three hearts and blue blood: inside the octopus",
                      "Reading the tide pools: why the sea rises twice a day",
                      "Trees that were growing when the Great Pyramid was built",
                      "The Arctic tern's pole-to-pole commute"]
        for title in titles.dropFirst() {
            point(0.85).press(forDuration: 0.05, thenDragTo: point(0.15))
            visible(app.webViews.staticTexts[title].firstMatch)
        }
        point(0.85).press(forDuration: 0.05, thenDragTo: point(0.15))
        visible(app.webViews.staticTexts[titles.last!].firstMatch)
        for title in titles.dropLast().reversed() {
            point(0.15).press(forDuration: 0.05, thenDragTo: point(0.85))
            visible(app.webViews.staticTexts[title].firstMatch)
        }
        photograph("Previous article page")
        // No wrap at the first article.
        point(0.15).press(forDuration: 0.05, thenDragTo: point(0.85))
        visible(app.webViews.staticTexts[first].firstMatch)
    }

    func testEmptySmartFeeds() {
        app.activate()
        // Today is selected initially. Repeat it after Back to cover same-route activation.
        for title in ["Today", "Today", "All Unread", "Starred", "All Articles", "Today"] {
            let row = app.staticTexts[title].firstMatch
            visible(row)
            row.tap()
            let empty = app.staticTexts[title == "All Unread" ? "All caught up" : "No articles"].firstMatch
            visible(empty)
            photograph(title)
            app.navigationBars.buttons.element(boundBy: 0).tap()
            visible(app.staticTexts["All Articles"].firstMatch)
        }
    }

    func testPullToRefresh() {
        // Pre-subscribe to /progress/known and return to the feeds root. The slow fixture
        // makes a real request observable; no production subscriptions/accounts are touched.
        app.activate()
        visible(app.staticTexts["Known length fixture"].firstMatch)
        func hits() -> Int {
            let done = expectation(description: "fixture stats")
            var count = -1
            URLSession.shared.dataTask(with: URL(string: "http://127.0.0.1:28762/stats")!) { data, _, error in
                if let data, let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                   let hits = object["hits"] as? [String: Int] {
                    count = hits["/progress/known"] ?? 0
                }
                XCTAssertNil(error)
                done.fulfill()
            }.resume()
            wait(for: [done], timeout: 5)
            return count
        }
        let before = hits()
        let start = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.3))
        let end = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.8))
        start.press(forDuration: 0.05, thenDragTo: end)
        // XCTest waits for UIKit animation quiescence, which can outlast this request.
        // The Android real-touch regression separately checks duplicate pulls while busy.
        photograph("Pull refresh")
        XCTAssertEqual(hits(), before + 1, "Pull must start exactly one HTTP request")
        sleep(7)
        start.press(forDuration: 0.05, thenDragTo: end)
        XCTAssertEqual(hits(), before + 2, "Completion must allow another pull")
        sleep(7)
        photograph("Pull refresh complete")
        app.staticTexts["All Articles"].firstMatch.tap()
        visible(app.staticTexts["Synthetic progress article"].firstMatch)
    }
}
