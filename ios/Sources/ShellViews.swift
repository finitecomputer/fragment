import SwiftUI
import UIKit

struct DesktopWallpaper: View {
    var body: some View {
        GeometryReader { proxy in
            Image("DesktopWallpaper").resizable().scaledToFill()
                .frame(width: proxy.size.width, height: proxy.size.height).clipped()
        }.ignoresSafeArea().accessibilityHidden(true)
    }
}

struct AgentAvatar: View {
    let agent: FragmentSummary?
    var size: CGFloat = 32
    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: size * 0.28).fill(color.gradient)
            if let picture = agent?.picture, let url = URL(string: picture) {
                AuthenticatedImage(url: url) { image in
                    if let image { image.resizable().scaledToFill() } else { Color.clear }
                }
                    .frame(width: size, height: size).clipShape(RoundedRectangle(cornerRadius: size * 0.28))
            } else {
            Text(String((agent?.displayTitle ?? "?").prefix(1))).font(.system(size: size * 0.46, weight: .semibold)).foregroundStyle(.white)
            }
        }.frame(width: size, height: size).accessibilityHidden(true)
    }
    private var color: Color {
        let index = (agent?.name.utf8.reduce(0) { ($0 + Int($1)) % 5 }) ?? 0
        return [FragmentStyle.lilac, .teal, .orange, .indigo, .pink][index]
    }
}

struct SidebarDrawer: View {
    @Binding var isPresented: Bool
    let library: Library
    let page: ShellPage
    let api: FragmentAPI
    let navigate: (ShellPage) -> Void
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var dragOffset: CGFloat = 0
    private var settle: Animation? { reduceMotion ? nil : .snappy(duration: 0.25) }

    var body: some View {
        GeometryReader { geometry in
            let width = min(350, max(1, geometry.size.width - 36))
            ZStack(alignment: .leading) {
                if isPresented {
                    Color.black.opacity(0.25 * max(0, 1 + Double(dragOffset / width)))
                        .ignoresSafeArea()
                        .onTapGesture { close() }
                        .accessibilityHidden(true)
                        .transition(.opacity)
                    SidebarView(library: library, page: page, api: api, navigate: navigate)
                        .frame(width: width)
                        .offset(x: dragOffset)
                        .transition(.move(edge: .leading))
                        .accessibilityAddTraits(.isModal)
                        .accessibilityAction(.escape) { close() }
                        .gesture(SidebarPan(changed: { distance in
                            dragOffset = max(-width, min(0, distance))
                        }, ended: { distance, velocity in
                            let projected = distance + min(0, velocity) * 0.2
                            withAnimation(settle) {
                                if distance < -60 && projected < -width * 0.35 { isPresented = false }
                                dragOffset = 0
                            }
                        }))
                }
            }
            .animation(settle, value: isPresented)
        }
        .allowsHitTesting(isPresented)
        .onChange(of: isPresented) { _, _ in dragOffset = 0 }
        .task(id: isPresented) { if isPresented { await library.load(api) } }
    }
    private func close() { withAnimation(settle) { isPresented = false; dragOffset = 0 } }
}

// Directional recognition leaves vertical scrolling to the native scroll view
// and cancels button touches once a horizontal dismissal starts.
private struct SidebarPan: UIGestureRecognizerRepresentable {
    let changed: (CGFloat) -> Void
    let ended: (CGFloat, CGFloat) -> Void

    func makeCoordinator(converter: CoordinateSpaceConverter) -> Coordinator { Coordinator() }
    func makeUIGestureRecognizer(context: Context) -> UIPanGestureRecognizer {
        let pan = UIPanGestureRecognizer()
        pan.maximumNumberOfTouches = 1
        pan.delegate = context.coordinator
        return pan
    }
    func handleUIGestureRecognizerAction(_ pan: UIPanGestureRecognizer, context: Context) {
        // The window is stable while the drawer itself moves beneath the finger.
        let distance = pan.translation(in: pan.view?.window).x
        switch pan.state {
        case .began, .changed: changed(distance)
        case .ended: ended(distance, pan.velocity(in: pan.view?.window).x)
        case .cancelled, .failed: ended(0, 0)
        default: break
        }
    }
    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
            guard let pan = gesture as? UIPanGestureRecognizer else { return false }
            let translation = pan.translation(in: pan.view?.window)
            return translation.x < 0 && abs(translation.x) > abs(translation.y)
        }
    }
}

