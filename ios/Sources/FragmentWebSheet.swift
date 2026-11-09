import SwiftUI
import WebKit

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


/// Where a navigation in a fragment's sheet goes. The sheet holds that
/// fragment's pages alone; everything else leaves it, and nothing goes
/// silently nowhere (docs/ios.md).
enum WebLinks {
    enum Route: Equatable {
        /// A page of this fragment: it stays in the sheet.
        case stay
        /// The platform's own page, where a page goes when its sign-in has
        /// ended (a fragment's refusal sends it there): the sheet says so.
        case signedOut
        /// Any other address: a fragment of the person's opens in the app,
        /// anything else in the system's browser.
        case leave
    }
    static func route(_ url: URL, fragment: URL, platform: URL, clicked: Bool) -> Route {
        if url.scheme == "about" || PlatformOrigin.same(url, fragment) { return .stay }
        if !clicked, PlatformOrigin.same(url, platform) { return .signedOut }
        return .leave
    }

    /// The address of `page` that may be shared: this fragment's page alone,
    /// with nothing that is the person's. No query and no hash (a session
    /// key `s=`, a redemption's token, or anything else a page put there),
    /// no user or password, and the redemption itself is the app's front.
    static func shareable(_ page: URL, fragment: URL) -> URL? {
        guard PlatformOrigin.same(page, fragment), var parts = URLComponents(url: page, resolvingAgainstBaseURL: false) else { return nil }
        parts.query = nil; parts.fragment = nil; parts.user = nil; parts.password = nil
        if parts.path.hasSuffix("/__signin") || parts.path.isEmpty { parts.path = "/" }
        return parts.url
    }
}

/// The page a sheet shows, for its toolbar: the web view, and the address
/// it is at now (a page's own history changes count).
@MainActor @Observable
final class WebPage {
    @ObservationIgnored weak var view: WKWebView?
    var url: URL?
}

struct FragmentWebSheet: View {
    @Environment(\.dismiss) private var dismiss
    let fragment: FragmentSummary
    /// Opens another fragment of the person's from a link to it, as a link in
    /// a chat opens one (`Library.fragment(linkedBy:)`); false if it is none.
    let openLink: (URL) -> Bool
    @State private var model: WebSheetModel
    @State private var page = WebPage()
    init(fragment: FragmentSummary, api: FragmentAPI, openLink: @escaping (URL) -> Bool = { _ in false }) {
        self.fragment = fragment
        self.openLink = openLink
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
                    // a new redemption is a new web view: one never reloads a spent one
                    WebContent(url: url, platform: model.api.baseURL, page: page, openLink: openLink, loading: $model.loading, error: $model.error)
                        .id(url)
                        .overlay { if model.loading { ProgressView() } }
                } else { ProgressView() }
            }
            .navigationTitle(fragment.displayTitle).navigationBarTitleDisplayMode(.inline)
            .toolbar {
                if let url = model.url, model.error == nil, !model.needsConsent {
                    ToolbarItemGroup(placement: .topBarTrailing) {
                        Button("Reload", systemImage: "arrow.clockwise") { reload() }.accessibilityIdentifier("reloadApp")
                        if let shared = page.url.flatMap({ WebLinks.shareable($0, fragment: url) }) {
                            // titled by the app: the system's own look at the address would
                            // see a page for someone signed out (a sign-in's title)
                            ShareLink(item: shared, preview: SharePreview(fragment.displayTitle)) {
                                Label("Share", systemImage: "square.and.arrow.up")
                            }.accessibilityIdentifier("shareApp")
                        }
                    }
                }
            }
            .task { await model.load() }
        }
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        .accessibilityAction(.escape) { dismiss() }
    }

    /// The page it is at, again; never the spent redemption (a page not yet
    /// past it asks for a new one).
    private func reload() {
        if let web = page.view, let at = web.url, !at.path.hasSuffix("/__signin") { web.reload() }
        else { Task { await model.load() } }
    }
}

private struct WebContent: UIViewRepresentable {
    let url: URL
    let platform: URL
    let page: WebPage
    let openLink: (URL) -> Bool
    @Binding var loading: Bool
    @Binding var error: String?

    func makeCoordinator() -> Coordinator {
        Coordinator(origin: url, platform: platform, page: page, openLink: openLink, loading: $loading, error: $error)
    }
    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        // The app's own web data, kept as Safari keeps an origin's, so a page
        // has what it saved the next time it opens; cleared whenever the
        // person changes (`AccountConnection`). Native credentials never
        // enter a fragment page; it signs in by a scoped redemption (docs/ios.md).
        configuration.websiteDataStore = .default()
        let web = WKWebView(frame: .zero, configuration: configuration)
        web.navigationDelegate = context.coordinator
        web.uiDelegate = context.coordinator
        web.allowsBackForwardNavigationGestures = true
        context.coordinator.watch(web)
        web.load(URLRequest(url: url))
        return web
    }
    func updateUIView(_ uiView: WKWebView, context: Context) {}

    @MainActor final class Coordinator: NSObject, WKNavigationDelegate, WKUIDelegate {
        let origin: URL
        let platform: URL
        let page: WebPage
        let openLink: (URL) -> Bool
        @Binding var loading: Bool
        @Binding var error: String?
        private var watching: NSKeyValueObservation?
        init(origin: URL, platform: URL, page: WebPage, openLink: @escaping (URL) -> Bool, loading: Binding<Bool>, error: Binding<String?>) {
            self.origin = origin; self.platform = platform; self.page = page; self.openLink = openLink
            _loading = loading; _error = error
        }
        /// The toolbar's page: this web view, and the address it is at.
        func watch(_ web: WKWebView) {
            page.view = web
            watching = web.observe(\.url, options: [.initial, .new]) { [weak self] web, _ in
                MainActor.assumeIsolated { self?.page.url = web.url }
            }
        }
        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { loading = false }
        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
            self.error = error.localizedDescription; loading = false
        }
        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
            if (error as NSError).code == NSURLErrorCancelled { return }   // one the sheet sent elsewhere
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
        func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction) async -> WKNavigationActionPolicy {
            guard let url = action.request.url else { return .cancel }
            let main = action.targetFrame?.isMainFrame ?? true
            let clicked = action.navigationType == .linkActivated
            switch WebLinks.route(url, fragment: origin, platform: platform, clicked: clicked) {
            case .stay:
                // a new window of its own (target=_blank) opens in its place
                if action.targetFrame == nil { webView.load(URLRequest(url: url)); return .cancel }
                return .allow
            case .signedOut where main:
                error = "This app’s sign-in ended. Try again to open it."
                loading = false
            case .signedOut, .leave:
                // another site framed in a page stays out, as before; a page of
                // its own, or a click, leaves the sheet as a person would expect
                if main || clicked { leave(url) }
            }
            return .cancel
        }
        /// `window.open`: no new web view; where it goes is decided as a link's.
        func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                     for action: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
            guard let url = action.request.url else { return nil }
            if WebLinks.route(url, fragment: origin, platform: platform, clicked: true) == .stay { webView.load(URLRequest(url: url)) }
            else { leave(url) }
            return nil
        }
        /// Out of the sheet: a fragment of the person's opens in the app, any
        /// other address in the system's own browser (or app, for `mailto:`).
        private func leave(_ url: URL) {
            if ["http", "https"].contains(url.scheme), openLink(url) { return }
            UIApplication.shared.open(url)
        }
    }
}
