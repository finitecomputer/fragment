import SwiftUI
import ImageIO
import PDFKit
import QuickLook
import UniformTypeIdentifiers

/// A message's files, as Messages shows them: pictures inline (several in a
/// grid), sounds as players, a PDF as its first page, any other file as a
/// row. The person's own are at the trailing edge, as their words are.
/// Every size here is fixed before its file is read, or once from what is
/// kept, so a row's height settles once (FIN-419).
struct MessageAttachments: View {
    let attachments: [ChatAttachment]
    let isYou: Bool
    let api: FragmentAPI
    let fragment: String

    var body: some View {
        let pictures = attachments.filter { AttachmentKind($0) == .image }
        VStack(alignment: isYou ? .trailing : .leading, spacing: 8) {
            if !pictures.isEmpty { PictureGroup(pictures: pictures, api: api, fragment: fragment) }
            ForEach(attachments.filter { AttachmentKind($0) != .image }) { attachment in
                switch AttachmentKind(attachment) {
                case .voice, .audio: SoundPlayer(attachment: attachment, isYou: isYou, api: api, fragment: fragment)
                case .pdf: PDFCard(attachment: attachment, api: api, fragment: fragment)
                case .image, .file: FileRow(attachment: attachment, api: api, fragment: fragment)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: isYou ? .trailing : .leading)
    }
}

/// The width every attachment but a picture takes: one column of them.
enum AttachmentWidth { static let card: CGFloat = 260 }

/// The sizes shown before a file is read again: a picture's, once read on
/// this iPhone (the record names none), and a PDF's pages.
@MainActor
final class ShownFacts {
    static let shared = ShownFacts()
    static let keptMax = 4_000
    private var sizes: [String: CGSize] = [:]
    private var pages: [String: Int] = [:]
    func size(_ id: String) -> CGSize? { sizes[id] }
    func pages(_ id: String) -> Int? { pages[id] }
    func keep(size: CGSize, for id: String) { if sizes.count >= Self.keptMax { sizes.removeAll() }; sizes[id] = size }
    func keep(pages count: Int, for id: String) { if pages.count >= Self.keptMax { pages.removeAll() }; pages[id] = count }
    func removeAll() { sizes.removeAll(); pages.removeAll() }
}

/// Pictures made from a file on this iPhone, at most `pixels` on a side,
/// decoded at that size (never whole), turned as their metadata says.
nonisolated enum PictureFile {
    static func image(_ file: URL, pixels: Int) -> UIImage? {
        guard let source = CGImageSourceCreateWithURL(file as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary),
              let image = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                kCGImageSourceCreateThumbnailFromImageAlways: true, kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceThumbnailMaxPixelSize: pixels, kCGImageSourceShouldCacheImmediately: true,
              ] as CFDictionary) else { return nil }
        return UIImage(cgImage: image)
    }
}

/// How much of a file is in or out, as a ring that fills: iOS draws a
/// circular ProgressView as a spinner whatever its value.
struct ProgressRing: View {
    let fraction: Double
    let label: String
    var body: some View {
        ZStack {
            Circle().stroke(.tertiary, lineWidth: 3).opacity(0.5)
            Circle().trim(from: 0, to: min(1, max(0.03, fraction)))
                .stroke(style: StrokeStyle(lineWidth: 3, lineCap: .round))
                .rotationEffect(.degrees(-90))
        }
        .frame(width: 22, height: 22)
        .animation(.linear(duration: 0.2), value: fraction)
        .accessibilityElement()
        .accessibilityLabel(label)
        .accessibilityValue("\(Int((min(1, fraction) * 100).rounded())) percent")
    }
}

