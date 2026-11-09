package party.gentoo.irchad

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import com.twilio.audioswitch.AudioDevice
import io.livekit.android.ConnectOptions
import io.livekit.android.LiveKit
import io.livekit.android.LiveKitOverrides
import io.livekit.android.RoomOptions
import io.livekit.android.audio.AudioBufferCallback
import io.livekit.android.events.DisconnectReason
import io.livekit.android.events.RoomEvent
import io.livekit.android.events.collect
import io.livekit.android.room.Room
import io.livekit.android.room.participant.AudioTrackPublishDefaults
import io.livekit.android.room.participant.ConnectionQuality
import io.livekit.android.room.participant.Participant
import io.livekit.android.room.track.LocalAudioTrack
import io.livekit.android.room.track.LocalAudioTrackOptions
import io.livekit.android.room.track.RemoteAudioTrack
import io.livekit.android.room.track.RemoteTrackPublication
import io.livekit.android.room.track.Track
import io.livekit.android.room.track.VideoTrack
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.security.KeyStore
import java.security.cert.CertificateFactory
import javax.net.ssl.SSLContext
import javax.net.ssl.TrustManagerFactory
import javax.net.ssl.X509TrustManager
import kotlinx.coroutines.CoroutineScope
import okhttp3.OkHttpClient
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch
import party.gentoo.irchad.core.Call
import party.gentoo.irchad.core.Denoiser
import party.gentoo.irchad.core.Voice

/** Participant attribute naming whose screen share we watch (D22), same as desktop. */
private const val WATCHING = "watching"
/** The soundboard's own track (D37). */
private const val BOARD = "soundboard"
/** Desktop's whisper track (D71): the SFU only lets its targets subscribe. */
private const val WHISPER = "whisper"

/** Someone in the call, by ergo account (chadd's token identity). */
data class Person(
    val identity: String,
    val local: Boolean,
    val speaking: Boolean,
    val muted: Boolean,
    val sharing: Boolean,
)

data class CallState(
    val connected: Boolean = false,
    val reconnecting: Boolean = false,
    /** Ourselves first. */
    val people: List<Person> = emptyList(),
    val muted: Boolean = false,
    val deafened: Boolean = false,
    /** Whose screen share we watch, and its track once subscribed. */
    val watching: String? = null,
    val screen: VideoTrack? = null,
    /** Our own connection to the SFU. */
    val quality: ConnectionQuality = ConnectionQuality.UNKNOWN,
    /** Where the call plays: earpiece, speaker, headset… */
    val outputs: List<AudioDevice> = emptyList(),
    val output: AudioDevice? = null,
)

/**
 * Plays the core's voice calls with the LiveKit Android SDK, one at a time.
 * The core decides when to join and leave; this connects, keeps the
 * foreground service up, and only subscribes to a screen share while it's
 * watched. Audio focus and headset/Bluetooth routing are the SDK's
 * AudioSwitch defaults.
 */
