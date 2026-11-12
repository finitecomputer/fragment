// Spike only. The controller: config, the optional embedded tsnet node,
// timings and a JSONL log in the app's Documents folder (read it with
// `simctl get_app_container … data`).
import Foundation
import Network
import Security
import SwiftUI
import WebKit
import os
#if canImport(TailscaleKit)
import TailscaleKit
#endif

struct SpikeConfig: Codable {
    var url = ""
    var mode = "direct"          // direct | probe | tsnet
    var proxy = "127.0.0.1:11080" // probe: SOCKS5 host:port
    var pass = ""                // probe: SOCKS5 password (user "tsnet")
    var ca = ""                  // base64 DER of a box CA to pin (option B)
    var pin = ""                 // host suffix the pinned CA applies to
    var hostname = ""            // tsnet node name
    var authKey = ""             // optional; else interactive login URL
    var store = "ephemeral"      // ephemeral | default
    var matchDomains = ""        // comma list: proxy only these domains (empty: all)
    var control = ""             // control server URL (empty: Tailscale's)

    static let key = "spikeConfig"
    static func load() -> SpikeConfig {
        let d = UserDefaults.standard
        var c = (d.data(forKey: key)).flatMap { try? JSONDecoder().decode(SpikeConfig.self, from: $0) } ?? SpikeConfig()
        // `-url …` style launch arguments override (UserDefaults argument domain).
        for (k, apply) in [("url", { c.url = $0 }), ("mode", { c.mode = $0 }), ("proxy", { c.proxy = $0 }),
                           ("pass", { c.pass = $0 }), ("ca", { c.ca = $0 }), ("pin", { c.pin = $0 }),
                           ("hostname", { c.hostname = $0 }), ("store", { c.store = $0 }),
                           ("matchDomains", { c.matchDomains = $0 }), ("control", { c.control = $0 })] as [(String, (String) -> Void)] {
            if let v = d.volatileDomain(forName: UserDefaults.argumentDomain)[k] as? String { apply(v) }
        }
        return c
    }
    func save() { UserDefaults.standard.set(try? JSONEncoder().encode(self), forKey: Self.key) }

    // tsspike://run?url=…&mode=…  (any field; "run" relaunches the page)
    mutating func apply(_ u: URL) {
        guard let items = URLComponents(url: u, resolvingAgainstBaseURL: false)?.queryItems else { return }
        for i in items {
            let v = i.value ?? ""
            switch i.name {
            case "url": url = v
            case "mode": mode = v
            case "proxy": proxy = v
            case "pass": pass = v
            case "ca": ca = v
            case "pin": pin = v
            case "hostname": hostname = v
            case "authKey": authKey = v
            case "store": store = v
            case "matchDomains": matchDomains = v
            case "control": control = v
            default: break
            }
        }
    }
}

func processStart() -> Date {
    var info = kinfo_proc()
    var size = MemoryLayout<kinfo_proc>.stride
    var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, getpid()]
    sysctl(&mib, u_int(mib.count), &info, &size, nil, 0)
    let tv = info.kp_proc.p_un.__p_starttime
    return Date(timeIntervalSince1970: Double(tv.tv_sec) + Double(tv.tv_usec) / 1e6)
}

func footprintMB() -> Double {
    var info = task_vm_info_data_t()
    var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
    let kr = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
            task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
        }
    }
    return kr == KERN_SUCCESS ? (Double(info.phys_footprint) / 1_048_576 * 10).rounded() / 10 : -1
}

@MainActor
final class Spike: ObservableObject {
    static let shared = Spike()
    let t0 = processStart()
    let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
    let osLog = Logger(subsystem: "build.finite.tsspike", category: "spike")
    @Published var config = SpikeConfig.load()
    @Published var lines: [String] = []
    @Published var loginURL: URL?
    @Published var status = "starting"
    @Published var proxy: ProxyConfiguration?
    @Published var webGeneration = 0
    var pinned: SecCertificate?
    var backgroundedAt: Date?
    var tsnetStarting = false
    weak var webView: WKWebView?
    #if canImport(TailscaleKit)
    var node: TailscaleNode?
    #endif

    var ms: Int { Int(Date().timeIntervalSince(t0) * 1000) }

