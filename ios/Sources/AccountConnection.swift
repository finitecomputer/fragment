import SwiftUI
import WebKit

@MainActor @Observable
final class AccountConnection {
    struct Code: Decodable { let code: String; let expiresAt: Int64 }
    var account: DeviceAccount?
    var code: Code?
    var error: String?
    var busy = false
    var preview = false
    @ObservationIgnored private var cachedAPI: FragmentAPI?
    var api: FragmentAPI? {
        guard let account, account.approved, let key = try? account.signingKey else { return nil }
        if let cachedAPI, cachedAPI.baseURL == account.origin, cachedAPI.identity?.publicKey == key.publicKey, cachedAPI.accountID == account.identity { return cachedAPI }
        let client = FragmentAPI(baseURL: account.origin, identity: key, accountID: account.identity)
        cachedAPI = client
        return client
    }
    init() {
        do {
            if let stored = try DeviceKeychain.read() {
                guard try PlatformOrigin.parse(stored.origin.absoluteString) == stored.origin else { throw APIError.invalidResponse }
                _ = try stored.signingKey
                account = stored
            }
        }
        catch { self.error = error.localizedDescription }
    }
    func begin(_ raw: String, newKey: Bool = false) async {
        busy = true; error = nil; code = nil; defer { busy = false }
        do {
            let origin = try PlatformOrigin.parse(raw)
            if let existing = account, existing.origin != origin && existing.approved {
                throw APIError.server("Disconnect this iPhone before pairing a different account.")
            }
            if newKey || account == nil || account?.origin != origin {
                let key = try DeviceIdentity()
                let next = DeviceAccount(origin: origin, secret: key.secret, approved: false)
                try DeviceKeychain.save(next); account = next
            }
            guard let account else { throw APIError.invalidResponse }
            let api = try FragmentAPI(baseURL: origin, identity: account.signingKey)
            if try await finish(api) { return }
            code = try await api.request("cli/begin", method: "POST", body: .object([:]))
        } catch { self.error = error.localizedDescription }
    }
    func poll() async {
        guard let code, let account else { return }
        do {
            let api = try FragmentAPI(baseURL: account.origin, identity: account.signingKey)
            // The same interval and ten-minute bound as fragment login.
            for _ in 0..<300 {
                try Task.checkCancellation()
                guard Int64(Date().timeIntervalSince1970 * 1000) < code.expiresAt else { error = "This code expired. Start again for a new code."; return }
                if try await finish(api) { self.code = nil; return }
                try await Task.sleep(for: .seconds(2))
            }
            error = "This code expired. Start again for a new code."
        } catch { if !Task.isCancelled { self.error = error.localizedDescription } }
    }
    private func finish(_ api: FragmentAPI) async throws -> Bool {
        do {
            let person: JSONValue = try await api.request("api/identities/me")
            try Task.checkCancellation()
            guard var next = account, next.origin == api.baseURL,
                  try next.signingKey.publicKey == api.identity?.publicKey else { throw CancellationError() }
            guard person["kind"].string == "person", let identity = person["id"].string else { throw APIError.invalidResponse }
            next.identity = identity; next.email = person["email"].string; next.approved = true
            try DeviceKeychain.save(next); account = next; preview = false; error = nil
            return true
        } catch APIError.unauthorized { return false }
    }
    func validate() async {
        guard let api else { return }
        do {
            if try await finish(api) == false, account?.origin == api.baseURL,
               (try? account?.signingKey.publicKey) == api.identity?.publicKey { expire() }
        } catch is CancellationError { return }
        catch { self.error = error.localizedDescription }
    }
    func expire() {
        guard var next = account, next.approved else { return }
        next.approved = false
        do { try DeviceKeychain.save(next); account = next; error = "This iPhone’s authorization ended. Pair again to reconnect." }
        catch { self.error = error.localizedDescription; account = nil }
    }
    func disconnect() async {
        busy = true; error = nil; defer { busy = false }
        do {
            let client = api
            if let api = client, let account {
                let _: JSONValue = try await api.request("api/identities/me/keys/\(try account.signingKey.publicKey)", method: "DELETE")
            }
            try await removeLocalAccount()
        } catch { self.error = error.localizedDescription }
    }
    func forget() async {
        busy = true; error = nil; defer { busy = false }
        do { try await removeLocalAccount() }
        catch { self.error = error.localizedDescription }
    }
    private func removeLocalAccount() async throws {
        if let account {
            let client = try FragmentAPI(baseURL: account.origin, identity: account.signingKey, accountID: account.identity)
            try ChatDraftStore.removeAccount(api: client)
        }
        try DeviceKeychain.clear(); account = nil; code = nil; cachedAPI = nil
        await WKWebsiteDataStore.default().removeData(ofTypes: WKWebsiteDataStore.allWebsiteDataTypes(), modifiedSince: .distantPast)
    }
}

