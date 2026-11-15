import SwiftUI

/// What tapping someone shows, as desktop's profile card: a bigger avatar,
/// names, activity, and what we know. A full-height drawer on iPhone, a
/// form sheet on iPad.
struct ProfileCard: View {
    @Environment(AppModel.self) private var model
    @Environment(\.colorScheme) private var scheme
    @Environment(\.dismiss) private var dismiss
    let net: Network
    /// Where they were tapped, for their channel role and away state.
    let channel: String
    let nick: String
    /// Their account as last seen on a message, when they aren't a member here.
    var account: String?
    let openDm: (String) -> Void

    var body: some View {
        let _ = model.tick
        let p = model.client.profile(network: net.network, nick: nick)
        let member = model.client.memberList(network: net.network, channel: channel)?
            .first { $0.nick.lowercased() == nick.lowercased() }
        let shown = member?.shown ?? p.displayName ?? nick
        let other = nick.lowercased() != net.nick.lowercased()
        NavigationStack {
            Form {
                Section {
                    VStack(spacing: 8) {
                        Avatar(network: net.network, nick: nick, url: p.avatar, size: 96)
                            .overlay(alignment: .bottomTrailing) {
                                if let member { statusDot(away: member.away) }
                            }
                        Text(shown).font(.title2.bold()).foregroundStyle(nickColor(nick, dark: scheme == .dark))
                        if shown != nick {
                            Text(nick).foregroundStyle(.secondary)
                        }
                        if let a = p.activity {
                            activity(a, music: p.music)
                        }
                    }
                    .frame(maxWidth: .infinity)
                    .listRowBackground(Color.clear)
                }
                Section {
                    if let member {
                        LabeledContent("Status", value: member.away ? "Away" : "Online")
                    }
                    if let a = member?.account ?? account {
                        LabeledContent("Account", value: a)
                    } else {
                        LabeledContent("Account", value: "👻 Guest (not logged in)")
                    }
                    let r = roles(member?.prefix ?? "")
                    if !r.isEmpty {
                        LabeledContent("Channel role", value: r.joined(separator: ", "))
                    }
                    if member?.oper == true {
                        LabeledContent("Network") { Label("IRC operator", systemImage: "crown.fill") }
                    }
                }
                if other {
                    Section {
                        Button {
                            dismiss()
                            openDm(nick)
                        } label: {
                            Label("Send DM", systemImage: "bubble.left")
                        }
                    }
                }
            }
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
        .presentationDragIndicator(.visible)
    }

    private func statusDot(away: Bool) -> some View {
        Circle()
            .fill(away ? Color(rgb: 0xF0B232) : Color(rgb: 0x23A55A))
            .frame(width: 22, height: 22)
            .overlay(Circle().stroke(Color(.systemGroupedBackground), lineWidth: 4))
            .accessibilityHidden(true)
    }

    /// The activity, a link when its track has one.
    @ViewBuilder private func activity(_ text: String, music: Music?) -> some View {
        if let u = music?.url.flatMap(URL.init(string:)) {
            SwiftUI.Link(destination: u) { ActivityLine(text: text, music: music) }
        } else {
            ActivityLine(text: text, music: music)
        }
    }
}

/// Channel roles from a member's prefixes, as on desktop.
func roles(_ prefix: String) -> [String] {
    prefix.compactMap { c in
        switch c {
        case "~": "Owner"
        case "&": "Admin"
        case "@": "Operator"
        case "%": "Half-op"
        case "+": "Voice"
        default: nil
        }
    }
}