    func log(_ event: String, _ fields: [String: Any] = [:]) {
        var f = fields
        f["event"] = event
        f["ms"] = ms
        f["t"] = ISO8601DateFormatter.string(from: Date(), timeZone: .gmt, formatOptions: [.withInternetDateTime, .withFractionalSeconds])
        f["mem_mb"] = footprintMB()
        let data = (try? JSONSerialization.data(withJSONObject: f, options: [.sortedKeys])) ?? Data()
        let line = String(data: data, encoding: .utf8) ?? event
        osLog.log("SPIKE \(line, privacy: .public)")
        lines.append("\(ms)ms \(event) \(fields.isEmpty ? "" : line.prefix(160).description)")
        if lines.count > 60 { lines.removeFirst(lines.count - 60) }
        let url = docs.appendingPathComponent("spike.jsonl")
        if let h = try? FileHandle(forWritingTo: url) {
            h.seekToEndOfFile(); h.write(data + Data("\n".utf8)); try? h.close()
        } else {
            try? (data + Data("\n".utf8)).write(to: url)
        }
    }

    func start() {
        log("launch", ["mode": config.mode, "url": config.url, "pin": config.pin, "store": config.store,
                       "process_start": t0.timeIntervalSince1970])
        if !config.ca.isEmpty, let der = Data(base64Encoded: config.ca.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/").padding(toLength: (config.ca.count + 3) / 4 * 4, withPad: "=", startingAt: 0)) {
            pinned = SecCertificateCreateWithData(nil, der as CFData)
            log("pinned-ca", ["ok": pinned != nil, "subject": pinned.map { SecCertificateCopySubjectSummary($0) as String? ?? "" } ?? ""])
        }
        switch config.mode {
        case "probe":
            setProxy(hostPort: config.proxy, password: config.pass)
        case "tsnet":
            // one node per process: a deep link at launch must not start a second
            if tsnetStarting {
                if proxy != nil { webGeneration += 1 }
            } else {
                tsnetStarting = true
                Task { await startTsnet() }
            }
        default:
            proxy = nil
            status = "direct"
            webGeneration += 1
        }
    }

    func setProxy(hostPort: String, password: String) {
        let parts = hostPort.split(separator: ":")
        guard parts.count == 2, let port = UInt16(parts[1]) else { log("bad-proxy", ["proxy": hostPort]); return }
        let ep = NWEndpoint.hostPort(host: NWEndpoint.Host(String(parts[0])), port: NWEndpoint.Port(rawValue: port)!)
        var pc = ProxyConfiguration(socksv5Proxy: ep)
        pc.applyCredential(username: "tsnet", password: password)
        pc.allowFailover = false
        if !config.matchDomains.isEmpty {
            pc.matchDomains = config.matchDomains.split(separator: ",").map(String.init)
        }
        proxy = pc
        status = "proxy \(hostPort)"
        log("proxy-set", ["proxy": hostPort, "allowFailover": pc.allowFailover, "matchDomains": pc.matchDomains])
        webGeneration += 1
        Task { await nativeCheck(pc) }
    }

    // The same request outside WebKit: URLSession with the same proxy and trust.
    func nativeCheck(_ pc: ProxyConfiguration?) async {
        guard let u = URL(string: config.url), let base = URL(string: "/whoami", relativeTo: u) else { return }
        let cfg = URLSessionConfiguration.ephemeral
        if let pc { cfg.proxyConfigurations = [pc] }
        let s = URLSession(configuration: cfg, delegate: NativeTrust(), delegateQueue: nil)
        let t = Date()
        do {
            let (d, r) = try await s.data(from: base)
            log("native-check", ["ok": true, "status": (r as? HTTPURLResponse)?.statusCode ?? 0, "ms": Int(Date().timeIntervalSince(t) * 1000), "body": String(data: d, encoding: .utf8)?.prefix(300).description ?? ""])
        } catch {
            let e = error as NSError
            log("native-check", ["ok": false, "code": e.code, "underlying": (e.userInfo[NSUnderlyingErrorKey] as? NSError).map { "\($0.domain) \($0.code) \($0.userInfo["NSDescription"] ?? "")" } ?? ""])
        }
    }

