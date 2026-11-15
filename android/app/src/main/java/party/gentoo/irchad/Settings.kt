package party.gentoo.irchad

import android.content.Context
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import kotlin.math.roundToInt
import kotlin.reflect.KProperty

enum class Theme { SYSTEM, LIGHT, DARK }

/** Which messages notify, as on desktop. Pushes only ever bring DMs and mentions. */
enum class NotifyLevel { ALL, MENTIONS, NEVER }

/**
 * The preferences that aren't anyone else's: looks, notifications, away and
 * voice. Same defaults as desktop's; what only makes sense there (devices,
 * text size, panes, updates) has no counterpart here.
 */
class Settings(app: Context) {
    private val prefs = app.getSharedPreferences("settings", Context.MODE_PRIVATE)

    /** Observable for Compose, written through to [prefs]. */
    private class Pref<T>(get: () -> T, private val put: (T) -> Unit) {
        private var v by mutableStateOf(get())
        operator fun getValue(o: Any?, p: KProperty<*>) = v
        operator fun setValue(o: Any?, p: KProperty<*>, x: T) {
            v = x
            put(x)
        }
    }

    private fun bool(key: String, default: Boolean) =
        Pref({ prefs.getBoolean(key, default) }) { prefs.edit().putBoolean(key, it).apply() }

    private fun float(key: String, default: Float, range: ClosedFloatingPointRange<Float>) =
        Pref({ prefs.getFloat(key, default).coerceIn(range) }) { prefs.edit().putFloat(key, it).apply() }

    private fun int(key: String, default: Int, range: IntRange) =
        Pref({ prefs.getInt(key, default).coerceIn(range) }) { prefs.edit().putInt(key, it).apply() }

    var theme by Pref(
        { runCatching { Theme.valueOf(prefs.getString("theme", null)!!) }.getOrDefault(Theme.SYSTEM) },
    ) { prefs.edit().putString("theme", it.name).apply() }
    var notify by Pref(
        { runCatching { NotifyLevel.valueOf(prefs.getString("notify", null)!!) }.getOrDefault(NotifyLevel.MENTIONS) },
    ) { prefs.edit().putString("notify", it.name).apply() }
    /** Away by choice, on every network. */
    var away by bool("away", false)
    /** A network has been signed in to: the first-launch cards are over. */
    var welcomed by bool("welcomed", false)
    /** Away by itself after this many minutes without a touch; 0 never. */
    var awayAfter by int("away_after", 10, 0..240)

    var echoCancellation by bool("echo_cancellation", true)
    var noiseSuppression by bool("noise_suppression", true)
    var rnnoise by bool("rnnoise", false)
    var autoGain by bool("auto_gain", false)
    var bitrateKbps by int("bitrate_kbps", 64, 16..128)
    /** What others hear, 0–200%. */
    var inputGain by float("input_gain", 1f, 0f..2f)
    /** Everyone's soundboard clips (ours too), for us, 0–200%. */
    var soundboardVolume by float("soundboard_volume", 1f, 0f..2f)
    /** Push to talk: the mic sends while the button in the voice panel is held. */
    var pushToTalk by bool("push_to_talk", false)
    /** Voice activity opens the mic above this level (dBFS). */
    var vadThresholdDb by float("vad_threshold_db", -50f, -80f..0f)
    var pttReleaseMs by int("ptt_release_ms", 200, 0..1000)

    /** Someone's voice, or their screen share's sound, for us, 0–200%; by LiveKit identity. */
    fun volume(identity: String, screen: Boolean): Float =
        prefs.getFloat(volumeKey(identity, screen), 1f).coerceIn(0f, 2f)

    fun setVolume(identity: String, screen: Boolean, v: Float) {
        prefs.edit().putFloat(volumeKey(identity, screen), v.coerceIn(0f, 2f)).apply()
    }

    private fun volumeKey(identity: String, screen: Boolean) = (if (screen) "screen_volume:" else "volume:") + identity
}

