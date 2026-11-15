import SwiftUI

/// Liquid Glass from iOS 26 for the app's own controls and bars; the flat
/// look before it. Content (messages, reactions, media) stays flat.
extension View {
    @ViewBuilder func glass(in shape: some Shape, tint: Color? = nil,
                            fallback: AnyShapeStyle = AnyShapeStyle(Color(.secondarySystemBackground))) -> some View {
        if #available(iOS 26, *) {
            glassEffect(.regular.tint(tint).interactive(), in: shape)
        } else {
            background(tint.map(AnyShapeStyle.init) ?? fallback, in: shape)
        }
    }

    @ViewBuilder func glassButton(prominent: Bool = false) -> some View {
        if #available(iOS 26, *) {
            if prominent { buttonStyle(.glassProminent) } else { buttonStyle(.glass) }
        } else {
            if prominent { buttonStyle(.borderedProminent) } else { buttonStyle(.bordered) }
        }
    }

    /// Pinned to the bottom, with iOS 26's soft edge where content scrolls under it.
    @ViewBuilder func bottomBar(@ViewBuilder _ content: () -> some View) -> some View {
        if #available(iOS 26, *) {
            safeAreaBar(edge: .bottom, spacing: 0, content: content)
        } else {
            safeAreaInset(edge: .bottom, spacing: 0, content: content)
        }
    }
}