/// How a file not read yet shows its state: a spinner (with its share in,
/// for a large one), a tap to read one too large to read by itself, or why
/// it failed, with a retry.
private struct LoadState: View {
    let load: BlobLoad
    let attachment: ChatAttachment
    var body: some View {
        switch load.state {
        case .loading(let fraction):
            if attachment.size > 2 * 1_024 * 1_024, fraction > 0 {
                ProgressRing(fraction: fraction, label: "Downloading")
            } else { ProgressView().accessibilityLabel("Downloading") }
        case .failed(let why):
            Button { load.begin() } label: {
                Label("Retry", systemImage: "arrow.clockwise").font(.footnote.weight(.semibold))
            }
            .buttonStyle(.bordered).buttonBorderShape(.capsule).controlSize(.small)
            .accessibilityHint(why)
        case .idle:
            Button { load.begin() } label: {
                Label(attachment.shownSize, systemImage: "arrow.down.circle").font(.footnote.weight(.semibold))
            }
            .buttonStyle(.bordered).buttonBorderShape(.capsule).controlSize(.small)
            .accessibilityLabel("Download, \(attachment.shownSize)")
        case .ready: EmptyView()
        }
    }
}

// MARK: Pictures

private struct PictureGroup: View {
    let pictures: [ChatAttachment]
    let api: FragmentAPI
    let fragment: String
    @State private var shown: Int?
    static let single = CGSize(width: 260, height: 300)
    static let tile: CGFloat = 128

    var body: some View {
        Group {
            if pictures.count == 1 {
                Picture(attachment: pictures[0], api: api, fragment: fragment, box: Self.single, fills: false) { shown = 0 }
            } else {
                Grid(horizontalSpacing: 4, verticalSpacing: 4) {
                    ForEach(Array(stride(from: 0, to: pictures.count, by: 2)), id: \.self) { row in
                        GridRow {
                            // a last one alone spans its row, so the group stays a rectangle
                            let alone = row + 1 == pictures.count
                            ForEach(row..<min(row + 2, pictures.count), id: \.self) { index in
                                Picture(attachment: pictures[index], api: api, fragment: fragment,
                                        box: CGSize(width: alone ? Self.tile * 2 + 4 : Self.tile, height: Self.tile), fills: true) { shown = index }
                                    .gridCellColumns(alone ? 2 : 1)
                            }
                        }
                    }
                }
                .clipShape(RoundedRectangle(cornerRadius: 18))
            }
        }
        .fullScreenCover(item: Binding(get: { shown.map { ShownIndex(index: $0) } }, set: { shown = $0?.index })) { start in
            PictureViewer(pictures: pictures, api: api, fragment: fragment, index: start.index)
        }
    }
    private struct ShownIndex: Identifiable { let index: Int; var id: Int { index } }
}

/// One picture inline: within `box` at its own aspect ratio (once known),
/// or filling a tile of a group's grid.
private struct Picture: View {
    let attachment: ChatAttachment
    let api: FragmentAPI
    let fragment: String
    let box: CGSize
    let fills: Bool
    let open: () -> Void
    @State private var load: BlobLoad?
    @State private var image: UIImage?
    static let pixels = 720

    var body: some View {
        let key = BlobKey(api: api, fragment: fragment, sha256: attachment.sha256)
        let shown = image ?? ImageCache.shared.cached(key.imageKey(Self.pixels))
        let frame = fills ? box : Self.fitted(ShownFacts.shared.size(attachment.sha256) ?? shown?.size, in: box)
        ZStack {
            if let shown {
                Image(uiImage: shown).resizable().scaledToFill()
                    .frame(width: frame.width, height: frame.height).clipped()
            } else {
                Rectangle().fill(FragmentStyle.surface)
                if let load { LoadState(load: load, attachment: attachment) }
            }
        }
        .frame(width: frame.width, height: frame.height)
        .clipShape(RoundedRectangle(cornerRadius: fills ? 0 : 18))
        .contentShape(Rectangle())
        .onTapGesture { if shown != nil { open() } else { load?.begin() } }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Picture")
        .accessibilityAddTraits(.isButton)
        .accessibilityIdentifier("picture-\(attachment.sha256.prefix(8))")
        .contextMenu { if let file = load?.file { ShareMenu(attachment: attachment, file: file) } }
        .task(id: attachment.sha256) {
            load = BlobStore.shared.load(attachment, api: api, fragment: fragment, begin: shown == nil && attachment.size <= BlobStore.previewBytesMax)
        }
        .task(id: load?.file) {
            guard image == nil, let file = load?.file else { return }
            let pixels = Self.pixels
            let made = await ImageCache.shared.image(key.imageKey(pixels)) {
                await Task.detached(priority: .userInitiated) { PictureFile.image(file, pixels: pixels) }.value
            }
            if let made { ShownFacts.shared.keep(size: made.size, for: attachment.sha256); image = made }
        }
    }