@Composable
fun SettingsDialog(app: IrChad, onDismiss: () -> Unit) {
    val s = app.settings
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Settings") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                Heading("Theme", first = true)
                for ((t, label) in listOf(Theme.SYSTEM to "Match system", Theme.LIGHT to "Light", Theme.DARK to "Dark")) {
                    Choice(label, s.theme == t) { s.theme = t }
                }

                Heading("Notifications while IrChad is closed")
                BackgroundChoices(app)

                Heading("Load images and link previews")
                for ((a, label) in listOf(
                    Media.AutoLoad.ALWAYS to "Always",
                    Media.AutoLoad.UNMETERED to "On Wi‑Fi only",
                    Media.AutoLoad.OFF to "Only when I tap",
                )) {
                    Choice(label, app.media.autoLoad == a) { app.media.autoLoad = a }
                }
                Toggle("Also from guests (no account)", app.media.guests) { app.media.guests = it }

                Heading("Notify me about")
                for ((n, label) in listOf(
                    NotifyLevel.ALL to "Every message",
                    NotifyLevel.MENTIONS to "Mentions and direct messages",
                    NotifyLevel.NEVER to "Nothing",
                )) {
                    Choice(label, s.notify == n) {
                        s.notify = n
                        app.client.notifyAll(n == NotifyLevel.ALL)
                    }
                }
                Hint("Pushes only bring direct messages and mentions.")

                Heading("Away after")
                for ((m, label) in listOf(0 to "Never", 5 to "5 minutes", 10 to "10 minutes", 30 to "30 minutes", 60 to "An hour")) {
                    Choice(label, s.awayAfter == m) { s.awayAfter = m }
                }
                Hint("Without touching IrChad, you show as away.")

                Heading("Voice")
                Percent("Input volume", s.inputGain) { s.inputGain = it }
                Hint("How loud others hear you.")
                Percent("Soundboard volume", s.soundboardVolume) { s.soundboardVolume = it }
                Hint("How loud sounds played in voice are for you.")

                Text("Input", Modifier.padding(top = 8.dp))
                Choice("Voice activity", !s.pushToTalk) { s.pushToTalk = false }
                Choice("Push to talk", s.pushToTalk) { s.pushToTalk = true }
                if (s.pushToTalk) {
                    Text("Release delay: ${s.pttReleaseMs} ms", Modifier.padding(top = 8.dp))
                    Slider(
                        value = s.pttReleaseMs.toFloat(),
                        onValueChange = { s.pttReleaseMs = it.roundToInt() },
                        valueRange = 0f..1000f,
                    )
                    Hint("Hold the button in the voice panel to talk.")
                } else {
                    Text("Sensitivity: ${s.vadThresholdDb.roundToInt()} dB", Modifier.padding(top = 8.dp))
                    Meter(app, s.vadThresholdDb)
                    Slider(
                        value = s.vadThresholdDb,
                        onValueChange = { s.vadThresholdDb = it.roundToInt().toFloat() },
                        valueRange = -80f..0f,
                    )
                }

                Toggle("Echo cancellation", s.echoCancellation) { s.echoCancellation = it }
                Toggle("Noise suppression", s.noiseSuppression) { s.noiseSuppression = it }
                Toggle("Extra noise suppression (RNNoise)", s.rnnoise) { s.rnnoise = it }
                Toggle("Automatic gain control", s.autoGain) { s.autoGain = it }
                Text("Quality: ${s.bitrateKbps} kbps", Modifier.padding(top = 8.dp))
                Slider(
                    value = s.bitrateKbps.toFloat(),
                    onValueChange = { s.bitrateKbps = it.roundToInt() },
                    valueRange = 16f..128f,
                )
                Hint("Echo cancellation, noise suppression, gain control and quality take effect the next time you join.")

                val call by app.calls.state.collectAsState()
                val mic = app.calls.mic
                Toggle("Hear yourself", mic.testing, enabled = call?.connected == true) { mic.testing = it }
                Hint(
                    if (call?.connected == true) "Plays your microphone back, processed the way others hear it."
                    else "Join voice to test your microphone.",
                )

                Heading("Blocked users")
                BlockedUsers(app)

                Text(
                    "IrChad ${BuildConfig.VERSION_NAME}",
                    Modifier.padding(top = 16.dp),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                var licenses by remember { mutableStateOf(false) }
                TextButton(onClick = { licenses = true }) { Text("Open-source licenses") }
                if (licenses) LicensesDialog { licenses = false }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Done") } },
    )
}

