// Spike only: the UI and the WKWebView host (iOS and macOS, same code).
import SwiftUI
import WebKit

@main
struct TSSpikeApp: App {
    @StateObject var spike = Spike.shared
    @Environment(\.scenePhase) var phase

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(spike)
                .onAppear { spike.start() }
                .onOpenURL { u in
                    var c = spike.config
                    c.apply(u)
                    c.save()
                    spike.config = c
                    // never log credentials (the probe's password, an auth key)
                    let redacted = u.absoluteString.replacingOccurrences(of: #"(pass|authKey)=[^&]*"#, with: "$1=REDACTED", options: .regularExpression)
                    spike.log("deeplink", ["url": redacted.prefix(300).description])
                    spike.start()
                }
        }
        .onChange(of: phase) { _, p in spike.scene(p) }
    }
}

struct ContentView: View {
    @EnvironmentObject var spike: Spike
    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(spike.status).font(.caption.bold())
                Spacer()
                Button("Reload") { spike.webGeneration += 1 }.font(.caption)
            }
            if let u = spike.loginURL {
                Link("Log in to the tailnet: \(u.absoluteString)", destination: u).font(.caption)
            }
            if !spike.config.url.isEmpty && (spike.config.mode == "direct" || spike.proxy != nil) {
                WebView(url: URL(string: spike.config.url)!, proxy: spike.proxy, ephemeral: spike.config.store == "ephemeral")
                    .id(spike.webGeneration)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                Spacer()
            }
            ScrollView {
                Text(spike.lines.suffix(14).joined(separator: "\n"))
                    .font(.system(size: 9, design: .monospaced))
                    .frame(maxWidth: .infinity, alignment: .leading)
            }.frame(height: 120)
        }
        .padding(6)
    }
}

final class Coordinator: NSObject, WKNavigationDelegate, WKScriptMessageHandler {
    func webView(_ webView: WKWebView, didReceive challenge: URLAuthenticationChallenge,
                 completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        MainActor.assumeIsolated {
            let (d, c) = Spike.shared.trust(challenge)
            completionHandler(d, c)
        }
    }
    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
        MainActor.assumeIsolated { Spike.shared.log("nav-start", ["url": webView.url?.absoluteString ?? ""]) }
    }
    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        MainActor.assumeIsolated { Spike.shared.log("nav-commit", ["url": webView.url?.absoluteString ?? ""]) }
    }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        MainActor.assumeIsolated { Spike.shared.log("nav-finish", ["url": webView.url?.absoluteString ?? ""]) }
    }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        MainActor.assumeIsolated {
            let e = error as NSError
            let under = (e.userInfo[NSUnderlyingErrorKey] as? NSError).map { "\($0.domain) \($0.code) \($0.userInfo.filter { $0.key != "NSErrorPeerCertificateChainKey" })" } ?? ""
            Spike.shared.log("nav-fail-provisional", ["code": e.code, "stream": "\(e.userInfo["_kCFStreamErrorCodeKey"] ?? "")", "underlying": under.prefix(1500).description, "keys": e.userInfo.keys.sorted()])
        }
    }
    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        MainActor.assumeIsolated { Spike.shared.log("nav-fail", ["error": "\(error)".prefix(400).description]) }
    }
    func userContentController(_ ucc: WKUserContentController, didReceive message: WKScriptMessage) {
        MainActor.assumeIsolated {
            let body = "\(message.body)"
            Spike.shared.log(body.hasPrefix("{\"final\"") ? "page-final" : "page-check", ["body": body.prefix(4000).description])
        }
    }
}

@MainActor
func makeWebView(url: URL, proxy: ProxyConfiguration?, ephemeral: Bool, coordinator: Coordinator) -> WKWebView {
    let store: WKWebsiteDataStore = ephemeral ? .nonPersistent() : .default()
    if let proxy { store.proxyConfigurations = [proxy] }
    let cfg = WKWebViewConfiguration()
    cfg.websiteDataStore = store
    cfg.userContentController.add(coordinator, name: "spike")
    let wv = WKWebView(frame: .zero, configuration: cfg)
    wv.navigationDelegate = coordinator
    wv.isInspectable = true
    Spike.shared.webView = wv
    Spike.shared.log("webview-load", ["url": url.absoluteString, "proxied": proxy != nil, "ephemeral": ephemeral])
    wv.load(URLRequest(url: url))
    return wv
}

#if os(iOS)
struct WebView: UIViewRepresentable {
    let url: URL; let proxy: ProxyConfiguration?; let ephemeral: Bool
    func makeCoordinator() -> Coordinator { Coordinator() }
    func makeUIView(context: Context) -> WKWebView { makeWebView(url: url, proxy: proxy, ephemeral: ephemeral, coordinator: context.coordinator) }
    func updateUIView(_ v: WKWebView, context: Context) {}
}
#else
struct WebView: NSViewRepresentable {
    let url: URL; let proxy: ProxyConfiguration?; let ephemeral: Bool
    func makeCoordinator() -> Coordinator { Coordinator() }
    func makeNSView(context: Context) -> WKWebView { makeWebView(url: url, proxy: proxy, ephemeral: ephemeral, coordinator: context.coordinator) }
    func updateNSView(_ v: WKWebView, context: Context) {}
}
#endif
