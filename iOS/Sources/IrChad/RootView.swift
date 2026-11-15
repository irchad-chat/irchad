import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var model
    @State private var adding = false
    /// The core lists saved networks shortly after start; give it a moment
    /// before deciding there are none. A join in progress is listed too, so
    /// the join screen stays up by `adding`, not by the list being empty.
    @State private var settled = false
    @State private var reason = ""

    var body: some View {
        let _ = model.tick
        let nets = model.client.networks()
        Group {
            if model.invite != nil || adding {
                JoinView(
                    invite: model.invite,
                    onDone: { sel in
                        model.focus = sel
                        close()
                    },
                    onCancel: nets.isEmpty ? nil : close
                )
            } else if nets.isEmpty {
                Color(.systemBackground).ignoresSafeArea()
            } else {
                MainView(nets: nets, onAdd: { adding = true })
            }
        }
        .overlay(alignment: .bottom) { Toast() }
        .alert(
            model.reporting?.msgid != nil ? "Report message" : "Report \(model.reporting?.nick ?? "")",
            isPresented: Binding(get: { model.reporting != nil }, set: { if !$0 { model.reporting = nil } }),
            presenting: model.reporting
        ) { r in
            TextField("What's wrong? (optional)", text: $reason)
            Button("Report") {
                model.client.report(
                    network: r.network, nick: r.nick, target: r.target,
                    msgid: r.msgid, text: r.text, reason: reason
                )
                reason = ""
            }
            Button("Cancel", role: .cancel) { reason = "" }
        } message: { r in
            Text((r.text.map { "\"\($0)\"\n\n" } ?? "") + "This goes privately to the server's moderators.")
        }
        .alert("Report sent", isPresented: Binding(get: { model.reported }, set: { model.reported = $0 })) {
            Button("OK") {}
        } message: {
            Text("Thanks for letting us know. The moderators of this server will look into it.")
        }
        .sheet(isPresented: Binding(get: { model.verify != nil }, set: { if !$0 { closeVerify() } })) {
            if let v = model.verify { VerifySheet(v: v) }
        }
        .task {
            try? await Task.sleep(for: .milliseconds(400))
            settled = true
        }
        .onChange(of: settled && nets.isEmpty, initial: true) { _, none in if none { adding = true } }
    }

    private func close() {
        adding = false
        model.invite = nil
    }

    private func closeVerify() {
        guard let v = model.verify else { return }
        if v.state != .done && !isFailed(v.state) {
            model.client.verification(network: v.network, nick: v.nick, step: .cancel)
        }
        model.verify = nil
    }
}

/// One line at the bottom for a few seconds.
private struct Toast: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if let text = model.notice {
            Text(text)
                .font(.callout)
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
                .glass(in: Capsule(), fallback: AnyShapeStyle(.regularMaterial))
                .padding(.bottom, 80)
                .padding(.horizontal, 16)
                .transition(.move(edge: .bottom).combined(with: .opacity))
                .onTapGesture { model.notice = nil }
                .task(id: text) {
                    UIAccessibility.post(notification: .announcement, argument: text)
                    try? await Task.sleep(for: .seconds(4))
                    if model.notice == text { withAnimation { model.notice = nil } }
                }
        }
    }
}

/// Comparing emoji with someone (SAS).
private struct VerifySheet: View {
    @Environment(AppModel.self) private var model
    let v: Verify

    var body: some View {
        NavigationStack {
            VStack(spacing: 20) {
                switch v.state {
                case .incoming:
                    Text("\(v.nick) wants to make sure your messages to each other are private, by comparing emoji.")
                case .waiting:
                    HStack(spacing: 12) {
                        ProgressView()
                        Text("Waiting for \(v.nick)…")
                    }
                case let .compare(emoji):
                    Text("Check that \(v.nick) sees the same emoji, in the same order. Compare them in person or on a call, not by message.")
                    let rows = stride(from: 0, to: emoji.count, by: 4).map { Array(emoji[$0..<min($0 + 4, emoji.count)]) }
                    ForEach(rows.indices, id: \.self) { i in
                        HStack(spacing: 16) {
                            ForEach(rows[i], id: \.name) { e in
                                VStack {
                                    Text(e.emoji).font(.largeTitle)
                                    Text(e.name).font(.caption)
                                }
                                .frame(minWidth: 64)
                                .accessibilityElement(children: .combine)
                            }
                        }
                    }
                case .done:
                    Text("\(v.nick)'s device is verified. Nobody is listening in.")
                case let .failed(reason):
                    Text(reason)
                }
                Spacer()
                buttons
            }
            .multilineTextAlignment(.center)
            .padding()
            .navigationTitle("Verify \(v.nick)")
            .navigationBarTitleDisplayMode(.inline)
        }
        .presentationDetents([.medium, .large])
        .interactiveDismissDisabled(v.state != .done && !isFailed(v.state))
    }

    @ViewBuilder private var buttons: some View {
        let step = { (s: SasStep) in model.client.verification(network: v.network, nick: v.nick, step: s) }
        switch v.state {
        case .incoming:
            Button("Start") { step(.accept) }.glassButton(prominent: true)
            Button("Not now") { step(.cancel) }
        case .waiting:
            Button("Cancel") { step(.cancel) }
        case .compare:
            Button("They match") { step(.match) }.glassButton(prominent: true)
            Button("They don't match", role: .destructive) { step(.cancel) }
        case .done, .failed:
            Button("Done") { model.verify = nil }.glassButton(prominent: true)
        }
    }
}