struct AccountConnectionView: View {
    let connection: AccountConnection
    var close: () -> Void = {}
    @State private var server = "https://finite.chat"
    @State private var replaceKey = false
    @State private var forgetConnection = false
    @Environment(\.openURL) private var openURL
    @Environment(\.scenePhase) private var scenePhase
    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Text("Finite.Computer").font(.title2.weight(.semibold))
                    Text("Connect this iPhone to your account.").foregroundStyle(.secondary)
                }
                if let account = connection.account, account.approved {
                    Section("Connected") {
                        Text(account.email ?? "Your account")
                        Text(account.origin.absoluteString).font(.footnote).foregroundStyle(.secondary)
                        Button("Disconnect this iPhone", role: .destructive) { Task { await connection.disconnect() } }
                            .accessibilityIdentifier("disconnectAccount")
                    }
                } else {
                    Section("Your Finite server") {
                        TextField("https://finite.chat", text: $server).textContentType(.URL).keyboardType(.URL)
                            .textInputAutocapitalization(.never).autocorrectionDisabled().accessibilityIdentifier("accountServer")
                        Button(connection.code == nil ? "Connect" : "Get a new code") { Task { await connection.begin(server) } }
                            .accessibilityIdentifier("pairAccount")
                    }
                    if let code = connection.code, let origin = connection.account?.origin {
                        Section("Approve this iPhone") {
                            Text(code.code).font(.system(.largeTitle, design: .monospaced).weight(.semibold)).textSelection(.enabled).accessibilityIdentifier("pairingCode")
                            Button("Copy code", systemImage: "doc.on.doc") { UIPasteboard.general.string = code.code }
                            Button("Open approval page", systemImage: "safari") { openURL(origin.appending(path: "cli")) }
                            Text("Enter this code on that page, check the Finite-iOS request, and approve. You can also use a browser on another device. Return here to finish connecting.").font(.footnote).foregroundStyle(.secondary)
                            ProgressView("Waiting for approval…")
                        }
                    }
                    if connection.account != nil && connection.code == nil {
                        Section { Button("Create a new device key") { replaceKey = true } }
                    }
                    #if DEBUG
                    Section {
                        Button("Try the local preview") { connection.preview = true; close() }.accessibilityIdentifier("tryPreview")
                    }
                    #endif
                }
                if connection.account != nil {
                    Section {
                        Button("Remove from this iPhone", role: .destructive) { forgetConnection = true }
                            .accessibilityIdentifier("forgetAccount")
                    } footer: {
                        Text("Use this if the server is unreachable. This removes the saved key and drafts from this iPhone; revoke its approved key from the website when you can.")
                    }
                }
                if connection.busy { ProgressView() }
                if let error = connection.error { Section { Text(error).foregroundStyle(.secondary) } }
            }
            .navigationTitle("Connect Finite").navigationBarTitleDisplayMode(.inline)
            .disabled(connection.busy)
            .task { server = connection.account?.origin.absoluteString ?? initialServer }
            .task(id: "\(connection.code?.code ?? "")|\(scenePhase)") { if connection.code != nil && scenePhase == .active { await connection.poll() } }
            .onChange(of: connection.account?.approved) { _, approved in if approved == true { close() } }
            .confirmationDialog("Remove this connection from this iPhone?", isPresented: $forgetConnection, titleVisibility: .visible) {
                Button("Remove saved connection", role: .destructive) { Task { await connection.forget() } }
            } message: { Text("The key may still be approved on the server. This removes only the key and saved drafts on this iPhone.") }
            .confirmationDialog("Create a new device key?", isPresented: $replaceKey, titleVisibility: .visible) {
                Button("Create new key") { Task { await connection.begin(server, newKey: true) } }
            } message: { Text("Use this if the previous key was revoked. Other devices keep their own keys.") }
        }
    }
    private var initialServer: String {
        #if DEBUG
        let args = ProcessInfo.processInfo.arguments
        if let i = args.firstIndex(of: "--pair-server"), args.indices.contains(i + 1) { return args[i + 1] }
        #endif
        return "https://finite.chat"
    }
}
