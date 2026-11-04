import PhotosUI
import SwiftUI

/// Your picture, display name and away state on a network, and this
/// account's devices with encrypted DMs (D52), as on desktop.
struct ProfileView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let network: String

    @State private var name = ""
    @State private var photo: PhotosPickerItem?

    var body: some View {
        let _ = model.tick
        if let net = model.client.networks().first(where: { $0.network == network }) {
            let profile = model.client.profile(network: network, nick: net.nick)
            let devices = model.client.devices(network: network, nick: net.nick)
            NavigationStack {
                Form {
                    Section {
                        HStack(spacing: 16) {
                            Avatar(network: network, nick: net.nick, url: profile.avatar, size: 64)
                            VStack(alignment: .leading, spacing: 8) {
                                if net.uploads != nil {
                                    PhotosPicker("Choose a picture", selection: $photo, matching: .images,
                                                 preferredItemEncoding: .compatible)
                                }
                                if profile.avatar != nil {
                                    Button("Remove picture", role: .destructive) { model.client.clearAvatar(network: network) }
                                }
                            }
                        }
                        .buttonStyle(.borderless)
                    }
                    Section {
                        TextField(net.nick, text: $name)
                            .onChange(of: name) { _, n in if n.count > 64 { name = String(n.prefix(64)) } }
                    } header: {
                        Text("Display name")
                    } footer: {
                        Text("People still see that you're \(net.nick).")
                    }
                    Section {
                        Toggle("Away", isOn: Binding(get: { model.settings.away }, set: { model.setAway($0) }))
                    } footer: {
                        Text("On every network, until you turn it off.")
                    }
                    if !devices.isEmpty {
                        Section("Devices with encrypted DMs") {
                            ForEach(devices, id: \.id) { d in
                                HStack {
                                    VStack(alignment: .leading) {
                                        Text(d.this ? "This device" : (d.verified ? "Verified device" : "Another device"))
                                        Text(d.fingerprint).font(.caption.monospaced()).foregroundStyle(.secondary)
                                    }
                                    Spacer()
                                    if !d.this {
                                        Button("Remove", role: .destructive) {
                                            model.client.removeDevice(network: network, id: d.id)
                                        }
                                        .buttonStyle(.borderless)
                                    }
                                }
                            }
                        }
                    }
                }
                .navigationTitle("Your profile on \(net.name)")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Save") {
                            model.client.setDisplayName(network: network, name: name.trimmingCharacters(in: .whitespaces))
                            dismiss()
                        }
                    }
                }
                .onAppear {
                    name = profile.displayName ?? ""
                    model.client.fetchDevices(network: network, nick: net.nick)
                }
                .onChange(of: photo) { _, item in
                    guard let item else { return }
                    photo = nil
                    Task { await upload(item, net: net) }
                }
            }
        }
    }

    private func upload(_ item: PhotosPickerItem, net: Network) async {
        guard let f = try? await item.loadTransferable(type: PickedFile.self) else {
            model.toast("Couldn't read that picture.")
            return
        }
        defer { try? FileManager.default.removeItem(at: f.url.deletingLastPathComponent()) }
        guard let limit = net.uploads,
              let data = try? Data(contentsOf: f.url), UInt64(data.count) <= limit
        else {
            model.toast("That picture is too big for \(net.name).")
            return
        }
        model.client.uploadAvatar(network: network, name: f.url.lastPathComponent, bytes: data)
    }
}
