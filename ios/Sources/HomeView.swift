import SwiftUI

struct HomeView: View {
    let library: Library
    let api: FragmentAPI
    @Binding var draft: String
    let open: (FragmentSummary) -> Void
    let start: () async -> Void
    let refresh: () async -> Void
    @State private var results = SearchResults()
    @State private var searching = false
    @State private var searchError: String?
    @FocusState private var focused: Bool
    private var query: String { draft.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var canStart: Bool { !query.isEmpty && query.utf8.count <= 32_768 && library.agent != nil && !library.creating }

    var body: some View {
        ZStack {
            DesktopWallpaper()
            VStack(spacing: 0) {
                Spacer(minLength: 24)
                VStack(spacing: 0) {
                    HStack(spacing: 10) {
                        Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                        TextField("Ask anything, or find a chat or app…", text: $draft)
                            .focused($focused).submitLabel(.go).accessibilityIdentifier("launcherInput")
                            .disabled(library.creating).onSubmit(launch)
                        if !draft.isEmpty {
                            Button("Clear search", systemImage: "xmark.circle.fill") { draft = "" }
                                .labelStyle(.iconOnly).foregroundStyle(.secondary).disabled(library.creating)
                        }
                        Button(action: launch) {
                            if library.creating { ProgressView().frame(width: 36, height: 36) }
                            else { Image(systemName: "arrow.up").frame(width: 36, height: 36) }
                        }.buttonStyle(.glassProminent).buttonBorderShape(.circle)
                            .disabled(!canStart).accessibilityLabel("Start new chat").accessibilityIdentifier("startChat")
                    }.padding(16)
                    if let agent = library.agent {
                        HStack {
                            Menu {
                                ForEach(library.agents) { agent in
                                    Button { library.selectedAgent = agent.name } label: {
                                        Label(agent.displayTitle, systemImage: agent.name == library.agent?.name ? "checkmark" : "person")
                                    }
                                }
                            } label: {
                                HStack(spacing: 8) { AgentAvatar(agent: agent, size: 22); Text(agent.displayTitle); Image(systemName: "chevron.down").font(.caption2) }
                                    .font(.caption.weight(.medium))
                            }.accessibilityIdentifier("launcherAgent")
                            Spacer()
                        }.padding(.horizontal, 18).padding(.bottom, 14)
                    }
                    if !query.isEmpty {
                        Divider()
                        ScrollView {
                            LazyVStack(alignment: .leading, spacing: 4) {
                                if library.agent != nil {
                                    Button(action: launch) {
                                        Label {
                                            VStack(alignment: .leading, spacing: 5) {
                                                Text("Start a new chat with \(library.agent?.displayTitle ?? "your agent")").font(.subheadline.weight(.medium))
                                                Text(query).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                                            }
                                        } icon: { Image(systemName: "square.and.pencil").frame(width: 30) }
                                            .frame(maxWidth: .infinity, alignment: .leading).padding(12)
                                    }.disabled(!canStart).accessibilityIdentifier("newChatResult")
                                }
                                foundFragments("Chats", kinds: ["chat"])
                                foundFragments("Apps", kinds: ["app", "brain"])
                                if !results.messages.isEmpty {
                                    heading("Messages")
                                    ForEach(results.messages) { hit in
                                        if let fragment = library.fragments.first(where: { $0.name == hit.fragment }) {
                                            Button { focused = false; open(fragment) } label: {
                                                VStack(alignment: .leading, spacing: 5) {
                                                    Text(fragment.displayTitle).font(.subheadline.weight(.medium))
                                                    Text(hit.snippet).font(.caption).foregroundStyle(.secondary).lineLimit(3)
                                                }.frame(maxWidth: .infinity, alignment: .leading).padding(12)
                                            }.accessibilityIdentifier("messageResult-\(hit.id)")
                                        }
                                    }
                                }
                                if searching { ProgressView("Searching…").padding() }
                                if let searchError { Text(searchError).font(.footnote).foregroundStyle(.secondary).padding() }
                            }.padding(6).buttonStyle(.plain)
                        }.frame(maxHeight: 330).scrollDismissesKeyboard(.interactively)
                    }
                    if library.agent == nil && !library.loading {
                        VStack(spacing: 8) {
                            Text("Your account has no agent yet.").foregroundStyle(.secondary)
                            Link("Set up your agent on the web", destination: api.baseURL.appending(path: "welcome"))
                        }.font(.footnote).padding()
                    }
                    if let error = library.error {
                        Text(error).font(.footnote).foregroundStyle(.secondary).padding()
                        Button("Try again") { Task { await refresh() } }.padding(.bottom)
                    }
                }
                .glassEffect(.regular, in: .rect(cornerRadius: 26))
                .frame(maxWidth: 620).padding(.horizontal, 16)
                Spacer(minLength: 24)
            }
        }
        .task(id: query) {
            results = SearchResults(); searchError = nil; searching = false
            guard !query.isEmpty else { return }
            let asked = query
            do {
                try await Task.sleep(for: .milliseconds(250)); searching = true
                let found = try await api.search(asked)
                try Task.checkCancellation()
                guard asked == query else { return }
                results = found; searching = false
            } catch { guard !Task.isCancelled, asked == query else { return }; searchError = error.localizedDescription; searching = false }
        }
    }
    private func heading(_ text: String) -> some View { Text(text).font(.caption.weight(.semibold)).foregroundStyle(.secondary).padding(.horizontal, 12).padding(.top, 10) }
    @ViewBuilder private func foundFragments(_ title: String, kinds: Set<String>) -> some View {
        let found = results.fragments.filter { kinds.contains($0.kind) }
        if !found.isEmpty {
            heading(title)
            ForEach(found) { fragment in
                Button { focused = false; open(fragment) } label: {
                    HStack(spacing: 12) {
                        if fragment.kind == "chat" { AgentAvatar(agent: library.agents(in: fragment).first, size: 28) }
                        else { Image(systemName: "app").frame(width: 28) }
                        VStack(alignment: .leading, spacing: 4) {
                            Text(fragment.displayTitle).font(.subheadline.weight(.medium))
                            if let preview = fragment.preview { Text(preview).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
                        }
                        Spacer(minLength: 0)
                        if fragment.archived == true { Image(systemName: "archivebox").font(.caption).foregroundStyle(.secondary) }
                    }.padding(12)
                }.accessibilityIdentifier("fragmentResult-\(fragment.name)")
            }
        }
    }
    private func launch() { guard canStart else { return }; focused = false; Task { await start() } }
}

struct ComposerView: View {
    @Binding var text: String
    let working: Bool
    let enabled: Bool
    var busy = false
    var recording = false
    let attach: () -> Void
    let record: () -> Void
    let send: () async -> Void
    let stop: () async -> Void
    @FocusState private var focused: Bool
    @State private var submitting = false
    var hasAttachments = false
    private var valid: Bool { (!text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || hasAttachments) && text.utf8.count <= 32_768 }
    private var canSend: Bool { !submitting && !busy && !recording && enabled && (working || valid) }
    var body: some View {
        // A single glass surface, with equal hit targets and no nested glass
        // button padding, keeps the single-line and growing field aligned.
        HStack(alignment: .bottom, spacing: 0) {
            Button(action: attach) {
                Image(systemName: "plus").font(.system(size: 20))
                    .frame(width: 44, height: 44).contentShape(Rectangle())
            }
            .accessibilityLabel("Attach files").disabled(busy || recording)
            Button(action: record) {
                Image(systemName: recording ? "stop.circle.fill" : "mic")
                    .font(.system(size: 20))
                    .foregroundStyle(recording ? Color.red : Color.primary)
                    .frame(width: 44, height: 44).contentShape(Rectangle())
            }
            .accessibilityLabel(recording ? "Finish voice memo" : "Record a voice memo")
            .disabled(busy)
            TextField("Message your agent", text: $text, axis: .vertical)
                .font(.body).lineLimit(1...6)
                .padding(.vertical, 10).padding(.horizontal, 6)
                .frame(minHeight: 44).focused($focused).accessibilityIdentifier("composer")
            Button {
                submitting = true
                Task { await (working ? stop() : send()); submitting = false }
            } label: {
                ZStack {
                    // The adaptive label color keeps the filled control high-contrast;
                    // the glass foreground style is softened by vibrancy.
                    Circle().fill(canSend ? Color(uiColor: .label) : Color(uiColor: .tertiarySystemFill))
                    if busy || submitting {
                        ProgressView().tint(.secondary)
                    } else {
                        Image(systemName: working ? "stop.fill" : "arrow.up")
                            .font(.system(size: 17, weight: .semibold))
                            .foregroundStyle(canSend ? Color(uiColor: .systemBackground) : Color.secondary)
                    }
                }
                .frame(width: 34, height: 34)
                .frame(width: 44, height: 44)
                .contentShape(Circle())
            }
            .disabled(!canSend)
            .accessibilityLabel(working ? "Stop response" : "Send message")
            .accessibilityIdentifier(working ? "stop" : "send")
        }
        .buttonStyle(.plain)
        .padding(6)
        .glassEffect(.regular, in: .rect(cornerRadius: 28))
        .frame(maxWidth: 660)
        .padding(.horizontal, 12).padding(.top, 8).padding(.bottom, 8)
        .frame(maxWidth: .infinity)
    }
}