    /// A picture's size within `box` at its aspect ratio; a square box's
    /// height before it is known.
    static func fitted(_ size: CGSize?, in box: CGSize) -> CGSize {
        guard let size, size.width > 0, size.height > 0 else { return CGSize(width: box.width, height: box.width * 0.75) }
        let scale = min(box.width / size.width, box.height / size.height)
        // a narrow one still shows as more than a sliver
        return CGSize(width: max(96, (size.width * scale).rounded()), height: max(72, (size.height * scale).rounded()))
    }
}

/// The pictures full screen, one at a time, swiped between: pinched or
/// double-tapped to zoom, swiped down to close, and shared.
struct PictureViewer: View {
    let pictures: [ChatAttachment]
    let api: FragmentAPI
    let fragment: String
    @State var index: Int
    @State private var drag: CGFloat = 0
    @State private var zoomed = false
    @State private var files: [String: URL] = [:]
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        let fade = max(0.2, 1 - abs(drag) / 400)
        ZStack {
            Color.black.opacity(fade).ignoresSafeArea()
            TabView(selection: $index) {
                ForEach(Array(pictures.enumerated()), id: \.offset) { offset, picture in
                    FullPicture(attachment: picture, api: api, fragment: fragment, zoomed: $zoomed) { files[picture.sha256] = $0 }
                        .tag(offset)
                }
            }
            .tabViewStyle(.page(indexDisplayMode: pictures.count > 1 ? .automatic : .never))
            .offset(y: drag)
            .ignoresSafeArea()
            .simultaneousGesture(DragGesture(minimumDistance: 12).onChanged { value in
                guard !zoomed, abs(value.translation.height) > abs(value.translation.width) else { return }
                drag = value.translation.height
            }.onEnded { value in
                guard !zoomed, drag != 0 else { return }
                if abs(value.predictedEndTranslation.height) > 220 || abs(drag) > 140 { dismiss() }
                else { withAnimation(.snappy(duration: 0.25)) { drag = 0 } }
            })
        }
        .overlay(alignment: .top) {
            HStack {
                Button { dismiss() } label: { Image(systemName: "xmark").frame(width: 24, height: 24) }
                    .accessibilityLabel("Close").accessibilityIdentifier("closePicture")
                Spacer()
                if pictures.indices.contains(index), let file = files[pictures[index].sha256],
                   let named = try? BlobStore.shared.named(pictures[index], file: file) {
                    ShareLink(item: named) { Image(systemName: "square.and.arrow.up").frame(width: 24, height: 24) }
                        .accessibilityLabel("Share").accessibilityIdentifier("sharePicture")
                }
            }
            .font(.title3).buttonStyle(.glass).buttonBorderShape(.circle).controlSize(.large)
            .padding(.horizontal, 16).padding(.top, 8)
            .opacity(drag == 0 ? 1 : 0)
        }
        .statusBarHidden()
        .preferredColorScheme(.dark)
    }
}

private struct FullPicture: View {
    let attachment: ChatAttachment
    let api: FragmentAPI
    let fragment: String
    @Binding var zoomed: Bool
    let ready: (URL) -> Void
    @State private var load: BlobLoad?
    @State private var image: UIImage?

