import AVKit
import LiveKit
import SwiftUI

/// Participant attribute naming whose screen share we watch (D22), same as desktop.
private let watchingKey = "watching"
/// The soundboard's own track (D37).
private let boardTrack = "soundboard"

/// Someone in the call, by ergo account (chadd's token identity).
struct Person: Equatable {
    let identity: String
    let local: Bool
    let speaking: Bool
    let muted: Bool
    let sharing: Bool
}

struct CallState {
    var connected = false
    var reconnecting = false
    /// Ourselves first.
    var people: [Person] = []
    var muted = false
    var deafened = false
    /// Whose screen share we watch, and its track once subscribed.
    var watching: String?
    var screen: VideoTrack?
}

/// The core's `Voice`: called on core threads, played on the main one.
final class VoiceBridge: Voice, @unchecked Sendable {
    @MainActor weak var calls: Calls?

    func join(id: UInt64, url: String, token: String, call: Call) {
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated { self?.calls?.join(id: id, url: url, token: token, call: call) }
        }
    }

    func hangUp(id: UInt64) {
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated { self?.calls?.hangUp(id: id) }
        }
    }
}

/// Plays the core's voice calls with the LiveKit Swift SDK, one at a time.
/// The core decides when to join and leave; this connects, and only
/// subscribes to a screen share while it's watched (D22). The SDK sets up
/// the audio session (speaker, Bluetooth and wired headsets), and the
/// `audio` background mode keeps a call going with the app in the background.
@MainActor @Observable
final class Calls {
    /// `nil` outside a call.
    private(set) var state: CallState?

    @ObservationIgnored private var id: UInt64 = 0
    @ObservationIgnored private(set) var room: Room?
    @ObservationIgnored private var events: RoomEvents?
    @ObservationIgnored private var call: Call?
    /// Subscriptions asked for, by track sid: `isSubscribed` lags the ask.
    @ObservationIgnored private var asked: [String: Bool] = [:]
    // Kept across calls, like desktop.
    @ObservationIgnored private var muted = false
    @ObservationIgnored private var deafened = false

    private var model: AppModel { AppModel.shared }

    fileprivate func join(id: UInt64, url: String, token: String, call: Call) {
        stop()
        self.id = id
        self.call = call
        let s = model.settings
        let options = RoomOptions(
            defaultAudioCaptureOptions: AudioCaptureOptions(
                echoCancellation: s.echoCancellation,
                autoGainControl: s.autoGain,
                noiseSuppression: s.noiseSuppression
            ),
            defaultAudioPublishOptions: AudioPublishOptions(encoding: AudioEncoding(maxBitrate: s.bitrateKbps * 1000)),
            // Adaptive stream pauses a video that isn't on screen, which
            // picture-in-picture is, as far as the SDK can tell.
            adaptiveStream: false
        )
        let events = RoomEvents(calls: self)
        let r = Room(delegate: events, connectOptions: ConnectOptions(autoSubscribe: false), roomOptions: options)
        room = r
        self.events = events
        state = CallState(muted: muted, deafened: deafened)
        Task {
            do {
                try await r.connect(url: url, token: token)
                guard room === r else { return }
                if !(muted || deafened) { try await r.localParticipant.setMicrophone(enabled: true) }
            } catch {
                if room === r {
                    stop()
                    call.ended(reason: "Couldn't join voice: \(error.localizedDescription)")
                }
                return
            }
            guard room === r else { return }
            subscribe(r)
            refresh(r)
            call.connected()
        }
    }

    fileprivate func hangUp(id: UInt64) {
        if id == self.id { stop() }
    }

    private func stop() {
        if let r = room {
            Task { await r.disconnect() }
        }
        room = nil
        events = nil
        call = nil
        asked = [:]
        state = nil
        model.sounds.stop()
    }

    func setMuted(_ on: Bool) {
        // Unmuting while deafened undeafens too, like desktop.
        if !on && deafened { return setDeafened(false) }
        muted = on
        apply()
    }

    func setDeafened(_ on: Bool) {
        deafened = on
        if !on { muted = false }
        apply()
    }