class Calls(private val app: IrChad) : Voice {
    /** `null` outside a call. */
    val state = MutableStateFlow<CallState?>(null)
    /** The watched share's frame, width to height, for picture-in-picture. */
    val frame = MutableStateFlow<Pair<Int, Int>?>(null)
    val mic by lazy { Mic(app.settings) }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private var id = 0UL
    private var room: Room? = null
    private var job: Job? = null
    /** Same trust as the core (D42): the system's CAs plus any in `files/certs`. */
    private val http: OkHttpClient? by lazy {
        val extra = File(app.filesDir, "certs").listFiles().orEmpty().filter { it.isFile }
        if (extra.isEmpty()) return@lazy null
        val ks = KeyStore.getInstance("AndroidCAStore").apply { load(null) }
        val roots = KeyStore.getInstance(KeyStore.getDefaultType()).apply { load(null) }
        for (alias in ks.aliases()) roots.setCertificateEntry(alias, ks.getCertificate(alias))
        val cf = CertificateFactory.getInstance("X.509")
        for (f in extra) {
            runCatching { f.inputStream().use { roots.setCertificateEntry("files:${f.name}", cf.generateCertificate(it)) } }
        }
        val tm = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm())
            .apply { init(roots) }.trustManagers.filterIsInstance<X509TrustManager>().first()
        val ssl = SSLContext.getInstance("TLS").apply { init(null, arrayOf(tm), null) }
        OkHttpClient.Builder().sslSocketFactory(ssl.socketFactory, tm).build()
    }

    // Kept across calls, like desktop.
    private var muted = false
    private var deafened = false

    // Both come from a core thread; everything below runs on the main thread, in order.
    override fun join(id: ULong, url: String, token: String, call: Call) {
        scope.launch { start(id, url, token, call) }
    }

    override fun hangUp(id: ULong) {
        scope.launch { if (id == this@Calls.id) stop() }
    }

    private suspend fun start(id: ULong, url: String, token: String, call: Call) {
        stop()
        this.id = id
        val s = app.settings
        val options = RoomOptions(
            adaptiveStream = true,
            audioTrackCaptureDefaults = LocalAudioTrackOptions(
                noiseSuppression = s.noiseSuppression,
                echoCancellation = s.echoCancellation,
                autoGainControl = s.autoGain,
            ),
            audioTrackPublishDefaults = AudioTrackPublishDefaults(audioBitrate = s.bitrateKbps * 1000),
        )
        val r = LiveKit.create(app, options, LiveKitOverrides(okHttpClient = http))
        room = r
        state.value = CallState(muted = muted, deafened = deafened)
        // Throws if the core rejoins while we're in the background; the call still works until Android stops us.
        runCatching { ContextCompat.startForegroundService(app, Intent(app, VoiceService::class.java)) }
        job = scope.launch {
            r.events.collect { ev ->
                when (ev) {
                    is RoomEvent.Disconnected -> {
                        if (room === r) {
                            stop()
                            call.ended(
                                ev.error?.message
                                    ?: "Voice disconnected (${ev.reason}).".takeIf { ev.reason != DisconnectReason.CLIENT_INITIATED },
                            )
                        }
                        return@collect
                    }
                    is RoomEvent.TrackPublished, is RoomEvent.ParticipantConnected -> subscribe(r)
                    is RoomEvent.TrackSubscribed -> volumes(r)
                    else -> {}
                }
                refresh(r, ev)
            }
        }
        try {
            r.connect(url, token, ConnectOptions(autoSubscribe = false))
            r.setSpeakerMute(deafened)
            if (!(muted || deafened)) r.localParticipant.setMicrophoneEnabled(true)
            mix(r)
            r.audioSwitchHandler?.let { h ->
                h.registerAudioDeviceChangeListener { list, selected ->
                    scope.launch { if (room === r) state.value = state.value?.copy(outputs = list, output = selected) }
                }
                state.value = state.value?.copy(outputs = h.availableAudioDevices, output = h.selectedAudioDevice)
            }
        } catch (e: Exception) {
            if (room === r) {
                stop()
                call.ended("Couldn't join voice: ${e.message}")
            }
            return
        }
        subscribe(r)
        refresh(r, null)
        call.connected()
    }

    private fun stop() {
        job?.cancel()
        job = null
        room?.let {
            it.disconnect()
            it.release()
        }
        room = null
        state.value = null
        frame.value = null
        app.sounds.stop()
        mic.reset()
    }

    /** Our processing of the mic, and soundboard clips mixed into it (D48). */
    private fun mix(r: Room) {
        (r.localParticipant.getTrackPublication(Track.Source.MICROPHONE)?.track as? LocalAudioTrack)
            ?.setAudioBufferCallback(mic)
    }

    /** Play the call there. */
    fun output(d: AudioDevice) {
        room?.audioSwitchHandler?.selectDevice(d)
    }

    /** Someone's voice, or their screen share's sound, for us (0–200%), kept for next time. */
    fun setVolume(identity: String, screen: Boolean, v: Float) {
        app.settings.setVolume(identity, screen, v)
        room?.let(::volumes)
    }

    /** Everyone's volumes as set, and the soundboard's. */
    fun volumes(r: Room) {
        val s = app.settings
        for (p in r.remoteParticipants.values) {
            val id = p.identity?.value ?: continue
            for (pub in p.trackPublications.values) {
                val t = pub.track as? RemoteAudioTrack ?: continue
                val v = when {
                    pub.source == Track.Source.MICROPHONE || pub.name == WHISPER -> s.volume(id, false)
                    pub.source == Track.Source.SCREEN_SHARE_AUDIO -> s.volume(id, true)
                    pub.name == BOARD -> s.soundboardVolume
                    else -> continue
                }
                t.setVolume(v.toDouble())
            }
        }
    }

    fun setMuted(on: Boolean) {
        // Unmuting while deafened undeafens too, like desktop.
        if (!on && deafened) return setDeafened(false)
        muted = on
        apply()
    }

    fun setDeafened(on: Boolean) {
        deafened = on
        if (!on) muted = false
        apply()
    }

    private fun apply() {
        val r = room ?: return
        r.setSpeakerMute(deafened)
        state.value = state.value?.copy(muted = muted, deafened = deafened)
        // Muting drops a clip that's playing, like desktop.
        if (muted || deafened) mic.clip = null
        scope.launch {
            runCatching { r.localParticipant.setMicrophoneEnabled(!(muted || deafened)) }
            mix(r)
            refresh(r, null)
        }
    }

    /** Watch someone's screen share, or stop with `null`. */
    fun watch(identity: String?) {
        val r = room ?: return
        val s = state.value ?: return
        if (s.watching == identity) return
        state.value = s.copy(watching = identity, screen = null)
        frame.value = null
        subscribe(r)
        // Tells the sharer how many watch (D22).
        runCatching { r.localParticipant.updateAttributes(mapOf(WATCHING to (identity ?: ""))) }
    }

    /**
     * Mics, whispers and the soundboard always (a whisper only arrives if we're its
     * target); a screen share and its audio only while watched.
     */
    private fun subscribe(r: Room) {
        val watching = state.value?.watching
        for (p in r.remoteParticipants.values) {
            for (pub in p.trackPublications.values) {
                pub as? RemoteTrackPublication ?: continue
                val want = when (pub.source) {
                    Track.Source.MICROPHONE -> true
                    Track.Source.UNKNOWN -> pub.name == BOARD || pub.name == WHISPER
                    Track.Source.SCREEN_SHARE, Track.Source.SCREEN_SHARE_AUDIO -> p.identity?.value == watching
                    else -> false
                }
                if (want != pub.isDesired) pub.setSubscribed(want)
            }
        }
    }

    private fun refresh(r: Room, ev: RoomEvent?) {
        if (room !== r) return
        val s = state.value ?: return
        fun Participant.person(local: Boolean) = Person(
            identity = identity?.value ?: "",
            local = local,
            speaking = isSpeaking,
            muted = trackPublications.values.none { it.source == Track.Source.MICROPHONE && !it.muted },
            sharing = trackPublications.values.any { it.source == Track.Source.SCREEN_SHARE },
        )
        val people = listOf(r.localParticipant.person(true)) +
            r.remoteParticipants.values.map { it.person(false) }.sortedBy { it.identity.lowercase() }
        val watched = r.remoteParticipants.values.firstOrNull { it.identity?.value == s.watching }
        val screen = watched?.trackPublications?.values
            ?.firstOrNull { it.source == Track.Source.SCREEN_SHARE }?.track as? VideoTrack
        // Their share ended: back to the chat.
        val watching = s.watching?.takeIf { watched != null && people.any { it.identity == s.watching && it.sharing } }
        state.value = s.copy(
            connected = r.state == Room.State.CONNECTED || ev is RoomEvent.Reconnected,
            reconnecting = when (ev) {
                is RoomEvent.Reconnecting -> true
                is RoomEvent.Reconnected -> false
                else -> s.reconnecting
            },
            people = people,
            watching = watching,
            screen = screen.takeIf { watching != null },
            quality = r.localParticipant.connectionQuality,
        )
        if (watching == null && s.watching != null) {
            runCatching { r.localParticipant.updateAttributes(mapOf(WATCHING to "")) }
        }
    }

    /** Lets the room view attach a renderer. */
    fun room(): Room? = room
}

