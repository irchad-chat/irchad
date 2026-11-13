import Foundation
import Observation
import Security
import os

/// A conversation: the network's origin and the buffer's target.
struct Sel: Hashable, Codable {
    let network: String
    let target: String
}

struct Verify: Equatable {
    let network: String
    let nick: String
    let state: Sas
}

/// Owns the core for the life of the process, like Android's `IrChad`.
/// Views read the client's snapshots in `body` and depend on `tick`, which
/// goes up after every core event.
@MainActor @Observable
final class AppModel {
    static let shared = AppModel()

    /// Bumped on every core event: read the client's snapshots again.
    private(set) var tick = 0
    /// Something to tell the user, shown as a toast.
    var notice: String?
    /// (invite, error) for a join that failed.
    var joinFailure: (invite: String, error: String)?
    /// The verification on screen, one person at a time.
    var verify: Verify?
    /// A report being written: who, and the message if it's about one.
    var reporting: ReportDraft?
    /// The server took a report: say so until dismissed.
    var reported = false
    /// A conversation to show: a tapped notification, a finished join.
    var focus: Sel?
    /// An `irchad://` invite that opened the app, until the join screen takes it.
    var invite: String?

    let settings = Settings()
    let media = Media()
    let calls = Calls()
    let sounds = Sounds()
    let notifier = Notifier()

    @ObservationIgnored let client: Client
    @ObservationIgnored private let bridge: Bridge
    @ObservationIgnored private let voice: VoiceBridge
    /// Networks we've told we're away (`Settings.away`), since the core started.
    @ObservationIgnored private var awaySent = Set<String>()

    private init() {
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "?"
        bridge = Bridge()
        voice = VoiceBridge()
        client = Client(
            dataDir: Self.dataDir().path,
            version: "IrChad \(version) (iOS)",
            secrets: KeychainSecrets(),
            voice: voice,
            listener: bridge
        )
        bridge.model = self
        voice.calls = calls
    }

    /// Application Support/IrChad, kept out of backups: a restored E2EE store
    /// would be a second copy of this device's Olm account (its key is in the
    /// keychain, this device only, so it couldn't be opened anyway).
    private static func dataDir() -> URL {
        var dir = URL.applicationSupportDirectory.appending(path: "IrChad", directoryHint: .isDirectory)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? dir.setResourceValues(values)
        return dir
    }

    fileprivate func changed() {
        tick &+= 1
        notifier.changed()
        // A network that appeared since gets told we're away too.
        if settings.away {
            for n in client.networks() where !awaySent.contains(n.network) {
                awaySent.insert(n.network)
                client.setAway(network: n.network, message: Settings.awayMessage)
            }
        }
    }

    /// Going away or coming back, on every network.
    func setAway(_ on: Bool) {
        settings.away = on
        awaySent.removeAll()
        for n in client.networks() {
            if on { awaySent.insert(n.network) }
            client.setAway(network: n.network, message: on ? Settings.awayMessage : nil)
        }
    }

    func toast(_ text: String) {
        notice = text
    }

    fileprivate func verification(network: String, nick: String, state: Sas) {
        // A request from someone else waits behind the one on screen.
        if let cur = verify,
           cur.network != network || cur.nick.lowercased() != nick.lowercased(),
           cur.state != .done, !isFailed(cur.state) {
            if state == .incoming { client.verification(network: network, nick: nick, step: .cancel) }
            return
        }
        verify = Verify(network: network, nick: nick, state: state)
    }
}

func isFailed(_ s: Sas) -> Bool {
    if case .failed = s { return true }
    return false
}

/// The core's listener. It's called on core threads, so everything hops to
/// the main queue, in order.
final class Bridge: Listener, @unchecked Sendable {
    @MainActor weak var model: AppModel?
    // One pending `changed` at a time: history loads fire hundreds.
    private let pending = OSAllocatedUnfairLock(initialState: false)

    private func main(_ f: @escaping @MainActor (AppModel) -> Void) {
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated {
                if let m = self?.model { f(m) }
            }
        }
    }

    func changed() {
        let first = pending.withLock { p -> Bool in
            defer { p = true }
            return !p
        }
        guard first else { return }
        DispatchQueue.main.async { [weak self] in
            self?.pending.withLock { $0 = false }
            MainActor.assumeIsolated { self?.model?.changed() }
        }
    }

    func notice(text: String) {
        main { $0.toast(text) }
    }

    func joinFailed(invite: String, error: String) {
        main { $0.joinFailure = (invite, error) }
    }

    func preview(network: String, url: String, preview: LinkPreview?) {
        main { $0.media.gotPreview(network: network, url: url, preview: preview) }
    }

    func media(network: String, url: String, bytes: Data?) {
        // Decoded off the core's thread and the main one.
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let img = bytes.flatMap(Media.decode)
            self?.main { $0.media.gotImage(network: network, url: url, image: img) }
        }
    }

    func gifs(network: String, query: String, pos: String?, page: GifPage) {
        main { $0.media.gotGifs(query: query, pos: pos, page: page) }
    }

    func pushAvailable(network: String, vapid: String) {
        main { $0.notifier.pushAvailable(network: network, vapid: vapid) }
    }

    func notify(network: String, target: String, from: String, text: String, channel: Bool) {
        main { $0.notifier.live(network: network, target: target, from: from, text: text, channel: channel) }
    }

    func verification(network: String, nick: String, state: Sas) {
        main { $0.verification(network: network, nick: nick, state: state) }
    }

    func sounds(network: String, clips: [Clip], error: String?) {
        main { $0.sounds.got(network: network, clips: clips, error: error) }
    }

    func sound(network: String, file: String, bytes: Data?) {
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let url = bytes.flatMap(Sounds.decode)
            self?.main { $0.sounds.gotFile(network: network, file: file, url: url) }
        }
    }

    func reported(network: String) {
        main { $0.reported = true }
    }
}

/// Someone to report to the network's moderators, or their message.
struct ReportDraft {
    let network: String
    let nick: String
    var target: String?
    var msgid: String?
    var text: String?
}

/// Passwords (and each network's E2EE store key, `<origin>#e2e`) in the
/// keychain, on this device only and readable after the first unlock, so a
/// push can use them while the phone is locked.
final class KeychainSecrets: Secrets, @unchecked Sendable {
    private let service = "party.gentoo.irchad"

    private func query(_ origin: String) -> [CFString: Any] {
        [kSecClass: kSecClassGenericPassword, kSecAttrService: service, kSecAttrAccount: origin]
    }

    func save(origin: String, account: String, password: String) -> String? {
        guard let data = try? JSONEncoder().encode([account, password]) else {
            return "Couldn't save the password."
        }
        SecItemDelete(query(origin) as CFDictionary)
        var q = query(origin)
        q[kSecValueData] = data
        q[kSecAttrAccessible] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let status = SecItemAdd(q as CFDictionary, nil)
        return status == errSecSuccess ? nil : "Couldn't save the password (error \(status))."
    }

    func load(origin: String) -> Credentials? {
        var q = query(origin)
        q[kSecReturnData] = true
        q[kSecMatchLimit] = kSecMatchLimitOne
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess,
              let data = out as? Data,
              let pair = try? JSONDecoder().decode([String].self, from: data),
              pair.count == 2
        else { return nil }
        return Credentials(account: pair[0], password: pair[1])
    }

    func delete(origin: String) {
        SecItemDelete(query(origin) as CFDictionary)
    }
}
