import Foundation
import Testing
@testable import Fragment

/// A UI test's launch (`--ui-test`): a store of its own, and loopback alone.
@MainActor
struct LaunchScopeTests {
    @Test func aUITestLaunchReachesLoopbackAlone() throws {
        let scope = LaunchScope.uiTest
        for raw in ["http://127.0.0.1:8790", "http://localhost:8790", "http://moss--a1.fragment.localhost:8790/__signin", "http://[::1]:8790"] {
            #expect(scope.allows(try #require(URL(string: raw))), "\(raw)")
        }
        for raw in ["https://finite.chat", "https://moss--a1.finite.place/", "https://127.0.0.1.example.com", "https://localhost.example"] {
            #expect(!scope.allows(try #require(URL(string: raw))), "\(raw)")
        }
        #expect(throws: APIError.self) { try PlatformOrigin.parse("https://finite.chat", scope: scope) }
        #expect(try PlatformOrigin.parse("http://127.0.0.1:8790", scope: scope).absoluteString == "http://127.0.0.1:8790")
        #expect(LaunchScope.app.allows(try #require(URL(string: "https://finite.chat"))))
        #expect(LaunchScope.uiTest.keychainService != LaunchScope.app.keychainService && LaunchScope.uiTest.supportFolder != LaunchScope.app.supportFolder)
    }

    @Test func aUITestLaunchNeverLoadsAnAccountPastLoopback() throws {
        let scope = LaunchScope(keychainService: "tests-" + UUID().uuidString, supportFolder: "Finite-Tests", loopbackOnly: true)
        defer { try? DeviceKeychain.clear(service: scope.keychainService) }
        let save = { (origin: String) in
            try DeviceKeychain.save(DeviceAccount(origin: try #require(URL(string: origin)), secret: try DeviceIdentity().secret, identity: "npub1person", approved: true), service: scope.keychainService)
        }
        try save("https://finite.example")
        let refused = AccountConnection(scope: scope)
        #expect(refused.account == nil && refused.api == nil && refused.error == "This launch reaches only this machine.")
        try save("http://127.0.0.1:8790")
        #expect(AccountConnection(scope: scope).api?.baseURL.absoluteString == "http://127.0.0.1:8790")
    }
}