/**
 * Keeps the mic and the call alive while the app is in the background,
 * as the ongoing-call notification with mute, deafen and leave.
 */
class VoiceService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL, "Voice calls", NotificationManager.IMPORTANCE_LOW),
        )
        val app = application as IrChad
        try {
            ServiceCompat.startForeground(
                this, ID, notification(app, app.calls.state.value ?: CallState()),
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE,
            )
        } catch (e: Exception) {
            // No mic permission (the UI asks before joining).
            stopSelf()
            return
        }
        scope.launch {
            app.calls.state.collectLatest { s ->
                if (s == null) {
                    ServiceCompat.stopForeground(this@VoiceService, ServiceCompat.STOP_FOREGROUND_REMOVE)
                    stopSelf()
                } else {
                    nm.notify(ID, notification(app, s))
                }
            }
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val app = application as IrChad
        val s = app.calls.state.value
        when (intent?.action) {
            MUTE -> s?.let { app.calls.setMuted(!it.muted) }
            DEAFEN -> s?.let { app.calls.setDeafened(!it.deafened) }
            LEAVE -> app.client.networks().firstOrNull { it.voice != null }
                ?.let { app.client.leaveVoice(it.network) }
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    private fun notification(app: IrChad, s: CallState): Notification {
        val net = app.client.networks().firstOrNull { it.voice != null }
        fun action(icon: Int, title: String, what: String) = NotificationCompat.Action(
            icon, title,
            PendingIntent.getService(
                this, what.hashCode(), Intent(this, VoiceService::class.java).setAction(what),
                PendingIntent.FLAG_IMMUTABLE,
            ),
        )
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE,
        )
        return NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(if (s.muted || s.deafened) R.drawable.ic_mic_off else R.drawable.ic_mic)
            .setContentTitle(net?.voice ?: "Voice")
            .setContentText(
                when {
                    !s.connected -> "Connecting…"
                    s.reconnecting -> "Reconnecting…"
                    else -> "${s.people.size} in voice" + (net?.let { " · ${it.name}" } ?: "")
                },
            )
            .setContentIntent(open)
            .setOngoing(true)
            .setSilent(true)
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .addAction(action(R.drawable.ic_mic_off, if (s.muted) "Unmute" else "Mute", MUTE))
            .addAction(action(R.drawable.ic_headset, if (s.deafened) "Undeafen" else "Deafen", DEAFEN))
            .addAction(action(R.drawable.ic_call_end, "Leave", LEAVE))
            .build()
    }

    private companion object {
        const val CHANNEL = "voice"
        const val ID = 1
        const val MUTE = "party.gentoo.irchad.MUTE"
        const val DEAFEN = "party.gentoo.irchad.DEAFEN"
        const val LEAVE = "party.gentoo.irchad.LEAVE"
    }
}

