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
    @Environment(\.horizontalSizeClass) private var size
    /// The member list as a column on wide screens, like desktop; remembered.
    @AppStorage("membersPane") private var pane = true
    /// The member list as a sheet on narrow ones.
    @State private var members = false

    var body: some View {
        let _ = model.tick
        let net = model.client.networks().first { $0.network == sel.network }
        let buf = model.client.buffers(network: sel.network).first { $0.target.lowercased() == sel.target.lowercased() }
        if let net, let buf {
            VStack(spacing: 0) {
                StatusBanner(net: net)
                if buf.encryption == .changed { KeysChanged(net: net, buf: buf) }
                MessageList(net: net, buf: buf, replyTo: $replyTo, editing: $editing, openDm: openDm(net))
            }
            .bottomBar {
                Composer(net: net, buf: buf, replyTo: $replyTo, editing: $editing)
            }
            .toolbar { toolbar(net: net, buf: buf) }
            .navigationBarTitleDisplayMode(.inline)
            // A column when there's room, a sheet when not (Split View, Slide Over, phones).
            .inspector(isPresented: membersShown(buf)) {
                MembersSheet(net: net, channel: buf.target, open: openDm(net))
                .inspectorColumnWidth(min: 200, ideal: 260, max: 360)
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
                Button { membersShown(buf).wrappedValue.toggle() } label: { Image(systemName: "person.2") }
                    .accessibilityLabel("Members")
            }
        }
    }

    private func openDm(_ net: Network) -> (String) -> Void {
        { nick in
            members = false
            model.client.openDm(network: net.network, nick: nick)
            select(Sel(network: net.network, target: nick))
        }
    }

    /// The column's choice only counts while wide: narrowing the window doesn't pop up the sheet.
    private func membersShown(_ buf: Buffer) -> Binding<Bool> {
        Binding(
            get: { !buf.dm && (size == .regular ? pane : members) },
            set: { if size == .regular { pane = $0 } else { members = $0 } }
        )
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
struct StatusBanner: View {
    @Environment(AppModel.self) private var model
    let net: Network
    @State private var input = ""

    var body: some View {
        if let status = net.status {
            VStack(alignment: .leading, spacing: 8) {
                Text(status).font(.callout)
                if net.failed && net.discord {
                    // No password to type: the browser again (D96).
                    if let f = model.joinFailure, f.invite == net.network {
                        Text(f.error).font(.callout).foregroundStyle(.red)
                    }
                    Button {
                        model.joinFailure = nil
                        model.discordStart(net.network)
                    } label: {
                        DiscordLabel()
                    }
                    .glassButton(prominent: true)
                    .tint(.blurple)
                } else if net.failed || net.needsCode {
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
                        .glassButton(prominent: true)
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
    let openDm: (String) -> Void
    @State private var reactTo: String?
    /// Whose profile card is up.
    @State private var card: String?
    /// Scrolled up far enough to offer a way back down.
    @State private var away = false
    @State private var scroller = ScrollerRef()
    /// Jumping to the end: rows measured after it lands (previews, images) push
    /// the end further down, so it follows the end until the user drags.
    @State private var chase = false
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
                                chase = false
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
                            onProfile: { card = m.nick },
                            target: buf.target,
                            seenAt: m.msgid != nil && m.msgid == buf.seen ? buf.seenAt : nil
                        )
                    }
                }
                .padding(.bottom, 8)
                .background(ScrollerFinder(ref: scroller))
            }
            .defaultScrollAnchor(.bottom)
            .defaultScrollAnchor(.bottom, for: .sizeChanges)
            .scrollDismissesKeyboard(.interactively)
            .onScrollGeometryChange(for: CGFloat.self) { g in
                g.contentSize.height - g.visibleRect.maxY
            } action: { _, left in
                if (left > 300) != away { withAnimation(.smooth(duration: 0.2)) { away = left > 300 } }
                if chase && left > 1 { toBottom() }
            }
            .onScrollPhaseChange { _, p in
                // Not `.tracking`: tapping the chip mid-fling touches the scroll view too.
                if p == .interacting { chase = false }
            }
            .overlay(alignment: .bottom) {
                if away {
                    Button {
                        chase = true
                        toBottom()
                    } label: {
                        HStack(spacing: 6) { Text("Jump to latest"); Image(systemName: "arrow.down") }
                            .font(.callout.weight(.medium))
                            .padding(.horizontal, 14)
                            .padding(.vertical, 8)
                            // A plain button only takes taps where it draws: make the whole pill count.
                            .contentShape(Capsule())
                    }
                    .buttonStyle(.plain)
                    .glass(in: Capsule(), fallback: AnyShapeStyle(.regularMaterial))
                    .padding(.bottom, 8)
                    .transition(.opacity.combined(with: .scale(scale: 0.85, anchor: .bottom)))
                }
            }
            .onChange(of: msgs.count) { _, _ in
                if let j = jump, byId[j] != nil {
                    jump = nil
                    proxy.scrollTo(j, anchor: .center)
                }
            }
        }
        // An inset, not an overlay: the list makes room instead of hiding its last message.
        // The room is kept while nobody's typing, so the list doesn't jump when someone starts.
        .safeAreaInset(edge: .bottom, spacing: 0) {
            let who = buf.typing
            let text = who.count == 1 ? "\(who[0]) is typing"
                : who.count == 2 ? "\(who[0]) and \(who[1]) are typing" : "Several people are typing"
            ZStack(alignment: .leading) {
                typing("Typing").hidden()
                if !who.isEmpty {
                    typing(text)
                        .transition(.opacity.combined(with: .scale(scale: 0.85, anchor: .leading)))
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .animation(.smooth(duration: 0.3), value: who.isEmpty)
            .animation(.smooth(duration: 0.2), value: text)
        }
        .sheet(item: Binding(get: { reactTo.map(IdBox.init) }, set: { reactTo = $0?.id })) { r in
            EmojiGifPicker(network: net.network, gifs: nil, onEmoji: { e in
                model.client.react(network: net.network, target: buf.target, msgid: r.id, emoji: e)
                reactTo = nil
            }, onGif: { _ in })
        }
        .sheet(item: Binding(get: { card.map(IdBox.init) }, set: { card = $0?.id })) { c in
            ProfileCard(net: net, channel: buf.target, nick: c.id,
                        account: msgs.last { $0.nick == c.id }?.account, openDm: openDm)
        }
    }

    /// Straight to the end of the content as measured right now. SwiftUI's own
    /// jump loses to a fling and does nothing when asked twice for the same edge.
    private func toBottom() {
        guard let sv = scroller.view else { return }
        let inset = sv.adjustedContentInset
        let y = max(-inset.top, sv.contentSize.height + inset.bottom - sv.bounds.height)
        sv.setContentOffset(CGPoint(x: sv.contentOffset.x, y: y), animated: false)
    }

    private func typing(_ text: String) -> some View {
        HStack(spacing: 4) {
            Image(systemName: "ellipsis")
                .symbolEffect(.variableColor.iterative.dimInactiveLayers)
            Text(text).contentTransition(.opacity)
        }
        .font(.caption)
        .foregroundStyle(.secondary)
        .padding(.horizontal, 12)
        .padding(.vertical, 4)
        .glass(in: Capsule(), fallback: AnyShapeStyle(.background))
        .padding(.horizontal, 10)
        .padding(.bottom, 2)
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
    let onProfile: () -> Void
    let target: String
    /// When they read up to this message, if "Seen" goes under it (D68).
    let seenAt: Int64?
    /// Swipe right to left to reply. `horizontal` is decided by the first movement, so scrolling never swipes;
    /// a `GestureState` springs back on its own even when the scroll view takes the touch.
    @GestureState(resetTransaction: Transaction(animation: .snappy)) private var swipe: (horizontal: Bool?, x: CGFloat) = (nil, 0)
    private let reach: CGFloat = 64
    /// A message's actions: a drawer on phones, a popover by the message on iPads. Not a
    /// context menu: its lift preview swells the row, and its long press is slower.
    @Environment(\.horizontalSizeClass) private var size
    @State private var drawer = false
    /// What the drawer's pick does once it has closed: a reply's keyboard or the next sheet needs it gone.
    @State private var pending: (() -> Void)?

    var body: some View {
        let actions = MessageDrawer(network: network, target: target, m: m, run: { act in
            pending = act
            drawer = false
        }, onReply: onReply, onEdit: onEdit, onReact: onReact)
        let row = row
            .simultaneousGesture(LongPressGesture(minimumDuration: 0.3).onEnded { _ in drawer = true })
            .sensoryFeedback(.impact, trigger: drawer) { _, open in open }
        if size == .compact {
            row.sheet(isPresented: $drawer, onDismiss: { pending?(); pending = nil }) { actions }
        } else {
            row
                .popover(isPresented: $drawer) { actions.frame(width: 360) }
                .onChange(of: drawer) { _, open in
                    if !open { pending?(); pending = nil }
                }
        }
    }

    @ViewBuilder private var row: some View {
        let highlight = m.mention && !dm
        HStack(alignment: .top, spacing: 10) {
            Rectangle().fill(highlight ? Color.accentColor : .clear).frame(width: 3)
            if head {
                Avatar(network: network, nick: m.nick, url: m.avatar, size: 40)
                    .onTapGesture(perform: onProfile)
            } else {
                Color.clear.frame(width: 40, height: 1)
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
                        Button(action: onProfile) {
                            Text(m.shown).fontWeight(.semibold).foregroundStyle(nickColor(m.nick, dark: scheme == .dark))
                        }
                        .buttonStyle(.plain)
                        .accessibilityHint("Shows their profile")
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
        .offset(x: swipe.x)
        .background(alignment: .trailing) {
            if swipe.x < 0 {
                Image(systemName: "arrowshape.turn.up.left.fill")
                    .foregroundStyle(.secondary)
                    .opacity(min(1, -swipe.x / reach))
                    .padding(.trailing, 20)
            }
        }
        .contentShape(Rectangle())
        .simultaneousGesture(swipeToReply, including: m.msgid != nil && !m.redacted ? .all : .subviews)
        .sensoryFeedback(.impact, trigger: swipe.x <= -reach) { _, past in past }
    }

    private var swipeToReply: some Gesture {
        DragGesture(minimumDistance: 15)
            .updating($swipe) { v, s, _ in
                if s.horizontal == nil { s.horizontal = abs(v.translation.width) > abs(v.translation.height) }
                if s.horizontal == true { s.x = min(0, max(-reach, v.translation.width)) }
            }
            .onEnded { v in
                if abs(v.translation.width) > abs(v.translation.height) && v.translation.width <= -reach { onReply() }
            }
    }

    /// The text without links to images that show inline.
    private var spans: [Span] {
        let hidden = shownImages(model, network: network, m)
        guard !hidden.isEmpty else { return m.spans }
        let blank = { (s: Span) in s.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        let kept = m.spans.filter { !($0.link && hidden.contains($0.text)) }
        guard let first = kept.firstIndex(where: { !blank($0) }), let last = kept.lastIndex(where: { !blank($0) }) else {
            return []
        }
        return Array(kept[first...last])
    }

    @ViewBuilder private var messageBody: some View {
        if m.redacted {
            Text("Message deleted").italic().foregroundStyle(.secondary)
        } else if m.undecryptable {
            Text("Couldn't decrypt this message.").italic().foregroundStyle(.red)
        } else if m.action {
            styledText(spans, prefix: "* \(m.shown) ", network: network, media: model.media).italic()
        } else if spans.isEmpty && !m.spans.isEmpty {
            // Nothing but shown images: no empty line above them.
            EmptyView()
        } else {
            styledText(spans, network: network, media: model.media)
                .foregroundStyle(m.pending || m.notice ? .secondary : .primary)
                .textSelection(.enabled)
        }
    }

    /// A custom emoji's image, or its `:name:` until that has loaded.
    private func reactionLabel(_ r: Reaction) -> Text {
        let _ = model.media.tick
        guard let url = r.image, let i = model.media.emojiImage(network: network, url: url, height: 18) else {
            return Text(r.emoji).font(.system(size: 16)) + Text("  \(r.nicks.count)")
        }
        return Text(Image(uiImage: i)).baselineOffset(-4) + Text("  \(r.nicks.count)")
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
                    // Discord's size: 28pt tall, an 18pt emoji, about 50pt wide.
                    reactionLabel(r)
                        .font(.system(size: 14))
                        .padding(.horizontal, 8)
                        .frame(height: 28)
                        .background(mine ? Color.accentColor.opacity(0.25) : Color(.secondarySystemBackground),
                                    in: RoundedRectangle(cornerRadius: 8))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("\(r.emoji), \(r.nicks.count): \(r.nicks.joined(separator: ", "))")
                .accessibilityHint(mine ? "Takes back your reaction" : "Reacts the same")
            }
        }
        .padding(.top, 2)
    }
}

/// A message's actions (phone drawer, iPad popover): quick reactions (as many as fit, then the full
/// picker), reply and copy, then the destructive ones.
struct MessageDrawer: View {
    @Environment(AppModel.self) private var model
    let network: String
    let target: String
    let m: Message
    /// Closes the drawer, then runs the action.
    let run: (@escaping () -> Void) -> Void
    let onReply: () -> Void
    let onEdit: () -> Void
    let onReact: () -> Void
    @State private var width: CGFloat = 0
    @State private var height: CGFloat = 300

    private static let quick = ["👍", "❤️", "😂", "😮", "😢", "🔥", "🎉", "👀", "🙏", "💯", "😡", "👎"]

    var body: some View {
        let sent = m.msgid != nil && !m.redacted
        let canReport = !m.own && model.client.reportTo(network: network) != nil
        VStack(spacing: 0) {
            if sent {
                // 48pt each, 8pt apart, and the picker's.
                let fit = max(1, Int((width + 8) / 56) - 1)
                HStack {
                    ForEach(Self.quick.prefix(fit), id: \.self) { e in
                        Button {
                            run { if let id = m.msgid { model.client.react(network: network, target: target, msgid: id, emoji: e) } }
                        } label: {
                            Text(e).font(.system(size: 24)).frame(width: 48, height: 48)
                                .background(Color(.secondarySystemBackground), in: Circle())
                        }
                        .accessibilityLabel("React \(e)")
                        Spacer(minLength: 0)
                    }
                    Button { run(onReact) } label: {
                        Image(systemName: "face.smiling").font(.system(size: 22)).frame(width: 48, height: 48)
                            .background(Color(.secondarySystemBackground), in: Circle())
                    }
                    .accessibilityLabel("More reactions")
                }
                .buttonStyle(.plain)
                .frame(maxWidth: .infinity)
                .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
                item("Reply", "arrowshape.turn.up.left", action: onReply)
            }
            if !m.redacted && !m.text.isEmpty {
                item("Copy text", "doc.on.doc") { UIPasteboard.general.string = m.text }
            }
            if m.own && sent && !m.notice { item("Edit", "pencil", action: onEdit) }
            if (m.own && sent) || !m.own { Divider().padding(.vertical, 8) }
            if m.own && sent {
                item("Delete", "trash", danger: true) {
                    if let id = m.msgid { model.client.redact(network: network, target: target, msgid: id) }
                }
            }
            if !m.own {
                item("Block \(m.shown)", "hand.raised", danger: true) {
                    model.client.block(network: network, nick: m.nick, account: m.account)
                }
                if canReport {
                    item("Report", "flag", danger: true) {
                        model.reporting = ReportDraft(
                            network: network, nick: m.nick, target: target,
                            msgid: m.msgid, text: m.text.isEmpty ? nil : m.text
                        )
                    }
                }
            }
        }
        .padding(.top, 20)
        .padding(.bottom, 8)
        .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height = $0 }
        .presentationDetents([.height(height)])
        .presentationDragIndicator(.visible)
    }

    private func item(_ label: String, _ icon: String, danger: Bool = false, action: @escaping () -> Void) -> some View {
        Button { run(action) } label: {
            Label(label, systemImage: icon)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 24)
                .padding(.vertical, 14)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(danger ? Color.red : Color.primary)
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
        let _ = url.map { _ in model.media.tick } // no picture, nothing to wait for
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

/// Someone's activity, with its track's progress ticking under it.
struct ActivityLine: View {
    let text: String
    let music: Music?

    var body: some View {
        Label(text, systemImage: icon)
            .font(.caption)
            .foregroundStyle(.secondary)
            .lineLimit(1)
        if let music, music.length > 0 {
            TimelineView(.periodic(from: .now, by: 1)) { ctx in
                ProgressView(value: Double(at(music, ctx.date)), total: Double(music.length))
                    .frame(width: 120)
            }
        }
    }

    private var icon: String {
        if text.hasPrefix("Listening to") { return "music.note" }
        if text.hasPrefix("Watching") { return "play.tv" }
        return "gamecontroller.fill"
    }

    private func at(_ m: Music, _ date: Date) -> UInt64 {
        let now = UInt64(max(date.timeIntervalSince1970, 0))
        return min(now - min(now, m.start), UInt64(m.length))
    }
}

/// One member in the list: tap for their profile card.
private struct MemberRow: View {
    @Environment(AppModel.self) private var model
    let net: Network
    let m: Member
    let open: (String) -> Void

    var body: some View {
        let _ = model.tick // profiles change without the member changing
        let p = model.client.profile(network: net.network, nick: m.nick)
        Button {
            open(m.nick)
        } label: {
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
                        Text(m.shown).foregroundStyle(m.away ? .secondary : .primary)
                    }
                    if let a = p.activity {
                        ActivityLine(text: a, music: nil) // progress is for the profile card
                    }
                }
            }
        }
        .buttonStyle(.plain)
    }
}

private struct MembersSheet: View {
    @Environment(AppModel.self) private var model
    let net: Network
    let channel: String
    let open: (String) -> Void
    @State private var card: String?

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
                                    MemberRow(net: net, m: m) { card = $0 }
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
        .sheet(item: Binding(get: { card.map(IdBox.init) }, set: { card = $0?.id })) { c in
            ProfileCard(net: net, channel: channel, nick: c.id, openDm: open)
        }
    }
}

/// The UIScrollView behind a SwiftUI ScrollView, for what SwiftUI can't do.
final class ScrollerRef {
    weak var view: UIScrollView?
}

/// Put in a ScrollView's content: finds the UIScrollView it sits in.
struct ScrollerFinder: UIViewRepresentable {
    let ref: ScrollerRef

    func makeUIView(context: Context) -> UIView {
        let v = UIView()
        v.isUserInteractionEnabled = false
        return v
    }

    func updateUIView(_ v: UIView, context: Context) {
        // Not in the hierarchy until the next turn of the run loop.
        DispatchQueue.main.async {
            var s = v.superview
            while let x = s, !(x is UIScrollView) { s = x.superview }
            ref.view = s as? UIScrollView
        }
    }
}
