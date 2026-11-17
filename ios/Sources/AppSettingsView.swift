import SwiftUI

struct AppManagementList: View {
    let library: Library
    let api: FragmentAPI
    @State private var search = ""
    private var apps: [FragmentSummary] {
        library.allApps.filter { search.isEmpty || $0.displayTitle.localizedCaseInsensitiveContains(search) }
    }
    var body: some View {
        List {
            ForEach(apps) { app in
                NavigationLink {
                    AppSettingsView(fragment: app, library: library, api: api)
                } label: {
                    HStack(spacing: 12) {
                        FragmentIcon(fragment: app, fallback: "app")
                        VStack(alignment: .leading, spacing: 4) {
                            Text(app.displayTitle)
                            if app.archived == true { Text("Archived").font(.caption).foregroundStyle(.secondary) }
                            else if app.dock == false { Text("Hidden from Your Apps").font(.caption).foregroundStyle(.secondary) }
                        }
                    }.padding(.vertical, 4)
                }.accessibilityIdentifier("manageApp-\(app.name)")
            }
            if apps.isEmpty { ContentUnavailableView.search(text: search) }
            if let error = library.error { Text(error).font(.footnote).foregroundStyle(.secondary) }
        }
        .navigationTitle("Manage Apps").navigationBarTitleDisplayMode(.inline)
        .searchable(text: $search, prompt: "Find an app")
        .refreshable { await library.load(api) }
        .task { await library.load(api) }
    }
}

struct AppSettingsView: View {
    let fragment: FragmentSummary
    let library: Library
    let api: FragmentAPI
    @State private var details: AppDetails?
    @State private var title = ""
    @State private var busy = false
    @State private var error: String?
    private var app: FragmentSummary { library.fragments.first { $0.name == fragment.name } ?? fragment }
    private var canEdit: Bool { details?.canEdit ?? false }
    var body: some View {
        Form {
            Section {
                HStack(spacing: 16) {
                    FragmentIcon(fragment: app, fallback: "app", size: 56)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(app.displayTitle).font(.headline)
                        Text(details?.role == "owner" ? "Your app" : "Shared with you").font(.subheadline).foregroundStyle(.secondary)
                    }
                }.padding(.vertical, 8)
            }
            Section("General") {
                if canEdit {
                    TextField("App name", text: $title).accessibilityIdentifier("appName")
                    Button("Save name") { Task { await change { try await api.rename(app, title: title) } } }
                        .disabled(title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || title == app.displayTitle)
                }
                Toggle("Show in Your Apps", isOn: Binding(
                    get: { app.dock != false },
                    set: { value in Task { await change { let _: JSONValue = try await api.request("api/fragments/\(app.name)/dock", method: "PUT", body: .object(["dock": .bool(value)])) } } }
                )).tint(.green).disabled(app.archived == true).accessibilityIdentifier("appShown")
                if app.archived == true { Text("Unarchive this app to show it in Your Apps.").font(.footnote).foregroundStyle(.secondary) }
            }
            Section {
                NavigationLink {
                    AppSharingView(fragment: app, library: library, api: api)
                } label: { Label("Sharing & Permissions", systemImage: "person.2") }.accessibilityIdentifier("appSharing")
                NavigationLink {
                    AppAppearanceView(fragment: app, library: library, api: api)
                } label: { Label("Icon & Preview", systemImage: "photo") }.accessibilityIdentifier("appAppearance")
            }
            if let address = details?.urls.canonical {
                Section("App address") {
                    Text(address.absoluteString).font(.footnote).textSelection(.enabled)
                    ShareLink(item: address) { Label("Share app address", systemImage: "square.and.arrow.up") }
                }
            }
            Section {
                Button(app.archived == true ? "Unarchive app" : "Archive app", systemImage: "archivebox") {
                    Task { await change { let _: JSONValue = try await api.request("api/fragments/\(app.name)/archived", method: "PUT", body: .object(["archived": .bool(app.archived != true)])) } }
                }.accessibilityIdentifier("archiveApp")
            } footer: { Text("Showing, hiding, and archiving change your own app list. They don’t change who can access the app.") }
            if busy { ProgressView("Saving…") }
            if let error { Section { Text(error).font(.footnote).foregroundStyle(.secondary); Button("Try again") { Task { await load() } } } }
        }
        .disabled(busy)
        .navigationTitle("App Settings").navigationBarTitleDisplayMode(.inline)
        .task { if details == nil { title = app.displayTitle; await load() } }
    }
    private func load() async {
        do { details = try await api.appDetails(app.name); error = nil }
        catch { self.error = error.localizedDescription }
    }
    private func change(_ action: () async throws -> Void) async {
        busy = true; error = nil; defer { busy = false }
        do { try await action(); await library.load(api); await load() }
        catch { self.error = error.localizedDescription }
    }
}
