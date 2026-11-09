import SafariServices
import SwiftUI

/// What a web sheet shows: a fragment's one-use `__signin` URL, asked of
/// `POST /api/f/{name}/session` (docs/ios.md), or why not. A redemption is
/// spent once its page loads: each load drops the last before asking for a
/// new one, so no web view ever opens a spent one.
@MainActor @Observable
final class WebSheetModel {
    let fragment: FragmentSummary
    let api: FragmentAPI
    private(set) var url: URL?
    var error: String?
    var loading = true
    private(set) var needsConsent = false
    private(set) var consentEmail: String?
    init(fragment: FragmentSummary, api: FragmentAPI) { self.fragment = fragment; self.api = api }

    func load(consent: Bool = false) async {
        url = nil; error = nil; needsConsent = false
        loading = true
        do {
            struct Status: Decodable { struct URLs: Decodable { let canonical: URL }; let urls: URLs }
            let status: Status = try await api.request("api/f/\(fragment.name)/status")
            guard ["http", "https"].contains(status.urls.canonical.scheme), status.urls.canonical.host != nil, status.urls.canonical.user == nil, status.urls.canonical.password == nil else {
                throw APIError.invalidResponse
            }
            var origin = URLComponents(url: status.urls.canonical, resolvingAgainstBaseURL: false)!
            origin.path = ""; origin.query = nil; origin.fragment = nil
            _ = try PlatformOrigin.parse(origin.url!.absoluteString)
            var components = URLComponents(url: status.urls.canonical, resolvingAgainstBaseURL: false)
            if let key = fragment.sessionKey { components?.queryItems = [URLQueryItem(name: "s", value: key)] }
            // a link's own page, path, query and hash, on this fragment's origin
            // alone (a link elsewhere opens its canonical page), as the web's Open
            if let link = fragment.link, PlatformOrigin.same(link, status.urls.canonical) {
                components = URLComponents(url: link, resolvingAgainstBaseURL: false)
            }
            guard let destination = components?.url else { throw APIError.invalidResponse }
            struct Session: Decodable { let consentRequired: Bool; var url: URL?; var email: String? }
            let back = components!.percentEncodedPath + (components!.percentEncodedQuery.map { "?" + $0 } ?? "")
                + (components!.percentEncodedFragment.map { "#" + $0 } ?? "")
            let session: Session = try await api.request("api/f/\(fragment.name)/session", method: "POST", body: .object([
                "returnTo": .string(back.isEmpty ? "/" : back), "consent": .bool(consent)
            ]))
            if session.consentRequired { consentEmail = session.email; needsConsent = true; loading = false; return }
            guard let redemption = session.url, PlatformOrigin.same(redemption, destination),
                  redemption.user == nil, redemption.password == nil, redemption.fragment == nil,
                  redemption.path.hasSuffix("/__signin") else { throw APIError.invalidResponse }
            url = redemption
        } catch { self.error = error.localizedDescription }
    }
}

struct FragmentWebSheet: View {
    @Environment(\.dismiss) private var dismiss
    let fragment: FragmentSummary
    @State private var model: WebSheetModel
    init(fragment: FragmentSummary, api: FragmentAPI) {
        self.fragment = fragment
        _model = State(initialValue: WebSheetModel(fragment: fragment, api: api))
    }

    var body: some View {
        @Bindable var model = model
        NavigationStack {
            Group {
                if model.needsConsent {
                    ContentUnavailableView {
                        Label("Open \(fragment.displayTitle)?", systemImage: "person.crop.circle.badge.checkmark")
                    } description: {
                        Text("Continue as \(model.consentEmail ?? "your account")? This app will be able to recognize you.")
                    } actions: {
                        Button("Continue") { Task { await model.load(consent: true) } }
                        Button("Cancel", role: .cancel) { dismiss() }
                    }
                } else if let error = model.error {
                    ContentUnavailableView {
                        Label("Couldn’t open this app", systemImage: "wifi.exclamationmark")
                    } description: { Text(error) } actions: {
                        Button("Try again") { Task { await model.load() } }
                    }
                } else if let url = model.url {
                    // a new redemption is a new Safari view: one never reloads a spent one
                    SafariPresenter(url: url) { dismiss() }.id(url)
                } else { ProgressView() }
            }
            .navigationTitle(fragment.displayTitle).navigationBarTitleDisplayMode(.inline)
            // once: Safari's full screen takes the sheet off screen, and its
            // coming back must close it, never ask for another redemption
            .task { if model.url == nil { await model.load() } }
        }
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        .accessibilityAction(.escape) { dismiss() }
    }
}

/// Opens a fragment's redemption in Safari's own view, presented full screen
/// as Apple asks (never embedded or covered): Safari's handling of the page,
/// and this app's Safari data, kept as Safari keeps it (docs/ios.md). Once
/// Safari's Close has taken it away, the sheet beneath closes too.
private struct SafariPresenter: UIViewControllerRepresentable {
    let url: URL
    let done: () -> Void
    func makeUIViewController(context: Context) -> Host { Host(url: url, done: done) }
    func updateUIViewController(_ host: Host, context: Context) {}

    final class Host: UIViewController {
        let url: URL
        let done: () -> Void
        private var presented = false
        init(url: URL, done: @escaping () -> Void) { self.url = url; self.done = done; super.init(nibName: nil, bundle: nil) }
        required init?(coder: NSCoder) { nil }
        // first shown: Safari over it; shown again: Safari has gone, wholly
        override func viewDidAppear(_ animated: Bool) {
            super.viewDidAppear(animated)
            if presented { done(); return }
            presented = true
            let safari = SFSafariViewController(url: url)
            safari.modalPresentationStyle = .fullScreen
            safari.dismissButtonStyle = .close
            present(safari, animated: true)
        }
    }
}
