import SwiftUI

private enum Mode { case login, register, guest }

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

/// The server's address first (an invite link answers that by itself), then
/// logging in, with signing up and being a guest as the quieter choices the
/// server offers. Closes itself once the network shows up; `onCancel` is nil
/// when there's nothing to go back to.
struct JoinView: View {
    @Environment(AppModel.self) private var model
    let invite: String?
    let onDone: (Sel?) -> Void
    let onCancel: (() -> Void)?

    @State private var link = "irchad.chat"
    /// What the server offers, once it answered: the sign-in step.
    @State private var server: Server?
    @State private var mode = Mode.login
    @State private var account = ""
    @State private var password = ""
    @State private var email = ""
    /// The invite sent to the core, while waiting for the outcome.
    @State private var busy: String?
    @State private var error: String?

    var body: some View {
        let _ = model.tick
        NavigationStack {
            Form {
                if let server {
                    signIn(server)
                } else {
                    address
                }
            }
            .navigationTitle(server?.name ?? "Join a network")
            .toolbar {
                if let onCancel {
                    ToolbarItem(placement: .cancellationAction) { Button("Cancel", action: onCancel) }
                }
            }
        }
        .onAppear { if let invite { discover(invite) } }
        .onChange(of: invite) { _, new in if let new { discover(new) } }
        .onChange(of: model.joinFailure?.invite) { _, _ in
            guard let f = model.joinFailure, f.invite == busy else { return }
            busy = nil
            error = f.error
            model.joinFailure = nil
        }
        .onChange(of: model.discovered?.invite) { _, _ in
            guard let d = model.discovered else { return }
            model.discovered = nil
            guard d.invite == busy else { return }
            busy = nil
            link = d.invite
            server = d.server
        }
        .onChange(of: model.tick) { _, _ in checkDone() }
    }

    /// Step one: which server.
    @ViewBuilder private var address: some View {
        Section {
            TextField("Server address", text: $link)
                .keyboardType(.URL)
                .textContentType(.URL)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .disabled(busy != nil)
        } footer: {
            Text("Paste the invite link or server address someone sent you. You'll log in or sign up on the next step.")
        }
        if let error {
            Section { Text(error).foregroundStyle(.red) }
        }
        Section {
            primary("Continue", off: link.trimmingCharacters(in: .whitespaces).isEmpty) { discover(link) }
        }
    }

    /// Step two: log in, or one of the quieter choices `server` offers.
    @ViewBuilder private func signIn(_ server: Server) -> some View {
        let mode = offered(server)
        let guest = mode == .guest
        Section {
            TextField(guest ? "Nickname" : "Username", text: $account)
                .textContentType(.username)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            if !guest {
                SecureField("Password", text: $password)
                    .textContentType(mode == .register ? .newPassword : .password)
            }
            if mode == .register {
                TextField("Email (if the network asks for one)", text: $email)
                    .keyboardType(.emailAddress)
                    .textContentType(.emailAddress)
                    .textInputAutocapitalization(.never)
            }
        } header: {
            HStack {
                Text(server.origin.components(separatedBy: "://").last ?? server.origin)
                Spacer()
                Button("Change") {
                    // The password doesn't follow to another server.
                    self.server = nil
                    password = ""
                    error = nil
                }
                .font(.footnote)
                .disabled(busy != nil)
            }
            .textCase(nil)
        } footer: {
            if guest && !server.classic {
                Text("Guests miss what's said while they're away, and anyone can take the nickname.")
            }
        }
        if let error {
            Section { Text(error).foregroundStyle(.red) }
        }
        Section {
            primary(mode == .login ? "Log in" : mode == .register ? "Sign up" : "Continue as guest", off: !ready(guest)) {
                submit(mode)
            }
        }
        if server.discord && !guest {
            Section {
                Button {
                    error = nil
                    busy = link
                    model.discordStart(link)
                    model.notifier.askPermission()
                } label: {
                    HStack {
                        Spacer()
                        DiscordLabel()
                        Spacer()
                    }
                }
                .foregroundStyle(.white)
                .listRowBackground(Color.blurple.opacity(busy != nil ? 0.5 : 1))
                .disabled(busy != nil)
            }
        }
        // The other ways in, quieter than the button; guest quietest.
        Section {
            VStack(spacing: 12) {
                if mode != .login {
                    ask("Have an account?", "Log in") { self.mode = .login }
                }
                if mode != .register && server.signUp {
                    ask("No account yet?", "Sign up") { self.mode = .register }
                }
                if !guest && server.guests {
                    Button("Continue as guest") {
                        self.mode = .guest
                        error = nil
                    }
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                }
            }
            .frame(maxWidth: .infinity)
            .buttonStyle(.borderless)
            .disabled(busy != nil)
            .listRowBackground(Color.clear)
        }
    }

