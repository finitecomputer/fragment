import SwiftUI
import WebKit

struct FragmentWebSheet: View {
    @Environment(\.dismiss) private var dismiss
    let fragment: FragmentSummary
    let api: FragmentAPI
    @State private var url: URL?
    @State private var error: String?
    @State private var loading = true
    @State private var needsConsent = false
    @State private var consentEmail: String?

    var body: some View {
        NavigationStack {
            Group {
                if needsConsent {
                    ContentUnavailableView {
                        Label("Open \(fragment.displayTitle)?", systemImage: "person.crop.circle.badge.checkmark")
                    } description: {
                        Text("Continue as \(consentEmail ?? "your account")? This app will be able to recognize you.")
                    } actions: {
                        Button("Continue") { Task { await load(consent: true) } }
                        Button("Cancel", role: .cancel) { dismiss() }
                    }
                } else if let error {
                    ContentUnavailableView {
                        Label("Couldn’t open this app", systemImage: "wifi.exclamationmark")
                    } description: { Text(error) } actions: {
                        Button("Try again") { Task { await load() } }
                    }
                } else if let url {
                    WebContent(url: url, loading: $loading, error: $error)
                        .overlay { if loading { ProgressView() } }
                } else { ProgressView() }
            }
            .navigationTitle(fragment.displayTitle).navigationBarTitleDisplayMode(.inline)
            .task { await load() }
        }
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        .accessibilityAction(.escape) { dismiss() }
    }

    private func load(consent: Bool = false) async {
        error = nil; needsConsent = false
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
            guard let destination = components?.url else { throw APIError.invalidResponse }
            struct Session: Decodable { let consentRequired: Bool; var url: URL?; var email: String? }
            let back = components!.percentEncodedPath + (components!.percentEncodedQuery.map { "?" + $0 } ?? "")
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

private struct WebContent: UIViewRepresentable {
    let url: URL
    @Binding var loading: Bool
    @Binding var error: String?

    func makeCoordinator() -> Coordinator { Coordinator(loading: $loading, error: $error, origin: url) }
    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        // Each sheet has its own web storage. Native credentials never enter a
        // fragment page; connected accounts use a scoped redemption (docs/ios.md).
        configuration.websiteDataStore = .nonPersistent()
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
