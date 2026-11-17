import SwiftUI

/// The files a message being written carries, each from its own bytes on
/// this iPhone: a picture's thumbnail, a voice memo to play, any other
/// file's icon and name. One being put says so (with its share sent); one
/// that failed says so, with Retry; each comes off with its ×.
struct ComposerAttachments: View {
    let session: ChatSession

    var body: some View {
        let uploads = session.uploads
        let attached = session.attachments
        if !uploads.isEmpty || !attached.isEmpty {
            ScrollView(.horizontal) {
                HStack(alignment: .top, spacing: 10) {
                    ForEach(attached) { file in
                        ComposerTile(attachment: file, session: session, upload: nil) { session.attachments.removeAll { $0.id == file.id } }
                    }
                    ForEach(uploads) { upload in
                        ComposerTile(attachment: upload.attachment, session: session, upload: upload) { session.removeUpload(upload.id) }
                    }
                }
                .padding(.vertical, 4)
            }
            .scrollIndicators(.hidden)
        }
    }
}

private struct ComposerTile: View {
    let attachment: ChatAttachment
    let session: ChatSession
    let upload: Upload?
    let remove: () -> Void
    @State private var image: UIImage?
    static let side: CGFloat = 64

    var body: some View {
        let key = BlobKey(api: session.api, fragment: session.fragment.name, sha256: attachment.sha256)
        let kind = AttachmentKind(attachment)
        VStack(alignment: .leading, spacing: 4) {
            ZStack {
                RoundedRectangle(cornerRadius: 14).fill(FragmentStyle.surface)
                if let image { Image(uiImage: image).resizable().scaledToFill().frame(width: Self.side, height: Self.side).clipped() }
                // under an upload's state, a file's icon would only crowd it
                else if upload != nil {}
                else if kind == .voice || kind == .audio { ComposerSound(attachment: attachment, key: key) }
                else { Image(systemName: kind == .pdf ? "doc.richtext" : attachment.symbol).font(.title2).foregroundStyle(.secondary) }
                if let upload {
                    Color.black.opacity(0.35)
                    if upload.failed != nil {
                        Button { Task { await session.retryUpload(upload.id) } } label: {
                            Image(systemName: "arrow.clockwise").font(.headline).foregroundStyle(.white)
                                .frame(width: Self.side, height: Self.side)
                        }
                        .accessibilityLabel("Retry upload").accessibilityHint(upload.failed ?? "").accessibilityIdentifier("retryUpload")
                    } else {
                        // all of it sent, the server still taking it: no share left to show
                        if upload.progress < 0.99 { ProgressRing(fraction: upload.progress, label: "Uploading").foregroundStyle(.white) }
                        else { ProgressView().tint(.white).accessibilityLabel("Uploading") }
                    }
                }
            }
            .frame(width: Self.side, height: Self.side)
            .clipShape(RoundedRectangle(cornerRadius: 14))
            .overlay(alignment: .topTrailing) {
                Button(action: remove) {
                    Image(systemName: "xmark.circle.fill").symbolRenderingMode(.palette)
                        .foregroundStyle(.white, .black.opacity(0.6)).font(.body)
                }
                .padding(3)
                .disabled(upload != nil && upload?.failed == nil)
                .opacity(upload != nil && upload?.failed == nil ? 0 : 1)
                .accessibilityLabel("Remove \(attachment.name)")
            }
            Text(caption).font(.caption2).foregroundStyle(upload?.failed != nil ? .red : .secondary)
                .lineLimit(1).truncationMode(.middle)
                .frame(width: Self.side, alignment: .leading)
        }
        .accessibilityIdentifier("composerFile-\(attachment.sha256.prefix(8))")
        .task(id: attachment.sha256) {
            guard kind == .image, let file = BlobStore.shared.cached(key) else { return }
            let pixels = 192
            image = await ImageCache.shared.image(key.imageKey(pixels)) {
                await Task.detached(priority: .userInitiated) { PictureFile.image(file, pixels: pixels) }.value
            }
        }
    }

    private var caption: String {
        if upload?.failed != nil { return "Failed" }
        if upload != nil { return "Uploading" }
        switch AttachmentKind(attachment) {
        case .voice: return "Voice"
        default: return attachment.name
        }
    }
}

/// A voice memo in the composer: played before it is sent, from its kept
/// bytes as iOS plays them (`BlobStore.playable`: a WebM rewrapped as Ogg).
/// One that cannot play here offers no Play.
private struct ComposerSound: View {
    let attachment: ChatAttachment
    let key: BlobKey
    @State private var playable: URL?
    @State private var duration: Double?
    @State private var unplayable = false

    var body: some View {
        let playback = AudioPlayback.shared
        let id = "draft:" + attachment.sha256
        let place = playback.place(id)
        Button {
            if let playable { playback.toggle(id, url: playable) }
        } label: {
            VStack(spacing: 2) {
                if unplayable {
                    // a sound this iPhone cannot play: what it is, and no Play
                    Image(systemName: "waveform").font(.title2)
                } else {
                    Image(systemName: place?.playing == true ? "pause.fill" : "play.fill").font(.headline)
                    Text(SoundPlayer.time(place.map(\.elapsed) ?? duration ?? 0)).font(.caption2.monospacedDigit())
                }
            }
            .foregroundStyle(playable == nil ? .secondary : .primary)
            .frame(width: ComposerTile.side, height: ComposerTile.side)
        }
        .buttonStyle(.plain)
        .disabled(playable == nil || playback.recording)
        .accessibilityLabel(unplayable ? "A sound that can’t play here" : place?.playing == true ? "Pause voice memo" : "Play voice memo")
        .task(id: attachment.sha256) {
            guard let file = BlobStore.shared.cached(key), let url = await BlobStore.shared.playable(attachment, file: file),
                  let facts = await SoundFacts.shared.facts(attachment.sha256, url: url) else { unplayable = true; return }
            duration = facts.duration
            playable = url
        }
    }
}