/** The level of silence, as desktop measures it. */
const val SILENT_DB = -100f
/** Voice activity keeps sending this long after the level drops, like desktop. */
private const val VAD_HANG_MS = 300L
/** RNNoise's frame: 10 ms at 48 kHz. */
private const val RNNOISE_FRAME = 480

/**
 * What desktop does to the mic, on the audio thread, in place: RNNoise,
 * input volume, the voice-activity or push-to-talk gate, the "hear yourself"
 * test, then the soundboard clip mixed in (D48). Unlike desktop it runs
 * before WebRTC's echo canceller and noise suppression, which sit after the
 * SDK's capture callback.
 */
class Mic(private val s: Settings) : AudioBufferCallback {
    /** The soundboard clip playing, if any. */
    @Volatile var clip: Playing? = null
    /** The push-to-talk button is down. */
    @Volatile var held = false
    /** Plays the processed mic back ("Hear yourself"). */
    @Volatile var testing = false
    /** For the settings' meter. */
    @Volatile var level = SILENT_DB
        private set
    @Volatile var transmitting = false
        private set

    private var openUntil = 0L
    private val denoiser by lazy { Denoiser() }
    private var pcm = ShortArray(0)
    private var frame = ByteArray(RNNOISE_FRAME * 2)
    private var echo: AudioTrack? = null
    private var echoShape = 0 to 0

    override fun onBuffer(
        buffer: ByteBuffer, audioFormat: Int, channelCount: Int, sampleRate: Int, bytesRead: Int, captureTimeNs: Long,
    ): Long {
        val float = audioFormat == AudioFormat.ENCODING_PCM_FLOAT
        if (!float && audioFormat != AudioFormat.ENCODING_PCM_16BIT) return 0
        val b = buffer.order(ByteOrder.nativeOrder())
        val n = (bytesRead / if (float) 4 else 2).coerceAtMost(b.capacity() / if (float) 4 else 2)
        if (pcm.size < n) pcm = ShortArray(n)
        for (i in 0 until n) {
            pcm[i] = if (float) (b.getFloat(i * 4) * 32767f).coerceIn(-32768f, 32767f).toInt().toShort() else b.getShort(i * 2)
        }
        process(n, maxOf(1, channelCount), sampleRate)
        for (i in 0 until n) if (float) b.putFloat(i * 4, pcm[i] / 32768f) else b.putShort(i * 2, pcm[i])
        // 0: the capture time stays as it was.
        return 0
    }

