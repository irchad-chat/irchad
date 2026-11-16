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
    @State private var focused = false

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
                                .glassButton()
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
                        round("plus")
                    }
                    .accessibilityLabel("Attach")
                }
                Button { picker = true } label: { round("face.smiling") }
                    .accessibilityLabel("Emoji and GIFs")
                HStack(alignment: .bottom, spacing: 4) {
                    MessageField(placeholder: "Message \(buf.target)", text: $draft, focused: $focused, onReturn: send)
                        .overlay(alignment: .topLeading) {
                            if draft.isEmpty {
                                Text("Message \(buf.target)").foregroundStyle(.tertiary).allowsHitTesting(false)
                            }
                        }
                        .padding(.vertical, 8)
                    Button(action: send) { Image(systemName: "arrow.up.circle.fill").font(.title2) }
                        .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .accessibilityLabel(editing == nil ? "Send" : "Save edit")
                        .padding(.bottom, 5)
                }
                .padding(.leading, 12)
                .padding(.trailing, 5)
                .glass(in: RoundedRectangle(cornerRadius: 19))
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
        }
        .background {
            // Glass floats over the messages; before it, an opaque bar down through the
            // keyboard's area too (with a hardware keyboard that's just the shortcut bar).
            if #available(iOS 26, *) { Color.clear } else { Rectangle().fill(.bar).ignoresSafeArea(edges: .bottom) }
        }
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
        .sheet(isPresented: $picker) {
            EmojiGifPicker(network: net.network, gifs: net.gifs, onEmoji: { draft += $0 }, onGif: { url in
                // Posted as a plain link: every client can open it, and IrChad shows it inline.
                model.client.send(network: net.network, target: buf.target, text: url, replyTo: nil, mentions: [])
                picker = false
            }, onSticker: { code in
                // Its `:name:`, a message of its own.
                model.client.send(network: net.network, target: buf.target, text: code, replyTo: nil, mentions: [])
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
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .glass(in: RoundedRectangle(cornerRadius: 14))
        .padding(.horizontal, 10)
        .padding(.top, 6)
    }

    private func round(_ symbol: String) -> some View {
        Image(systemName: symbol)
            .font(.title3)
            .frame(width: 38, height: 38)
            .glass(in: Circle())
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

/// The draft's text view: UIKit's, since SwiftUI's multi-line `TextField` ends
/// editing on a hardware keyboard's Return, dropping the keyboard on every send.
/// A hardware Return sends (`onReturn`); Shift-Return and the on-screen
/// keyboard's Return are new lines.
private struct MessageField: UIViewRepresentable {
    let placeholder: String
    @Binding var text: String
    @Binding var focused: Bool
    let onReturn: () -> Void

    final class Coordinator: NSObject, UITextViewDelegate {
        var parent: MessageField
        init(_ parent: MessageField) { self.parent = parent }

        func textViewDidChange(_ v: UITextView) { parent.text = v.text }
        func textViewDidBeginEditing(_ v: UITextView) { if !parent.focused { parent.focused = true } }
        func textViewDidEndEditing(_ v: UITextView) { if parent.focused { parent.focused = false } }
    }

    final class View: UITextView {
        var onReturn: () -> Void = {}

        override var keyCommands: [UIKeyCommand]? {
            let send = UIKeyCommand(input: "\r", modifierFlags: [], action: #selector(returnPressed))
            send.wantsPriorityOverSystemBehavior = true
            return (super.keyCommands ?? []) + [send]
        }

        @objc private func returnPressed() { onReturn() }

        // Mid-composition (an input method's marked text), Return commits it instead.
        override func canPerformAction(_ action: Selector, withSender sender: Any?) -> Bool {
            action == #selector(returnPressed) ? markedTextRange == nil : super.canPerformAction(action, withSender: sender)
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeUIView(context: Context) -> View {
        let v = View()
        v.delegate = context.coordinator
        v.font = .preferredFont(forTextStyle: .body)
        v.adjustsFontForContentSizeCategory = true
        v.backgroundColor = .clear
        v.textContainerInset = .zero
        v.textContainer.lineFragmentPadding = 0
        v.isScrollEnabled = false
        v.accessibilityLabel = placeholder
        v.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return v
    }

    func updateUIView(_ v: View, context: Context) {
        context.coordinator.parent = self
        v.onReturn = onReturn
        v.accessibilityLabel = placeholder
        if v.text != text { v.text = text }
        if focused != v.isFirstResponder {
            // Not during the update: the delegate writes `focused` back.
            DispatchQueue.main.async {
                if context.coordinator.parent.focused { v.becomeFirstResponder() } else { v.resignFirstResponder() }
            }
        }
    }

    /// Grows with the text up to six lines, then scrolls.
    func sizeThatFits(_ proposal: ProposedViewSize, uiView v: View, context: Context) -> CGSize? {
        guard let w = proposal.width else { return nil }
        let fit = v.sizeThatFits(CGSize(width: w, height: .greatestFiniteMagnitude)).height
        let most = ceil((v.font?.lineHeight ?? 20) * 6)
        v.isScrollEnabled = fit > most
        return CGSize(width: w, height: min(fit, most))
    }
}
