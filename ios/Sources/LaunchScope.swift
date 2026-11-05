import Foundation

/// Where this launch keeps its account and drafts, and which servers it may
/// reach. A UI test's launch (`--ui-test`, Debug only) gets a store of its
/// own, a Keychain service and an Application Support folder emptied as it
/// starts, and reaches loopback alone (this machine's `cargo xtask dev`): a
/// phone's own account is never read, and no other server is ever asked.
struct LaunchScope: Sendable {
    let keychainService: String
    let supportFolder: String
    let loopbackOnly: Bool

    static let app = LaunchScope(keychainService: "build.finite.fragment.device", supportFolder: "Finite", loopbackOnly: false)
    static let uiTest = LaunchScope(keychainService: "build.finite.fragment.device.ui-test", supportFolder: "Finite-UITest", loopbackOnly: true)
    static let current: LaunchScope = {
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--ui-test") { return .uiTest }
        #endif
        return .app
    }()

    /// Whether this launch may send anything to `url`'s host.
    func allows(_ url: URL) -> Bool {
        guard loopbackOnly else { return true }
        guard let host = url.host?.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: "[]")) else { return false }
        return host == "localhost" || host == "127.0.0.1" || host == "::1" || host.hasSuffix(".localhost")
    }

    static let refused = APIError.server("This launch reaches only this machine.")

    /// Before anything reads the store: a UI test's starts empty, or holding
    /// only the account its `--ui-test-saved-account <origin>` names
    /// (approved, with a fresh key), so a test can show that one past
    /// loopback is never used.
    static func start() {
        #if DEBUG
        guard current.loopbackOnly else { return }
        try? DeviceKeychain.clear()
        if let support = try? FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: false) {
            try? FileManager.default.removeItem(at: support.appending(path: current.supportFolder, directoryHint: .isDirectory))
        }
        let args = ProcessInfo.processInfo.arguments
        if let i = args.firstIndex(of: "--ui-test-saved-account"), args.indices.contains(i + 1), let origin = URL(string: args[i + 1]),
           let key = try? DeviceIdentity() {
            try? DeviceKeychain.save(DeviceAccount(origin: origin, secret: key.secret, identity: "npub1uitest", email: "ui-test@example.com", approved: true))
        }
        #endif
    }
}
