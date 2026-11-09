import CryptoKit
import SwiftUI
import WebKit

/// Where a fragment's pages keep what a browser keeps for them (cookies,
/// localStorage, IndexedDB, caches): a persistent store of their own, one per
/// person and fragment origin, so a page keeps its state from one visit to the
/// next as it does in Safari. One fragment's store holds only what its own
/// pages put there (its session cookie, from its `__signin`), native keys are
/// in none, and disconnecting this iPhone removes them all (docs/ios.md).
enum FragmentWebStore {
    /// The store of `origin`'s pages for `person` of `platform`, in `scope`:
    /// a UUID made from them by their hash (RFC 9562's version 8), the same
    /// each time, and another for any other person, fragment or launch scope.
    static func identifier(scope: LaunchScope = .current, platform: URL, person: String, origin: URL) -> UUID {
        // each part length first, so no part can end where another begins
        let parts = ["fragment-web-store/1", scope.keychainService, Self.origin(platform), person, Self.origin(origin)]
        let named = parts.map { "\($0.utf8.count):\($0)" }.joined()
        var bytes = Array(SHA256.hash(data: Data(named.utf8)).prefix(16))
        bytes[6] = (bytes[6] & 0x0F) | 0x80
        bytes[8] = (bytes[8] & 0x3F) | 0x80
        return bytes.withUnsafeBytes { UUID(uuid: $0.load(as: uuid_t.self)) }
    }

    /// `url`'s origin, as a browser keys its storage: scheme, host and port.
    static func origin(_ url: URL) -> String {
        var c = URLComponents()
        c.scheme = url.scheme?.lowercased(); c.host = url.host?.lowercased(); c.port = url.port
        return c.string ?? url.absoluteString
    }

    /// Removes every fragment's store, as this iPhone disconnects: no page's
    /// state, nor its session cookie, outlives the account it was kept for.
    @MainActor static func removeAll() async throws {
        for id in await WKWebsiteDataStore.allDataStoreIdentifiers {
            try await WKWebsiteDataStore.remove(forIdentifier: id)
        }
    }
}

/// What a web sheet shows: a fragment's one-use `__signin` URL, asked of
/// `POST /api/f/{name}/session` (docs/ios.md), or why not. A redemption is
/// spent once its page loads: each load drops the last before asking for a
/// new one, so no web view ever opens a spent one.
@MainActor @Observable
final class WebSheetModel {
    let fragment: FragmentSummary
    let api: FragmentAPI
    private(set) var url: URL?
    /// The store its pages keep their state in (`FragmentWebStore`).
    private(set) var store: UUID?
    var error: String?
    var loading = true
    private(set) var needsConsent = false
    private(set) var consentEmail: String?
    init(fragment: FragmentSummary, api: FragmentAPI) { self.fragment = fragment; self.api = api }

    func load(consent: Bool = false) async {
        url = nil; store = nil; error = nil; needsConsent = false
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
            // a connected account always names its person (`AccountConnection`)
            guard let person = api.accountID else { throw APIError.invalidResponse }
            store = FragmentWebStore.identifier(platform: api.baseURL, person: person, origin: redemption)
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
                } else if let url = model.url, let store = model.store {
                    // a new redemption is a new web view: one never reloads a spent one
                    WebContent(url: url, store: store, loading: $model.loading, error: $model.error)
                        .id(url)
                        .overlay { if model.loading { ProgressView() } }
                } else { ProgressView() }
            }
            .navigationTitle(fragment.displayTitle).navigationBarTitleDisplayMode(.inline)
            .task { await model.load() }
        }
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        .accessibilityAction(.escape) { dismiss() }
    }
}

private struct WebContent: UIViewRepresentable {
    let url: URL
    let store: UUID
    @Binding var loading: Bool
    @Binding var error: String?

    func makeCoordinator() -> Coordinator { Coordinator(loading: $loading, error: $error, origin: url) }
    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        // This fragment's own store, kept as Safari keeps an origin's. Native
        // credentials never enter a fragment page; connected accounts use a
        // scoped redemption (docs/ios.md).
        configuration.websiteDataStore = WKWebsiteDataStore(forIdentifier: store)
        let web = WKWebView(frame: .zero, configuration: configuration)
        web.navigationDelegate = context.coordinator
        web.allowsBackForwardNavigationGestures = true
        web.load(URLRequest(url: url))
        return web
    }
    func updateUIView(_ uiView: WKWebView, context: Context) {}

    @MainActor final class Coordinator: NSObject, WKNavigationDelegate {
        @Binding var loading: Bool
        @Binding var error: String?
        let origin: URL
        init(loading: Binding<Bool>, error: Binding<String?>, origin: URL) {
            _loading = loading; _error = error; self.origin = origin
        }
        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { loading = false }
        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
            self.error = error.localizedDescription; loading = false
        }
        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
            self.error = error.localizedDescription; loading = false
        }
        func webView(_ webView: WKWebView, decidePolicyFor navigationResponse: WKNavigationResponse) async -> WKNavigationResponsePolicy {
            if navigationResponse.isForMainFrame, let response = navigationResponse.response as? HTTPURLResponse, response.statusCode >= 400 {
                error = "The app could not be opened (HTTP \(response.statusCode)). Check your access and try again."
                loading = false
                return .cancel
            }
            return .allow
        }
        func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction) async -> WKNavigationActionPolicy {
            guard let url = navigationAction.request.url else { return .cancel }
            guard ["http", "https", "about"].contains(url.scheme) else { return .cancel }
            if url.scheme != "about", (url.host != origin.host || url.port != origin.port || url.scheme != origin.scheme) {
                if navigationAction.navigationType == .linkActivated { _ = await UIApplication.shared.open(url) }
                return .cancel
            }
            if navigationAction.targetFrame == nil, navigationAction.navigationType == .linkActivated {
                webView.load(URLRequest(url: url))
                return .cancel
            }
            return .allow
        }
    }
}
