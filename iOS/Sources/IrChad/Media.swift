import ImageIO
import Network
import SwiftUI
import UIKit

/// Decoded images are scaled to fit this many pixels a side.
private let maxPixels = 800
private let maxImages = 3
/// Larger sources are refused, not scaled: sampling still reads every pixel.
private let maxSide = 8192
/// Animated sources decode every frame.
private let maxAnimated = 2048 * 2048
/// What one animation's decoded frames may hold; later frames are dropped.
private let maxAnimationBytes = 48 << 20
/// What decoded images may hold in memory, all together.
private let cacheBytes = 64 << 20

/// Images and link previews for the chat. The core fetches them (own uploads
/// directly, the rest through chadd, D25), and decoding is bounded (D14).
@MainActor @Observable
final class Media {
    enum Img {
        case loading, ready(UIImage), failed
    }

    enum Prev {
        case loading, ready(LinkPreview), failed
    }

    private final class Box {
        let img: Img
        init(_ img: Img) { self.img = img }
    }

    /// Bumped when something arrives: read the caches again.
    private(set) var tick = 0
    /// Wi-Fi or Ethernet, as the system counts it.
    private(set) var unmetered = false
    /// Answers for the GIF picker on screen.
    @ObservationIgnored var onGifs: ((String, String?, GifPage) -> Void)?

    // Keyed by "network url": what's fetchable differs per network.
    @ObservationIgnored private let images: NSCache<NSString, Box> = {
        let c = NSCache<NSString, Box>()
        c.totalCostLimit = cacheBytes
        return c
    }()
    @ObservationIgnored private var previews: [String: Prev] = [:]
    /// Custom emoji at the size text wants them, by "network url height".
    @ObservationIgnored private var emoji: [String: UIImage] = [:]
    @ObservationIgnored private let monitor = NWPathMonitor()

    init() {
        monitor.pathUpdateHandler = { [weak self] path in
            let free = path.status == .satisfied && !path.isExpensive && !path.isConstrained
            DispatchQueue.main.async { MainActor.assumeIsolated { self?.unmetered = free } }
        }
        monitor.start(queue: .global(qos: .utility))
    }

    private var model: AppModel { AppModel.shared }

    /// Whether images and previews load without a tap right now.
    var auto: Bool {
        switch model.settings.autoLoad {
        case .always: true
        case .wifi: unmetered
        case .off: false
        }
    }

    /// `nil` when it isn't loaded and `ask` is off.
    func image(network: String, url: String, ask: Bool) -> Img? {
        let key = "\(network) \(url)" as NSString
        if let b = images.object(forKey: key) { return b.img }
        guard ask else { return nil }
        images.setObject(Box(.loading), forKey: key, cost: 1)
        model.client.media(network: network, url: url)
        return .loading
    }

    /// A custom emoji's image `height` points tall, for a line of text, once
    /// it has loaded. It's fetched whatever the media settings say: it comes
    /// from the network itself. An animated one is its first frame, because
    /// `Text` can't hold a view.
    func emojiImage(network: String, url: String, height: CGFloat) -> UIImage? {
        let key = "\(network) \(url) \(Int(height))"
        if let i = emoji[key] { return i }
        guard case let .ready(full)? = image(network: network, url: url, ask: true) else { return nil }
        let still = full.images?.first ?? full
        let h = max(1, height.rounded())
        // A wide one keeps its shape, up to a point.
        let w = min(h * 3, (still.size.width * h / max(still.size.height, 1)).rounded())
        let size = CGSize(width: max(1, w), height: h)
        let scaled = UIGraphicsImageRenderer(size: size).image { _ in still.draw(in: CGRect(origin: .zero, size: size)) }
        if emoji.count >= 512 { emoji.removeAll() }
        emoji[key] = scaled
        return scaled
    }

    /// `nil` when it isn't loaded and `ask` is off.
    func preview(network: String, url: String, ask: Bool) -> Prev? {
        let key = "\(network) \(url)"
        if let p = previews[key] { return p }
        guard ask else { return nil }
        if previews.count >= 256 { previews.removeAll() }
        previews[key] = .loading
        model.client.preview(network: network, url: url)
        return .loading
    }

    func gotPreview(network: String, url: String, preview: LinkPreview?) {
        previews["\(network) \(url)"] = preview.map { .ready($0) } ?? .failed
        bump()
    }

