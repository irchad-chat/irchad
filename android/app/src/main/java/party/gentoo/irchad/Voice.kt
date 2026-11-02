package party.gentoo.irchad

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import io.livekit.android.ConnectOptions
import io.livekit.android.LiveKit
import io.livekit.android.LiveKitOverrides
import io.livekit.android.RoomOptions
import io.livekit.android.events.DisconnectReason
import io.livekit.android.events.RoomEvent
import io.livekit.android.events.collect
import io.livekit.android.room.Room
import io.livekit.android.room.participant.Participant
import io.livekit.android.room.track.LocalAudioTrack
import io.livekit.android.room.track.RemoteTrackPublication
import io.livekit.android.room.track.Track
import io.livekit.android.room.track.VideoTrack
import java.io.File
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
import party.gentoo.irchad.core.Voice

/** Participant attribute naming whose screen share we watch (D22), same as desktop. */
private const val WATCHING = "watching"
/** The soundboard's own track (D37). */
private const val BOARD = "soundboard"

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
        val r = LiveKit.create(app, RoomOptions(adaptiveStream = true), LiveKitOverrides(okHttpClient = http))
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
    }

    /** Soundboard clips go out mixed into the mic (D48). */
    private fun mix(r: Room) {
        (r.localParticipant.getTrackPublication(Track.Source.MICROPHONE)?.track as? LocalAudioTrack)
            ?.setAudioBufferCallback(app.sounds.mixer)
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
        if (muted || deafened) app.sounds.mixer.clip = null
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

    /** Mics and the soundboard always; a screen share and its audio only while watched. */
    private fun subscribe(r: Room) {
        val watching = state.value?.watching
        for (p in r.remoteParticipants.values) {
            for (pub in p.trackPublications.values) {
                pub as? RemoteTrackPublication ?: continue
                val want = when (pub.source) {
                    Track.Source.MICROPHONE -> true
                    Track.Source.UNKNOWN -> pub.name == BOARD
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
