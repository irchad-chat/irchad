package party.gentoo.irchad

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import party.gentoo.irchad.core.Network

/** Text or files another app shared to IrChad. */
data class Shared(val text: String, val uris: List<Uri>)

private val io = CoroutineScope(SupervisorJob() + Dispatchers.IO)

/**
 * Uploads each file to `target`, whose link the core then posts, or with a
 * `null` target makes it our avatar. Reads at most the network's limit, so a
 * huge file is refused, not loaded.
 */
fun upload(app: IrChad, ctx: Context, net: Network, target: String?, uris: List<Uri>) {
    val limit = net.uploads
    if (limit == null) {
        app.notices.tryEmit("${net.name} doesn't take uploads.")
        return
    }
    val resolver = ctx.applicationContext.contentResolver
    io.launch {
        for (uri in uris) {
            val name = runCatching {
                resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
                    if (it.moveToFirst()) it.getString(0) else null
                }
            }.getOrNull()?.takeIf { it.isNotBlank() } ?: uri.lastPathSegment ?: "file"
            // `null`: bigger than the limit.
            val bytes = runCatching {
                val s = resolver.openInputStream(uri) ?: throw java.io.IOException("no stream")
                s.use { readAtMost(it, minOf(limit, Int.MAX_VALUE.toULong() - 1u).toInt()) }
            }
            val b = bytes.getOrNull()
            when {
                bytes.isFailure -> app.notices.tryEmit("Couldn't read $name.")
                b == null -> app.notices.tryEmit("$name is too big for ${net.name} (${limit / 1_000_000u} MB at most).")
                else -> withContext(Dispatchers.Main) {
                    app.notices.tryEmit("Uploading $name…")
                    if (target == null) app.client.uploadAvatar(net.network, name, b)
                    else app.client.upload(net.network, target, name, b)
                }
            }
        }
    }
}

/** All of `s`, or `null` if it's longer than `max` bytes. */
internal fun readAtMost(s: java.io.InputStream, max: Int): ByteArray? {
    val out = java.io.ByteArrayOutputStream()
    val buf = ByteArray(64 * 1024)
    while (true) {
        val n = s.read(buf)
        if (n < 0) return out.toByteArray()
        if (out.size() + n > max) return null
        out.write(buf, 0, n)
    }
}

/** Where shared text or files go: any channel or DM on any network. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ShareSheet(app: IrChad, shared: Shared, nets: List<Network>, onSent: (Focus) -> Unit, onDismiss: () -> Unit) {
    val ctx = LocalContext.current
    var text by remember(shared) { mutableStateOf(shared.text) }
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.navigationBarsPadding()) {
            Text(
                if (shared.uris.isEmpty()) "Send to…"
                else "Send ${shared.uris.size} file${if (shared.uris.size == 1) "" else "s"} to…",
                style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp),
            )
            OutlinedTextField(
                text, { text = it }, maxLines = 4, label = { Text("Message") },
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
            )
            LazyColumn {
                for (n in nets) {
                    val bufs = app.client.buffers(n.network)
                    item(key = "n:" + n.network) {
                        Text(
                            n.name, style = MaterialTheme.typography.titleSmall,
                            color = MaterialTheme.colorScheme.primary,
                            modifier = Modifier.padding(start = 24.dp, top = 16.dp, bottom = 4.dp),
                        )
                    }
                    items(bufs, key = { n.network + " " + it.target }) { b ->
                        Text(
                            b.target,
                            Modifier.fillMaxWidth().clickable {
                                if (text.isNotBlank()) app.client.send(n.network, b.target, text.trim(), null, emptyList())
                                if (shared.uris.isNotEmpty()) upload(app, ctx, n, b.target, shared.uris)
                                onSent(Focus(n.network, b.target))
                            }.padding(horizontal = 32.dp, vertical = 12.dp),
                        )
                    }
                }
            }
        }
    }
}