    private func apply() {
        guard let r = room else { return }
        state?.muted = muted
        state?.deafened = deafened
        // Muting drops a clip that's playing, like desktop.
        if muted || deafened { model.sounds.stopPlaying() }
        deafen(r)
        let on = !(muted || deafened)
        Task {
            _ = try? await r.localParticipant.setMicrophone(enabled: on)
            refresh(r)
        }
    }

    /// Every remote audio track at volume 0 while deafened.
    private func deafen(_ r: Room) {
        for p in r.remoteParticipants.values {
            for pub in p.trackPublications.values {
                (pub.track as? RemoteAudioTrack)?.volume = deafened ? 0 : 1
            }
        }
    }

    /// Watch someone's screen share, or stop with `nil`.
    func watch(_ identity: String?) {
        guard let r = room, var s = state, s.watching != identity else { return }
        s.watching = identity
        s.screen = nil
        state = s
        subscribe(r)
        // Tells the sharer how many watch (D22).
        Task { try? await r.localParticipant.set(attributes: [watchingKey: identity ?? ""]) }
    }

    /// Mics and the soundboard always; a screen share and its audio only while watched.
    private func subscribe(_ r: Room) {
        let watching = state?.watching
        for p in r.remoteParticipants.values {
            for case let pub as RemoteTrackPublication in p.trackPublications.values {
                let want = switch pub.source {
                case .microphone: true
                case .unknown: pub.name == boardTrack
                case .screenShareVideo, .screenShareAudio: p.identity?.stringValue == watching
                default: false
                }
                let sid = pub.sid.stringValue
                if (asked[sid] ?? pub.isSubscribed) != want {
                    asked[sid] = want
                    Task { try? await pub.set(subscribed: want) }
                }
            }
        }
    }

    fileprivate func event(_ r: Room, _ e: RoomEvents.Kind) {
        guard room === r else { return }
        switch e {
        case let .disconnected(error):
            let call = self.call
            stop()
            call?.ended(reason: error.map { "Voice disconnected: \($0.localizedDescription)" })
            return
        case .reconnecting: state?.reconnecting = true
        case .reconnected: state?.reconnecting = false
        case .tracks: subscribe(r)
        case .subscribed: deafen(r)
        case .other: break
        }
        refresh(r)
    }

    private func refresh(_ r: Room) {
        guard room === r, var s = state else { return }
        func person(_ p: Participant, local: Bool) -> Person {
            let pubs = p.trackPublications.values
            return Person(
                identity: p.identity?.stringValue ?? "",
                local: local,
                speaking: p.isSpeaking,
                muted: !pubs.contains { $0.source == .microphone && !$0.isMuted },
                sharing: pubs.contains { $0.source == .screenShareVideo }
            )
        }
        let people = [person(r.localParticipant, local: true)]
            + r.remoteParticipants.values.map { person($0, local: false) }
            .sorted { $0.identity.lowercased() < $1.identity.lowercased() }
        let watched = r.remoteParticipants.values.first { $0.identity?.stringValue == s.watching }
        let screen = watched?.trackPublications.values.first { $0.source == .screenShareVideo }?.track as? VideoTrack
        // Their share ended: back to the chat.
        let was = s.watching
        if watched == nil || !people.contains(where: { $0.identity == s.watching && $0.sharing }) {
            s.watching = nil
        }
        s.connected = r.connectionState == .connected
        s.people = people
        s.screen = s.watching == nil ? nil : screen
        state = s
        if s.watching == nil && was != nil {
            Task { try? await r.localParticipant.set(attributes: [watchingKey: ""]) }
        }
    }
}

/// The room's delegate, which the SDK calls on its own queues.
private final class RoomEvents: NSObject, RoomDelegate, @unchecked Sendable {
    enum Kind {
        case disconnected(LiveKitError?), reconnecting, reconnected, tracks, subscribed, other
    }

    @MainActor private weak var calls: Calls?

    @MainActor init(calls: Calls) {
        self.calls = calls
    }

