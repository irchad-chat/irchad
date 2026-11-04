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
                    // `irchad://join?origin=…` from chadd's landing page.
                    if url.scheme == "irchad" { model.invite = url.absoluteString }
                }
        }
        .onChange(of: phase) { _, p in model.notifier.phase(p) }
    }
}
