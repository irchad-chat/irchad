import SwiftUI

private enum Mode: String, CaseIterable {
    case login = "Log in"
    case register = "Sign up"
    case guest = "Guest"
}

extension Color {
    /// Discord's blurple, for "Sign in with Discord".
    static let blurple = Color(red: 0x58 / 255, green: 0x65 / 255, blue: 0xF2 / 255)
}

/// Phosphor's Discord logo and "Sign in with Discord".
struct DiscordLabel: View {
    var body: some View {
        Label {
            Text("Sign in with Discord")
        } icon: {
            // A loose @2x/@3x PNG (xtool.yml), tinted like the text.
            Image(uiImage: UIImage(named: "discord-logo") ?? UIImage())
                .renderingMode(.template)
                .accessibilityHidden(true)
        }
    }
}

/// Paste an invite, log in (or sign up, or be a guest), done. Closes itself
/// once the network shows up; `onCancel` is nil when there's nothing to go
/// back to.
struct JoinView: View {
    @Environment(AppModel.self) private var model
    let invite: String?
    let onDone: (Sel?) -> Void
    let onCancel: (() -> Void)?

    @State private var link = "irchad.chat"
    @State private var mode = Mode.login
    @State private var account = ""
    @State private var password = ""
    @State private var email = ""
    /// The invite being joined.
    @State private var busy: String?
    @State private var error: String?

    private var discordOff: Bool {
        link.trimmingCharacters(in: .whitespaces).isEmpty || busy != nil
    }

    var body: some View {
        let _ = model.tick
        NavigationStack {
            Form {
                Section {
                    TextField("Server URL", text: $link)
                        .keyboardType(.URL)
                        .textContentType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                } footer: {
                    Text("Paste the server URL someone sent you.")
                }
                Section {
                    Picker("How", selection: $mode) {
                        ForEach(Mode.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    TextField(mode == .guest ? "Nickname" : "Username", text: $account)
                        .textContentType(.username)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    if mode != .guest {
                        SecureField("Password", text: $password)
                            .textContentType(mode == .register ? .newPassword : .password)
                    }
                    if mode == .register {
                        TextField("Email (if the network asks for one)", text: $email)
                            .keyboardType(.emailAddress)
                            .textContentType(.emailAddress)
                            .textInputAutocapitalization(.never)
                    }
                }
                if let error {
                    Section { Text(error).foregroundStyle(.red) }
                }
                if mode != .guest {
                    Section {
                        Button {
                            let inv = link.trimmingCharacters(in: .whitespacesAndNewlines)
                            error = nil
                            busy = inv
                            model.discordStart(inv)
                            model.notifier.askPermission()
                        } label: {
                            HStack {
                                Spacer()
                                DiscordLabel()
                                Spacer()
                            }
                        }
                        .foregroundStyle(.white)
                        .listRowBackground(Color.blurple.opacity(discordOff ? 0.5 : 1))
                        .disabled(discordOff)
                    } footer: {
                        Text("No password needed: sign in with your Discord account.")
                    }
                }
                Section {
                    Button(action: submit) {
                        HStack {
                            Spacer()
                            if busy != nil { ProgressView() } else { Text(mode == .register ? "Sign up" : "Join").bold() }
                            Spacer()
                        }
                    }
                    .disabled(!ready || busy != nil)
                }
            }
            .navigationTitle("Join a network")
            .toolbar {
                if let onCancel {
                    ToolbarItem(placement: .cancellationAction) { Button("Cancel", action: onCancel) }
                }
            }
        }
        .onAppear { if let invite { link = invite } }
        .onChange(of: invite) { _, new in if let new { link = new } }
        .onChange(of: model.joinFailure?.invite) { _, _ in
            guard let f = model.joinFailure, f.invite == busy else { return }
            busy = nil
            error = f.error
            model.joinFailure = nil
        }
        .onChange(of: model.tick) { _, _ in checkDone() }
    }

    private var ready: Bool {
        !link.trimmingCharacters(in: .whitespaces).isEmpty && !account.trimmingCharacters(in: .whitespaces).isEmpty
            && (mode == .guest || !password.isEmpty)
    }

    private func submit() {
        let inv = link.trimmingCharacters(in: .whitespacesAndNewlines)
        let acct = account.trimmingCharacters(in: .whitespaces)
        error = nil
        busy = inv
        switch mode {
        case .login: model.client.join(invite: inv, account: acct, password: password)
        case .register:
            let e = email.trimmingCharacters(in: .whitespaces)
            model.client.register(invite: inv, account: acct, password: password, email: e.isEmpty ? nil : e)
        case .guest: model.client.joinGuest(invite: inv, nick: acct)
        }
        model.notifier.askPermission()
    }

    /// Done once the network is up, or waiting on the sign-up email's code;
    /// a failed join comes as `joinFailure` instead.
    private func checkDone() {
        guard let b = busy, let inv = parseInvite(invite: b),
              let n = model.client.networks().first(where: { $0.network == inv.origin }),
              n.status == nil || n.needsCode
        else { return }
        busy = nil
        onDone(inv.channel.map { Sel(network: inv.origin, target: $0) } ?? Sel(network: inv.origin, target: ""))
    }
}
