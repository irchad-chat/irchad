package party.gentoo.irchad

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.media.MediaCodec
import android.media.MediaDataSource
import android.media.MediaExtractor
import android.media.MediaFormat
import io.livekit.android.audio.MixerAudioBufferCallback
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import party.gentoo.irchad.core.Clip

/** Clips play at most this long (D37). */
private const val MAX_SECONDS = 10
private const val MAX_RATE = 192_000

/** A decoded clip: mono 16-bit PCM at `rate`. */
class Pcm(val samples: ShortArray, val rate: Int)

/** The open soundboard: its buttons, or why there are none. */
data class Board(val network: String, val clips: List<Clip>?, val error: String?)

/**
 * The soundboard (D37) on Android. The SDK has one capture path, so a clip
 * can't be its own track: it's mixed into the mic (D48). It's dropped while
 * muted, and plays locally too, so the player hears it.
 */
class Sounds(private val app: IrChad) {
    val board = MutableStateFlow<Board?>(null)
    private val decoded = HashMap<Pair<String, String>, Pcm>()
    /** The clip to play once its bytes arrive. */
    private var wanted: Pair<String, String>? = null
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    val mixer = Mixer()

    fun open(network: String) {
        board.value = Board(network, null, null)
        app.client.sounds(network)
    }

    internal fun got(network: String, clips: List<Clip>, error: String?) {
        scope.launch { if (board.value?.network == network) board.value = Board(network, clips, error) }
    }

    fun play(network: String, file: String) {
        val s = app.calls.state.value
        if (s == null || !s.connected) return
        if (s.muted || s.deafened) {
            app.notices.tryEmit("Unmute to play sounds.")
            return
        }
        val key = network to file
        decoded[key]?.let { return start(it) }
        wanted = key
        app.client.sound(network, file)
    }

    internal fun gotBytes(network: String, file: String, bytes: ByteArray?) {
        scope.launch {
            val key = network to file
            val pcm = bytes?.let { b -> kotlinx.coroutines.withContext(Dispatchers.Default) { decode(b) } }
            if (pcm == null) {
                app.notices.tryEmit("That sound didn't play: it's damaged.")
                return@launch
            }
            decoded[key] = pcm
            if (wanted == key) {
                wanted = null
                play(network, file)
            }
        }
    }

    private fun start(pcm: Pcm) {
        mixer.clip = Playing(pcm)
        local(pcm)
    }

    private var player: AudioTrack? = null

    /** The player hears it as a call sound, so the echo canceller keeps it off the mic. */
    private fun local(pcm: Pcm) {
        player?.release()
        val bytes = pcm.samples.size * 2
        val t = AudioTrack.Builder()
            .setAudioAttributes(
                AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC).build(),
            )
            .setAudioFormat(
                AudioFormat.Builder().setEncoding(AudioFormat.ENCODING_PCM_16BIT).setSampleRate(pcm.rate)
                    .setChannelMask(AudioFormat.CHANNEL_OUT_MONO).build(),
            )
            .setTransferMode(AudioTrack.MODE_STATIC)
            .setBufferSizeInBytes(maxOf(bytes, 2))
            .build()
        t.write(pcm.samples, 0, pcm.samples.size)
        t.play()
        player = t
    }

    /** Out of a call: stop and forget what's playing. */
    fun stop() {
        mixer.clip = null
        player?.release()
        player = null
        board.value = null
    }
}

/**
 * A clip part-way through, resampled for the mic: ahead of time to 48 kHz
 * (what WebRTC records at), again on the audio thread only if that's wrong.
 */
class Playing(val pcm: Pcm) {
    var rate = 48_000
    var samples = resample(pcm, rate)
    var pos = 0
}

/** Adds the playing clip to what the mic captured (D48). Called on the audio thread. */
class Mixer : MixerAudioBufferCallback() {
    @Volatile var clip: Playing? = null

    override fun onBufferRequest(
        originalBuffer: ByteBuffer, audioFormat: Int, channelCount: Int, sampleRate: Int,
        bytesRead: Int, captureTimeNs: Long,
    ): BufferResponse? {
        val c = clip ?: return null
        if (c.rate != sampleRate) {
            c.pos = (c.pos.toLong() * sampleRate / c.rate).toInt()
            c.samples = resample(c.pcm, sampleRate)
            c.rate = sampleRate
        }
        val float = audioFormat == AudioFormat.ENCODING_PCM_FLOAT
        if (!float && audioFormat != AudioFormat.ENCODING_PCM_16BIT) return null
        val frames = bytesRead / ((if (float) 4 else 2) * maxOf(1, channelCount))
        val out = ByteBuffer.allocateDirect(bytesRead).order(ByteOrder.nativeOrder())
        for (i in 0 until frames) {
            val s = c.samples.getOrElse(c.pos + i) { 0 }
            repeat(channelCount) { if (float) out.putFloat(s / 32768f) else out.putShort(s) }
        }
        c.pos += frames
        if (c.pos >= c.samples.size) clip = null
        out.rewind()
        return BufferResponse(out)
    }
}