    func gotImage(network: String, url: String, image: UIImage?) {
        let cost = image.map { i in Int(i.size.width * i.size.height * i.scale * i.scale) * 4 * max(1, i.images?.count ?? 1) } ?? 1
        images.setObject(Box(image.map { .ready($0) } ?? .failed), forKey: "\(network) \(url)" as NSString, cost: cost)
        bump()
    }

    @ObservationIgnored private var bumping = false

    /// Scrolling back fetches in bursts, and every view showing media redraws
    /// on a tick: one per burst, not one per arrival.
    private func bump() {
        guard !bumping else { return }
        bumping = true
        Task {
            try? await Task.sleep(for: .milliseconds(100))
            bumping = false
            tick &+= 1
        }
    }

    func gotGifs(query: String, pos: String?, page: GifPage) {
        onGifs?(query, pos, page)
    }

    /// PNG, JPEG, GIF or WebP by magic bytes.
    nonisolated static func isImage(_ b: Data) -> Bool {
        func at(_ i: Int, _ m: [UInt8]) -> Bool {
            b.count >= i + m.count && m.indices.allSatisfy { b[b.startIndex + i + $0] == m[$0] }
        }
        return at(0, [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) || at(0, [0xFF, 0xD8, 0xFF])
            || at(0, [0x47, 0x49, 0x46, 0x38]) || (at(0, [0x52, 0x49, 0x46, 0x46]) && at(8, [0x57, 0x45, 0x42, 0x50]))
    }

    /// Server-sent bytes (D14): only the formats desktop and chadd take,
    /// bounded in size, and any failure only loses the image.
    nonisolated static func decode(_ data: Data) -> UIImage? {
        guard isImage(data),
              let src = CGImageSourceCreateWithData(data as CFData, [kCGImageSourceShouldCache: false] as CFDictionary)
        else { return nil }
        let count = CGImageSourceGetCount(src)
        guard count > 0,
              let props = CGImageSourceCopyPropertiesAtIndex(src, 0, nil) as? [CFString: Any],
              let w = props[kCGImagePropertyPixelWidth] as? Int, let h = props[kCGImagePropertyPixelHeight] as? Int,
              w > 0, h > 0, w <= maxSide, h <= maxSide
        else { return nil }
        let opts = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceShouldCacheImmediately: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixels,
        ] as CFDictionary
        if count == 1 {
            return CGImageSourceCreateThumbnailAtIndex(src, 0, opts).map { UIImage(cgImage: $0) }
        }
        guard w * h <= maxAnimated else { return nil }
        var frames: [UIImage] = []
        var duration = 0.0
        var bytes = 0
        for i in 0..<count {
            guard let f = CGImageSourceCreateThumbnailAtIndex(src, i, opts) else { break }
            bytes += f.width * f.height * 4
            if bytes > maxAnimationBytes { break }
            frames.append(UIImage(cgImage: f))
            duration += delay(src, i)
        }
        if frames.count <= 1 { return frames.first }
        return UIImage.animatedImage(with: frames, duration: duration)
    }

    /// A frame's delay, with browsers' floor for "as fast as possible".
    private nonisolated static func delay(_ src: CGImageSource, _ i: Int) -> Double {
        let p = CGImageSourceCopyPropertiesAtIndex(src, i, nil) as? [CFString: Any]
        let gif = p?[kCGImagePropertyGIFDictionary] as? [CFString: Any]
        let webp = p?[kCGImagePropertyWebPDictionary] as? [CFString: Any]
        let d = (gif?[kCGImagePropertyGIFUnclampedDelayTime] as? Double)
            ?? (gif?[kCGImagePropertyGIFDelayTime] as? Double)
            ?? (webp?[kCGImagePropertyWebPUnclampedDelayTime] as? Double)
            ?? (webp?[kCGImagePropertyWebPDelayTime] as? Double) ?? 0.1
        return d < 0.011 ? 0.1 : d
    }
}

/// A fetched image fitted inside `maxW` × `maxH` (or filling its frame with
/// `fill`); a placeholder while loading, a button when it waits for a tap.
struct RemoteImage: View {
    @Environment(AppModel.self) private var model
    let network: String
    let url: String
    var maxW: CGFloat = 280
    var maxH: CGFloat = 220
    var fill = false
    var ask: Bool?
    var onOpen: ((UIImage) -> Void)?
    @State private var tapped = false