/** Everyone blocked, on every network, each with an Unblock button. */
@Composable
private fun BlockedUsers(app: IrChad) {
    val tick by app.version.collectAsState()
    val nets = remember(tick) { app.client.networks().map { it to app.client.blocks(it.network) } }
    if (nets.all { it.second.isEmpty() }) Hint("Nobody blocked.")
    for ((net, blocks) in nets) {
        if (blocks.isNotEmpty() && nets.size > 1) Text(net.name, Modifier.padding(top = 8.dp))
        for (b in blocks) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(b.nick)
                    Hint(b.account?.let { "account $it" } ?: b.mask)
                }
                TextButton(onClick = { app.client.unblock(net.network, b.mask) }) { Text("Unblock") }
            }
        }
    }
    Hint("You won't see anything they send. Only on this device; they aren't told.")
}

@Composable
internal fun Hint(text: String) = Text(
    text,
    style = MaterialTheme.typography.bodySmall,
    color = MaterialTheme.colorScheme.onSurfaceVariant,
)

/** A 0–200% volume, like desktop's. */
@Composable
fun Percent(label: String, v: Float, onChange: (Float) -> Unit) {
    Text("$label: ${(v * 100).roundToInt()}%", Modifier.padding(top = 8.dp))
    Slider(value = v, onValueChange = { onChange((it * 20).roundToInt() / 20f) }, valueRange = 0f..2f)
}

/** The mic's level while in a call, with the voice-activity threshold marked. */
@Composable
private fun Meter(app: IrChad, threshold: Float) {
    var level by remember { mutableStateOf(SILENT_DB) }
    var open by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) {
        while (true) {
            level = app.calls.mic.level
            open = app.calls.mic.transmitting
            delay(50)
        }
    }
    val track = MaterialTheme.colorScheme.surfaceVariant
    val fill = if (open) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline
    val mark = MaterialTheme.colorScheme.onSurface
    Canvas(Modifier.fillMaxWidth().height(8.dp).padding(top = 2.dp)) {
        fun x(db: Float) = size.width * ((db + 80f) / 80f).coerceIn(0f, 1f)
        drawRect(track)
        drawRect(fill, size = Size(x(level), size.height))
        drawRect(mark, topLeft = Offset(x(threshold) - 1f, 0f), size = Size(2f, size.height))
    }
}

@Composable
private fun Heading(text: String, first: Boolean = false) =
    Text(text, Modifier.padding(top = if (first) 0.dp else 16.dp), style = MaterialTheme.typography.titleSmall)

@Composable
internal fun Choice(label: String, selected: Boolean, onClick: () -> Unit) = Row(
    Modifier.fillMaxWidth().selectable(selected, role = Role.RadioButton, onClick = onClick).padding(vertical = 8.dp),
    verticalAlignment = Alignment.CenterVertically,
) {
    RadioButton(selected = selected, onClick = null)
    Text(label, Modifier.padding(start = 12.dp))
}

@Composable
internal fun Toggle(label: String, on: Boolean, enabled: Boolean = true, set: (Boolean) -> Unit) = Row(
    Modifier.fillMaxWidth().toggleable(value = on, enabled = enabled, role = Role.Switch, onValueChange = set).padding(vertical = 8.dp),
    verticalAlignment = Alignment.CenterVertically,
) {
    Text(label, Modifier.weight(1f))
    Switch(checked = on, onCheckedChange = null, enabled = enabled)
}