    var body: some View {
        ZStack {
            if let image { ZoomingImage(image: image, zoomed: $zoomed) }
            else if let load { LoadState(load: load, attachment: attachment).tint(.white) }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .task {
            let load = BlobStore.shared.load(attachment, api: api, fragment: fragment, begin: true)
            self.load = load
        }
        .task(id: load?.file) {
            guard let file = load?.file else { return }
            ready(file)
            // the screen's size at its scale, twice for zooming in; held by this page alone
            let pixels = 2_400
            image = await Task.detached(priority: .userInitiated) { PictureFile.image(file, pixels: pixels) }.value
        }
    }
}

/// A picture in a scroll view that zooms it: pinched, or double-tapped,
/// kept centered while it is smaller than the screen.
private struct ZoomingImage: UIViewRepresentable {
    let image: UIImage
    @Binding var zoomed: Bool

    func makeUIView(context: Context) -> ZoomScroll {
        let scroll = ZoomScroll()
        scroll.delegate = context.coordinator
        scroll.picture.image = image
        let tap = UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.doubleTapped(_:)))
        tap.numberOfTapsRequired = 2
        scroll.addGestureRecognizer(tap)
        return scroll
    }
    func updateUIView(_ scroll: ZoomScroll, context: Context) {
        context.coordinator.zoomed = $zoomed
        if scroll.picture.image !== image { scroll.picture.image = image; scroll.setNeedsLayout() }
    }
    func makeCoordinator() -> Coordinator { Coordinator(zoomed: $zoomed) }

    @MainActor final class Coordinator: NSObject, UIScrollViewDelegate {
        var zoomed: Binding<Bool>
        init(zoomed: Binding<Bool>) { self.zoomed = zoomed }
        func viewForZooming(in scrollView: UIScrollView) -> UIView? { (scrollView as? ZoomScroll)?.picture }
        func scrollViewDidZoom(_ scrollView: UIScrollView) {
            (scrollView as? ZoomScroll)?.center()
            let now = scrollView.zoomScale > scrollView.minimumZoomScale + 0.01
            if zoomed.wrappedValue != now { zoomed.wrappedValue = now }
        }
        @objc func doubleTapped(_ tap: UITapGestureRecognizer) {
            guard let scroll = tap.view as? ZoomScroll else { return }
            if scroll.zoomScale > scroll.minimumZoomScale { scroll.setZoomScale(scroll.minimumZoomScale, animated: true); return }
            let point = tap.location(in: scroll.picture)
            let size = CGSize(width: scroll.bounds.width / 2.5, height: scroll.bounds.height / 2.5)
            scroll.zoom(to: CGRect(origin: CGPoint(x: point.x - size.width / 2, y: point.y - size.height / 2), size: size), animated: true)
        }
    }
}

/// The scroll view `ZoomingImage` zooms in: its picture fitted to it unzoomed.
final class ZoomScroll: UIScrollView {
    let picture = UIImageView()
    private var fitted: CGSize = .zero

    override init(frame: CGRect) {
        super.init(frame: frame)
        minimumZoomScale = 1
        maximumZoomScale = 5
        showsVerticalScrollIndicator = false
        showsHorizontalScrollIndicator = false
        contentInsetAdjustmentBehavior = .never
        backgroundColor = .clear
        picture.isAccessibilityElement = true
        picture.accessibilityLabel = "Picture"
        addSubview(picture)
    }
    required init?(coder: NSCoder) { nil }

    override func layoutSubviews() {
        super.layoutSubviews()
        guard let image = picture.image, image.size.width > 0, image.size.height > 0, bounds.width > 0 else { return }
        let scale = min(bounds.width / image.size.width, bounds.height / image.size.height)
        let size = CGSize(width: (image.size.width * scale).rounded(), height: (image.size.height * scale).rounded())
        if size != fitted, zoomScale == minimumZoomScale {
            fitted = size
            picture.frame = CGRect(origin: .zero, size: size)
            contentSize = size
        }
        center()
    }

