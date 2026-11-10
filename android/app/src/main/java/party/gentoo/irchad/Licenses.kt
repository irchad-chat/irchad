package party.gentoo.irchad

import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.mikepenz.aboutlibraries.ui.compose.m3.LibrariesContainer
import com.mikepenz.aboutlibraries.ui.compose.android.produceLibraries

/**
 * The licenses of everything in the APK: the Gradle dependencies (AboutLibraries),
 * then the Rust crates' notices that build-rust.sh puts in assets for release builds.
 */
@Composable
fun LicensesDialog(onDismiss: () -> Unit) {
    val context = LocalContext.current
    val libraries by produceLibraries(R.raw.aboutlibraries)
    val rust = remember {
        runCatching { context.assets.open("THIRD_PARTY_LICENSES.txt").bufferedReader().readLines() }
            .getOrElse { listOf("Rust crate notices are only bundled in release builds.") }
    }
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize()) {
            LibrariesContainer(
                libraries,
                Modifier.fillMaxSize(),
                footer = {
                    // One item per line: the file is ~0.5 MB, too much for one Text.
                    items(rust) {
                        Text(
                            it,
                            Modifier.padding(horizontal = 16.dp),
                            style = MaterialTheme.typography.bodySmall,
                            fontFamily = FontFamily.Monospace,
                        )
                    }
                },
            )
        }
    }
}
