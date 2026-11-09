import XCTest

/// What the app shows with no server behind it. Flows against a real
/// platform (`cargo xtask dev`, paired through `/cli`) are their own suite.
@MainActor
final class FragmentUITests: XCTestCase {
    private func screenshot(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot()); shot.name = name; shot.lifetime = .keepAlways; add(shot)
    }

    /// A UI test's launch keeps a store of its own and reaches loopback
    /// alone (`--ui-test`): even an approved account past loopback saved
    /// there is never used, so it asks for the server, and a server that
    /// does not answer says so, leaving the screen to try again.
    func testConnectAsksForTheServerAndSaysWhenItIsUnreachable() throws {
        let app = XCUIApplication()
        // a closed port on this machine: nothing answers it
        app.launchArguments = ["--ui-test", "--ui-test-saved-account", "https://finite.example", "--pair-server", "http://127.0.0.1:9"]
        app.launch()
        let changeServer = app.buttons["changeServer"]
        XCTAssertTrue(changeServer.waitForExistence(timeout: 10), "a saved account past loopback is never loaded")
        changeServer.tap()
        let server = app.textFields["accountServer"]
        XCTAssertTrue(server.waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["This launch reaches only this machine."].exists)
        XCTAssertFalse(app.buttons["forgetAccount"].exists)
        // never ask any other server than the closed port
        guard server.value as? String == "http://127.0.0.1:9" else {
            return XCTFail("Connect shows \(server.value ?? "nothing"), not the --pair-server address")
        }
        XCTAssertFalse(app.buttons["tryPreview"].exists, "there is no unsigned preview")
        screenshot(app, "Connect")
        app.buttons["pairAccount"].tap()
        XCTAssertTrue(app.staticTexts.containing(NSPredicate(format: "label CONTAINS[c] %@", "could not connect")).firstMatch.waitForExistence(timeout: 20))
        XCTAssertFalse(app.staticTexts["pairingCode"].exists)
        XCTAssertTrue(app.buttons["pairAccount"].isEnabled)
    }

    /// Home's composer rests above the keyboard as Home opens, and again
    /// after Your Apps and Back: the keyboard Home takes back as it returns
    /// waits for Back's transition to end, so Home's layout makes room for
    /// it. An approved account at a closed loopback port is Home with
    /// nothing behind it.
    func testHomesComposerStaysAboveTheKeyboardAfterYourApps() {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-test", "--ui-test-saved-account", "http://127.0.0.1:9"]
        app.launch()
        let composer = app.textViews["composer"]
        XCTAssertTrue(composer.waitForExistence(timeout: 10), "Home, its saved account approved")
        let keyboard = app.keyboards.firstMatch
        XCTAssertTrue(keyboard.waitForExistence(timeout: 10), "Home opens with the keyboard")
        aboveTheKeyboard(composer, keyboard, "as Home opens")
        screenshot(app, "Home")

        app.buttons["yourApps"].tap()
        XCTAssertTrue(app.navigationBars["Your Apps"].waitForExistence(timeout: 5))
        XCTAssertTrue(keyboard.waitForNonExistence(timeout: 5), "Your Apps has no keyboard")
        app.navigationBars["Your Apps"].buttons.element(boundBy: 0).tap()
        XCTAssertTrue(keyboard.waitForExistence(timeout: 10), "Home takes the keyboard back")
        aboveTheKeyboard(composer, keyboard, "after Your Apps and Back")
        screenshot(app, "Home after Your Apps")
    }

    /// Words that wrap take the composer's whole width, its buttons a row
    /// beneath them; back on one line, they sit between the buttons again.
    /// Right at the wrap point it changes once each way, and the keyboard
    /// stays up throughout: every word typed lands in the field.
    func testComposerWordsTakeTheFullWidthOnceTheyWrap() {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-test", "--ui-test-saved-account", "http://127.0.0.1:9"]
        app.launch()
        let composer = app.textViews["composer"], attach = app.buttons["attach"], send = app.buttons["send"]
        XCTAssertTrue(composer.waitForExistence(timeout: 10), "Home, its saved account approved")
        let keyboard = app.keyboards.firstMatch
        XCTAssertTrue(keyboard.waitForExistence(timeout: 10), "Home opens with the keyboard")

        var typed = "Hello"
        composer.typeText(typed)
        XCTAssertTrue(send.waitForExistence(timeout: 5))
        expect(app, oneRow: true, composer, attach, send, "one line")
        screenshot(app, "Composer one line")
        // a word at a time to the wrap point: the first that wraps stacks it
        while beside(composer, attach), typed.count < 200 { composer.typeText(" wrap"); typed += " wrap" }
        expect(app, oneRow: false, composer, attach, send, "wrapped onto a second line")
        composer.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 5))
        typed.removeLast(5)
        expect(app, oneRow: true, composer, attach, send, "the wrapping word deleted")
        composer.typeText(" wrap"); typed += " wrap"
        expect(app, oneRow: false, composer, attach, send, "the wrapping word typed again")

        for line in 2...6 { composer.typeText("\nLine \(line)"); typed += "\nLine \(line)" }
        expect(app, oneRow: false, composer, attach, send, "six lines")
        aboveTheKeyboard(composer, keyboard, "six lines")
        screenshot(app, "Composer six lines")
        XCTAssertEqual(composer.value as? String, typed, "every key reached the field: it kept the keyboard")

        composer.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: typed.count - 5))
        expect(app, oneRow: true, composer, attach, send, "back to one line")
        XCTAssertEqual(composer.value as? String, "Hello")
        aboveTheKeyboard(composer, keyboard, "back to one line")
    }

    /// Whether the field sits to the right of the + (one row), not above it.
    private func beside(_ field: XCUIElement, _ attach: XCUIElement) -> Bool { field.frame.minX >= attach.frame.maxX - 0.5 }

    /// Waits for the composer's one row (the field between its buttons), or
    /// its two (the field across the buttons' full span, the buttons under it).
    private func expect(_ app: XCUIApplication, oneRow: Bool, _ field: XCUIElement, _ attach: XCUIElement, _ send: XCUIElement, _ when: String) {
        let laidOut = NSPredicate { _, _ in
            let f = field.frame, a = attach.frame, s = send.frame
            if oneRow { return self.beside(field, attach) && f.maxX <= s.minX + 0.5 && abs(f.maxY - a.maxY) < 1 }
            // the full span less the field's margin either side (`ComposerLayout.inset`)
            return f.maxY <= a.minY + 0.5 && f.maxY <= s.minY + 0.5 && f.width >= s.maxX - a.minX - 2 * 8 - 1
        }
        let settled = XCTWaiter().wait(for: [XCTNSPredicateExpectation(predicate: laidOut, object: nil)], timeout: 5)
        XCTAssertEqual(settled, .completed, "\(when): field \(field.frame), + \(attach.frame), send \(send.frame)")
        if settled != .completed { screenshot(app, "Composer \(when)") }
        XCTAssertTrue(app.keyboards.firstMatch.exists, "\(when): the keyboard stays up")
    }

    /// Waits for `field` to rest wholly above `keyboard`, as the keyboard
    /// and the layout around it settle.
    private func aboveTheKeyboard(_ field: XCUIElement, _ keyboard: XCUIElement, _ when: String) {
        let above = NSPredicate { _, _ in field.exists && keyboard.exists && field.frame.maxY <= keyboard.frame.minY + 0.5 }
        let settled = XCTWaiter().wait(for: [XCTNSPredicateExpectation(predicate: above, object: nil)], timeout: 5)
        XCTAssertEqual(settled, .completed, "\(when): the composer ends at \(field.frame.maxY), the keyboard begins at \(keyboard.frame.minY)")
    }

    /// The boundary holds for the address Connect starts with, too: a test
    /// launch tapping Connect for Finite's own server asks nothing of it.
    func testATestLaunchRefusesAServerPastLoopback() {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-test"]
        app.launch()
        let changeServer = app.buttons["changeServer"]
        XCTAssertTrue(changeServer.waitForExistence(timeout: 10))
        changeServer.tap()
        let server = app.textFields["accountServer"]
        XCTAssertTrue(server.waitForExistence(timeout: 5))
        XCTAssertEqual(server.value as? String, "https://finite.chat")
        app.buttons["pairAccount"].tap()
        XCTAssertTrue(app.staticTexts["This launch reaches only this machine."].waitForExistence(timeout: 5))
        XCTAssertFalse(app.staticTexts["pairingCode"].exists)
    }

    /// What the person is told (FIN-458), against a real platform: a dev
    /// stack (`cargo xtask dev`, its port forwarded to this Mac) whose
    /// person's computer has an update ready and saves that fail, signed as
    /// that person by a key it approved ahead. Its notices show over Home in
    /// the server's words, the warning with its countdown and the platform's
    /// own words, each with its actions; Later is the update's, and the
    /// update goes. Named by `TEST_RUNNER_FRAGMENT_UITEST_SERVER` and
    /// `TEST_RUNNER_FRAGMENT_UITEST_KEY` (the key's 64 hex); skipped without.
    func testTheComputersNoticesShowAndTheirActionsWork() throws {
        let env = ProcessInfo.processInfo.environment
        guard let server = env["FRAGMENT_UITEST_SERVER"], let key = env["FRAGMENT_UITEST_KEY"] else {
            throw XCTSkip("no dev stack named (FRAGMENT_UITEST_SERVER, FRAGMENT_UITEST_KEY)")
        }
        XCUIDevice.shared.appearance = .light
        let app = XCUIApplication()
        app.launchArguments = ["--ui-test", "--ui-test-saved-account", server, "--ui-test-key", key]
        app.launch()
        // a new simulator's first keyboard offers its tip, over Home
        if app.buttons["Continue"].waitForExistence(timeout: 3) { app.buttons["Continue"].tap() }
        let notice = { (id: String) in app.descendants(matching: .any).matching(NSPredicate(format: "identifier BEGINSWITH %@", "notice:\(id)")).firstMatch }
        let update = notice("update:"), unsaved = notice("unsaved")
        XCTAssertTrue(update.waitForExistence(timeout: 30), "the update ready is told")
        XCTAssertTrue(unsaved.waitForExistence(timeout: 10), "and the saves failing")
        XCTAssertTrue(app.staticTexts["An update is ready for your computer"].exists)
        XCTAssertTrue(app.staticTexts["Your computer can't save right now"].exists)
        XCTAssertTrue(app.staticTexts.containing(NSPredicate(format: "label BEGINSWITH %@", "If it still can't save, it stops in")).firstMatch.exists, "its countdown")
        XCTAssertTrue(update.buttons["Restart"].exists && update.buttons["Later"].exists)
        XCTAssertTrue(unsaved.buttons["Restart now"].exists)
        screenshot(app, "Notices, light")
        // the system's appearance applies a moment after it is set: the shot waits for it
        XCUIDevice.shared.appearance = .dark
        _ = XCTWaiter.wait(for: [XCTestExpectation(description: "dark, drawn")], timeout: 2)
        screenshot(app, "Notices, dark")
        XCUIDevice.shared.appearance = .light
        _ = XCTWaiter.wait(for: [XCTestExpectation(description: "light, drawn")], timeout: 2)
        update.buttons["Later"].tap()
        XCTAssertTrue(update.waitForNonExistence(timeout: 20), "Later: the update is told no more")
        XCTAssertTrue(unsaved.exists, "the other stays")
        screenshot(app, "After Later")
    }
}