    func center() {
        let x = max(0, (bounds.width - contentSize.width) / 2), y = max(0, (bounds.height - contentSize.height) / 2)
        let inset = UIEdgeInsets(top: y, left: x, bottom: y, right: x)
        if contentInset != inset { contentInset = inset }
    }
}

// MARK: Sounds

/// A sound's player: play and pause, its level bars (dragged to move its
/// place), and its time. A voice memo shows no name; any other sound its
/// name, small.
struct SoundPlayer: View {
    let attachment: ChatAttachment
    let isYou: Bool
    let api: FragmentAPI
    let fragment: String
    @State private var load: BlobLoad?
    @State private var playable: URL?
    @State private var unplayable = false
    @State private var facts: SoundFacts.Facts?
    @State private var scrub: Double?
    static let width = AttachmentWidth.card

    var body: some View {
        let playback = AudioPlayback.shared
        let id = attachment.sha256
        let place = playback.place(id)
        let duration = facts?.duration ?? (place != nil ? playback.duration : 0)
        let elapsed = scrub.map { $0 * duration } ?? place?.elapsed ?? 0
        let fraction = duration > 0 ? min(1, elapsed / duration) : 0
        Group {
            if unplayable {
                FileRow(attachment: attachment, api: api, fragment: fragment, note: "This sound can’t play here.")
            } else {
                VStack(alignment: .leading, spacing: 6) {
                    HStack(spacing: 10) {
                        Button {
                            if let playable { playback.toggle(id, url: playable) } else { load?.begin() }
                        } label: {
                            ZStack {
                                Circle().fill(isYou ? Color.primary.opacity(0.12) : FragmentStyle.lilac.opacity(0.22))
                                if playable == nil, let load, case .loading = load.state { ProgressView().controlSize(.small) }
                                else if playable == nil, let load, case .failed = load.state { Image(systemName: "arrow.clockwise").font(.callout.weight(.semibold)) }
                                else { Image(systemName: place?.playing == true ? "pause.fill" : "play.fill").font(.callout.weight(.semibold)) }
                            }
                            .frame(width: 36, height: 36)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel(place?.playing == true ? "Pause" : "Play")
                        .accessibilityIdentifier("play-\(id.prefix(8))")
                        LevelBars(levels: facts?.levels ?? [], fraction: fraction)
                            .frame(height: 28)
                            .contentShape(Rectangle())
                            // a sideways drag moves its place; one up or down scrolls the chat
                            .gesture(SidewaysDrag { x in
                                guard playable != nil, duration > 0 else { return }
                                scrub = max(0, min(1, x / Self.barsWidth))
                            } ended: {
                                if let scrub, let playable { playback.seek(id, url: playable, to: scrub, of: duration) }
                                scrub = nil
                            })
                            .accessibilityElement()
                            .accessibilityLabel("Position")
                            .accessibilityValue("\(Self.time(elapsed)) of \(Self.time(duration))")
                            .accessibilityAdjustableAction { direction in
                                guard let playable, duration > 0 else { return }
                                let step = 5 / duration
                                playback.seek(id, url: playable, to: fraction + (direction == .increment ? step : -step), of: duration)
                            }
                        Text(place != nil || scrub != nil ? Self.time(elapsed) : Self.time(duration))
                            .font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                            .frame(width: 38, alignment: .trailing)
                            .accessibilityIdentifier("time-\(id.prefix(8))")
                    }
                    if AttachmentKind(attachment) == .audio {
                        Text(attachment.name).font(.caption2).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                            .padding(.leading, 46)
                    }
                }
                .padding(.vertical, 8).padding(.horizontal, 10)
                .frame(width: Self.width, alignment: .leading)
                .background(isYou ? FragmentStyle.bubble : FragmentStyle.surface, in: RoundedRectangle(cornerRadius: 20))
                .contextMenu { if let file = load?.file { ShareMenu(attachment: attachment, file: file) } }
            }
        }
        .task(id: id) {
            facts = SoundFacts.shared.cached(id)
            let load = BlobStore.shared.load(attachment, api: api, fragment: fragment, begin: attachment.size <= BlobStore.previewBytesMax)
            self.load = load
        }
        .task(id: load?.file) {
            guard playable == nil, let file = load?.file else { return }
            guard let url = await BlobStore.shared.playable(attachment, file: file) else { unplayable = true; return }
            let read = await SoundFacts.shared.facts(id, url: url)
            // a file AVFoundation cannot read is no sound to play here
            guard read != nil else { unplayable = true; return }
            facts = read
            playable = url
        }
    }

    /// The bars' width: the player's less its button, time and spacing.
    static let barsWidth: CGFloat = width - 20 - 36 - 38 - 20

    static func time(_ seconds: Double) -> String {
        let whole = Int(max(0, seconds.isFinite ? seconds : 0).rounded(.down))
        return whole >= 3_600 ? String(format: "%d:%02d:%02d", whole / 3_600, whole / 60 % 60, whole % 60) : String(format: "%d:%02d", whole / 60, whole % 60)
    }
}

/// A drag that begins only sideways (a pan more across than up or down), so
/// a chat scrolled over a player scrolls, and a player dragged across moves
/// its place: `moved` hears the finger's x in the view's own space.
struct SidewaysDrag: UIGestureRecognizerRepresentable {
    let moved: (CGFloat) -> Void
    let ended: () -> Void

