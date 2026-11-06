import SwiftUI

private struct FragmentAPIKey: EnvironmentKey { static let defaultValue: FragmentAPI? = nil }
extension EnvironmentValues {
    var fragmentAPI: FragmentAPI? {
        get { self[FragmentAPIKey.self] }
        set { self[FragmentAPIKey.self] = newValue }
    }
}

/// Every picture the app shows, read once a session and kept by its URL: a
/// listed picture's URL names its version (`?v=<hash>`, docs/api.md), so a
/// new picture is a new URL and one kept is never stale. Pictures shown at
/// once share one read (`loading`). Bounded by `costMax` decoded bytes;
/// past it the system lets the least used go, and they are read again.
/// Emptied when the account changes (`FragmentRoot.resetNavigation`).
@MainActor
final class ImageCache {
    static let shared = ImageCache()
    static let costMax = 64 * 1_024 * 1_024
    /// A picture larger than this is never one the app shows.
    static let bytesMax = 2 * 1_024 * 1_024

    private let images: NSCache<NSURL, UIImage> = {
        let cache = NSCache<NSURL, UIImage>()
        cache.totalCostLimit = ImageCache.costMax
        return cache
    }()
    private var loading: [URL: Task<UIImage?, Never>] = [:]
    /// Reads made since it was last emptied: one per picture.
    private(set) var reads = 0

    func cached(_ url: URL) -> UIImage? { images.object(forKey: url as NSURL) }

    func image(_ url: URL, api: FragmentAPI?) async -> UIImage? {
        if let image = cached(url) { return image }
        if let task = loading[url] { return await task.value }
        reads += 1
        let task = Task<UIImage?, Never> {
            do {
                let request = URLRequest(url: url)
                let data: Data
                let response: URLResponse
                if let api, PlatformOrigin.same(api.baseURL, url) { (data, response) = try await api.data(for: request) }
                else { (data, response) = try await URLSession.shared.data(for: request) }
                guard (response as? HTTPURLResponse)?.statusCode == 200, data.count <= Self.bytesMax else { return nil }
                return UIImage(data: data)
            } catch { return nil }
        }
        loading[url] = task
        let image = await task.value
        // emptied meanwhile (another account): what it read is not kept
        guard loading[url] == task else { return image }
        loading[url] = nil
        if let image {
            let pixels = image.size.width * image.scale * image.size.height * image.scale
            images.setObject(image, forKey: url as NSURL, cost: Int(pixels) * 4)
        }
        return image
    }

    func removeAll() {
        images.removeAllObjects()
        for task in loading.values { task.cancel() }
        loading.removeAll()
        reads = 0
    }
}

struct AuthenticatedImage<Content: View>: View {
    let url: URL
    @ViewBuilder let content: (Image?) -> Content
    @Environment(\.fragmentAPI) private var api
    @State private var loaded: (url: URL, image: UIImage)?

    var body: some View {
        // one already read is drawn in its first frame, never a placeholder first
        let image = loaded?.url == url ? loaded?.image : ImageCache.shared.cached(url)
        content(image.map { Image(uiImage: $0) })
            .task(id: url) {
                // held here too, so the cache letting it go never blanks one shown;
                // one that cannot be read leaves the caller's placeholder
                if let image = await ImageCache.shared.image(url, api: api), loaded?.image !== image { loaded = (url, image) }
            }
    }
}
