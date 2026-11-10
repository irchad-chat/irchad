import AVFoundation
import SwiftUI

/// A new header after this much quiet, even from the same person.
private let groupGapMs: Int64 = 5 * 60 * 1000

struct ChatView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var phase
    let sel: Sel
    let select: (Sel) -> Void

    @State private var replyTo: Message?
    @State private var editing: Message?
    @State private var members = false

    var body: some View {
        let _ = model.tick
        let net = model.client.networks().first { $0.network == sel.network }
        let buf = model.client.buffers(network: sel.network).first { $0.target.lowercased() == sel.target.lowercased() }
        if let net, let buf {
            VStack(spacing: 0) {
                StatusBanner(net: net)
                if buf.encryption == .changed { KeysChanged(net: net, buf: buf) }
                MessageList(net: net, buf: buf, replyTo: $replyTo, editing: $editing)
            }
            .safeAreaInset(edge: .bottom, spacing: 0) {
                Composer(net: net, buf: buf, replyTo: $replyTo, editing: $editing)
            }
            .toolbar { toolbar(net: net, buf: buf) }
            .navigationBarTitleDisplayMode(.inline)
            .sheet(isPresented: $members) {
                MembersSheet(net: net, channel: buf.target) { nick in
                    members = false
                    model.client.openDm(network: net.network, nick: nick)
                    select(Sel(network: net.network, target: nick))
                }
            }
            .task(id: "\(buf.unread) \(phase == .active)") {
                guard phase == .active else { return }
                model.notifier.dismiss(network: net.network, target: buf.target)
                if buf.unread > 0 { model.client.markRead(network: net.network, target: buf.target) }
            }
            .task(id: net.status == nil) {
                if net.status == nil && !buf.dm { model.client.members(network: net.network, channel: buf.target) }
            }
        } else {
            // Just opened: the core lists it in a moment.
            ProgressView()
        }
    }

    @ToolbarContentBuilder
    private func toolbar(net: Network, buf: Buffer) -> some ToolbarContent {
        ToolbarItem(placement: .principal) {
            VStack(spacing: 0) {
                HStack(spacing: 4) {
                    Text(buf.target).font(.headline).lineLimit(1)
                    if buf.encryption != .none && buf.encryption != .off {
                        Image(systemName: "lock.fill")
                            .font(.caption)
                            .foregroundStyle(buf.encryption == .changed ? .red : .accentColor)
                            .accessibilityLabel(buf.encryption == .verified ? "Encrypted, verified" : "Encrypted")
                    }
                }
                Text(buf.topic.isEmpty ? net.name : buf.topic)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            .accessibilityElement(children: .combine)
        }
        ToolbarItemGroup(placement: .topBarTrailing) {
            if !buf.dm && buf.voiceAllowed && !net.guest && net.voice?.lowercased() != buf.target.lowercased() {
                Button { joinVoice(net: net, channel: buf.target) } label: { Image(systemName: "phone") }
                    .accessibilityLabel("Join voice")
            }
            if buf.dm && net.status == nil && (buf.encryption == .on || buf.encryption == .changed) {
                Button("Verify") { model.client.verification(network: net.network, nick: buf.target, step: .start) }
            }
            if !buf.dm {
                Button { members = true } label: { Image(systemName: "person.2") }
                    .accessibilityLabel("Members")
            }
        }
    }

    /// Joins voice once the mic is allowed: without it there's no voice, not
    /// even listen-only (same as Android).
    private func joinVoice(net: Network, channel: String) {
        AVAudioApplication.requestRecordPermission { ok in
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    if ok {
                        model.client.joinVoice(network: net.network, channel: channel)
                    } else {
                        model.toast("Voice needs the microphone. Allow it in Settings › IrChad.")
                    }
                }
            }
        }
    }
}

/// Connecting, reconnecting, a failed login, or a sign-up waiting on its code.
private struct StatusBanner: View {
    @Environment(AppModel.self) private var model
    let net: Network
    @State private var input = ""

