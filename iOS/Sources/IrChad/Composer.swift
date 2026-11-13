import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

struct Composer: View {
    @Environment(AppModel.self) private var model
    let net: Network
    let buf: Buffer
    @Binding var replyTo: Message?
    @Binding var editing: Message?

    @State private var draft = ""
    /// Nicks picked from the `@` list, sent as `+chad/mentions` (D66).
    @State private var picked: [String] = []
    @State private var picker = false
    @State private var files = false
    @State private var photos: [PhotosPickerItem] = []
    @State private var photoPicker = false
    @FocusState private var focused: Bool
    /// When the last message went: focus lost just after is the field's doing, not the user's.
    @State private var sentAt = Date.distantPast

    var body: some View {
        VStack(spacing: 0) {
            if let r = replyTo {
                bar("Replying to \(r.shown): \(r.text)", close: "Don't reply") { replyTo = nil }
            }
            if editing != nil {
                bar("Editing your message", close: "Stop editing") {
                    editing = nil
                    draft = ""
                }
            }
            let nicks = suggestions
            if !nicks.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(nicks, id: \.nick) { n in
                            Button(n.shown) { complete(n.nick) }
                                .buttonStyle(.bordered)
                                .controlSize(.small)
                                .accessibilityLabel("Mention \(n.shown)")
                        }
                    }
                    .padding(.horizontal, 10)
                }
                .padding(.top, 6)
            }
            HStack(alignment: .bottom, spacing: 8) {
                if net.uploads != nil {
                    Menu {
                        Button("Photos and videos", systemImage: "photo.on.rectangle") { photoPicker = true }
                        Button("Files", systemImage: "folder") { files = true }
                    } label: {
                        Image(systemName: "plus.circle.fill").font(.title2)
                    }
                    .accessibilityLabel("Attach")
                }
                Button { picker = true } label: { Image(systemName: "face.smiling").font(.title2) }
                    .accessibilityLabel("Emoji and GIFs")
                TextField("Message \(buf.target)", text: $draft, axis: .vertical)
                    .lineLimit(1...6)
                    .focused($focused)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(Color(.secondarySystemBackground), in: RoundedRectangle(cornerRadius: 20))
                    // A hardware keyboard's Return sends; Shift-Return is a new line.
                    .onKeyPress(.return, phases: .down) { press in
                        if press.modifiers.contains(.shift) { return .ignored }
                        send()
                        return .handled
                    }
                Button(action: send) { Image(systemName: "arrow.up.circle.fill").font(.title) }
                    .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    .accessibilityLabel(editing == nil ? "Send" : "Save edit")
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
        }
        .background(.bar)
        .onChange(of: draft) { _, d in
            let blank = d.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            model.client.typing(network: net.network, target: buf.target, active: !blank)
            if blank { picked = [] }
        }
        // Finished uploads to here: their links go in the draft, to send when the user says.
        .onChange(of: model.uploaded["\(net.network) \(buf.target)"], initial: true) { _, links in
            guard let links else { return }
            model.uploaded["\(net.network) \(buf.target)"] = nil
            for l in links {
                if let c = draft.last, !c.isWhitespace { draft += " " }
                draft += l
            }
        }
        .onChange(of: editing?.key) { _, _ in
            // Prefilled with the text without its formatting, like desktop.
            if let e = editing {
                draft = e.text
                picked = e.mentions
                focused = true
            }
        }
        .onChange(of: replyTo?.key) { _, k in if k != nil { focused = true } }
        // A hardware Return ends editing some time after `send()`; take focus back.
        .onChange(of: focused) { _, f in
            if !f, Date().timeIntervalSince(sentAt) < 0.5 { focused = true }
        }
        .sheet(isPresented: $picker) {
            EmojiGifPicker(network: net.network, gifs: net.gifs, onEmoji: { draft += $0 }, onGif: { url in
                // Posted as a plain link: every client can open it, and IrChad shows it inline.
                model.client.send(network: net.network, target: buf.target, text: url, replyTo: nil, mentions: [])
                picker = false
            })
        }
        .photosPicker(isPresented: $photoPicker, selection: $photos, maxSelectionCount: 10,
                      matching: .any(of: [.images, .videos]), preferredItemEncoding: .compatible)
        .onChange(of: photos) { _, items in
            guard !items.isEmpty else { return }
            photos = []
            Task { await uploadPhotos(items) }
        }
        .fileImporter(isPresented: $files, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
            guard case let .success(urls) = result else { return }
            for u in urls { upload(u, name: u.lastPathComponent, scoped: true) }
        }
    }

    /// Where the draft's last word starts, and the word. SwiftUI's TextField
    /// doesn't expose the cursor, so the `@` list works on the end of the draft.
    private var lastWord: (start: String.Index, word: Substring) {
        let start = draft.lastIndex(where: \.isWhitespace).map(draft.index(after:)) ?? draft.startIndex
        return (start, draft[start...])
    }

    /// The `@` list for the word being typed: nicks and display names that match.
    private var suggestions: [Nick] {
        let w = lastWord.word
        guard w.hasPrefix("@") else { return [] }
        return model.client.nicks(network: net.network, target: buf.target, query: String(w.dropFirst()))
    }

    /// Replaces the `@` word with `nick`, the way IRC clients highlight on:
    /// `nick: ` to start a message, `nick ` elsewhere.
    private func complete(_ nick: String) {
        let start = lastWord.start
        let put = start == draft.startIndex ? "\(nick): " : "\(nick) "
        draft.replaceSubrange(start..., with: put)
        if !picked.contains(nick) { picked.append(nick) }
        focused = true
    }

    private func bar(_ text: String, close: String, action: @escaping () -> Void) -> some View {
        HStack {
            Text(text).font(.caption).lineLimit(1)
            Spacer()
            Button(action: action) { Image(systemName: "xmark.circle.fill") }
                .accessibilityLabel(close)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 6)
        .background(Color(.tertiarySystemBackground))
    }

    private func send() {
        let text = draft.replacingOccurrences(of: "\\s+$", with: "", options: .regularExpression)
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        if let e = editing, let id = e.msgid {
            model.client.edit(network: net.network, target: buf.target, msgid: id, text: text, mentions: picked)
        } else {
            model.client.send(network: net.network, target: buf.target, text: text, replyTo: replyTo?.msgid,
                              mentions: picked)
        }
        draft = ""
        picked = []
        replyTo = nil
        editing = nil
        // Keep the field focused for the next message (the button takes focus).
        sentAt = Date()
        focused = true
    }

    private func uploadPhotos(_ items: [PhotosPickerItem]) async {
        for item in items {
            do {
                guard let f = try await item.loadTransferable(type: PickedFile.self) else { continue }
                upload(f.url, name: f.url.lastPathComponent, scoped: false)
                try? FileManager.default.removeItem(at: f.url.deletingLastPathComponent())
            } catch {
                model.toast("Couldn't read that photo.")
            }
        }
    }

    /// Uploads the file to this conversation, whose link the core then posts.
    /// A file over the network's limit is refused before it's read.
    private func upload(_ url: URL, name: String, scoped: Bool) {
        guard let limit = net.uploads else {
            model.toast("\(net.name) doesn't take uploads.")
            return
        }
        let ok = scoped ? url.startAccessingSecurityScopedResource() : false
        defer { if ok { url.stopAccessingSecurityScopedResource() } }
        let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize).map(UInt64.init) ?? 0
        if size > limit {
            model.toast("\(name) is too big for \(net.name) (\(limit / 1_000_000) MB at most).")
            return
        }
        guard let data = try? Data(contentsOf: url, options: .mappedIfSafe), UInt64(data.count) <= limit else {
            model.toast("Couldn't read \(name).")
            return
        }
        model.toast("Uploading \(name)…")
        model.client.upload(network: net.network, target: buf.target, name: name, bytes: data)
    }
}

/// A photo or video from the picker, copied out of the system's temporary
/// file. The JPEG, PNG or GIF the picker converts to (`.compatible`) comes
/// first, so other clients can show it.
struct PickedFile: Transferable {
    let url: URL

    static var transferRepresentation: some TransferRepresentation {
        rep(.gif)
        rep(.png)
        rep(.jpeg)
        rep(.mpeg4Movie)
        rep(.quickTimeMovie)
        rep(.item)
    }

    private static func rep(_ type: UTType) -> FileRepresentation<PickedFile> {
        FileRepresentation(importedContentType: type) { received in
            let dir = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            let dst = dir.appending(path: received.file.lastPathComponent)
            try FileManager.default.copyItem(at: received.file, to: dst)
            return PickedFile(url: dst)
        }
    }
}