struct SidebarView: View {
    let library: Library
    let page: ShellPage
    let api: FragmentAPI
    let navigate: (ShellPage) -> Void
    @State private var renamingChat: FragmentSummary?
    @State private var chatTitle = ""
    @State private var managingChat: FragmentSummary?
    @State private var actionError: String?

    private var currentChat: FragmentSummary? {
        guard case .chat(let name) = page else { return nil }
        return library.fragments.first { $0.name == name }
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Finite.Computer").font(.headline)
                Spacer()
                Button("Search or start a new chat", systemImage: "square.and.pencil") { navigate(.launcher) }
                    .labelStyle(.iconOnly).buttonStyle(.glass).accessibilityIdentifier("sidebarNewChat")
            }.padding(20)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 8) {
                    ForEach(library.agents) { agent in
                        Menu {
                            Button("New chat with \(agent.displayTitle)", systemImage: "square.and.pencil") {
                                library.selectedAgent = agent.name; navigate(.launcher)
                            }
                            Button("Agent settings", systemImage: "person.crop.circle") { library.selectedAgent = agent.name; navigate(.settings) }
                            if let chat = currentChat, let identity = library.identities[agent.name], (chat.agents ?? []).contains(identity) {
                                Section(chat.displayTitle) { chatActions(chat) }
                            }
                        } label: {
                            HStack(spacing: 12) {
                                AgentAvatar(agent: agent)
                                Text(agent.displayTitle).font(.headline)
                                Spacer()
                                Image(systemName: "chevron.down").font(.caption).foregroundStyle(.secondary)
                            }.padding(12).background(.primary.opacity(0.045), in: RoundedRectangle(cornerRadius: 18))
                        }.buttonStyle(.plain).accessibilityIdentifier("agentMenu-\(agent.name)")
                        ForEach(library.chats.filter { ($0.agents ?? []).first == library.identities[agent.name] }) { chat in
                            chatRow(chat)
                        }
                    }
                    let ungrouped = library.chats.filter { library.agents(in: $0).isEmpty }
                    if !ungrouped.isEmpty {
                        Text("Shared chats").font(.caption).foregroundStyle(.secondary).padding(.top)
                        ForEach(ungrouped) { chatRow($0) }
                    }
                    if library.loading && library.fragments.isEmpty { ProgressView().padding() }
                    if let error = library.error {
                        Text(error).font(.footnote).foregroundStyle(.secondary)
                        Button("Try again") { Task { await library.load(api) } }
                    }
                }.padding(.horizontal, 12)
            }.refreshable { await library.load(api) }
            VStack(spacing: 4) {
                footer("History", icon: "clock", page: .history)
                footer("Your Apps", icon: "square.grid.2x2", page: .apps)
                footer("Settings", icon: "gearshape", page: .settings)
            }.padding(12)
        }
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 24))
        .padding(.vertical, 8).padding(.leading, 8)
        .alert("Name this chat", isPresented: Binding(get: { renamingChat != nil }, set: { if !$0 { renamingChat = nil } })) {
            TextField("Chat name", text: $chatTitle)
            Button("Save") {
                guard let chat = renamingChat else { return }
                Task {
                    do { try await api.rename(chat, title: chatTitle); await library.load(api) }
                    catch { actionError = error.localizedDescription }
                }
                renamingChat = nil
            }
            Button("Cancel", role: .cancel) { renamingChat = nil }
        }
        .alert("Couldn’t update chat", isPresented: Binding(get: { actionError != nil }, set: { if !$0 { actionError = nil } })) {
            Button("OK") { actionError = nil }
        } message: { Text(actionError ?? "") }
        .sheet(item: $managingChat) { chat in AgentMembershipView(chat: chat, library: library, api: api) }
    }
    @ViewBuilder private func chatActions(_ chat: FragmentSummary) -> some View {
        Button("Rename chat…", systemImage: "pencil") { chatTitle = chat.displayTitle; renamingChat = chat }
        Button("Agents in this chat", systemImage: "person.2") { managingChat = chat }
        Button(chat.archived == true ? "Unarchive chat" : "Archive chat", systemImage: "archivebox") {
            Task { await library.archive(chat, api: api) }
        }
    }
    private func chatRow(_ chat: FragmentSummary) -> some View {
        Button { navigate(.chat(chat.name)) } label: {
            VStack(alignment: .leading, spacing: 5) {
                HStack {
                    Text(chat.displayTitle).font(.subheadline.weight(.medium)).lineLimit(1)
                    Spacer(minLength: 4)
                    if let at = chat.previewAt { Text(Date(timeIntervalSince1970: Double(at) / 1000), style: .relative).font(.caption2).foregroundStyle(.secondary).lineLimit(1) }
                }
                HStack(spacing: 4) {
                    if library.agents(in: chat).count > 1 {
                        ForEach(library.agents(in: chat).prefix(3)) { AgentAvatar(agent: $0, size: 16) }
                    }
                    Text(chat.preview ?? "No messages yet").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }.padding(12).frame(maxWidth: .infinity, alignment: .leading)
                .background(page == .chat(chat.name) ? Color.primary.opacity(0.08) : Color.clear, in: RoundedRectangle(cornerRadius: 12))
        }.buttonStyle(.plain).accessibilityIdentifier("sidebarChat-\(chat.name)")
        .contextMenu { chatActions(chat) }
    }
    private func footer(_ title: String, icon: String, page destination: ShellPage) -> some View {
        Button { navigate(destination) } label: { Label(title, systemImage: icon).frame(maxWidth: .infinity, alignment: .leading).padding(12) }
            .buttonStyle(.plain).accessibilityIdentifier("sidebar-\(title.lowercased())")
    }
}

