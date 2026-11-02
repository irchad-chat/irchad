package party.gentoo.irchad

import android.content.Context
import androidx.compose.foundation.layout.Column
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import kotlin.math.roundToInt
import kotlin.reflect.KProperty

enum class Theme { SYSTEM, LIGHT, DARK }

/**
 * The preferences that aren't anyone else's: looks and voice. Same defaults
 * as desktop's; what only makes sense there (devices, push to talk, text
 * size, panes, updates) has no counterpart here.
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

    var theme by Pref(
        { runCatching { Theme.valueOf(prefs.getString("theme", null)!!) }.getOrDefault(Theme.SYSTEM) },
    ) { prefs.edit().putString("theme", it.name).apply() }
    var echoCancellation by bool("echo_cancellation", true)
    var noiseSuppression by bool("noise_suppression", true)
    var autoGain by bool("auto_gain", false)
    var bitrateKbps by Pref({ prefs.getInt("bitrate_kbps", 64).coerceIn(16, 128) }) {
        prefs.edit().putInt("bitrate_kbps", it).apply()
    }
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

                Heading("Voice")
                Toggle("Echo cancellation", s.echoCancellation) { s.echoCancellation = it }
                Toggle("Noise suppression", s.noiseSuppression) { s.noiseSuppression = it }
                Toggle("Automatic gain control", s.autoGain) { s.autoGain = it }
                Text("Quality: ${s.bitrateKbps} kbps", Modifier.padding(top = 8.dp))
                Slider(
                    value = s.bitrateKbps.toFloat(),
                    onValueChange = { s.bitrateKbps = it.roundToInt() },
                    valueRange = 16f..128f,
                )
                Text(
                    "Voice changes take effect the next time you join.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )

                Text(
                    "IrChad ${BuildConfig.VERSION_NAME}",
                    Modifier.padding(top = 16.dp),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Done") } },
    )
}

@Composable
private fun Heading(text: String, first: Boolean = false) =
    Text(text, Modifier.padding(top = if (first) 0.dp else 16.dp), style = MaterialTheme.typography.titleSmall)

@Composable
private fun Choice(label: String, selected: Boolean, onClick: () -> Unit) = Row(
    Modifier.fillMaxWidth().selectable(selected, role = Role.RadioButton, onClick = onClick).padding(vertical = 8.dp),
    verticalAlignment = Alignment.CenterVertically,
) {
    RadioButton(selected = selected, onClick = null)
    Text(label, Modifier.padding(start = 12.dp))
}

@Composable
private fun Toggle(label: String, on: Boolean, set: (Boolean) -> Unit) = Row(
    Modifier.fillMaxWidth().toggleable(on, role = Role.Switch, onValueChange = set).padding(vertical = 8.dp),
    verticalAlignment = Alignment.CenterVertically,
) {
    Text(label, Modifier.weight(1f))
    Switch(checked = on, onCheckedChange = null)
}
