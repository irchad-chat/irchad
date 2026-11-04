import AVFoundation
import LiveKit
import SwiftUI

/// Clips play at most this long (D37).
private let maxSeconds = 10.0
private let maxRate = 192_000.0

/// The soundboard (D37) on iOS. LiveKit's `SoundPlayer` mixes a clip into
/// the mic for the others and plays it locally too, like Android's mixer
/// (D48), so listeners hear it at this person's voice volume. A clip needs
/// the mic published: it's dropped while muted, as on desktop.
@MainActor @Observable
final class Sounds {
    /// The open soundboard: its buttons, or why there are none.
    struct Board {
        let network: String
        var clips: [Clip]?
        var error: String?
    }

    var board: Board?
    /// Prepared clips by "network file".
    @ObservationIgnored private var ready: [String: SoundHandle] = [:]
    /// The clip to play once its bytes arrive.
    @ObservationIgnored private var wanted: String?

    private var model: AppModel { AppModel.shared }

    func open(network: String) {
        board = Board(network: network)
        model.client.sounds(network: network)
    }

    func got(network: String, clips: [Clip], error: String?) {
        guard board?.network == network else { return }
        board = Board(network: network, clips: clips, error: error)
    }

    func play(network: String, file: String) {
        guard let s = model.calls.state, s.connected else { return }
        if s.muted || s.deafened {
            model.toast("Unmute to play sounds.")
            return
        }
        let key = "\(network) \(file)"
        if let h = ready[key] {
            Task { try? await h.play() }
            return
        }
        wanted = key
        model.client.sound(network: network, file: file)
    }

    func gotFile(network: String, file: String, url: URL?) {
        guard let url else {
            model.toast("That sound didn't play: it's damaged.")
            return
        }
        let key = "\(network) \(file)"
        Task {
            defer { try? FileManager.default.removeItem(at: url) }
            guard let h = try? await SoundPlayer.shared.prepare(fileURL: url, named: key) else {
                model.toast("That sound didn't play: it's damaged.")
                return
            }
            ready[key] = h
            if wanted == key {
                wanted = nil
                play(network: network, file: file)
            }
        }
    }

    func stopPlaying() {
        Task { await SoundPlayer.shared.stopAll() }
    }

    /// Out of a call: stop, and close the board.
    func stop() {
        stopPlaying()
        board = nil
        wanted = nil
    }

    /// The first 10 s of a clip, written as a CAF file, or `nil`. The bytes
    /// are the server's (D14): any failure only loses the clip, and the
    /// output is bounded.
    nonisolated static func decode(_ bytes: Data) -> URL? {
        let dir = FileManager.default.temporaryDirectory
        let raw = dir.appending(path: "clip-\(UUID().uuidString)")
        let out = dir.appending(path: "clip-\(UUID().uuidString).caf")
        defer { try? FileManager.default.removeItem(at: raw) }
        do {
            try bytes.write(to: raw)
            let f = try AVAudioFile(forReading: raw)
            let fmt = f.processingFormat
            guard fmt.sampleRate > 0, fmt.sampleRate <= maxRate, (1...8).contains(fmt.channelCount) else { return nil }
            let frames = AVAudioFrameCount(min(Double(f.length), maxSeconds * fmt.sampleRate))
            guard frames > 0, let buf = AVAudioPCMBuffer(pcmFormat: fmt, frameCapacity: frames) else { return nil }
            try f.read(into: buf, frameCount: frames)
            let w = try AVAudioFile(forWriting: out, settings: fmt.settings, commonFormat: fmt.commonFormat,
                                    interleaved: fmt.isInterleaved)
            try w.write(from: buf)
            return out
        } catch {
            try? FileManager.default.removeItem(at: out)
            return nil
        }
    }
}

/// The soundboard's buttons (D37): a clip goes out to everyone in the call.
struct SoundboardSheet: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        NavigationStack {
            ScrollView {
                if let b = model.sounds.board {
                    if let e = b.error {
                        Text(e).foregroundStyle(.red).padding()
                    } else if let clips = b.clips {
                        if clips.isEmpty {
                            Text("This network has no sounds.").foregroundStyle(.secondary).padding()
                        } else {
                            LazyVGrid(columns: [GridItem(.adaptive(minimum: 120))], spacing: 8) {
                                ForEach(clips, id: \.file) { c in
                                    Button(c.name) { model.sounds.play(network: b.network, file: c.file) }
                                        .buttonStyle(.bordered)
                                        .lineLimit(1)
                                }
                            }
                            .padding()
                        }
                    } else {
                        ProgressView().padding()
                    }
                }
            }
            .navigationTitle("Soundboard")
            .navigationBarTitleDisplayMode(.inline)
        }
        .presentationDetents([.medium])
    }
}