private enum AppsLibrarySheet: Identifiable {
    case settings(FragmentSummary), manage
    var id: String { switch self { case .settings(let app): "settings-" + app.name; case .manage: "manage" } }
}

struct AppsLibraryView: View {
    let library: Library
    let api: FragmentAPI
    @State private var sheet: AppsLibrarySheet?
    let open: (FragmentSummary) -> Void
    static let addApp = FragmentSummary(name: "add-app", kind: "app", title: "Add an app")
    var body: some View {
        ZStack {
            DesktopWallpaper()
            List {
                Section {
                    ForEach(library.shownApps) { app in
                        appRow(app, symbol: app.kind == "brain" ? "brain" : "app")
                    }
                }
                Section {
                    appRow(Self.addApp, symbol: "plus")
                }
            }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
        }
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("Manage Apps", systemImage: "slider.horizontal.3") { sheet = .manage }.accessibilityIdentifier("manageApps")
            }
        }
        .sheet(item: $sheet) { destination in
            NavigationStack {
                Group {
                    switch destination {
                    case .settings(let app): AppSettingsView(fragment: app, library: library, api: api)
                    case .manage: AppManagementList(library: library, api: api)
                    }
                }
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Close", systemImage: "xmark") { sheet = nil } } }
            }
            .presentationDragIndicator(.visible)
            .presentationDetents([.large])
        }
    }
    private func appRow(_ app: FragmentSummary, symbol: String) -> some View {
        HStack(spacing: 8) {
            Button { open(app) } label: {
                HStack(spacing: 14) {
                    FragmentIcon(fragment: app, fallback: symbol)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(app.displayTitle).font(.body.weight(.medium))
                        if let preview = app.preview, !preview.isEmpty {
                            Text(preview).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                        }
                    }
                    Spacer(minLength: 8)
                    if app.name == "add-app" { Image(systemName: "chevron.right").font(.caption.weight(.semibold)).foregroundStyle(.tertiary) }
                }
                .padding(.vertical, 12).frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
            }.buttonStyle(.plain).accessibilityIdentifier("app-\(app.name)")
            if app.name != "add-app" {
                Button { sheet = .settings(app) } label: {
                    Image(systemName: "gearshape").frame(width: 44, height: 44).contentShape(Rectangle())
                }.buttonStyle(.borderless).accessibilityLabel("Settings for \(app.displayTitle)")
                    .accessibilityIdentifier("appSettings-\(app.name)")
            }
        }.listRowBackground(Rectangle().fill(.regularMaterial))
    }
}