    func makeUIGestureRecognizer(context: Context) -> UIPanGestureRecognizer {
        let pan = UIPanGestureRecognizer()
        pan.delegate = context.coordinator
        return pan
    }
    func handleUIGestureRecognizerAction(_ pan: UIPanGestureRecognizer, context: Context) {
        switch pan.state {
        case .began, .changed: moved(context.converter.localLocation.x)
        case .ended, .cancelled, .failed: ended()
        default: break
        }
    }
    func makeCoordinator(converter: CoordinateSpaceConverter) -> Coordinator { Coordinator() }

    @MainActor final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        func gestureRecognizerShouldBegin(_ recognizer: UIGestureRecognizer) -> Bool {
            guard let pan = recognizer as? UIPanGestureRecognizer else { return true }
            // how far it moved before it began (a pan begins past a few points)
            let moved = pan.translation(in: pan.view)
            return abs(moved.x) > abs(moved.y)
        }
        // the chat's scrolling waits for this to fail, which a drag up or down does at once
        func gestureRecognizer(_ recognizer: UIGestureRecognizer, shouldBeRequiredToFailBy other: UIGestureRecognizer) -> Bool {
            other.view is UIScrollView
        }
    }
}

/// A sound's loudness as bars, those played filled.
private struct LevelBars: View {
    let levels: [Float]
    let fraction: Double
    var body: some View {
        Canvas { context, size in
            let count = SoundFacts.bars
            let gap: CGFloat = 2
            let width = max(1, (size.width - gap * CGFloat(count - 1)) / CGFloat(count))
            for bar in 0..<count {
                let level = levels.indices.contains(bar) ? CGFloat(levels[bar]) : 0.3
                let height = max(3, level * size.height)
                let rect = CGRect(x: CGFloat(bar) * (width + gap), y: (size.height - height) / 2, width: width, height: height)
                let played = (Double(bar) + 0.5) / Double(count) <= fraction
                context.fill(Path(roundedRect: rect, cornerRadius: width / 2), with: .color(played ? .primary : .secondary.opacity(0.45)))
            }
        }
    }
}

// MARK: PDFs and files

