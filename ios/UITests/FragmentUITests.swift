import CryptoKit
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

    /// Paul's sequence: from Your Apps, open a sheet (here Add an app, which
    /// needs no server) and close it, then Back to Home. The keyboard waits
    /// for Home itself: it never rises over Your Apps as the sheet goes, and
    /// back on Home the composer rests above it.
    func testHomesComposerStaysAboveTheKeyboardAfterASheetFromYourApps() {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-test", "--ui-test-saved-account", "http://127.0.0.1:9"]
        app.launch()
        let composer = app.textViews["composer"], keyboard = app.keyboards.firstMatch
        XCTAssertTrue(composer.waitForExistence(timeout: 10), "Home, its saved account approved")
        XCTAssertTrue(keyboard.waitForExistence(timeout: 10), "Home opens with the keyboard")

        app.buttons["yourApps"].tap()
        XCTAssertTrue(app.navigationBars["Your Apps"].waitForExistence(timeout: 5))
        XCTAssertTrue(keyboard.waitForNonExistence(timeout: 5), "Your Apps has no keyboard")
        app.buttons["app-add-app"].tap()
        let sheet = app.navigationBars["Add an app"]
        XCTAssertTrue(sheet.waitForExistence(timeout: 5))
        sheet.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.3))
            .press(forDuration: 0.1, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.98)))
        XCTAssertTrue(sheet.waitForNonExistence(timeout: 5), "the sheet closed")
        XCTAssertTrue(app.navigationBars["Your Apps"].exists)
        // the keyboard is Home's alone: none rises over Your Apps
        XCTAssertFalse(keyboard.waitForExistence(timeout: 2), "no keyboard over Your Apps once the sheet closes")
        screenshot(app, "Your Apps after a sheet")

        app.navigationBars["Your Apps"].buttons.element(boundBy: 0).tap()
        XCTAssertTrue(keyboard.waitForExistence(timeout: 10), "Home takes the keyboard back")
        aboveTheKeyboard(composer, keyboard, "after a sheet from Your Apps, and Back")
        screenshot(app, "Home after a sheet from Your Apps")
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
}