struct FragmentIcon: View {
    let fragment: FragmentSummary
    let fallback: String
    var size: CGFloat = 40
    var body: some View {
        Group {
            if let raw = fragment.picture, let url = URL(string: raw) {
                AuthenticatedImage(url: url) { loaded in
                    if let image = loaded { image.resizable().scaledToFit() }
                    else { placeholder }
                }
            } else { placeholder }
        }.frame(width: size, height: size).accessibilityHidden(true)
    }
    private var placeholder: some View {
        Image(systemName: fallback).font(.system(size: size * 0.6)).foregroundStyle(.secondary)
    }
}

enum HistoryFilter: String, CaseIterable { case all = "All", archived = "Archived", imported = "Imported" }
struct HistoryView: View {
    let library: Library
    let api: FragmentAPI
    let open: (FragmentSummary) -> Void
    @State private var search = ""
    @State private var filter = HistoryFilter.all
    @State private var messageHits: [String: String] = [:]
    @State private var importedHits: [String: String] = [:]
    @State private var error: String?
    private struct Row: Identifiable {
        let id: String
        let fragment: FragmentSummary
        let at: Int64
        let preview: String
    }
    private var rows: [Row] {
        var rows: [Row] = []
        if filter != .imported {
            for chat in library.fragments where chat.kind == "chat" && chat.previewAt != nil {
                guard filter != .archived || chat.archived == true else { continue }
                guard search.isEmpty || chat.displayTitle.localizedCaseInsensitiveContains(search) || messageHits[chat.name] != nil else { continue }
                rows.append(Row(id: "historyChat-\(chat.name)", fragment: chat, at: chat.previewAt ?? 0, preview: messageHits[chat.name] ?? chat.preview ?? ""))
            }
        }
        if filter != .archived, let archive = library.fragments.first(where: { $0.kind == "sessions" && $0.role == "owner" }) {
            for entry in library.imported {
                guard search.isEmpty || (entry.title ?? "").localizedCaseInsensitiveContains(search) || (entry.preview ?? "").localizedCaseInsensitiveContains(search) || importedHits[entry.key] != nil else { continue }
                let fragment = FragmentSummary(name: archive.name, kind: "imported", title: entry.title ?? "Untitled", sessionKey: entry.key)
                rows.append(Row(id: "importedSession-\(entry.key)", fragment: fragment, at: entry.at, preview: importedHits[entry.key] ?? entry.preview ?? ""))
            }
        }
        return rows.sorted { $0.at > $1.at }
    }
    var body: some View {
        List {
            Picker("Show", selection: $filter) { ForEach(HistoryFilter.allCases, id: \.self) { Text($0.rawValue).tag($0) } }.pickerStyle(.segmented).listRowBackground(Color.clear)
            ForEach(rows) { row in
                Button { open(row.fragment) } label: {
                    HStack(spacing: 12) {
                        if row.fragment.kind == "imported" { Image(systemName: "tray.and.arrow.down").frame(width: 32) }
                        else { AgentAvatar(agent: library.agents(in: row.fragment).first) }
                        VStack(alignment: .leading, spacing: 4) {
                            HStack {
                                Text(row.fragment.displayTitle).foregroundStyle(.primary)
                                Spacer()
                                if row.fragment.archived == true { Image(systemName: "archivebox").font(.caption) }
                                Text(Date(timeIntervalSince1970: Double(row.at) / 1000), style: .date).font(.caption2).foregroundStyle(.secondary)
                            }
                            Text(row.preview).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                            if row.fragment.kind == "imported" { Text("Imported · Read only").font(.caption2).foregroundStyle(.tertiary) }
                        }
                    }.padding(.vertical, 4)
                }.accessibilityIdentifier(row.id)
                .swipeActions {
                    if row.fragment.kind == "chat" {
                        Button(row.fragment.archived == true ? "Unarchive" : "Archive", systemImage: "archivebox") { Task { await library.archive(row.fragment, api: api) } }.tint(.indigo)
                    }
                }
            }
            if rows.isEmpty { ContentUnavailableView.search(text: search) }
            if let error { Text(error).font(.footnote).foregroundStyle(.secondary) }
            if let error = library.error { Text(error).font(.footnote).foregroundStyle(.secondary) }
        }.searchable(text: $search, prompt: "Search chats").refreshable { await library.load(api) }
            .scrollContentBackground(.hidden).background(FragmentStyle.background)
            .task(id: search) {
                messageHits = [:]; importedHits = [:]; error = nil
                guard !search.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
                do {
                    try await Task.sleep(for: .milliseconds(250))
                    let result = try await api.search(search)
                    try Task.checkCancellation()
                    for hit in result.messages where messageHits[hit.fragment] == nil { messageHits[hit.fragment] = hit.snippet }
                    if let archive = library.fragments.first(where: { $0.kind == "sessions" && $0.role == "owner" }) {
                        let result: JSONValue = try await api.request("api/f/\(archive.name)/search", method: "POST", body: .object(["q": .string(FragmentAPI.searchHint(search)), "limit": .number(50), "mode": .string("lexical"), "fields": .object(["kind": .string("chat")])]))
                        try Task.checkCancellation()
                        for hit in result["results"].array {
                            guard let source = hit["source"].string, source.hasPrefix("sessions/"), source.hasSuffix(".json") else { continue }
                            let key = String(source.dropFirst(9).dropLast(5))
                            if importedHits[key] == nil { importedHits[key] = hit["snippet"].string ?? "" }
                        }
                    }
                } catch { if !Task.isCancelled { self.error = "Some messages could not be searched: \(error.localizedDescription)" } }
            }
    }
}