    private fun process(n: Int, channels: Int, rate: Int) {
        // RNNoise only takes 48 kHz mono, which is what WebRTC records.
        if (s.rnnoise && channels == 1 && rate == 48_000 && n % RNNOISE_FRAME == 0) {
            for (at in 0 until n step RNNOISE_FRAME) {
                for (i in 0 until RNNOISE_FRAME) {
                    val x = pcm[at + i].toInt()
                    frame[2 * i] = x.toByte()
                    frame[2 * i + 1] = (x shr 8).toByte()
                }
                val out = denoiser.process(frame)
                if (out.size != frame.size) continue
                for (i in 0 until RNNOISE_FRAME) {
                    pcm[at + i] = ((out[2 * i].toInt() and 0xff) or (out[2 * i + 1].toInt() shl 8)).toShort()
                }
            }
        }
        val gain = s.inputGain
        if (gain != 1f) for (i in 0 until n) pcm[i] = (pcm[i] * gain).coerceIn(-32768f, 32767f).toInt().toShort()

        val db = dbfs(pcm, n)
        val now = System.nanoTime() / 1_000_000
        val open = if (s.pushToTalk) {
            if (held) openUntil = now + s.pttReleaseMs
            held || now < openUntil
        } else {
            if (db > s.vadThresholdDb) openUntil = now + VAD_HANG_MS
            now < openUntil
        }
        level = db
        transmitting = open
        if (!open) pcm.fill(0, 0, n)

        hear(n, channels, rate)

        val c = clip ?: return
        if (c.rate != rate) {
            c.pos = (c.pos.toLong() * rate / c.rate).toInt()
            c.samples = resample(c.pcm, rate)
            c.rate = rate
        }
        val frames = n / channels
        for (f in 0 until frames) {
            val x = c.samples.getOrElse(c.pos + f) { 0 }
            for (ch in 0 until channels) {
                val i = f * channels + ch
                pcm[i] = (pcm[i] + x).coerceIn(-32768, 32767).toShort()
            }
        }
        c.pos += frames
        if (c.pos >= c.samples.size) clip = null
    }

    /** "Hear yourself": the gated mic, before the clip, to the earpiece or headset. */
    private fun hear(n: Int, channels: Int, rate: Int) {
        if (!testing || echoShape != (rate to channels)) {
            echo?.release()
            echo = null
        }
        if (!testing) return
        val t = echo ?: runCatching {
            AudioTrack.Builder()
                .setAudioAttributes(
                    AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build(),
                )
                .setAudioFormat(
                    AudioFormat.Builder().setEncoding(AudioFormat.ENCODING_PCM_16BIT).setSampleRate(rate)
                        .setChannelMask(if (channels == 1) AudioFormat.CHANNEL_OUT_MONO else AudioFormat.CHANNEL_OUT_STEREO)
                        .build(),
                )
                .setTransferMode(AudioTrack.MODE_STREAM)
                .setBufferSizeInBytes(rate / 5 * channels * 2)
                .build().also { it.play() }
        }.getOrNull()?.also {
            echo = it
            echoShape = rate to channels
        } ?: return
        // Never blocks the audio thread: what doesn't fit is dropped.
        t.write(pcm, 0, n, AudioTrack.WRITE_NON_BLOCKING)
    }

    /** Out of a call. */
    fun reset() {
        clip = null
        held = false
        testing = false
        level = SILENT_DB
        transmitting = false
        // The audio thread is gone by now; the next call makes a new one.
        echo?.release()
        echo = null
    }
}

internal fun dbfs(pcm: ShortArray, n: Int): Float {
    if (n == 0) return SILENT_DB
    var sum = 0.0
    for (i in 0 until n) sum += pcm[i].toDouble() * pcm[i]
    val rms = Math.sqrt(sum / n) / 32768.0
    return if (rms <= 0.0) SILENT_DB else maxOf(SILENT_DB, (20 * Math.log10(rms)).toFloat())
}
