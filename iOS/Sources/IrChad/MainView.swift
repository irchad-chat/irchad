import SwiftUI

/// Networks and their conversations on the left, the chat on the right; one
/// screen at a time on a phone.
struct MainView: View {
    @Environment(AppModel.self) private var model
    let nets: [Network]
    let onAdd: () -> Void

    @State private var selection: Sel?
    /// The selected conversation has been in the list: once it's gone again
    /// (parted, closed elsewhere) it's deselected. A just-opened one isn't
    /// listed until the core gets to it.
    @State private var listed = false
    @State private var settings = false
    @State private var dialog: Dialog?
    @State private var profile: String?
    @State private var collapsed = Set<String>()
    @State private var column = NavigationSplitViewVisibility.automatic

    private enum Dialog: Identifiable {
        case channel(String), dm(String)
        var id: String {
            switch self {
            case let .channel(n): "c" + n
            case let .dm(n): "d" + n
            }
        }
    }

    var body: some View {
        NavigationSplitView(columnVisibility: $column) {
            sidebar
        } detail: {
            if let sel = selection, !sel.target.isEmpty {
                ChatView(sel: sel, select: { selection = $0 })
                    .id(sel)
            } else {
                ContentUnavailableView("Pick a conversation", systemImage: "bubble.left.and.bubble.right")
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            if model.calls.state != nil { VoicePanel(nets: nets) }
        }
        .fullScreenCover(isPresented: Binding(
            get: { model.calls.state?.watching != nil },
            set: { if !$0 { model.calls.watch(nil) } }
        )) {
            ScreenViewer()
        }
        .sheet(isPresented: $settings) { SettingsView() }
        .sheet(item: Binding(get: { profile.map(IdBox.init) }, set: { profile = $0?.id })) { p in
            ProfileView(network: p.id)
        }
        .sheet(isPresented: Binding(get: { model.sounds.board != nil }, set: { if !$0 { model.sounds.board = nil } })) {
            SoundboardSheet()
        }
        .alert(dialogTitle, isPresented: Binding(get: { dialog != nil }, set: { if !$0 { dialog = nil } })) {
            DialogFields(id: dialog?.id ?? "") { network, name, channel in
                if channel {
                    model.client.joinChannel(network: network, channel: name)
                } else {
                    model.client.openDm(network: network, nick: name)
                }
                selection = Sel(network: network, target: name)
            }
        }
        .onChange(of: model.focus, initial: true) { _, f in
            guard let f else { return }
            model.focus = nil
            let target = f.target.isEmpty
                ? (model.client.buffers(network: f.network).first?.target ?? "")
                : f.target
            selection = Sel(network: f.network, target: target)
        }
        .onChange(of: selection) { _, _ in listed = false }
        .onChange(of: model.tick) { _, _ in
            guard let s = selection, !s.target.isEmpty else { return }
            let there = model.client.buffers(network: s.network).contains { $0.target.lowercased() == s.target.lowercased() }
            if there {
                listed = true
            } else if listed {
                selection = nil
            }
        }
    }

    private var dialogTitle: String {
        switch dialog {
        case .channel: "Join a channel"
        case .dm: "Message someone"
        case nil: ""
        }
    }

    private var sidebar: some View {
        List(selection: $selection) {
            ForEach(nets, id: \.network) { n in
                Section(isExpanded: Binding(
                    get: { !collapsed.contains(n.network) },
                    set: { if $0 { collapsed.remove(n.network) } else { collapsed.insert(n.network) } }
                )) {
                    if let status = n.status {
                        Label(status, systemImage: n.failed ? "exclamationmark.triangle" : "arrow.triangle.2.circlepath")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                    let bufs = model.client.buffers(network: n.network)
                    ForEach(bufs, id: \.target) { b in
                        BufferRow(net: n, buf: b)
                            .tag(Sel(network: n.network, target: b.target))
                    }
                } header: {
                    NetworkHeader(
                        net: n,
                        onChannel: { dialog = .channel(n.network) },
                        onDm: { dialog = .dm(n.network) },
                        onProfile: { profile = n.network }
                    )
                }
            }
        }
        .listStyle(.sidebar)
        .navigationTitle("IrChad")
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button { settings = true } label: { Image(systemName: "gearshape") }
                    .accessibilityLabel("Settings")
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button(action: onAdd) { Image(systemName: "plus") }
                    .accessibilityLabel("Join a network")
            }
        }
    }
}

struct IdBox: Identifiable {
    let id: String
}

private struct NetworkHeader: View {
    @Environment(AppModel.self) private var model
    let net: Network
    let onChannel: () -> Void
    let onDm: () -> Void
    let onProfile: () -> Void
    @State private var leaving = false
    @State private var signingUp = false

