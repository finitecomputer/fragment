import SwiftUI
import UniformTypeIdentifiers

struct AppAppearanceView: View {
    let fragment: FragmentSummary
    let library: Library
    let api: FragmentAPI
    @State private var details: AppDetails?
    @State private var hint = ""
    @State private var options: AppIconOptions?
    @State private var preview: Data?
    @State private var busy = false
    @State private var making = false
    @State private var importer = false
    @State private var error: String?
    @State private var previewRun: UUID?
    @State private var previewSaid: String?
    private var app: FragmentSummary { library.fragments.first { $0.name == fragment.name } ?? fragment }
    private var canEdit: Bool { details?.canEdit ?? false }
    private var inFlight: Bool { busy || making || previewRun != nil }
    var body: some View {
        Form {
            Section("App icon") {
                HStack { Spacer(); FragmentIcon(fragment: app, fallback: "app", size: 96); Spacer() }.padding(.vertical, 12)
                if canEdit {
                    TextField("Describe an icon (optional)", text: $hint, axis: .vertical).lineLimit(1...3).accessibilityIdentifier("iconHint")
                    Button("Generate icons", systemImage: "sparkles") { Task { await makeIcons() } }.accessibilityIdentifier("makeAppIcons")
                        .disabled(inFlight || hint.count > 200)
                    Button("Upload icon…", systemImage: "square.and.arrow.up") { importer = true }.disabled(inFlight)
                    if app.picture != nil {
                        Button("Remove icon", systemImage: "trash", role: .destructive) { Task { await change { let _: JSONValue = try await api.request("api/f/\(app.name)/icon", method: "DELETE") } } }
                            .disabled(inFlight).accessibilityIdentifier("removeAppIcon")
                    }
                }
            }
            if making { Section { ProgressView("Generating icons…") } }
            if let options {
                Section("Choose an icon") {
                    Text(options.subject).font(.subheadline).foregroundStyle(.secondary)
                    ForEach(Array(options.candidates.prefix(8).enumerated()), id: \.offset) { index, candidate in
                        if let data = candidate.data, let image = UIImage(data: data) {
                            Button { Task { await chooseIcon(data) } } label: {
                                HStack(spacing: 16) {
                                    Image(uiImage: image).resizable().scaledToFit().frame(width: 72, height: 72)
                                    Text("Use this icon")
                                    Spacer()
                                }.padding(.vertical, 4)
                            }.disabled(inFlight).accessibilityIdentifier("useAppIcon-\(index)")
                        }
                    }
                    if options.candidates.isEmpty { Text("No icons came out right. Try another description.").foregroundStyle(.secondary) }
                }
            }
            Section("Preview") {
                if let preview, let image = UIImage(data: preview) {
                    Image(uiImage: image).resizable().scaledToFit().clipShape(RoundedRectangle(cornerRadius: 12)).accessibilityLabel("App preview").accessibilityIdentifier("appPreviewImage")
                } else { Text("No preview yet.").foregroundStyle(.secondary) }
                if canEdit && details?.visibility != "members" {
                    Button("Generate preview", systemImage: "camera") { previewSaid = nil; previewRun = UUID() }.disabled(inFlight).accessibilityIdentifier("makeAppPreview")
                } else if canEdit { Text("Previews are available for apps shared by link or publicly. Restricted apps are not photographed.").font(.footnote).foregroundStyle(.secondary) }
                if previewRun != nil { ProgressView("Taking a new preview…") }
                if let previewSaid { Text(previewSaid).font(.footnote).foregroundStyle(.secondary) }
                if let report = details?.page, !report.errors.isEmpty {
                    DisclosureGroup("Page reported \(report.errors.count) issues") {
                        ForEach(Array(report.errors.prefix(10).enumerated()), id: \.offset) { _, issue in
                            Text("\(issue.kind): \(issue.text)").font(.caption).textSelection(.enabled)
                        }
                    }
                }
            }
            if busy { ProgressView("Saving…") }
            if let error { Section { Text(error).font(.footnote).foregroundStyle(.secondary); Button("Reload appearance") { Task { await load() } } } }
        }
        .navigationTitle("Icon & Preview").navigationBarTitleDisplayMode(.inline)
        .task { await load() }
        .task(id: previewRun) { if previewRun != nil { await generatePreview() } }
        .fileImporter(isPresented: $importer, allowedContentTypes: [.png, .jpeg, .gif, UTType("org.webmproject.webp") ?? .image]) { result in
            switch result {
            case .success(let url): Task { await upload(url) }
            case .failure(let error): self.error = error.localizedDescription
            }
        }
    }
    private func load() async {
        do {
            async let status = api.appDetails(app.name)
            async let image = api.appImage(app.name, kind: "card")
            (details, preview) = try await (status, image); error = nil
        } catch { self.error = error.localizedDescription }
    }
    private func change(_ action: () async throws -> Void) async {
        busy = true; error = nil; defer { busy = false }
        do { try await action(); await library.load(api) }
        catch { self.error = error.localizedDescription }
    }
    private func chooseIcon(_ data: Data) async {
        await change { try await api.putAppIcon(data, name: app.name) }
        if error == nil { options = nil }
    }
    private func makeIcons() async {
        making = true; options = nil; error = nil; defer { making = false }
        do { options = try await api.makeAppIcons(app.name, hint: hint) }
        catch { self.error = error.localizedDescription }
    }
    private func upload(_ url: URL) async {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        await change { try await api.putAppIcon(AppIconImage.fitted(url), name: app.name) }
    }
    private func generatePreview() async {
        defer { previewRun = nil }
        do {
            struct Asked: Decodable { let since: Int64 }
            let asked: Asked = try await api.request("api/f/\(app.name)/card", method: "POST")
            // Same bound as the web Apps page. The view's task cancels polling
            // when dismissed; a slow capture is never reported as a success.
            for _ in 0..<180 {
                try Task.checkCancellation()
                let status = try await api.appDetails(app.name)
                if let report = status.page, report.at >= asked.since {
                    details = status
                    preview = try await api.appImage(app.name, kind: "card")
                    previewSaid = preview == nil ? "The page reported, but no preview image is available." : "Preview updated."
                    return
                }
                try await Task.sleep(for: .seconds(1))
            }
            previewSaid = "The preview is still being taken. Reload appearance to check again."
        } catch { if !Task.isCancelled { self.error = error.localizedDescription } }
    }
}
