import SwiftUI

enum Theme: String, CaseIterable {
    case system, light, dark

    var scheme: ColorScheme? {
        switch self {
        case .system: nil
        case .light: .light
        case .dark: .dark
        }
    }
}

/// When images and link previews load without a tap (D25).
enum AutoLoad: String, CaseIterable {
    case always, wifi, off
}

/// The preferences that are only this device's: looks, media and voice.
/// Same defaults as desktop and Android.
@MainActor @Observable
final class Settings {
    static let awayMessage = "Away"
    @ObservationIgnored private let d = UserDefaults.standard

    var theme: Theme { didSet { d.set(theme.rawValue, forKey: "theme") } }
    var autoLoad: AutoLoad { didSet { d.set(autoLoad.rawValue, forKey: "auto_load") } }
    /// Images and previews in guests' messages; off, their links stay plain.
    var guestMedia: Bool { didSet { d.set(guestMedia, forKey: "guest_media") } }
    var echoCancellation: Bool { didSet { d.set(echoCancellation, forKey: "echo_cancellation") } }
    var noiseSuppression: Bool { didSet { d.set(noiseSuppression, forKey: "noise_suppression") } }
    var autoGain: Bool { didSet { d.set(autoGain, forKey: "auto_gain") } }
    var bitrateKbps: Int { didSet { d.set(bitrateKbps, forKey: "bitrate_kbps") } }
    /// Away on every network, until turned off.
    var away: Bool { didSet { d.set(away, forKey: "away") } }

    init() {
        d.register(defaults: [
            "guest_media": true, "echo_cancellation": true, "noise_suppression": true,
            "auto_gain": false, "bitrate_kbps": 64, "away": false,
        ])
        theme = Theme(rawValue: d.string(forKey: "theme") ?? "") ?? .system
        autoLoad = AutoLoad(rawValue: d.string(forKey: "auto_load") ?? "") ?? .always
        guestMedia = d.bool(forKey: "guest_media")
        echoCancellation = d.bool(forKey: "echo_cancellation")
        noiseSuppression = d.bool(forKey: "noise_suppression")
        autoGain = d.bool(forKey: "auto_gain")
        bitrateKbps = min(max(d.integer(forKey: "bitrate_kbps"), 16), 128)
        away = d.bool(forKey: "away")
    }
}

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var leaving: Network?

    var body: some View {
        @Bindable var s = model.settings
        let _ = model.tick
        let nets = model.client.networks()
        NavigationStack {
            Form {
                Section("Theme") {
                    Picker("Theme", selection: $s.theme) {
                        Text("Match system").tag(Theme.system)
                        Text("Light").tag(Theme.light)
                        Text("Dark").tag(Theme.dark)
                    }
                    .pickerStyle(.inline)
                    .labelsHidden()
                }
                Section {
                    Picker("Load images and link previews", selection: $s.autoLoad) {
                        Text("Always").tag(AutoLoad.always)
                        Text("On Wi‑Fi only").tag(AutoLoad.wifi)
                        Text("Only when I tap").tag(AutoLoad.off)
                    }
                    Toggle("Also from guests (no account)", isOn: $s.guestMedia)
                } header: {
                    Text("Images and link previews")
                }
                Section {
                    Toggle("Echo cancellation", isOn: $s.echoCancellation)
                    Toggle("Noise suppression", isOn: $s.noiseSuppression)
                    Toggle("Automatic gain control", isOn: $s.autoGain)
                    VStack(alignment: .leading) {
                        Text("Quality: \(s.bitrateKbps) kbps")
                        Slider(
                            value: Binding(get: { Double(s.bitrateKbps) }, set: { s.bitrateKbps = Int($0.rounded()) }),
                            in: 16...128
                        )
                        .accessibilityLabel("Voice quality")
                        .accessibilityValue("\(s.bitrateKbps) kilobits per second")
                    }
                } header: {
                    Text("Voice")
                } footer: {
                    Text("Voice changes take effect the next time you join.")
                }
                Section {
                    let blocked = nets.flatMap { n in model.client.blocks(network: n.network).map { (n, $0) } }
                    if blocked.isEmpty {
                        Text("Nobody blocked.").foregroundStyle(.secondary)
                    }
                    ForEach(blocked, id: \.1.mask) { n, b in
                        HStack {
                            VStack(alignment: .leading) {
                                Text(b.nick)
                                Text(nets.count > 1 ? "\(b.account.map { "account \($0)" } ?? b.mask) · \(n.name)" : (b.account.map { "account \($0)" } ?? b.mask))
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                            Spacer()
                            Button("Unblock") { model.client.unblock(network: n.network, mask: b.mask) }
                        }
                    }
                } header: {
                    Text("Blocked users")
                } footer: {
                    Text("You won't see anything they send. Only on this device; they aren't told.")
                }
                Section {
                    ForEach(nets, id: \.network) { n in
                        HStack {
                            VStack(alignment: .leading) {
                                Text(n.name)
                                Text("\(n.nick) · \(n.network)").font(.caption).foregroundStyle(.secondary)
                            }
                            Spacer()
                            Button("Leave", role: .destructive) { leaving = n }
                        }
                    }
                } header: {
                    Text("Networks")
                } footer: {
                    Text(version).padding(.top, 8)
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
            .confirmationDialog(
                "Leave \(leaving?.name ?? "")?",
                isPresented: Binding(get: { leaving != nil }, set: { if !$0 { leaving = nil } }),
                titleVisibility: .visible
            ) {
                Button("Leave network", role: .destructive) {
                    if let n = leaving { model.client.remove(network: n.network) }
                    leaving = nil
                }
                if let n = leaving, !n.guest {
                    Button("Delete account and leave", role: .destructive) {
                        model.client.deleteAccount(network: n.network)
                        leaving = nil
                    }
                }
            } message: {
                Text("This device forgets the network, its messages and your password. Deleting the account also removes it and its channels from the server.")
            }
        }
    }

    private var version: String {
        "IrChad \(Bundle.main.infoDictionary?["ChadVersion"] as? String ?? "")"
    }
}
