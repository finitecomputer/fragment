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
}
