import SwiftUI
import UserNotifications

enum SettingsSection: String, CaseIterable, Identifiable {
    case account = "Account", billing = "Billing", computer = "Computer", agents = "Agents", skills = "Skills", connections = "Connections", developer = "Build with Claude Code or Codex", data = "Your data", device = "This iPhone"
    var id: String { rawValue }
    var icon: String {
        switch self {
        case .account: "person.crop.circle"
        case .billing: "creditcard"
        case .computer: "desktopcomputer"
        case .agents: "person.2"
        case .skills: "sparkles"
        case .connections: "point.3.connected.trianglepath.dotted"
        case .developer: "terminal"
        case .data: "square.and.arrow.down"
        case .device: "iphone"
        }
    }
}

struct SettingsView: View {
    @Binding var localServer: String
    let library: Library
    let api: FragmentAPI
    var connect: () -> Void = {}
    let open: (FragmentSummary) -> Void
    var body: some View {
        List {
            Section {
                HStack(spacing: 14) {
                    Image(systemName: "circle.hexagongrid.fill").font(.title).foregroundStyle(FragmentStyle.lilac)
                    VStack(alignment: .leading, spacing: 4) { Text("Finite.Computer").font(.headline); Text("A computer of your own.").font(.subheadline).foregroundStyle(.secondary) }
                }.padding(.vertical, 8)
            }
            Section {
                NavigationLink { AppManagementList(library: library, api: api) } label: {
                    Label("Apps", systemImage: "app.badge").padding(.vertical, 4)
                }.accessibilityIdentifier("settings-Apps")
                ForEach(SettingsSection.allCases) { section in
                    NavigationLink { SettingsDetail(section: section, localServer: $localServer, library: library, api: api, open: open) } label: {
                        Label(section.rawValue, systemImage: section.icon).padding(.vertical, 4)
                    }.accessibilityIdentifier("settings-\(section.id)")
                }
            }
            Section {
                Button(api.isSigned ? "Account connection" : "Connect your account", systemImage: "person.crop.circle.badge.checkmark", action: connect).accessibilityIdentifier("accountConnection")
                NavigationLink("Open-source notices") {
                    ScrollView {
                        Text(thirdPartyNotices).font(.footnote).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading).padding()
                    }.navigationTitle("Open-source notices")
                }
                LabeledContent("Version", value: "0.3")
            } footer: { Text(api.isSigned ? "Connected to " + api.baseURL.absoluteString : "Local preview with fixture replies.") }
        }.scrollContentBackground(.hidden).background(FragmentStyle.background)
    }
    private var thirdPartyNotices: String {
        guard let url = Bundle.main.url(forResource: "ThirdPartyNotices", withExtension: "txt"),
              let text = try? String(contentsOf: url, encoding: .utf8) else { return "Notices could not be loaded." }
        return text
    }

}