    func startTsnet() async {
        #if canImport(TailscaleKit)
        // one state dir per control server, so switching never reuses a node
        let ctl = config.control.isEmpty ? kDefaultControlURL : config.control
        let tag = ctl.replacingOccurrences(of: #"[^A-Za-z0-9]+"#, with: "_", options: .regularExpression)
        let dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent(config.control.isEmpty ? "tailscale" : "tailscale-" + tag)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let goLog = docs.appendingPathComponent("go.log")
        FileManager.default.createFile(atPath: goLog.path, contents: nil)
        let fd = open(goLog.path, O_WRONLY | O_APPEND)
        #if os(iOS)
        let defaultName = "tsspike-ios"
        #else
        let defaultName = "tsspike-mac"
        #endif
        let cfg = Configuration(hostName: config.hostname.isEmpty ? defaultName : config.hostname,
                                path: dir.path, authKey: config.authKey.isEmpty ? nil : config.authKey,
                                controlURL: ctl, ephemeral: false)
        status = "tsnet starting"
        log("tsnet-new", ["control": ctl, "state": dir.lastPathComponent])
        let watcher = Task.detached { [goLog] in
            // The interactive login URL appears in tsnet's log while up() waits.
            for _ in 0..<1200 {
                if let s = try? String(contentsOf: goLog, encoding: .utf8),
                   let r = s.range(of: #"https?://[^\s"]+/(a|register)/[A-Za-z0-9_-]+"#, options: .regularExpression) {
                    let u = String(s[r])
                    await MainActor.run { Spike.shared.loginURL = URL(string: u); Spike.shared.log("login-url", ["url": u]) }
                    return
                }
                try? await Task.sleep(nanoseconds: 250_000_000)
            }
        }
        do {
            let n = try TailscaleNode(config: cfg, logger: GoLog(fd: fd))
            node = n
            log("tsnet-started")
            try await n.up()
            watcher.cancel()
            loginURL = nil
            let ips = try await n.addrs()
            log("tsnet-up", ["ip4": ips.ip4 ?? "", "ip6": ips.ip6 ?? ""])
            let lb = try await n.loopback()
            log("tsnet-loopback", ["address": lb.address])
            setProxy(hostPort: lb.address, password: lb.proxyCredential)
            status = "tsnet \(ips.ip4 ?? "?")"
        } catch {
            log("tsnet-error", ["error": "\(error)"])
            status = "tsnet error"
        }
        #else
        log("tsnet-unavailable")
        status = "no TailscaleKit in this build"
        #endif
    }

    func scene(_ phase: ScenePhase) {
        switch phase {
        case .background:
            backgroundedAt = Date()
            log("background")
        case .active:
            guard let b = backgroundedAt else { return }
            backgroundedAt = nil
            log("foreground", ["away_s": Int(Date().timeIntervalSince(b))])
            Task { await probeAfterResume() }
        default: break
        }
    }

    // After a resume: how long until the page can reach the box again?
    func probeAfterResume() async {
        guard let wv = webView else { return }
        let start = Date()
        for attempt in 1...60 {
            let js = "const c = new AbortController(); setTimeout(() => c.abort(), 5000); const r = await fetch('/whoami', {cache: 'no-store', signal: c.signal}); return (await r.json()).remote;"
            do {
                let v = try await wv.callAsyncJavaScript(js, contentWorld: .page)
                log("resume-probe-ok", ["attempt": attempt, "ms_after_resume": Int(Date().timeIntervalSince(start) * 1000), "remote": "\(v ?? "")"])
                return
            } catch {
                log("resume-probe-fail", ["attempt": attempt, "error": "\(error)".prefix(200).description])
                try? await Task.sleep(nanoseconds: 500_000_000)
            }
        }
    }

    // Option B: the pinned CA, only for the box's names.
    func trust(_ challenge: URLAuthenticationChallenge) -> (URLSession.AuthChallengeDisposition, URLCredential?) {
        let ps = challenge.protectionSpace
        guard ps.authenticationMethod == NSURLAuthenticationMethodServerTrust, let st = ps.serverTrust else {
            log("challenge-other", ["host": ps.host, "method": ps.authenticationMethod])
            return (.performDefaultHandling, nil)
        }
        let host = ps.host.lowercased()
        let pin = config.pin.lowercased()
        guard let ca = pinned, !pin.isEmpty, host == pin || host.hasSuffix("." + pin) else {
            log("challenge-default", ["host": host, "port": ps.port])
            return (.performDefaultHandling, nil)
        }
        SecTrustSetAnchorCertificates(st, [ca] as CFArray)
        SecTrustSetAnchorCertificatesOnly(st, true)
        var err: CFError?
        let ok = SecTrustEvaluateWithError(st, &err)
        log("challenge-pinned", ["host": host, "port": ps.port, "ok": ok, "error": err.map { "\($0)" } ?? ""])
        return ok ? (.useCredential, URLCredential(trust: st)) : (.cancelAuthenticationChallenge, nil)
    }
}

final class NativeTrust: NSObject, URLSessionDelegate {
    func urlSession(_ session: URLSession, didReceive challenge: URLAuthenticationChallenge) async -> (URLSession.AuthChallengeDisposition, URLCredential?) {
        await MainActor.run { Spike.shared.trust(challenge) }
    }
}

#if canImport(TailscaleKit)
struct GoLog: LogSink {
    let fd: Int32
    var logFileHandle: Int32? { fd }
    func log(_ message: String) {
        Task { @MainActor in Spike.shared.log("tskit", ["msg": message.prefix(300).description]) }
    }
}
#endif
