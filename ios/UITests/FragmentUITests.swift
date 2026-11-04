import XCTest

@MainActor
final class FragmentUITests: XCTestCase {
    override func setUp() async throws {
        // Keep UI flows independent of mutations and extra apps made by transport tests.
        var request = URLRequest(url: URL(string: "http://localhost:8910/test/reset")!)
        request.httpMethod = "POST"
        let (_, response) = try await URLSession.shared.data(for: request)
        XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
    }
    private func waitUntilGone(_ element: XCUIElement) {
        let gone = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: element)
        XCTAssertEqual(XCTWaiter.wait(for: [gone], timeout: 5), .completed)
    }
    private func launch() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["--server", "http://localhost:8910"]
        app.launch()
        XCTAssertTrue(app.textFields["launcherInput"].waitForExistence(timeout: 10))
        return app
    }
    private func screenshot(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot()); shot.name = name; shot.lifetime = .keepAlways; add(shot)
    }
    func testLauncherSearchAndNewChat() {
        let app = launch()
        let input = app.textFields["launcherInput"]
        screenshot(app, "Wallpaper launcher")
        input.tap(); input.typeText("quiet minutes")
        let hit = app.buttons["messageResult-morning:chat:2"]
        XCTAssertTrue(hit.waitForExistence(timeout: 10))
        XCTAssertTrue(app.buttons["newChatResult"].exists)
        hit.tap()
        XCTAssertTrue(app.navigationBars["A calmer morning"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.buttons["Attach files"].exists)
        XCTAssertTrue(app.buttons["Record a voice memo"].exists)
        app.buttons["launcher"].tap()
        app.buttons["Clear search"].tap()
        input.tap(); input.typeText("A fresh request from the launcher\n")
        XCTAssertTrue(app.buttons["stop"].waitForExistence(timeout: 10))
        app.buttons["stop"].tap()
        XCTAssertTrue(app.buttons["send"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.staticTexts["Fragment"].exists)
    }
    func testSidebarDragCancelsAndDismisses() {
        let app = launch()
        app.buttons["sidebar"].tap()
        let history = app.buttons["sidebar-history"]
        XCTAssertTrue(history.waitForExistence(timeout: 5))
        let before = history.frame
        // A slow, short drag should settle back to the same position.
        let start = app.coordinate(withNormalizedOffset: CGVector(dx: 0.65, dy: 0.5))
        start.press(forDuration: 0.1, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.55, dy: 0.5)), withVelocity: .slow, thenHoldForDuration: 0.2)
        XCTAssertTrue(history.exists)
        XCTAssertEqual(history.frame.minX, before.minX, accuracy: 2)
        // A longer drag completes the dismissal and restores the launcher.
        start.press(forDuration: 0.1, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.05, dy: 0.5)), withVelocity: .slow, thenHoldForDuration: 0.1)
        let input = app.textFields["launcherInput"]
        input.tap(); input.typeText("A draft after closing the sidebar")
        XCTAssertEqual(input.value as? String, "A draft after closing the sidebar")
        app.buttons["sidebar"].tap()
        XCTAssertTrue(history.waitForExistence(timeout: 5))
        XCTAssertTrue(history.isHittable)
        history.tap()
        XCTAssertTrue(app.navigationBars["History"].waitForExistence(timeout: 5))
    }

    func testSidebarMultipleAgentsAndHistory() {
        let app = launch()
        app.buttons["sidebar"].tap()
        XCTAssertTrue(app.buttons["agentMenu-juniper"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.buttons["agentMenu-moss"].exists)
        screenshot(app, "Multi-agent sidebar")
        app.buttons["sidebarChat-planning"].tap()
        XCTAssertTrue(app.navigationBars["A weekend outside"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.buttons["chatRecipients"].exists)
        XCTAssertFalse(app.buttons["chatOptions"].exists)
        app.buttons["sidebar"].tap()
        app.buttons["agentMenu-moss"].tap()
        XCTAssertTrue(app.buttons["Rename chat…"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["Archive chat"].exists)
        app.buttons["Agents in this chat"].tap()
        XCTAssertTrue(app.buttons["joinAgent-juniper"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["joinAgent-juniper"].isEnabled)
        XCTAssertTrue(app.descendants(matching: .any).matching(identifier: "messageRecipient").firstMatch.waitForExistence(timeout: 5))
        app.buttons["Done"].tap()
        app.buttons["sidebar-history"].tap()
        XCTAssertTrue(app.searchFields["Search chats"].waitForExistence(timeout: 10))
        app.buttons["Archived"].tap()
        XCTAssertTrue(app.buttons["historyChat-archived-chat"].waitForExistence(timeout: 5))
        app.buttons["Imported"].tap()
        XCTAssertTrue(app.buttons["importedSession-imported-chat"].waitForExistence(timeout: 5))
    }
    func testYourAppsListSheetAndPreservedComposer() {
        let app = launch()
        let input = app.textFields["launcherInput"]
        input.tap(); input.typeText("quiet minutes")
        let hit = app.buttons["messageResult-morning:chat:2"]
        XCTAssertTrue(hit.waitForExistence(timeout: 10)); hit.tap()
        let composer = app.textViews["composer"]
        // SwiftUI may expose the multiline field as either a text view or field.
        let field = composer.exists ? composer : app.textFields["composer"]
        field.tap(); field.typeText("A draft to keep")
        app.buttons["yourApps"].tap()
        XCTAssertTrue(app.navigationBars["Your Apps"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.keyboards.firstMatch.exists)
        XCTAssertTrue(app.buttons["manageApps"].exists)
        XCTAssertFalse(app.buttons["launcher"].exists)
        XCTAssertTrue(app.buttons["app-reading"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["app-add-app"].exists)
        XCTAssertTrue(app.tables.firstMatch.exists || app.collectionViews.firstMatch.exists)
        screenshot(app, "Your Apps list")
        app.buttons["app-reading"].tap()
        XCTAssertTrue(app.webViews.staticTexts["A few good things."].waitForExistence(timeout: 10))
        XCTAssertFalse(app.buttons["Done"].exists)
        let header = app.navigationBars["Reading list"]
        header.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.1))
            .press(forDuration: 0.1, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.95)))
        waitUntilGone(app.webViews.firstMatch)
        XCTAssertTrue(app.buttons["app-reading"].waitForExistence(timeout: 5))
        app.buttons["sidebar"].tap()
        let launcherButton = app.buttons["sidebarNewChat"]
        let drawerOpen = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in launcherButton.exists && launcherButton.isHittable && launcherButton.frame.minX > 250 }, object: launcherButton)
        XCTAssertEqual(XCTWaiter.wait(for: [drawerOpen], timeout: 5), .completed)
        launcherButton.tap()
        XCTAssertTrue(app.buttons["messageResult-morning:chat:2"].waitForExistence(timeout: 10))
        app.buttons["messageResult-morning:chat:2"].tap()
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        XCTAssertEqual(field.value as? String, "A draft to keep")
    }
    func testAppSettingsHideRestoreAppearanceAndInvitations() {
        let app = launch()
        app.buttons["yourApps"].tap()
        XCTAssertTrue(app.buttons["manageApps"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["launcher"].exists)
        app.buttons["appSettings-reading"].tap()
        XCTAssertTrue(app.navigationBars["App Settings"].waitForExistence(timeout: 5))
        let shown = app.switches["appShown"]
        XCTAssertTrue(shown.waitForExistence(timeout: 5))
        shown.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap()
        app.buttons["Close"].tap()
        waitUntilGone(app.buttons["app-reading"])
        app.buttons["manageApps"].tap()
        XCTAssertTrue(app.buttons["manageApp-reading"].waitForExistence(timeout: 5))
        app.buttons["manageApp-reading"].tap()
        XCTAssertTrue(shown.waitForExistence(timeout: 5))
        shown.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap()
        app.buttons["appAppearance"].tap()
        XCTAssertTrue(app.buttons["makeAppIcons"].waitForExistence(timeout: 5))
        app.buttons["makeAppIcons"].tap()
        XCTAssertTrue(app.buttons["useAppIcon-0"].waitForExistence(timeout: 10))
        app.buttons["useAppIcon-0"].tap()
        app.swipeUp()
        let capture = app.buttons["makeAppPreview"]
        XCTAssertTrue(capture.waitForExistence(timeout: 5)); capture.tap()
        XCTAssertTrue(app.images["appPreviewImage"].waitForExistence(timeout: 10))
        app.swipeUp()
        XCTAssertTrue(app.staticTexts["Preview updated."].waitForExistence(timeout: 5))
        screenshot(app, "App icon and generated preview")
        app.buttons["BackButton"].tap()
        app.buttons["appSharing"].tap()
        XCTAssertTrue(app.navigationBars["Sharing & Permissions"].waitForExistence(timeout: 5))
        app.swipeUp()
        let email = app.textFields["inviteEmail"]
        XCTAssertTrue(email.waitForExistence(timeout: 5)); email.tap(); email.typeText("ios-guest@finite.local")
        app.buttons["sendAppInvite"].tap()
        app.swipeUp()
        XCTAssertTrue(app.staticTexts["ios-guest@finite.local"].waitForExistence(timeout: 10))
        let revoke = app.buttons["revokeInvite-ios-guest@finite.local"]
        XCTAssertTrue(revoke.exists); revoke.tap()
        waitUntilGone(app.staticTexts["ios-guest@finite.local"])
    }

    func testBrainUsesWebFilesReaderAndLinks() {
        let app = launch()
        app.buttons["yourApps"].tap()
        let brain = app.buttons["app-test-brain"]
        XCTAssertTrue(brain.waitForExistence(timeout: 5)); brain.tap()
        let web = app.webViews.firstMatch
        XCTAssertTrue(web.staticTexts["Sample Brain"].waitForExistence(timeout: 10))
        XCTAssertTrue(web.buttons["Show the files"].exists)
        web.links["Morning rituals"].tap()
        XCTAssertTrue(web.staticTexts["Morning rituals"].waitForExistence(timeout: 5))
        XCTAssertTrue(web.staticTexts.containing(NSPredicate(format: "label CONTAINS %@", "ten quiet minutes")).firstMatch.exists)
        web.buttons["Show the files"].tap()
        let raw = web.buttons["raw"]
        XCTAssertTrue(raw.waitForExistence(timeout: 5)); raw.tap()
        let checklist = web.links["checklist.txt"]
        XCTAssertTrue(checklist.waitForExistence(timeout: 5)); checklist.tap()
        XCTAssertTrue(web.staticTexts.containing(NSPredicate(format: "label CONTAINS %@", "Open a window")).firstMatch.waitForExistence(timeout: 5))
        screenshot(app, "Brain web Files reader")
        XCTAssertFalse(app.buttons["Done"].exists)
    }

    func testAppSettingsForViewerKeepOwnerControlsHidden() {
        let app = launch()
        app.buttons["yourApps"].tap()
        app.buttons["appSettings-shared-reading"].tap()
        XCTAssertTrue(app.navigationBars["App Settings"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.textFields["appName"].exists)
        app.buttons["appSharing"].tap()
        XCTAssertTrue(app.navigationBars["People with Access"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["appVisibility"].exists)
        XCTAssertFalse(app.textFields["inviteEmail"].exists)
        app.buttons["BackButton"].tap()
        app.buttons["appAppearance"].tap()
        XCTAssertTrue(app.navigationBars["Icon & Preview"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["makeAppIcons"].exists)
        XCTAssertFalse(app.buttons["makeAppPreview"].exists)
    }

    func testComposerAlignmentAndGrowth() {
        let app = launch()
        let input = app.textFields["launcherInput"]
        input.tap(); input.typeText("quiet minutes")
        let hit = app.buttons["messageResult-morning:chat:2"]
        XCTAssertTrue(hit.waitForExistence(timeout: 10)); hit.tap()
        let composer = app.textViews["composer"]
        let field = composer.exists ? composer : app.textFields["composer"]
        XCTAssertTrue(field.waitForExistence(timeout: 10))
        let send = app.buttons["send"]
        let attach = app.buttons["Attach files"]
        XCTAssertFalse(send.isEnabled)
        XCTAssertLessThanOrEqual(send.frame.height, 46)
        XCTAssertEqual(send.frame.midY, attach.frame.midY, accuracy: 2)
        XCTAssertEqual(field.frame.midY, send.frame.midY, accuracy: 3)
        XCTAssertFalse(app.buttons["chatRecipients"].exists)
        let singleLineHeight = field.frame.height
        field.tap(); field.typeText("A first line\nA second line\nA third line")
        XCTAssertGreaterThan(field.frame.height, singleLineHeight)
        XCTAssertLessThanOrEqual(send.frame.height, 46)
        XCTAssertEqual(send.frame.maxY, attach.frame.maxY, accuracy: 2)
        screenshot(app, "Composer multiline alignment")
    }

    func testSettingsAndAddApp() {
        let app = launch()
        app.buttons["sidebar"].tap(); app.buttons["sidebar-settings"].tap()
        XCTAssertTrue(app.buttons["settings-Account"].waitForExistence(timeout: 10))
        app.buttons["settings-Account"].tap()
        XCTAssertTrue(app.staticTexts.containing(NSPredicate(format: "label CONTAINS %@", "preview@finite.local")).firstMatch.waitForExistence(timeout: 10))
        app.buttons["BackButton"].tap()
        app.buttons["yourApps"].tap()
        app.buttons["app-add-app"].tap()
        XCTAssertTrue(app.staticTexts["Ask for an app"].waitForExistence(timeout: 10))
        app.buttons["A grocery list for the family"].tap()
        let input = app.textFields["launcherInput"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        XCTAssertEqual(input.value as? String, "Make a shared grocery list my family can use")
    }
}