/// A PDF: its first page, its name, and its pages; a tap opens it whole.
private struct PDFCard: View {
    let attachment: ChatAttachment
    let api: FragmentAPI
    let fragment: String
    @State private var load: BlobLoad?
    @State private var page: UIImage?
    static let size = CGSize(width: AttachmentWidth.card, height: 170)

    var body: some View {
        let key = BlobKey(api: api, fragment: fragment, sha256: attachment.sha256)
        let shown = page ?? ImageCache.shared.cached(key.imageKey(0))
        let pages = ShownFacts.shared.pages(attachment.sha256)
        VStack(alignment: .leading, spacing: 0) {
            ZStack {
                Rectangle().fill(Color.white)
                if let shown { Image(uiImage: shown).resizable().scaledToFill().frame(width: Self.size.width, height: Self.size.height, alignment: .top).clipped() }
                else if let load { LoadState(load: load, attachment: attachment).tint(.black) }
            }
            .frame(width: Self.size.width, height: Self.size.height)
            VStack(alignment: .leading, spacing: 2) {
                Text(attachment.name).font(.footnote.weight(.medium)).lineLimit(1).truncationMode(.middle)
                Text([pages.map { $0 == 1 ? "1 page" : "\($0) pages" }, "PDF", attachment.shownSize].compactMap { $0 }.joined(separator: " · "))
                    .font(.caption2).foregroundStyle(.secondary)
            }
            .padding(.horizontal, 12).padding(.vertical, 9)
            .frame(width: Self.size.width, alignment: .leading)
            .background(FragmentStyle.surface)
        }
        .clipShape(RoundedRectangle(cornerRadius: 16))
        .overlay(RoundedRectangle(cornerRadius: 16).strokeBorder(.quaternary))
        .contentShape(Rectangle())
        .onTapGesture {
            if let file = load?.file, let named = try? BlobStore.shared.named(attachment, file: file) { FilePreview.show(named) } else { load?.begin() }
        }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isButton)
        .accessibilityIdentifier("pdf-\(attachment.sha256.prefix(8))")
        .contextMenu { if let file = load?.file { ShareMenu(attachment: attachment, file: file) } }
        .task(id: attachment.sha256) {
            let load = BlobStore.shared.load(attachment, api: api, fragment: fragment, begin: attachment.size <= BlobStore.previewBytesMax)
            self.load = load
        }
        .task(id: load?.file) {
            guard page == nil, let file = load?.file else { return }
            if let kept = ImageCache.shared.cached(key.imageKey(0)), pages != nil { page = kept; return }
            let size = Self.size
            let (image, count) = await Task.detached(priority: .userInitiated) { Self.firstPage(file, size: size) }.value
            if let count { ShownFacts.shared.keep(pages: count, for: attachment.sha256) }
            page = await ImageCache.shared.image(key.imageKey(0)) { image }
        }
    }

    nonisolated static func firstPage(_ file: URL, size: CGSize) -> (UIImage?, Int?) {
        guard let document = PDFDocument(url: file), let first = document.page(at: 0) else { return (nil, nil) }
        let bounds = first.bounds(for: .cropBox)
        // its width fills the card's, at the screen's scale
        let scale = size.width * 3 / max(1, bounds.width)
        return (first.thumbnail(of: CGSize(width: bounds.width * scale, height: bounds.height * scale), for: .cropBox), document.pageCount)
    }
}

/// Any other file: its icon, name, size and type. A tap reads it (with
/// its progress, for a large one) and opens it in Quick Look, or Share
/// for a type Quick Look cannot show.
struct FileRow: View {
    let attachment: ChatAttachment
    let api: FragmentAPI
    let fragment: String
    var note: String?
    @State private var load: BlobLoad?
    @State private var sharing: URL?
    @State private var opens = false