    /// The form's one filled button, or its spinner while busy.
    private func primary(_ title: String, off: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack {
                Spacer()
                if busy != nil { ProgressView() } else { Text(title).bold() }
                Spacer()
            }
        }
        .foregroundStyle(.white)
        .listRowBackground(Color.accentColor.opacity(off || busy != nil ? 0.5 : 1))
        .disabled(off || busy != nil)
    }

    /// A muted question and the link that answers it.
    private func ask(_ question: String, _ link: String, action: @escaping () -> Void) -> some View {
        HStack(spacing: 4) {
            Text(question).foregroundStyle(.secondary)
            Button(link) {
                action()
                error = nil
            }
        }
        .font(.subheadline)
    }

    /// `mode`, or logging in when the server doesn't offer it.
    private func offered(_ server: Server) -> Mode {
        switch mode {
        case .register where !server.signUp, .guest where !server.guests: .login
        default: mode
        }
    }

    private func ready(_ guest: Bool) -> Bool {
        !account.trimmingCharacters(in: .whitespaces).isEmpty && (guest || !password.isEmpty)
    }

    /// Ask the server at `invite` what it offers; a link does this by itself.
    private func discover(_ invite: String) {
        let inv = invite.trimmingCharacters(in: .whitespacesAndNewlines)
        link = inv
        server = nil
        error = nil
        busy = inv
        model.client.discover(invite: inv)
    }

    private func submit(_ mode: Mode) {
        let acct = account.trimmingCharacters(in: .whitespaces)
        error = nil
        busy = link
        switch mode {
        case .login: model.client.join(invite: link, account: acct, password: password)
        case .register:
            let e = email.trimmingCharacters(in: .whitespaces)
            model.client.register(invite: link, account: acct, password: password, email: e.isEmpty ? nil : e)
        case .guest: model.client.joinGuest(invite: link, nick: acct)
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

/// First launch: the core's cards, one at a time, then `onDone` (the join
/// screen). Skipping is `onDone` too.
struct WelcomeView: View {
    let onDone: () -> Void
    @State private var at = 0
    private let cards = welcomeCards()

    var body: some View {
        let last = at >= cards.count - 1
        VStack(spacing: 16) {
            Spacer()
            Image(uiImage: UIImage(named: "logo") ?? UIImage())
                .resizable()
                .frame(width: 88, height: 88)
                .accessibilityHidden(true)
            if cards.indices.contains(at) {
                Text(cards[at].title).font(.title.bold()).multilineTextAlignment(.center)
                Text(cards[at].body).font(.body).foregroundStyle(.secondary).multilineTextAlignment(.center)
            }
            HStack(spacing: 8) {
                ForEach(cards.indices, id: \.self) { i in
                    Circle().fill(i == at ? Color.accentColor : Color.secondary.opacity(0.4)).frame(width: 8, height: 8)
                }
            }
            .accessibilityElement()
            .accessibilityLabel("Step \(at + 1) of \(cards.count)")
            Spacer()
            Button {
                if last { onDone() } else { withAnimation { at += 1 } }
            } label: {
                Text(last ? "Get started" : "Next").bold().frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            HStack(spacing: 24) {
                if at > 0 { Button("Back") { withAnimation { at -= 1 } } }
                if !last { Button("Skip", action: onDone).font(.footnote).foregroundStyle(.secondary) }
            }
            .frame(minHeight: 32)
        }
        .padding(32)
    }
}