private struct SettingsDetail: View {
    let section: SettingsSection
    @Binding var localServer: String
    let library: Library
    let api: FragmentAPI
    let open: (FragmentSummary) -> Void
    @State private var value: JSONValue = .null
    @State private var loading = false
    @State private var error: String?
    @State private var server = ""
    @State private var renameAgent: FragmentSummary?
    @State private var agentName = ""
    @State private var confirmRestart = false
    @State private var notificationStatus = "Not enabled"
    var body: some View {
        Form {
            if loading { ProgressView("Loading…") }
            if let error { Section { Text(error).foregroundStyle(.secondary); Button("Try again") { Task { await load() } } } }
            fields
        }
        .navigationTitle(section == .developer ? "Build with an agent" : section.rawValue).navigationBarTitleDisplayMode(.inline)
        .task { server = localServer; await load() }
        .refreshable { await load() }
        .alert("Name your agent", isPresented: Binding(get: { renameAgent != nil }, set: { if !$0 { renameAgent = nil } })) {
            TextField("Agent name", text: $agentName)
            Button("Save") { if let agent = renameAgent { Task { await rename(agent) } } }
            Button("Cancel", role: .cancel) { renameAgent = nil }
        }
        .confirmationDialog("Restart your computer?", isPresented: $confirmRestart, titleVisibility: .visible) {
            Button("Restart computer", role: .destructive) { Task { await restart() } }
        } message: { Text("Saves your computer if it can, then starts it fresh. Anything its agents are doing stops.") }
    }
    @ViewBuilder private var fields: some View {
        switch section {
        case .account:
            Section("Account") {
                LabeledContent("Email", value: value["email"].string ?? "Not connected")
                LabeledContent("Identity", value: value["id"].string ?? "—").font(.caption).textSelection(.enabled)
            }
            Section {
                Text(api.isSigned ? "This iPhone uses its own approved device key. Manage or disconnect it from Account connection in Settings." : "Connect your account from the main Settings screen.").foregroundStyle(.secondary)
            }
            Section { Link("Privacy", destination: URL(string: "https://finite.chat/privacy")!); Link("Terms", destination: URL(string: "https://finite.chat/terms")!) }
        case .billing:
            Section("Your seat") {
                LabeledContent("Plan", value: value["seat"]["kind"].string ?? "No seat")
                LabeledContent("Organization", value: value["seat"]["org"]["name"].string ?? "—")
                Text("Manage purchases and subscriptions on your Finite website.").font(.footnote).foregroundStyle(.secondary)
            }
        case .computer:
            if let computer = computer {
                Section("Your computer") {
                    LabeledContent("State", value: computer["state"].string?.capitalized ?? "—")
                    LabeledContent("Version", value: computer["version"].string ?? "—")
                    Button("Restart computer", systemImage: "arrow.clockwise") { confirmRestart = true }
                }
            } else if !loading { ContentUnavailableView("No computer", systemImage: "desktopcomputer", description: Text("Your computer will appear here when connected.")) }
        case .agents:
            ForEach(library.agents) { agent in
                Section {
                    HStack { AgentAvatar(agent: agent); Text(agent.displayTitle).font(.headline) }
                    Button("Rename", systemImage: "pencil") { agentName = agent.displayTitle; renameAgent = agent }
                    Button("Start a new chat", systemImage: "square.and.pencil") { library.selectedAgent = agent.name; openLauncher() }
                    LabeledContent("Chats", value: String(library.chats.filter { ($0.agents ?? []).contains(library.identities[agent.name] ?? "") }.count))
                }
            }
        case .skills:
            Section {
                Text("What your agents know how to do, kept in one place.")
                if let skills = library.fragments.first(where: { $0.kind == "skills" || $0.name == "skills" }) { Button("Open skills", systemImage: "sparkles") { open(skills) } }
                else { Text("No skills library is connected.").foregroundStyle(.secondary) }
            }
        case .connections:
            ForEach(Array(value["providers"].array.enumerated()), id: \.offset) { _, provider in
                Section(provider["provider"].string ?? "Connection") {
                    LabeledContent("Status", value: (provider["state"].string ?? "Not connected").replacingOccurrences(of: "_", with: " ").capitalized)
                    if let account = provider["account"].string { LabeledContent("Account", value: account) }
                    if let note = provider["note"].string { Text(note).font(.footnote).foregroundStyle(.secondary) }
                }
            }
            Section { Text("Connect and authorize providers on the web. Credentials stay on your computer’s connection service.").font(.footnote).foregroundStyle(.secondary) }
        case .developer:
            Section("Build with Claude Code or Codex") {
                Text("Paste this into your coding agent and finish the sentence.")
                Text(prompt).font(.callout.monospaced()).textSelection(.enabled)
                ShareLink(item: prompt) { Label("Share prompt", systemImage: "square.and.arrow.up") }
                Button("Copy prompt", systemImage: "doc.on.doc") { UIPasteboard.general.string = prompt }
            }
        case .data:
            Section("Export") {
                Text("Download your fragments, messages, app data, and your computer’s newest save.")
                LabeledContent("Status", value: value["export"]["state"].string ?? "No export yet")
                Button("Export everything", systemImage: "square.and.arrow.down") { Task { await export() } }.disabled(loading)
                if let raw = value["export"]["url"].string, let url = URL(string: raw), ["http", "https"].contains(url.scheme) { ShareLink(item: url) { Text("Share export") } }
            }
        case .device:
            Section("Notifications") {
                LabeledContent("Reply notifications", value: notificationStatus)
                Button("Enable notifications", systemImage: "bell") { Task { await notifications() } }
                Text("Notification delivery while the app is closed is not connected yet.").font(.footnote).foregroundStyle(.secondary)
            }
            #if DEBUG
            Section("Local development") {
                TextField("Server URL", text: $server).keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled().accessibilityIdentifier("serverURL")
                Button("Connect") {
                    let clean = server.trimmingCharacters(in: .whitespacesAndNewlines)
                    guard let url = URL(string: clean), ["http", "https"].contains(url.scheme), url.host != nil, url.user == nil, url.password == nil, url.query == nil, url.fragment == nil else { error = "Enter an HTTP or HTTPS server URL."; return }
                    localServer = clean
                }
                Text("Fixture replies only. No real agents or paid model calls.").font(.footnote).foregroundStyle(.secondary)
            }
            #endif
        }
    }
    private var computer: JSONValue? { value["computer"].string == nil ? value["computers"].array.first : value }
    private var prompt: String { "Read \(api.url("llms.txt").absoluteString) and help me build: " }
    private func openLauncher() { open(FragmentSummary(name: "__launcher", kind: "launcher")) }
    private func load() async {
        loading = true; error = nil; defer { loading = false }
        let path: String
        switch section {
        case .account: path = "api/identities/me"
        case .billing: path = "api/seat"
        case .computer: path = "api/computers"
        case .connections: path = "api/connections"
        case .data: path = "api/export"
        default: return
        }
        do { value = try await api.request(path) } catch { self.error = error.localizedDescription }
    }
    private func rename(_ agent: FragmentSummary) async {
        do { try await api.rename(agent, title: agentName); await library.load(api); renameAgent = nil }
        catch { self.error = error.localizedDescription }
    }
    private func restart() async {
        guard let computer, let id = computer["computer"].string else { return }
        do { value = try await api.request("api/computers/\(id)/restart", method: "POST", body: .object(["generation": computer["generation"]])) }
        catch { self.error = error.localizedDescription }
    }
    private func export() async {
        loading = true; defer { loading = false }
        do { let result: JSONValue = try await api.request("api/export", method: "POST", body: .object([:])); value = .object(["export": result]) }
        catch { self.error = error.localizedDescription }
    }
    private func notifications() async {
        do { let granted = try await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]); notificationStatus = granted ? "Allowed" : "Disabled in iOS Settings" }
        catch { self.error = error.localizedDescription }
    }
}