/// Flows against a real platform: this machine's `cargo xtask dev`, named by
/// `FRAGMENT_DEV_HOST` (`TEST_RUNNER_FRAGMENT_DEV_HOST=http://127.0.0.1:8790`
/// for `xcodebuild test`), its WorkOS fake beside it. Without one they skip.
/// The test signs a person in through the fake, makes their apps with that
/// browser session, as the web shell would, and approves the app's pairing
/// code at `/cli`, as the person would.
@MainActor
final class DevStackUITests: XCTestCase {
    private func screenshot(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot()); shot.name = name; shot.lifetime = .keepAlways; add(shot)
    }

    /// An app's page keeps what it saved in the browser (localStorage) from
    /// one opening of it to the next, and across a relaunch, as it does in
    /// Safari, and a server-backed app's writes stick as well. A new key's
    /// approval, or disconnecting this iPhone, removes what pages kept.
    func testAnAppsPageKeepsItsStateAcrossSheetsUntilDisconnect() async throws {
        let platform = try await DevPlatform.start()
        let notes = try await platform.app(template: "blank", title: "Notes kept here", page: Self.notesPage)
        let todo = try await platform.app(template: "todo", title: "Sheet todo", page: nil)
        let app = XCUIApplication()
        app.launchArguments = ["--ui-test", "--pair-server", platform.base.absoluteString]
        app.launch()
        try await pair(app, platform)

        // a page that keeps its state in localStorage
        let note = "milk \(UUID().uuidString.prefix(6))"
        open(app, notes)
        let field = app.webViews.textFields["note"]
        XCTAssertTrue(field.waitForExistence(timeout: 20), "the notes page loads")
        field.tap(); field.typeText(note)
        app.webViews.buttons["Save"].tap()
        XCTAssertTrue(app.webViews.staticTexts[note].waitForExistence(timeout: 5), "the page saved the note")
        screenshot(app, "Notes saved")
        close(app, "Notes kept here", notes)
        open(app, notes)
        XCTAssertTrue(app.webViews.staticTexts["1 saved"].waitForExistence(timeout: 20), "reopened, the page still has its note")
        XCTAssertTrue(app.webViews.staticTexts[note].exists)
        screenshot(app, "Notes reopened")
        close(app, "Notes kept here", notes)

        // a page whose writes are the server's (the todo template's operations):
        // a todo the person added elsewhere, ticked on the page, still ticked
        let task = "eggs \(UUID().uuidString.prefix(6))"
        try await platform.call(todo, "add", ["text": task])
        open(app, todo)
        let box = app.webViews.checkBoxes.firstMatch.exists ? app.webViews.checkBoxes.firstMatch : app.webViews.switches.firstMatch
        XCTAssertTrue(app.webViews.staticTexts[task].waitForExistence(timeout: 20), "the todo page loads, its todo there")
        box.tap()
        XCTAssertTrue(app.webViews.staticTexts["finished \u{201C}\(task)\u{201D}"].waitForExistence(timeout: 10), "the server took the tick")
        close(app, "Sheet todo", todo)
        open(app, todo)
        XCTAssertTrue(app.webViews.staticTexts[task].waitForExistence(timeout: 20))
        XCTAssertTrue(app.webViews.staticTexts["finished \u{201C}\(task)\u{201D}"].exists, "reopened, the tick is there")
        screenshot(app, "Todo reopened")
        close(app, "Sheet todo", todo)

        // relaunched, its account kept as an app's relaunch keeps it
        app.terminate()
        app.launchArguments.append("--ui-test-keep")
        app.launch()
        XCTAssertTrue(app.buttons["yourApps"].waitForExistence(timeout: 20), "relaunched, Home")
        open(app, notes)
        XCTAssertTrue(app.webViews.staticTexts["1 saved"].waitForExistence(timeout: 20), "relaunched, the page still has its note")
        XCTAssertTrue(app.webViews.staticTexts[note].exists)
        screenshot(app, "Notes after relaunch")
        close(app, "Notes kept here", notes)

        // a new key without a Disconnect (as after a key revoked on the web):
        // its approval clears what pages kept, whoever it names
        app.terminate()
        app.launchArguments.removeAll { $0 == "--ui-test-keep" }
        app.launch()
        try await pair(app, platform)
        open(app, notes)
        XCTAssertTrue(app.webViews.staticTexts["0 saved"].waitForExistence(timeout: 20), "a new key's pages start with nothing kept")
        XCTAssertFalse(app.webViews.staticTexts[note].exists)
        let again = app.webViews.textFields["note"]
        again.tap(); again.typeText("bread")
        app.webViews.buttons["Save"].tap()
        XCTAssertTrue(app.webViews.staticTexts["1 saved"].waitForExistence(timeout: 5))
        close(app, "Notes kept here", notes)

        // disconnect, then pair again as the same person: nothing kept
        app.navigationBars["Your Apps"].buttons.element(boundBy: 0).tap()
        XCTAssertTrue(app.buttons["sidebar"].waitForExistence(timeout: 5)); app.buttons["sidebar"].tap()
        let settings = app.buttons["sidebar-settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 5)); settings.tap()
        XCTAssertTrue(app.navigationBars["Settings"].waitForExistence(timeout: 5))
        let connection = app.buttons["accountConnection"]
        for _ in 0..<4 where !connection.isHittable { app.swipeUp() }
        XCTAssertTrue(connection.isHittable, "Account connection, at the end of Settings"); connection.tap()
        let disconnect = app.buttons["disconnectAccount"]
        XCTAssertTrue(disconnect.waitForExistence(timeout: 5)); disconnect.tap()
        try await pair(app, platform)
        open(app, notes)
        XCTAssertTrue(app.webViews.staticTexts["0 saved"].waitForExistence(timeout: 20), "after a disconnect, the page kept nothing")
        XCTAssertFalse(app.webViews.staticTexts["bread"].exists)
        screenshot(app, "Notes after disconnect")
    }

    /// Connect, its code approved at `/cli` by the person, then Home.
    private func pair(_ app: XCUIApplication, _ platform: DevPlatform) async throws {
        let connect = app.buttons["pairAccount"].firstMatch
        XCTAssertTrue(connect.waitForExistence(timeout: 10))
        // after a disconnect from Settings, its sheet and Home both show Connect,
        // one code; a tap as the sheet goes away is tapped again
        let code = app.staticTexts["pairingCode"].firstMatch
        for _ in 0..<3 where !code.exists { connect.tap(); _ = code.waitForExistence(timeout: 8) }
        XCTAssertTrue(code.exists, "a pairing code")
        try await platform.approve(code: code.label)
        XCTAssertTrue(app.textViews["composer"].waitForExistence(timeout: 20), "approved, Home")
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 10), "Home settles, its keyboard up")
    }

    private func open(_ app: XCUIApplication, _ name: String) {
        if !app.navigationBars["Your Apps"].exists {
            app.buttons["yourApps"].tap()
            XCTAssertTrue(app.navigationBars["Your Apps"].waitForExistence(timeout: 10), "Your Apps")
        }
        let row = app.buttons["app-\(name)"]
        XCTAssertTrue(row.waitForExistence(timeout: 20), "\(name) is in Your Apps"); row.tap()
    }

    /// Swipes its sheet away, back to Your Apps: dragged by its title bar
    /// down off the screen, as a person dismisses it.
    private func close(_ app: XCUIApplication, _ title: String, _ name: String) {
        let bar = app.navigationBars[title]
        XCTAssertTrue(bar.waitForExistence(timeout: 5))
        bar.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.3))
            .press(forDuration: 0.1, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.98)))
        XCTAssertTrue(app.buttons["app-\(name)"].waitForExistence(timeout: 10), "it closed")
        XCTAssertTrue(app.webViews.firstMatch.waitForNonExistence(timeout: 10))
    }

    /// A page that keeps its notes in localStorage, as many agents' apps do.
    static let notesPage = """
    <!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Notes kept here</title>
    <style>body{font:17px -apple-system,system-ui,sans-serif;margin:24px}input,button{font:inherit;padding:8px}</style></head><body>
    <form id="f"><input id="t" aria-label="note" placeholder="A note"><button>Save</button></form>
    <p id="count">0 saved</p><ul id="list"></ul>
    <script>
    const load = () => JSON.parse(localStorage.getItem("notes") || "[]");
    function draw() {
      document.getElementById("count").textContent = load().length + " saved";
      document.getElementById("list").replaceChildren(...load().map(n => Object.assign(document.createElement("li"), { textContent: n })));
    }
    document.getElementById("f").onsubmit = (e) => {
      e.preventDefault();
      const t = document.getElementById("t").value.trim();
      if (t) { localStorage.setItem("notes", JSON.stringify([...load(), t])); document.getElementById("t").value = ""; draw(); }
    };
    draw();
    </script></body></html>
    """
}