struct AgentMembershipView: View {
    @Environment(\.dismiss) private var dismiss
    let chat: FragmentSummary
    let library: Library
    let api: FragmentAPI
    @State private var working = false
    @State private var error: String?
    var body: some View {
        NavigationStack {
            List {
                Section("Agents in this chat") {
                    ForEach(library.agents) { agent in
                        let joined = library.fragments.first(where: { $0.name == chat.name })?.agents?.contains(library.identities[agent.name] ?? "") == true
                        Button { Task { await add(agent) } } label: {
                            HStack { AgentAvatar(agent: agent); Text(agent.displayTitle); Spacer(); Image(systemName: joined ? "checkmark" : "plus") }
                        }.disabled(joined || working).accessibilityIdentifier("joinAgent-\(agent.name)")
                    }
                }
                if let session = library.sessions[chat.name] {
                    Section("Messages") {
                        Picker("Send to", selection: Binding(
                            get: { session.recipients.count > 1 ? "all" : session.recipients.first ?? "lead" },
                            set: { selection in
                                let members = library.fragments.first(where: { $0.name == chat.name })?.agents ?? []
                                session.recipients = selection == "lead" ? [] : selection == "all" ? members : [selection]
                            }
                        )) {
                            Text("Lead agent").tag("lead")
                            ForEach(library.agents(in: library.fragments.first(where: { $0.name == chat.name }) ?? chat)) { agent in
                                if let identity = library.identities[agent.name] { Text(agent.displayTitle).tag(identity) }
                            }
                            Text("All agents").tag("all")
                        }.accessibilityIdentifier("messageRecipient")
                    }
                }
                if let error { Text(error).foregroundStyle(.secondary) }
            }.navigationTitle("Agents").toolbar { Button("Done") { dismiss() } }
        }
    }
    private func add(_ agent: FragmentSummary) async {
        working = true; defer { working = false }
        do { try await api.join(agent, to: chat); await library.load(api) }
        catch { self.error = error.localizedDescription }
    }
}
