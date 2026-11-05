import SwiftUI

private struct FragmentAPIKey: EnvironmentKey { static let defaultValue: FragmentAPI? = nil }
extension EnvironmentValues {
    var fragmentAPI: FragmentAPI? {
        get { self[FragmentAPIKey.self] }
        set { self[FragmentAPIKey.self] = newValue }
    }
}

struct AuthenticatedImage<Content: View>: View {
    let url: URL
    @ViewBuilder let content: (Image?) -> Content
    @Environment(\.fragmentAPI) private var api
    @State private var image: UIImage?
    var body: some View {
        content(image.map { Image(uiImage: $0) })
            .task(id: url) {
                image = nil
                do {
                    let request = URLRequest(url: url)
                    let data: Data
                    let response: URLResponse
                    if let api, PlatformOrigin.same(api.baseURL, url) { (data, response) = try await api.data(for: request) }
                    else { (data, response) = try await URLSession.shared.data(for: request) }
                    guard (response as? HTTPURLResponse)?.statusCode == 200, data.count <= 2 * 1_024 * 1_024 else { return }
                    image = UIImage(data: data)
                } catch { /* The caller's placeholder remains when an icon is unavailable. */ }
            }
    }
}
