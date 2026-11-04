import XCTest

/// What the app shows with no server behind it. Flows against a real
/// platform (`cargo xtask dev`, paired through `/cli`) are their own suite.
@MainActor
final class FragmentUITests: XCTestCase {
    private func screenshot(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot()); shot.name = name; shot.lifetime = .keepAlways; add(shot)
    }

    /// A first launch asks for the person's Finite server, and a server that
    /// does not answer says so, leaving the screen to try again.
    func testConnectAsksForTheServerAndSaysWhenItIsUnreachable() throws {
        let app = XCUIApplication()
        // a closed port on this machine: nothing answers it
        app.launchArguments = ["--pair-server", "http://127.0.0.1:9"]
        app.launch()
        let server = app.textFields["accountServer"]
        guard server.waitForExistence(timeout: 10) else {
            throw XCTSkip("This simulator holds a connected account; reset its keychain (`xcrun simctl keychain booted reset`) and run again.")
        }
        XCTAssertEqual(server.value as? String, "http://127.0.0.1:9")
        XCTAssertFalse(app.buttons["tryPreview"].exists, "there is no unsigned preview")
        screenshot(app, "Connect")
        app.buttons["pairAccount"].tap()
        XCTAssertTrue(app.staticTexts.containing(NSPredicate(format: "label CONTAINS[c] %@", "could not connect")).firstMatch.waitForExistence(timeout: 20))
        XCTAssertFalse(app.staticTexts["pairingCode"].exists)
        XCTAssertTrue(app.buttons["pairAccount"].isEnabled)
    }
}