    var body: some View {
        if let status = net.status {
            VStack(alignment: .leading, spacing: 8) {
                Text(status).font(.callout)
                if net.failed || net.needsCode {
                    HStack {
                        Group {
                            if net.needsCode {
                                TextField("Code from the email", text: $input)
                                    .textContentType(.oneTimeCode)
                            } else {
                                SecureField("Password", text: $input)
                                    .textContentType(.password)
                            }
                        }
                        .textFieldStyle(.roundedBorder)
                        Button(net.needsCode ? "Verify" : "Sign in") {
                            if net.needsCode {
                                model.client.verify(network: net.network, code: input.trimmingCharacters(in: .whitespaces))
                            } else {
                                model.client.signIn(network: net.network, password: input)
                            }
                            input = ""
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(input.trimmingCharacters(in: .whitespaces).isEmpty)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(12)
            .background(Color.accentColor.opacity(0.12))
        }
    }
}

private struct KeysChanged: View {
    @Environment(AppModel.self) private var model
    let net: Network
    let buf: Buffer

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("\(buf.target)'s encryption keys changed. That's normal after they reinstall, but it can also mean someone is listening in. Verify to be sure.")
                .font(.callout)
            Button("Accept the new keys") { model.client.acceptKeys(network: net.network, nick: buf.target) }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(Color.red.opacity(0.15))
    }
}

private struct MessageList: View {
    @Environment(AppModel.self) private var model
    let net: Network
    let buf: Buffer
    @Binding var replyTo: Message?
    @Binding var editing: Message?
    @State private var reactTo: String?
    /// A reply's parent asked for with `loadAround`, to scroll to once loaded.
    @State private var jump: String?

    var body: some View {
        // Own messages change nothing in `buf`, so depend on the core directly.
        let _ = model.tick
        let msgs = model.client.messages(network: net.network, target: buf.target)
        let connected = net.status == nil
        let byId = Dictionary(msgs.compactMap { m in m.msgid.map { ($0, m) } }, uniquingKeysWith: { a, _ in a })
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    if !buf.historyStart {
                        ProgressView()
                            .frame(maxWidth: .infinity)
                            .padding()
                            // Asks again each time a page lands and it's still in view.
                            .task(id: "\(msgs.count) \(connected)") {
                                if connected { model.client.loadOlder(network: net.network, target: buf.target) }
                            }
                    }
                    ForEach(Array(msgs.enumerated()), id: \.element.key) { i, m in
                        let prev = i > 0 ? msgs[i - 1] : nil
                        let head = prev == nil || prev!.nick != m.nick || m.time - prev!.time > groupGapMs
                            || prev!.action || m.action || m.replyTo != nil
                        MessageRow(
                            network: net.network, m: m, head: head, dm: buf.dm, ownNick: net.nick,
                            parent: m.replyTo.flatMap { byId[$0] },
                            onParent: { id in
                                if byId[id] != nil {
                                    withAnimation { proxy.scrollTo(id, anchor: .center) }
                                } else {
                                    jump = id
                                    model.client.loadAround(network: net.network, target: buf.target, msgid: id)
                                }
                            },
                            onReply: { replyTo = m; editing = nil },
                            onEdit: { editing = m; replyTo = nil },
                            onReact: { reactTo = m.msgid },
                            target: buf.target,
                            seenAt: m.msgid != nil && m.msgid == buf.seen ? buf.seenAt : nil
                        )
                    }
                }
                .padding(.bottom, 8)
            }
            .defaultScrollAnchor(.bottom)
            .defaultScrollAnchor(.bottom, for: .sizeChanges)
            .scrollDismissesKeyboard(.interactively)
            .onChange(of: msgs.count) { _, _ in
                if let j = jump, byId[j] != nil {
                    jump = nil
                    proxy.scrollTo(j, anchor: .center)
                }
            }
        }
        .overlay(alignment: .bottomLeading) {
            if !buf.typing.isEmpty {
                let who = buf.typing
                Text(who.count == 1 ? "\(who[0]) is typing…"
                    : who.count == 2 ? "\(who[0]) and \(who[1]) are typing…" : "Several people are typing…")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 2)
                    .background(.background)
            }
        }
        .sheet(item: Binding(get: { reactTo.map(IdBox.init) }, set: { reactTo = $0?.id })) { r in
            EmojiGifPicker(network: net.network, gifs: nil, onEmoji: { e in
                model.client.react(network: net.network, target: buf.target, msgid: r.id, emoji: e)
                reactTo = nil
            }, onGif: { _ in })
        }
    }
}