/// The dev stack, as a person's browser reaches it: signed in through its
/// WorkOS fake (`/auth/login`), the shell's calls on that session, and
/// `/cli`'s forms (docs/ios.md, Connect an account).
private struct DevPlatform {
    let base: URL
    let email: String
    let session: String
    private static let http: URLSession = {
        let c = URLSessionConfiguration.ephemeral
        c.httpShouldSetCookies = false; c.httpCookieAcceptPolicy = .never
        return URLSession(configuration: c, delegate: NoRedirects(), delegateQueue: nil)
    }()

    /// The dev stack `FRAGMENT_DEV_HOST` names, a new person signed in
    /// there: nothing anyone kept before is theirs.
    static func start() async throws -> DevPlatform {
        guard let host = ProcessInfo.processInfo.environment["FRAGMENT_DEV_HOST"], let base = URL(string: host) else {
            throw XCTSkip("no dev stack named (FRAGMENT_DEV_HOST)")
        }
        let email = "ios-sheet-\(UUID().uuidString.prefix(8).lowercased())@dev.test"
        return DevPlatform(base: base, email: email, session: try await signIn(base: base, email: email))
    }

    private static func signIn(base: URL, email: String) async throws -> String {
        var login = URLComponents(url: base.appending(path: "auth/login"), resolvingAgainstBaseURL: false)!
        login.queryItems = [URLQueryItem(name: "return", value: "/"), URLQueryItem(name: "login_hint", value: email)]
        let (_, start) = try await get(login.url!)
        let bound = try cookie("fragment_login", start)
        let (_, fake) = try await get(URL(string: try header("location", start))!)
        let (_, done) = try await get(URL(string: try header("location", fake))!, cookie: bound)
        return String(try cookie("fragment_session", done).dropFirst("fragment_session=".count))
    }

    /// A fragment of theirs from `template`, titled, its page `page` if
    /// given, deployed: its name.
    func app(template: String, title: String, page: String?) async throws -> String {
        let made = try await shell("api/fragments", ["label": "sheet", "template": template, "title": title, "visibility": "members"])
        guard let name = made["name"] as? String else { throw URLError(.badServerResponse) }
        if let page {
            _ = try await shell("api/f/\(name)/files", ["files": [["path": "site/index.html", "text": page]]])
            _ = try await shell("api/f/\(name)/deploy", [:])
        }
        return name
    }

