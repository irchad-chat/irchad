import SwiftUI
import UIKit
import UserNotifications

/// How long the app stays connected after it goes to the background (D49).
/// iOS gives a background task about 30 s, so this ends it in time.
private let pauseAfter = Duration.seconds(20)

/// Message notifications from the live connection, and pausing the
/// connections in the background (D39, D49).
///
/// iOS suspends an app soon after it leaves the screen, unless it's in a
/// call (the `audio` background mode), so DMs and mentions only notify
/// while the core is still running: during a call, or the moments after
/// leaving the app.
// shortcut: no push while suspended. ergo's draft/webpush can't reach APNs
// without a relay holding the app's APNs key; see docs/phases/8-ios.md.
@MainActor @Observable
final class Notifier: NSObject, UNUserNotificationCenterDelegate {
    @ObservationIgnored private var visible = true
    @ObservationIgnored private var pause: Task<Void, Never>?
    @ObservationIgnored private var task = UIBackgroundTaskIdentifier.invalid
    @ObservationIgnored private var badge = -1

    private var model: AppModel { AppModel.shared }

    override init() {
        super.init()
        UNUserNotificationCenter.current().delegate = self
    }

    func askPermission() {
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) { _, _ in }
    }

    /// The scene came to the front or left it. Leaving pauses the connections
    /// shortly after (not one in voice: the core keeps it), coming back
    /// reconnects them at once, and the usual catch-up fills the gap.
    func phase(_ p: ScenePhase) {
        switch p {
        case .active:
            visible = true
            pause?.cancel()
            pause = nil
            endTask()
            model.client.background(on: false)
        case .background:
            visible = false
            guard pause == nil else { return }
            task = UIApplication.shared.beginBackgroundTask(withName: "pause") { [weak self] in
                MainActor.assumeIsolated { self?.pauseNow() }
            }
            pause = Task { [weak self] in
                try? await Task.sleep(for: pauseAfter)
                if !Task.isCancelled { self?.pauseNow() }
            }
        default:
            break
        }
    }

    private func pauseNow() {
        pause = nil
        if !visible { model.client.background(on: true) }
        // Give the core a moment to close its sockets cleanly.
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(1))
            self?.endTask()
        }
    }

    private func endTask() {
        if task != .invalid {
            UIApplication.shared.endBackgroundTask(task)
            task = .invalid
        }
    }

    /// After every core event: the app icon's badge is the mentions waiting.
    func changed() {
        let n = model.client.networks().reduce(0) { $0 + Int($1.mentions) }
        guard n != badge else { return }
        badge = n
        UNUserNotificationCenter.current().setBadgeCount(n)
    }

    func pushAvailable(network: String, vapid: String) {
        // See the shortcut above: there's nowhere to point ergo yet.
    }

    /// A fresh DM or mention on the live connection. Nothing shows while the
    /// app is on screen.
    func live(network: String, target: String, from: String, text: String, channel: Bool) {
        guard !visible else { return }
        let c = UNMutableNotificationContent()
        c.title = channel ? "\(from) in \(target)" : from
        c.body = text
        c.sound = .default
        c.threadIdentifier = thread(network, target)
        c.userInfo = ["network": network, "target": target]
        let req = UNNotificationRequest(identifier: UUID().uuidString, content: c, trigger: nil)
        UNUserNotificationCenter.current().add(req)
    }

    /// The conversation is on screen: clear its notifications.
    func dismiss(network: String, target: String) {
        let id = thread(network, target)
        Task {
            let center = UNUserNotificationCenter.current()
            let ids = await center.deliveredNotifications()
                .filter { $0.request.content.threadIdentifier == id }.map(\.request.identifier)
            if !ids.isEmpty { center.removeDeliveredNotifications(withIdentifiers: ids) }
        }
    }

    private func thread(_ network: String, _ target: String) -> String {
        "\(network) \(target.lowercased())"
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse
    ) async {
        let info = response.notification.request.content.userInfo
        guard let network = info["network"] as? String, let target = info["target"] as? String else { return }
        await MainActor.run { model.focus = Sel(network: network, target: target) }
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        []
    }
}