    var body: some View {
        let _ = model.media.tick
        let img = model.media.image(network: network, url: url, ask: (ask ?? model.media.auto) || tapped)
        switch img {
        case nil:
            Button("Show image") { tapped = true }.font(.callout)
        case .loading?:
            RoundedRectangle(cornerRadius: 8).fill(Color(.secondarySystemBackground))
                .frame(width: fill ? nil : min(maxW, 160), height: fill ? nil : 90)
        case .failed?:
            EmptyView()
        case let .ready(i)?:
            let scale = fill ? 1 : min(1, maxW / max(i.size.width, 1), maxH / max(i.size.height, 1))
            Animated(image: i, fill: fill)
                .frame(width: fill ? nil : i.size.width * scale, height: fill ? nil : i.size.height * scale)
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .contentShape(Rectangle())
                .onTapGesture { onOpen?(i) }
                .accessibilityLabel("Image")
                .accessibilityAddTraits(onOpen != nil ? .isButton : [])
        }
    }
}

/// SwiftUI's `Image` doesn't play an animated `UIImage`; a `UIImageView` does.
struct Animated: UIViewRepresentable {
    let image: UIImage
    var fill = false

    func makeUIView(context: Context) -> UIImageView {
        let v = UIImageView()
        v.clipsToBounds = true
        v.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        v.setContentCompressionResistancePriority(.defaultLow, for: .vertical)
        v.setContentHuggingPriority(.defaultLow, for: .horizontal)
        v.setContentHuggingPriority(.defaultLow, for: .vertical)
        return v
    }

    func updateUIView(_ v: UIImageView, context: Context) {
        v.contentMode = fill ? .scaleAspectFill : .scaleAspectFit
        if v.image !== image { v.image = image }
    }
}

/// Whether `MessageLinks` shows anything for `m`.
@MainActor
private func showsMedia(_ model: AppModel, _ m: Message) -> Bool {
    !(m.notice || m.redacted || m.undecryptable) && (model.settings.guestMedia || m.account != nil || m.own)
}

/// The image links `MessageLinks` shows inline.
@MainActor
private func inlineImages(_ model: AppModel, _ m: Message) -> [Link] {
    showsMedia(model, m) ? Array(m.links.filter { $0.kind == .image }.prefix(maxImages)) : []
}

/// Links whose image is showing inline: the image stands in for the link.
@MainActor
func shownImages(_ model: AppModel, network: String, _ m: Message) -> Set<String> {
    let urls = inlineImages(model, m)
    // Rows without images mustn't redraw on every image that arrives.
    guard !urls.isEmpty else { return [] }
    let _ = model.media.tick
    return Set(urls.map(\.url).filter {
        if case .ready? = model.media.image(network: network, url: $0, ask: false) { true } else { false }
    })
}

/// Images inline, videos and files as cards, and a preview of the first page.
struct MessageLinks: View {
    @Environment(AppModel.self) private var model
    let network: String
    let m: Message
    @State private var open: ImageBox?

    var body: some View {
        if !m.links.isEmpty && showsMedia(model, m) {
            VStack(alignment: .leading, spacing: 4) {
                ForEach(inlineImages(model, m), id: \.url) { l in
                    RemoteImage(network: network, url: l.url, onOpen: { open = ImageBox(image: $0, url: l.url) })
                }
                ForEach(m.links.filter { $0.kind == .video || $0.kind == .file }, id: \.url) { l in
                    FileCard(link: l)
                }
                ForEach(m.links.filter { $0.kind == .page }.prefix(1), id: \.url) { l in
                    PreviewCard(network: network, url: l.url)
                }
            }
            .padding(.top, 4)
            .fullScreenCover(item: $open) { b in
                ImageViewer(image: b.image, url: b.url)
            }
        }
    }
}

private struct ImageBox: Identifiable {
    let image: UIImage
    let url: String
    var id: ObjectIdentifier { ObjectIdentifier(image) }
}

/// There's no player: a video or file opens outside.
private struct FileCard: View {
    let link: Link

    var body: some View {
        let name = URL(string: link.url)?.lastPathComponent ?? link.url
        if let u = URL(string: link.url) {
            SwiftUI.Link(destination: u) {
                Label(name.isEmpty ? link.url : name, systemImage: link.kind == .video ? "play.rectangle" : "doc")
                    .lineLimit(1)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)
                    .background(Color(.secondarySystemBackground), in: RoundedRectangle(cornerRadius: 8))
            }
        }
    }
}

