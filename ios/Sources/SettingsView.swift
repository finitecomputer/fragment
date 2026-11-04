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
                    NavigationLink { SettingsDetail(section: section, library: library, api: api, open: open) } label: {
                        Label(section.rawValue, systemImage: section.icon).padding(.vertical, 4)
                    }.accessibilityIdentifier("settings-\(section.id)")
                }
            }
            Section {
                Button("Account connection", systemImage: "person.crop.circle.badge.checkmark", action: connect).accessibilityIdentifier("accountConnection")
                NavigationLink("Open-source notices") {
                    ScrollView {
                        Text(thirdPartyNotices).font(.footnote).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading).padding()
                    }.navigationTitle("Open-source notices")
                }
                LabeledContent("Version", value: "0.3")
            } footer: { Text("Connected to " + api.baseURL.absoluteString) }
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
    let library: Library
    let api: FragmentAPI
    let open: (FragmentSummary) -> Void
    @State private var value: JSONValue = .null
    @State private var loading = false
    @State private var error: String?
    @State private var confirmRestart = false
    @State private var notificationStatus = "Not enabled"
    @State private var exports: ExportModel
    init(section: SettingsSection, library: Library, api: FragmentAPI, open: @escaping (FragmentSummary) -> Void) {
        self.section = section; self.library = library; self.api = api; self.open = open
        _exports = State(initialValue: ExportModel(api: api))
    }
    var body: some View {
        Form {
            if loading { ProgressView("Loading…") }
            if let error { Section { Text(error).foregroundStyle(.secondary); Button("Try again") { Task { await load() } } } }
            fields
        }
        .navigationTitle(section == .developer ? "Build with an agent" : section.rawValue).navigationBarTitleDisplayMode(.inline)
        .task { await load() }
        .refreshable { await load() }
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
                Text("This iPhone uses its own approved device key. Manage or disconnect it from Account connection in Settings.").foregroundStyle(.secondary)
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
                    LabeledContent("State", value: ComputerPhase.said(computer["phase"].string))
                    LabeledContent("Version", value: computer["version"].string ?? "—")
                    Button("Restart computer", systemImage: "arrow.clockwise") { confirmRestart = true }
                }
            } else if !loading { ContentUnavailableView("No computer", systemImage: "desktopcomputer", description: Text("Your computer will appear here when connected.")) }
        case .agents:
            ForEach(library.agents) { agent in
                Section {
                    HStack { AgentAvatar(agent: agent); Text(agent.displayTitle).font(.headline) }
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
            ExportSection(model: exports)
        case .device:
            Section("Notifications") {
                LabeledContent("Reply notifications", value: notificationStatus)
                Button("Enable notifications", systemImage: "bell") { Task { await notifications() } }
                Text("Notification delivery while the app is closed is not connected yet.").font(.footnote).foregroundStyle(.secondary)
            }
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
        // the Form's pull to refresh asks the export again too
        case .data: return await exports.refresh()
        default: return
        }
        do { value = try await api.request(path) } catch { self.error = error.localizedDescription }
    }
    private func restart() async {
        guard let computer, let id = computer["computer"].string else { return }
        do { value = try await api.request("api/computers/\(id)/restart", method: "POST", body: .object(["generation": computer["generation"]])) }
        catch { self.error = error.localizedDescription }
    }
    private func notifications() async {
        do { let granted = try await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]); notificationStatus = granted ? "Allowed" : "Disabled in iOS Settings" }
        catch { self.error = error.localizedDescription }
    }
}

/// The person's export (docs/api.md, Export) as the web's Settings shows it
/// (cell/shell/exports.js): made on the platform, followed while it builds,
/// downloaded with this iPhone's own signature into a file of its own and
/// shared from there, and made again at any time.
private struct ExportSection: View {
    let model: ExportModel
    var body: some View {
        Section("Export") {
            Text("Download your fragments, messages, app data, and your computer’s newest save.")
            LabeledContent("Status", value: model.export?.said ?? "No export yet")
            if let export = model.export, export.state == "ready", export.download != nil {
                if let file = model.file { ShareLink(item: file) { Label("Share export", systemImage: "square.and.arrow.up") } }
                else { Button("Download export", systemImage: "arrow.down.circle") { Task { await model.download() } }.disabled(model.busy).accessibilityIdentifier("downloadExport") }
            }
            Button(model.startLabel, systemImage: "square.and.arrow.down") { Task { await model.start() } }
                .disabled(model.busy || model.export?.state == "building").accessibilityIdentifier("startExport")
            if model.busy { ProgressView() }
            if let error = model.error { Text(error).font(.footnote).foregroundStyle(.secondary) }
        }
        .task { await model.refresh() }
        .task(id: model.export?.state == "building") { await model.follow() }
    }
}

/// A computer's `phase` (`ComputerView`, docs/api.md), as Settings says it.
enum ComputerPhase {
    static func said(_ phase: String?) -> String {
        switch phase {
        case "asleep": "Asleep"
        case "starting": "Starting"
        case "awake": "Awake"
        case "sleeping": "Going to sleep"
        case "wont_wake": "Won’t wake"
        default: "—"
        }
    }
}