    private func send(_ room: Room, _ k: Kind) {
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated { self?.calls?.event(room, k) }
        }
    }

    func room(_ room: Room, didDisconnectWithError error: LiveKitError?) { send(room, .disconnected(error)) }
    func room(_ room: Room, didStartReconnectWithMode reconnectMode: ReconnectMode) { send(room, .reconnecting) }
    func room(_ room: Room, didCompleteReconnectWithMode reconnectMode: ReconnectMode) { send(room, .reconnected) }
    func room(_ room: Room, didUpdateConnectionState connectionState: ConnectionState, from oldConnectionState: ConnectionState) {
        send(room, .other)
    }

    func room(_ room: Room, participantDidConnect participant: RemoteParticipant) { send(room, .tracks) }
    func room(_ room: Room, participantDidDisconnect participant: RemoteParticipant) { send(room, .other) }
    func room(_ room: Room, participant: RemoteParticipant, didPublishTrack publication: RemoteTrackPublication) {
        send(room, .tracks)
    }

    func room(_ room: Room, participant: RemoteParticipant, didUnpublishTrack publication: RemoteTrackPublication) {
        send(room, .other)
    }

    func room(_ room: Room, participant: RemoteParticipant, didSubscribeTrack publication: RemoteTrackPublication) {
        send(room, .subscribed)
    }

    func room(_ room: Room, participant: RemoteParticipant, didUnsubscribeTrack publication: RemoteTrackPublication) {
        send(room, .other)
    }

    func room(_ room: Room, participant: LocalParticipant, didPublishTrack publication: LocalTrackPublication) {
        send(room, .other)
    }

    func room(_ room: Room, participant: LocalParticipant, didUnpublishTrack publication: LocalTrackPublication) {
        send(room, .other)
    }

    func room(_ room: Room, didUpdateSpeakingParticipants participants: [Participant]) { send(room, .other) }
    func room(_ room: Room, participant: Participant, trackPublication: TrackPublication, didUpdateIsMuted isMuted: Bool) {
        send(room, .other)
    }
}

/// Over the bottom of the screen while in a call on any network.
struct VoicePanel: View {
    @Environment(AppModel.self) private var model
    let nets: [Network]

    var body: some View {
        if let call = model.calls.state, let net = nets.first(where: { $0.voice != nil }) {
            let calls = model.calls
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 10) {
                    VStack(alignment: .leading) {
                        let ok = call.connected && !call.reconnecting
                        Text(!call.connected ? "Connecting…" : call.reconnecting ? "Reconnecting…" : "Voice connected")
                            .font(.subheadline.bold())
                            .foregroundStyle(ok ? .green : .orange)
                        Text("\(net.voice ?? "") · \(net.name)").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                    Spacer()
                    toggle(call.muted, on: "mic.slash.fill", off: "mic.fill", label: call.muted ? "Unmute" : "Mute") {
                        calls.setMuted(!call.muted)
                    }
                    toggle(call.deafened, on: "speaker.slash.fill", off: "headphones",
                           label: call.deafened ? "Undeafen" : "Deafen") {
                        calls.setDeafened(!call.deafened)
                    }
                    Button { model.sounds.open(network: net.network) } label: { Image(systemName: "music.note") }
                        .disabled(!call.connected)
                        .accessibilityLabel("Soundboard")
                    SwiftUIAudioRoutePickerButton()
                        .frame(width: 28, height: 28)
                        .accessibilityLabel("Audio output")
                    Button { model.client.leaveVoice(network: net.network) } label: {
                        Image(systemName: "phone.down.fill")
                            .foregroundStyle(.white)
                            .padding(8)
                            .background(.red, in: Circle())
                    }
                    .accessibilityLabel("Leave voice")
                }
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(call.people, id: \.identity) { p in
                            let watchable = p.sharing && !p.local
                            Button { if watchable { calls.watch(p.identity) } } label: {
                                HStack(spacing: 4) {
                                    if p.muted { Image(systemName: "mic.slash").font(.caption2) }
                                    Text(p.identity).font(.caption)
                                    if p.sharing { Image(systemName: "display").font(.caption2) }
                                }
                                .padding(.horizontal, 10)
                                .padding(.vertical, 5)
                                .overlay(Capsule().stroke(p.speaking ? .green : Color(.separator), lineWidth: p.speaking ? 2 : 1))
                            }
                            .buttonStyle(.plain)
                            .disabled(!watchable)
                            .accessibilityLabel(p.identity + (p.muted ? ", muted" : "") + (p.speaking ? ", speaking" : "")
                                + (p.sharing ? ", sharing their screen" : ""))
                            .accessibilityHint(watchable ? "Watch their screen" : "")
                        }
                    }
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(.bar)
        }
    }

    private func toggle(_ on: Bool, on onIcon: String, off: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: on ? onIcon : off)
                .frame(width: 32, height: 32)
                .background(on ? Color.red.opacity(0.2) : Color(.secondarySystemBackground), in: Circle())
        }
        .accessibilityLabel(label)
    }
}