private struct PreviewCard: View {
    @Environment(AppModel.self) private var model
    let network: String
    let url: String
    @State private var tapped = false

    var body: some View {
        let _ = model.media.tick
        let ask = model.media.auto || tapped
        switch model.media.preview(network: network, url: url, ask: ask) {
        case nil:
            Button("Show link preview") { tapped = true }.font(.callout)
        case let .ready(p)? where p.title != nil || p.description != nil || p.image != nil:
            if let u = URL(string: url) {
                SwiftUI.Link(destination: u) {
                    HStack(spacing: 0) {
                        Rectangle().fill(Color.accentColor).frame(width: 4)
                        VStack(alignment: .leading, spacing: 2) {
                            if let s = p.site { Text(s).font(.caption2).foregroundStyle(.secondary) }
                            if let t = p.title { Text(t).fontWeight(.semibold).lineLimit(2) }
                            if let d = p.description {
                                Text(d).font(.footnote).foregroundStyle(.primary).lineLimit(3)
                            }
                            if let img = p.image {
                                RemoteImage(network: network, url: img, maxW: 300, maxH: 160, ask: ask)
                                    .padding(.top, 4)
                            }
                        }
                        .multilineTextAlignment(.leading)
                        .padding(10)
                    }
                    .frame(maxWidth: 320, alignment: .leading)
                    .background(Color(.secondarySystemBackground))
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                }
            }
        default:
            EmptyView()
        }
    }
}

/// An image over everything: pinch to zoom, drag to pan, double-tap to reset.
// shortcut: shows the inline decode (800 px a side); decode the bytes again
// larger, like desktop's viewer, if that looks soft on an iPad.
struct ImageViewer: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    let image: UIImage
    let url: String

    var body: some View {
        ZStack(alignment: .topTrailing) {
            Color.black.ignoresSafeArea()
            Zoomable { Animated(image: image) }
                .ignoresSafeArea()
            VStack(alignment: .leading, spacing: 4) {
                Text(url).font(.caption).foregroundStyle(.white.opacity(0.7)).lineLimit(1).truncationMode(.middle)
                HStack(spacing: 16) {
                    if let u = URL(string: url) {
                        Button("Open in browser", systemImage: "arrow.up.right.square") { openURL(u) }
                    }
                    Button("Copy image link", systemImage: "doc.on.doc") { UIPasteboard.general.string = url }
                }
                .font(.callout)
            }
            .padding()
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
            .background(alignment: .bottom) {
                LinearGradient(colors: [.clear, .black.opacity(0.6)], startPoint: .top, endPoint: .bottom)
                    .frame(height: 120).ignoresSafeArea()
            }
            Button { dismiss() } label: {
                Image(systemName: "xmark.circle.fill").font(.title).foregroundStyle(.white, .white.opacity(0.3))
            }
            .accessibilityLabel("Close")
            .padding()
        }
    }
}

/// Pinch to zoom (1–8×), drag to pan while zoomed, double-tap to reset.
struct Zoomable<Content: View>: View {
    @ViewBuilder let content: Content
    @State private var scale: CGFloat = 1
    @State private var base: CGFloat = 1
    @State private var offset: CGSize = .zero
    @State private var start: CGSize = .zero

    var body: some View {
        content
            .scaleEffect(scale)
            .offset(offset)
            .gesture(
                MagnifyGesture()
                    .onChanged { scale = min(max(base * $0.magnification, 1), 8) }
                    .onEnded { _ in
                        base = scale
                        if scale == 1 { offset = .zero; start = .zero }
                    }
                    .simultaneously(with: DragGesture()
                        .onChanged { g in
                            guard scale > 1 else { return }
                            offset = CGSize(width: start.width + g.translation.width, height: start.height + g.translation.height)
                        }
                        .onEnded { _ in start = offset })
            )
            .onTapGesture(count: 2) {
                withAnimation {
                    scale = 1; base = 1; offset = .zero; start = .zero
                }
            }
            .accessibilityZoomAction { a in
                scale = min(max(scale * (a.direction == .zoomIn ? 2 : 0.5), 1), 8)
                base = scale
                if scale == 1 { offset = .zero; start = .zero }
            }
    }
}