struct MessageRow: View {
    @Environment(AppModel.self) private var model
    @Environment(\.colorScheme) private var scheme
    let network: String
    let m: Message
    let head: Bool
    let dm: Bool
    let ownNick: String
    let parent: Message?
    let onParent: (String) -> Void
    let onReply: () -> Void
    let onEdit: () -> Void
    let onReact: () -> Void
    let target: String
    /// When they read up to this message, if "Seen" goes under it (D68).
    let seenAt: Int64?

    var body: some View {
        let highlight = m.mention && !dm
        HStack(alignment: .top, spacing: 10) {
            Rectangle().fill(highlight ? Color.accentColor : .clear).frame(width: 3)
            if head {
                Avatar(network: network, nick: m.nick, url: m.avatar)
            } else {
                Color.clear.frame(width: 36, height: 1)
            }
            VStack(alignment: .leading, spacing: 2) {
                if let rid = m.replyTo {
                    Button {
                        onParent(rid)
                    } label: {
                        Text("↪ " + (parent.map { p in "\(p.shown): " + (p.redacted ? "Message deleted" : p.text) }
                            ?? "Replying to an earlier message"))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                    .buttonStyle(.plain)
                    .accessibilityHint("Shows the message it answers")
                }
                if head {
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(m.shown).fontWeight(.semibold).foregroundStyle(nickColor(m.nick, dark: scheme == .dark))
                        Text(Date(timeIntervalSince1970: TimeInterval(m.time) / 1000), style: .time)
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                        if m.encrypted {
                            Image(systemName: "lock.fill").font(.caption2).foregroundStyle(.secondary)
                                .accessibilityLabel("Encrypted")
                        }
                    }
                }
                messageBody
                if m.edited && !m.redacted {
                    Text("(edited)").font(.caption2).foregroundStyle(.secondary)
                }
                MessageLinks(network: network, m: m)
                if let f = m.failed {
                    Text("Not sent: \(f)").font(.caption).foregroundStyle(.red)
                }
                if !m.reactions.isEmpty { reactions }
                if let at = seenAt {
                    let when = Date(timeIntervalSince1970: TimeInterval(at) / 1000)
                    Text("Seen \(when.formatted(date: .abbreviated, time: .shortened))")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.trailing, 16)
        .padding(.top, head ? 10 : 1)
        .padding(.bottom, 1)
        .background(highlight ? Color.accentColor.opacity(0.08) : .clear)
        .contentShape(Rectangle())
        .contextMenu { menu }
    }

    @ViewBuilder private var messageBody: some View {
        if m.redacted {
            Text("Message deleted").italic().foregroundStyle(.secondary)
        } else if m.undecryptable {
            Text("Couldn't decrypt this message.").italic().foregroundStyle(.red)
        } else if m.action {
            styledText(m.spans, prefix: "* \(m.shown) ", network: network, media: model.media).italic()
        } else {
            styledText(m.spans, network: network, media: model.media)
                .foregroundStyle(m.pending || m.notice ? .secondary : .primary)
                .textSelection(.enabled)
        }
    }

    /// A custom emoji's image, or its `:name:` until that has loaded.
    private func reactionLabel(_ r: Reaction) -> Text {
        let _ = model.media.tick
        guard let url = r.image, let i = model.media.emojiImage(network: network, url: url, height: 16) else {
            return Text("\(r.emoji) \(r.nicks.count)")
        }
        return Text(Image(uiImage: i)).baselineOffset(-3) + Text(" \(r.nicks.count)")
    }

    private var reactions: some View {
        HStack(spacing: 4) {
            ForEach(m.reactions, id: \.emoji) { r in
                let mine = r.nicks.contains { $0.lowercased() == ownNick.lowercased() }
                Button {
                    guard let id = m.msgid else { return }
                    if mine {
                        model.client.unreact(network: network, target: target, msgid: id, emoji: r.emoji)
                    } else {
                        model.client.react(network: network, target: target, msgid: id, emoji: r.emoji)
                    }
                } label: {
                    reactionLabel(r)
                        .font(.caption)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 2)
                        .background(mine ? Color.accentColor.opacity(0.25) : Color(.secondarySystemBackground), in: Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("\(r.emoji), \(r.nicks.count): \(r.nicks.joined(separator: ", "))")
                .accessibilityHint(mine ? "Takes back your reaction" : "Reacts the same")
            }
        }
        .padding(.top, 2)
    }

    @ViewBuilder private var menu: some View {
        if m.msgid != nil && !m.redacted {
            Button("Reply", systemImage: "arrowshape.turn.up.left", action: onReply)
            ForEach(["👍", "❤️", "😂", "😮"], id: \.self) { e in
                Button("React \(e)") {
                    if let id = m.msgid { model.client.react(network: network, target: target, msgid: id, emoji: e) }
                }
            }
            Button("React…", systemImage: "face.smiling", action: onReact)
        }
        if !m.redacted && !m.text.isEmpty {
            Button("Copy text", systemImage: "doc.on.doc") { UIPasteboard.general.string = m.text }
        }
        if m.own && m.msgid != nil && !m.redacted && !m.notice {
            Button("Edit", systemImage: "pencil", action: onEdit)
            Button("Delete", systemImage: "trash", role: .destructive) {
                if let id = m.msgid { model.client.redact(network: network, target: target, msgid: id) }
            }
        }
        if !m.own {
            Button("Block \(m.shown)", systemImage: "hand.raised", role: .destructive) {
                model.client.block(network: network, nick: m.nick, account: m.account)
            }
        }
    }
}

/// Their picture if they set one (loaded through chadd like any image), else
/// their initial on their colour.
struct Avatar: View {
    @Environment(AppModel.self) private var model
    @Environment(\.colorScheme) private var scheme
    let network: String
    let nick: String
    let url: String?
    var size: CGFloat = 36

    var body: some View {
        let _ = model.media.tick
        let color = nickColor(nick, dark: scheme == .dark)
        ZStack {
            Circle().fill(color.opacity(0.25))
            if let url, case let .ready(img)? = model.media.image(network: network, url: url, ask: model.media.auto) {
                Image(uiImage: img).resizable().scaledToFill()
            } else {
                Text(nick.prefix(1).uppercased()).font(.system(size: size * 0.45, weight: .bold)).foregroundStyle(color)
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .accessibilityHidden(true)
    }
}

/// Stable per nick (FNV-1a, same as desktop and Android), readable on both themes.
func nickColor(_ nick: String, dark: Bool) -> Color {
    var h: UInt32 = 2_166_136_261
    for b in nick.utf8 { h = (h ^ UInt32(b)) &* 16_777_619 }
    return Color(hue: Double(h % 360) / 360, saturation: 0.55, brightness: dark ? 0.9 : 0.55)
}

/// `styled`, with each custom emoji that has loaded as its image. `Text` takes
/// an image only as another `Text`, so the runs between them are joined.
@MainActor
func styledText(_ spans: [Span], prefix: String = "", network: String, media: Media) -> Text {
    guard spans.contains(where: { $0.emoji != nil }) else { return Text(styled(spans, prefix: prefix)) }
    let _ = media.tick
    let height = UIFont.preferredFont(forTextStyle: .body).lineHeight
    var out = Text(verbatim: "")
    var run: [Span] = []
    var lead = prefix
    for sp in spans {
        guard let url = sp.emoji, let i = media.emojiImage(network: network, url: url, height: height) else {
            run.append(sp)
            continue
        }
        out = out + Text(styled(run, prefix: lead))
            + Text(Image(uiImage: i)).baselineOffset(-height * 0.2).accessibilityLabel(sp.text)
        run = []
        lead = ""
    }
    return out + Text(styled(run, prefix: lead))
}

/// Message text with its IRC formatting, and its links tappable.
func styled(_ spans: [Span], prefix: String = "") -> AttributedString {
    var out = AttributedString(prefix)
    for sp in spans {
        // An `@`-mention, with the person's current name (D66).
        if let name = sp.mention {
            var a = AttributedString("@\(name)")
            a.font = sp.bold ? Font.body.bold() : Font.body.weight(.semibold)
            a.foregroundColor = .accentColor
            a.backgroundColor = Color.accentColor.opacity(0.15)
            out += a
            continue
        }
        var a = AttributedString(sp.text)
        var font = Font.body
        if sp.mono { font = font.monospaced() }
        if sp.bold { font = font.bold() }
        if sp.italic { font = font.italic() }
        a.font = font
        if sp.underline || sp.link { a.underlineStyle = .single }
        if sp.strike { a.strikethroughStyle = .single }
        if let fg = sp.fg { a.foregroundColor = Color(rgb: fg) } else if sp.link { a.foregroundColor = .accentColor }
        if let bg = sp.bg { a.backgroundColor = Color(rgb: bg) }
        // The core only marks http(s) as links.
        if sp.link, let u = URL(string: sp.text), u.scheme == "https" || u.scheme == "http" { a.link = u }
        out += a
    }
    return out
}

extension Color {
    init(rgb: UInt32) {
        self.init(
            red: Double((rgb >> 16) & 0xFF) / 255,
            green: Double((rgb >> 8) & 0xFF) / 255,
            blue: Double(rgb & 0xFF) / 255
        )
    }
}

private struct MembersSheet: View {
    @Environment(AppModel.self) private var model
    let net: Network
    let channel: String
    let open: (String) -> Void

    var body: some View {
        let _ = model.tick
        let list = model.client.memberList(network: net.network, channel: channel)
        NavigationStack {
            List {
                if let list {
                    ForEach([false, true], id: \.self) { away in
                        let group = list.filter { $0.away == away }
                        if !group.isEmpty {
                            Section("\(away ? "Away" : "Online") — \(group.count)") {
                                ForEach(group, id: \.nick) { m in
                                    Button {
                                        if m.nick.lowercased() != net.nick.lowercased() { open(m.nick) }
                                    } label: {
                                        let p = model.client.profile(network: net.network, nick: m.nick)
                                        HStack(spacing: 12) {
                                            Avatar(network: net.network, nick: m.nick, url: p.avatar)
                                            VStack(alignment: .leading, spacing: 2) {
                                                HStack {
                                                    if m.oper {
                                                        Image(systemName: "crown.fill").font(.caption)
                                                            .foregroundStyle(.yellow)
                                                            .accessibilityLabel("IRC operator")
                                                    }
                                                    if m.account == nil {
                                                        Text("👻").accessibilityLabel("Guest (not logged in)")
                                                    }
                                                    Text(m.prefix + m.shown).foregroundStyle(m.away ? .secondary : .primary)
                                                }
                                                if let a = p.activity {
                                                    Label(a, systemImage: a.hasPrefix("Listening to") ? "music.note" : "gamecontroller.fill")
                                                        .font(.caption)
                                                        .foregroundStyle(.secondary)
                                                        .lineLimit(1)
                                                }
                                            }
                                        }
                                    }
                                    .buttonStyle(.plain)
                                }
                            }
                        }
                    }
                } else {
                    ProgressView()
                }
            }
            .navigationTitle("Members" + (list.map { " · \($0.count)" } ?? ""))
            .navigationBarTitleDisplayMode(.inline)
        }
        .presentationDetents([.medium, .large])
    }
}