    /// One of `name`'s operations, called as the person (`POST /api/f/{name}/ops/{op}`).
    func call(_ name: String, _ op: String, _ input: [String: String]) async throws {
        _ = try await shell("api/f/\(name)/ops/\(op)", ["id": UUID().uuidString, "input": input])
    }

    /// Types `code` at `/cli` and approves the key it names.
    func approve(code: String) async throws {
        let kept = code.replacingOccurrences(of: "-", with: "").uppercased()
        try await form("cli", ["form": token("cli"), "code": code])
        try await form("cli/approve", ["form": token("cli-approve:\(kept)"), "code": kept])
    }

    /// A form token as `/cli`'s page holds one (`fragment_core::form`),
    /// dated past its armed delay.
    private func token(_ purpose: String) -> String {
        let at = Int64(Date().timeIntervalSince1970 * 1000) - 5000
        let mac = HMAC<SHA256>.authenticationCode(for: Data("fragment-form/1\n\(purpose.utf8.count)\n\(purpose)\n\(at)".utf8), using: SymmetricKey(data: Data(session.utf8)))
        return "\(at)." + mac.map { String(format: "%02x", $0) }.joined()
    }

    private func form(_ path: String, _ fields: [String: String]) async throws {
        var body = URLComponents(); body.queryItems = fields.map { URLQueryItem(name: $0.key, value: $0.value) }
        var request = URLRequest(url: base.appending(path: path))
        request.httpMethod = "POST"
        request.httpBody = Data((body.percentEncodedQuery ?? "").replacingOccurrences(of: "+", with: "%2B").utf8)
        request.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "content-type")
        request.setValue("fragment_session=\(session)", forHTTPHeaderField: "cookie")
        request.setValue(Self.origin(base), forHTTPHeaderField: "origin")
        let (_, response) = try await Self.http.http(request)
        guard response.statusCode == 200 else { throw URLError(.userAuthenticationRequired, userInfo: [NSLocalizedDescriptionKey: "POST /\(path): \(response.statusCode)"]) }
    }

    /// One of the shell's own calls on the person's session.
    private func shell(_ path: String, _ body: [String: Any]) async throws -> [String: Any] {
        var request = URLRequest(url: base.appending(path: path))
        request.httpMethod = "POST"
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        for (name, value) in ["content-type": "application/json", "cookie": "fragment_session=\(session)", "x-fragment-shell": "1",
                              "sec-fetch-site": "same-origin", "origin": Self.origin(base)] { request.setValue(value, forHTTPHeaderField: name) }
        let (data, response) = try await Self.http.http(request)
        guard (200..<300).contains(response.statusCode) else {
            throw URLError(.badServerResponse, userInfo: [NSLocalizedDescriptionKey: "POST /\(path): \(response.statusCode) \(String(decoding: data, as: UTF8.self))"])
        }
        return (try JSONSerialization.jsonObject(with: data) as? [String: Any]) ?? [:]
    }

    private static func origin(_ url: URL) -> String { "\(url.scheme!)://\(url.host!)" + (url.port.map { ":\($0)" } ?? "") }

    private static func get(_ url: URL, cookie: String? = nil) async throws -> (Data, HTTPURLResponse) {
        var request = URLRequest(url: url)
        if let cookie { request.setValue(cookie, forHTTPHeaderField: "cookie") }
        let (data, response) = try await http.http(request)
        return (data, response)
    }

    private static func header(_ name: String, _ response: HTTPURLResponse) throws -> String {
        guard let value = response.value(forHTTPHeaderField: name) else { throw URLError(.badServerResponse, userInfo: [NSLocalizedDescriptionKey: "no \(name) (\(response.statusCode))"]) }
        return value
    }

    /// `name=value` of a cookie `response` sets.
    private static func cookie(_ name: String, _ response: HTTPURLResponse) throws -> String {
        let all = HTTPCookie.cookies(withResponseHeaderFields: response.allHeaderFields as? [String: String] ?? [:], for: response.url!)
        guard let found = all.first(where: { $0.name == name }) else { throw URLError(.badServerResponse, userInfo: [NSLocalizedDescriptionKey: "no \(name) cookie (\(response.statusCode))"]) }
        return "\(name)=\(found.value)"
    }
}

private final class NoRedirects: NSObject, URLSessionTaskDelegate, Sendable {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest) async -> URLRequest? { nil }
}

private extension URLSession {
    func http(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        let (data, response) = try await data(for: request, delegate: nil)
        guard let http = response as? HTTPURLResponse else { throw URLError(.badServerResponse) }
        return (data, http)
    }
}