/// Emoji, and GIFs when `gifs` (the provider) is set. Emoji stay open for
/// more; a GIF closes it.
struct EmojiGifPicker: View {
    let network: String
    let gifs: String?
    let onEmoji: (String) -> Void
    let onGif: (String) -> Void
    @State private var tab = 0

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                if gifs != nil {
                    Picker("Kind", selection: $tab) {
                        Text("Emoji").tag(0)
                        Text("GIFs").tag(1)
                    }
                    .pickerStyle(.segmented)
                    .padding(.horizontal)
                }
                if tab == 1, let gifs {
                    GifTab(network: network, provider: gifs, onGif: onGif)
                } else {
                    EmojiTab(network: network, onEmoji: onEmoji)
                }
            }
            .navigationBarTitleDisplayMode(.inline)
        }
        .presentationDetents([.medium, .large])
    }
}

private struct EmojiTab: View {
    @Environment(AppModel.self) private var model
    let network: String
    let onEmoji: (String) -> Void
    @State private var query = ""

    var body: some View {
        let list = emojis(query: query)
        // The network's own come first.
        let _ = (model.tick, model.media.tick)
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        let custom = model.client.customEmoji(network: network).filter { q.isEmpty || $0.name.contains(q) }
        VStack(spacing: 0) {
            TextField("Find an emoji", text: $query)
                .textFieldStyle(.roundedBorder)
                .textInputAutocapitalization(.never)
                .padding()
            ScrollView {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 44))], spacing: 4) {
                    ForEach(custom, id: \.name) { e in
                        let code = ":\(e.name):"
                        Button { onEmoji(code) } label: {
                            if let i = model.media.emojiImage(network: network, url: e.url, height: 32) {
                                Image(uiImage: i)
                            } else {
                                Text(e.name).font(.caption2).lineLimit(1)
                            }
                        }
                        .buttonStyle(.plain)
                        .frame(width: 44, height: 44)
                        .accessibilityLabel(code)
                    }
                    ForEach(list, id: \.self) { e in
                        Button { onEmoji(e) } label: { Text(e).font(.title) }
                            .buttonStyle(.plain)
                            .frame(width: 44, height: 44)
                    }
                }
                .padding(.horizontal, 8)
            }
        }
        // The set may have changed since we connected.
        .task { model.client.refreshEmoji(network: network) }
    }
}

private struct GifTab: View {
    @Environment(AppModel.self) private var model
    let network: String
    let provider: String
    let onGif: (String) -> Void
    @State private var query = ""
    @State private var gifs: [Gif] = []
    @State private var next: String?
    @State private var error: String?
    /// The `pos` asked for and not answered yet; "" for the first page.
    @State private var asked: String?

    var body: some View {
        VStack(spacing: 0) {
            TextField("Search GIFs", text: $query)
                .textFieldStyle(.roundedBorder)
                .padding()
            if let error { Text(error).foregroundStyle(.red).padding(.horizontal) }
            ScrollView {
                LazyVGrid(columns: [GridItem(.flexible(), spacing: 4), GridItem(.flexible(), spacing: 4)], spacing: 4) {
                    ForEach(gifs, id: \.url) { g in
                        let ratio = g.width > 0 && g.height > 0 ? CGFloat(g.width) / CGFloat(g.height) : 1
                        Button { onGif(g.url) } label: {
                            Color(.secondarySystemBackground)
                                .aspectRatio(min(max(ratio, 0.4), 3), contentMode: .fit)
                                .overlay { RemoteImage(network: network, url: g.preview, fill: true, ask: true) }
                                .clipShape(RoundedRectangle(cornerRadius: 6))
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel(g.title.isEmpty ? "GIF" : g.title)
                        .onAppear { if g.url == gifs.last?.url { more() } }
                    }
                }
                .padding(.horizontal, 8)
            }
            Text("Powered by \(provider)").font(.caption2).foregroundStyle(.secondary).padding(8)
        }
        .task(id: query) {
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled else { return }
            gifs = []
            next = nil
            error = nil
            asked = ""
            model.media.onGifs = { q, pos, page in
                guard q == query.trimmingCharacters(in: .whitespaces), (pos ?? "") == asked else { return }
                asked = nil
                gifs += page.gifs
                next = page.next
                error = page.error
            }
            model.client.gifs(network: network, query: query.trimmingCharacters(in: .whitespaces), pos: nil)
        }
        .onDisappear { model.media.onGifs = nil }
    }

    private func more() {
        guard let pos = next, asked == nil else { return }
        asked = pos
        model.client.gifs(network: network, query: query.trimmingCharacters(in: .whitespaces), pos: pos)
    }
}