/** Linear, which is plenty for a soundboard. */
internal fun resample(p: Pcm, rate: Int): ShortArray {
    if (p.rate == rate || p.samples.isEmpty()) return p.samples
    val n = (p.samples.size.toLong() * rate / p.rate).toInt()
    return ShortArray(n) { i ->
        val x = i.toDouble() * p.rate / rate
        val a = x.toInt().coerceAtMost(p.samples.size - 1)
        val b = (a + 1).coerceAtMost(p.samples.size - 1)
        val f = x - a
        (p.samples[a] * (1 - f) + p.samples[b] * f).toInt().toShort()
    }
}

/**
 * The first [MAX_SECONDS] of a clip as mono PCM, or `null`. The bytes are the
 * server's (D14): any failure only loses the clip, and output is bounded.
 */
internal fun decode(bytes: ByteArray): Pcm? {
    val ex = MediaExtractor()
    var codec: MediaCodec? = null
    return try {
        ex.setDataSource(object : MediaDataSource() {
            override fun readAt(position: Long, buffer: ByteArray, offset: Int, size: Int): Int {
                if (position >= bytes.size) return -1
                val n = minOf(size.toLong(), bytes.size - position).toInt()
                System.arraycopy(bytes, position.toInt(), buffer, offset, n)
                return n
            }

            override fun getSize() = bytes.size.toLong()
            override fun close() {}
        })
        val track = (0 until ex.trackCount).firstOrNull {
            ex.getTrackFormat(it).getString(MediaFormat.KEY_MIME)?.startsWith("audio/") == true
        } ?: return null
        ex.selectTrack(track)
        val fmt = ex.getTrackFormat(track)
        var rate = fmt.getInteger(MediaFormat.KEY_SAMPLE_RATE)
        var channels = fmt.getInteger(MediaFormat.KEY_CHANNEL_COUNT)
        if (rate !in 1..MAX_RATE || channels !in 1..8) return null
        val c = MediaCodec.createDecoderByType(fmt.getString(MediaFormat.KEY_MIME)!!)
        codec = c
        c.configure(fmt, null, null, 0)
        c.start()
        var float = false
        val out = ShortArray(MAX_SECONDS * MAX_RATE)
        var n = 0
        val info = MediaCodec.BufferInfo()
        var inputDone = false
        var limit = MAX_SECONDS * rate
        // Bounded however the stream behaves.
        var spins = 0
        while (n < limit && spins++ < 200_000) {
            if (!inputDone) {
                val i = c.dequeueInputBuffer(10_000)
                if (i >= 0) {
                    val size = ex.readSampleData(c.getInputBuffer(i)!!, 0)
                    if (size < 0) {
                        c.queueInputBuffer(i, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM)
                        inputDone = true
                    } else {
                        c.queueInputBuffer(i, 0, size, ex.sampleTime, 0)
                        ex.advance()
                    }
                }
            }
            val o = c.dequeueOutputBuffer(info, 10_000)
            if (o == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                val f = c.outputFormat
                rate = f.getInteger(MediaFormat.KEY_SAMPLE_RATE)
                channels = f.getInteger(MediaFormat.KEY_CHANNEL_COUNT)
                float = f.containsKey(MediaFormat.KEY_PCM_ENCODING) &&
                    f.getInteger(MediaFormat.KEY_PCM_ENCODING) == AudioFormat.ENCODING_PCM_FLOAT
                if (rate !in 1..MAX_RATE || channels !in 1..8) return null
                limit = MAX_SECONDS * rate
            }
            if (o < 0) continue
            val buf = c.getOutputBuffer(o)!!.order(ByteOrder.nativeOrder())
            buf.position(info.offset)
            buf.limit(info.offset + info.size)
            // Mono: the average of the channels.
            while (n < limit && buf.remaining() >= channels * (if (float) 4 else 2)) {
                var sum = 0f
                repeat(channels) { sum += if (float) buf.getFloat() * 32767f else buf.getShort().toFloat() }
                out[n++] = (sum / channels).coerceIn(-32768f, 32767f).toInt().toShort()
            }
            c.releaseOutputBuffer(o, false)
            if (info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) break
        }
        if (n == 0) null else Pcm(out.copyOf(n), rate)
    } catch (e: Exception) {
        null
    } finally {
        runCatching { codec?.stop() }
        runCatching { codec?.release() }
        ex.release()
    }
}