    var body: some View {
        HStack(spacing: 12) {
            ZStack {
                RoundedRectangle(cornerRadius: 10).fill(FragmentStyle.lilac.opacity(0.18))
                if let load, case .loading(let fraction) = load.state {
                    if fraction > 0 { ProgressRing(fraction: fraction, label: "Downloading") }
                    else { ProgressView().controlSize(.small) }
                } else { Image(systemName: attachment.symbol).font(.title3).foregroundStyle(FragmentStyle.lilac) }
            }
            .frame(width: 40, height: 40)
            VStack(alignment: .leading, spacing: 2) {
                Text(attachment.name.isEmpty ? "File" : attachment.name).font(.footnote.weight(.medium)).lineLimit(2).truncationMode(.middle)
                Text(detail).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer(minLength: 0)
        }
        .padding(10)
        .frame(width: AttachmentWidth.card, alignment: .leading)
        .background(FragmentStyle.surface, in: RoundedRectangle(cornerRadius: 16))
        .contentShape(Rectangle())
        .onTapGesture { opens = true; if let file = load?.file { open(file) } else { load?.begin() } }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isButton)
        .accessibilityIdentifier("file-\(attachment.sha256.prefix(8))")
        .contextMenu { if let file = load?.file { ShareMenu(attachment: attachment, file: file) } }
        .sheet(item: Binding(get: { sharing.map(SharedFile.init) }, set: { sharing = $0?.url })) { shared in
            ShareSheet(url: shared.url).presentationDetents([.medium, .large])
        }
        .task(id: attachment.sha256) { load = BlobStore.shared.load(attachment, api: api, fragment: fragment, begin: false) }
        .onChange(of: load?.file) { _, file in if opens, let file { open(file) } }
    }

    private var detail: String {
        if let load, case .failed(let why) = load.state { return why }
        if let note { return note }
        return [attachment.shownSize, attachment.shownType].compactMap { $0 }.joined(separator: " · ")
    }

    private func open(_ file: URL) {
        opens = false
        guard let named = try? BlobStore.shared.named(attachment, file: file) else { return }
        if QLPreviewController.canPreview(named as NSURL) { FilePreview.show(named) } else { sharing = named }
    }
    private struct SharedFile: Identifiable { let url: URL; var id: URL { url } }
}

/// A read file's Share, in its long-press menu.
private struct ShareMenu: View {
    let attachment: ChatAttachment
    let file: URL
    var body: some View {
        if let named = try? BlobStore.shared.named(attachment, file: file) {
            ShareLink(item: named) { Label("Share", systemImage: "square.and.arrow.up") }
        }
    }
}

private struct ShareSheet: UIViewControllerRepresentable {
    let url: URL
    func makeUIViewController(context: Context) -> UIActivityViewController { UIActivityViewController(activityItems: [url], applicationActivities: nil) }
    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}

/// Quick Look over the whole app, showing a read file with its Share and,
/// for a PDF, its pages and search. Never its markup: a file here is the
/// one kept by its hash, which nothing edits.
final class FilePreview: QLPreviewController, QLPreviewControllerDataSource, @preconcurrency QLPreviewControllerDelegate {
    private let url: URL
    init(_ url: URL) {
        self.url = url
        super.init(nibName: nil, bundle: nil)
        dataSource = self
        delegate = self
    }
    required init?(coder: NSCoder) { nil }

    static func show(_ url: URL) {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        guard var top = (scenes.first { $0.activationState == .foregroundActive } ?? scenes.first)?.keyWindow?.rootViewController else { return }
        while let next = top.presentedViewController { top = next }
        top.present(FilePreview(url), animated: true)
    }

    func numberOfPreviewItems(in controller: QLPreviewController) -> Int { 1 }
    func previewController(_ controller: QLPreviewController, previewItemAt index: Int) -> any QLPreviewItem { url as NSURL }
    func previewController(_ controller: QLPreviewController, editingModeFor previewItem: any QLPreviewItem) -> QLPreviewItemEditingMode { .disabled }
}