    var body: some View {
        HStack {
            Text(net.name)
            if net.mentions > 0 {
                Text("\(net.mentions)")
                    .font(.caption2.bold())
                    .padding(.horizontal, 6)
                    .background(.red, in: Capsule())
                    .foregroundStyle(.white)
                    .accessibilityLabel("\(net.mentions) mentions")
            }
            Spacer()
            Menu {
                Button("Join a channel", systemImage: "number", action: onChannel)
                Button("Message someone", systemImage: "person", action: onDm)
                Button("Your profile", systemImage: "person.crop.circle", action: onProfile)
                if net.guest {
                    Button("Create an account…", systemImage: "person.badge.plus") { signingUp = true }
                }
                Button("Leave network…", systemImage: "rectangle.portrait.and.arrow.right", role: .destructive) {
                    leaving = true
                }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .accessibilityLabel("\(net.name) options")
        }
        .confirmationDialog("Leave \(net.name)?", isPresented: $leaving, titleVisibility: .visible) {
            Button("Leave network", role: .destructive) { model.client.remove(network: net.network) }
        } message: {
            Text("This device forgets the network, its messages and your password. Your account stays.")
        }
        .sheet(isPresented: $signingUp) { SignUpView(net: net) }
    }
}

/// A guest makes an account on `net`, keeping their channels. A failure
/// comes back as a toast; an email code is asked for in the chat's banner.
private struct SignUpView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let net: Network
    @State private var account = ""
    @State private var password = ""
    @State private var email = ""

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Username", text: $account)
                        .textContentType(.username)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    SecureField("Password", text: $password)
                        .textContentType(.newPassword)
                    TextField("Email (if the network asks for one)", text: $email)
                        .keyboardType(.emailAddress)
                        .textContentType(.emailAddress)
                        .textInputAutocapitalization(.never)
                } footer: {
                    Text("Keep your name, catch up on what's said while you're away, and use voice and encrypted DMs. You'll reconnect for a moment; your channels come with you.")
                }
            }
            .navigationTitle("Create an account")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Sign up") {
                        let e = email.trimmingCharacters(in: .whitespaces)
                        model.client.signUp(
                            network: net.network,
                            account: account.trimmingCharacters(in: .whitespaces),
                            password: password,
                            email: e.isEmpty ? nil : e
                        )
                        dismiss()
                    }
                    .disabled(account.trimmingCharacters(in: .whitespaces).isEmpty || password.isEmpty)
                }
            }
        }
        .onAppear { account = net.nick }
    }
}

private struct BufferRow: View {
    @Environment(AppModel.self) private var model
    let net: Network
    let buf: Buffer

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 6) {
                Image(systemName: buf.dm ? (buf.online == true ? "person.fill" : "person") : "number")
                    .foregroundStyle(.secondary)
                    .frame(width: 18)
                    .accessibilityHidden(true)
                Text(buf.target)
                    .fontWeight(buf.unread > 0 ? .bold : .regular)
                    .foregroundStyle(buf.dm && buf.online == false ? .secondary : .primary)
                    .lineLimit(1)
                if buf.encryption == .on || buf.encryption == .verified || buf.encryption == .changed {
                    Image(systemName: "lock.fill")
                        .font(.caption2)
                        .foregroundStyle(buf.encryption == .changed ? .red : .secondary)
                        .accessibilityLabel("Encrypted")
                }
            }
            if !buf.voice.isEmpty {
                Label(buf.voice.joined(separator: ", "), systemImage: "speaker.wave.2.fill")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                    .padding(.leading, 24)
            }
        }
        .badge(Int(buf.mentions))
        .swipeActions {
            Button(buf.dm ? "Close" : "Leave", role: .destructive, action: leave)
        }
        .contextMenu {
            Button(buf.dm ? "Close conversation" : "Leave channel", systemImage: "xmark", role: .destructive, action: leave)
        }
    }

    private func leave() {
        if buf.dm {
            model.client.closeDm(network: net.network, nick: buf.target)
        } else {
            model.client.part(network: net.network, channel: buf.target)
        }
    }
}

/// The text field and buttons of the join-a-channel / message-someone alert.
/// `id` is `Dialog.id`: "c" or "d", then the network.
private struct DialogFields: View {
    let id: String
    let done: (String, String, Bool) -> Void
    @State private var name = ""

    var body: some View {
        let channel = id.hasPrefix("c")
        TextField(channel ? "#channel" : "Nickname", text: $name)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
        Button(channel ? "Join" : "Open") {
            var n = name.trimmingCharacters(in: .whitespaces)
            if channel && !n.hasPrefix("#") { n = "#" + n }
            guard n.count > (channel ? 1 : 0) else { return }
            done(String(id.dropFirst()), n, channel)
        }
        Button("Cancel", role: .cancel) {}
    }
}