/// Someone's screen share, full screen: rotate the device or pinch to zoom.
/// It carries on in picture-in-picture when the app goes to the background.
struct ScreenViewer: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ZStack(alignment: .top) {
            Color.black.ignoresSafeArea()
            if let track = model.calls.state?.screen {
                Zoomable { ScreenVideo(track: track) }
                    .ignoresSafeArea()
                    .accessibilityLabel("\(model.calls.state?.watching ?? "")'s screen")
            } else {
                ProgressView().tint(.white).frame(maxHeight: .infinity)
            }
            HStack {
                Text("\(model.calls.state?.watching ?? "")'s screen").foregroundStyle(.white)
                Spacer()
                Button { model.calls.watch(nil) } label: {
                    Image(systemName: "xmark.circle.fill").font(.title).foregroundStyle(.white, .white.opacity(0.3))
                }
                .accessibilityLabel("Stop watching")
            }
            .padding()
        }
    }
}

/// LiveKit's `VideoView` drawing into an `AVSampleBufferDisplayLayer`, which
/// is what picture-in-picture takes.
private struct ScreenVideo: UIViewRepresentable {
    let track: VideoTrack

    func makeCoordinator() -> PiP { PiP() }

    func makeUIView(context: Context) -> VideoView {
        let v = VideoView()
        v.layoutMode = .fit
        v.renderMode = .sampleBuffer
        v.delegates.add(delegate: context.coordinator)
        v.track = track
        return v
    }

    func updateUIView(_ v: VideoView, context: Context) {
        if v.track !== track { v.track = track }
        context.coordinator.attach(v)
    }

    static func dismantleUIView(_ v: VideoView, coordinator: PiP) {
        coordinator.controller?.stopPictureInPicture()
        v.track = nil
    }
}

/// Live content for the system's picture-in-picture: no timeline, never paused.
private final class PiP: NSObject, VideoViewDelegate, AVPictureInPictureSampleBufferPlaybackDelegate, @unchecked Sendable {
    var controller: AVPictureInPictureController?

    @MainActor func attach(_ v: VideoView) {
        guard controller == nil, AVPictureInPictureController.isPictureInPictureSupported(),
              let layer = v.avSampleBufferDisplayLayer
        else { return }
        let c = AVPictureInPictureController(contentSource: .init(sampleBufferDisplayLayer: layer, playbackDelegate: self))
        c.canStartPictureInPictureAutomaticallyFromInline = true
        c.requiresLinearPlayback = true
        controller = c
    }

    // The layer exists once the first frame is drawn.
    func videoView(_ videoView: VideoView, didUpdate isRendering: Bool) {
        guard isRendering else { return }
        DispatchQueue.main.async { MainActor.assumeIsolated { self.attach(videoView) } }
    }

    func pictureInPictureController(_ c: AVPictureInPictureController, setPlaying playing: Bool) {}

    func pictureInPictureControllerTimeRangeForPlayback(_ c: AVPictureInPictureController) -> CMTimeRange {
        CMTimeRange(start: .negativeInfinity, duration: .positiveInfinity)
    }

    func pictureInPictureControllerIsPlaybackPaused(_ c: AVPictureInPictureController) -> Bool { false }

    func pictureInPictureController(_ c: AVPictureInPictureController, didTransitionToRenderSize size: CMVideoDimensions) {}

    func pictureInPictureController(_ c: AVPictureInPictureController, skipByInterval interval: CMTime) async {}
}
