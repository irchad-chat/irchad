import SwiftUI

@main
struct IrChadApp: App {
    @State private var model = AppModel.shared
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .preferredColorScheme(model.settings.theme.scheme)
                .onOpenURL { url in
                    // `irchad://join?origin=…` from chadd's landing page; a Discord
                    // sign-in (D96) normally comes back through the auth session
                    // instead, and the core drops one it didn't start.
                    if url.scheme == "irchad", url.host() == "oauth" {
                        model.client.discordCallback(url: url.absoluteString)
                    } else if url.scheme == "irchad" {
                        model.invite = url.absoluteString
                    }
                }
        }
        .onChange(of: phase) { _, p in model.notifier.phase(p) }
    }
}
