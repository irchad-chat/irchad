package party.gentoo.irchad

import android.app.PictureInPictureParams
import android.content.Intent
import android.content.res.Configuration
import android.net.Uri
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import android.util.Rational
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.platform.LocalContext

class MainActivity : ComponentActivity() {
    /** An `irchad://` invite that opened the app, until the join screen takes it. */
    private val invite = mutableStateOf<String?>(null)
    private val pip = mutableStateOf(false)
    /** A notification that was tapped: the conversation to show. */
    private val open = mutableStateOf<Focus?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val app = application as IrChad
        if (savedInstanceState == null) {
            invite.value = intent.inviteOrNull()
            open.value = intent.focusOrNull()
            intent.sharedOrNull()?.let { app.shared.value = it }
        }
        // Picture-in-picture follows the share's shape, and enters by itself on API 31+.
        lifecycleScope.launch {
            app.calls.state.map { it?.watching != null }.distinctUntilChanged()
                .combine(app.calls.frame) { _, _ -> }
                .collect { runCatching { setPictureInPictureParams(pipParams()) } }
        }
        setContent {
            IrChadTheme {
                Root(app, invite.value, open.value, pip.value) { invite.value = null }
            }
        }
    }

    override fun onStart() {
        super.onStart()
        (application as IrChad).notifier.foreground(true)
    }

    override fun onResume() {
        super.onResume()
        (application as IrChad).notifier.resumes++
    }

    override fun onStop() {
        super.onStop()
        (application as IrChad).notifier.foreground(false)
    }

    private fun watching() = (application as IrChad).calls.state.value?.watching != null

    private fun pipParams(): PictureInPictureParams {
        val (w, h) = (application as IrChad).calls.frame.value ?: (16 to 9)
        // Android refuses anything wider than 2.39:1 either way.
        val f = w.toFloat() / h.coerceAtLeast(1)
        val ratio = when {
            f > 2.39f -> Rational(239, 100)
            f < 1 / 2.39f -> Rational(100, 239)
            else -> Rational(w, h)
        }
        val b = PictureInPictureParams.Builder().setAspectRatio(ratio)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) b.setAutoEnterEnabled(watching())
        return b.build()
    }

    /** Leaving the app while watching a screen share keeps it in a small window (by itself on API 31+). */
    override fun onUserLeaveHint() {
        super.onUserLeaveHint()
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S && watching()) {
            runCatching { enterPictureInPictureMode(pipParams()) }
        }
    }

    override fun onPictureInPictureModeChanged(isInPictureInPictureMode: Boolean, newConfig: Configuration) {
        super.onPictureInPictureModeChanged(isInPictureInPictureMode, newConfig)
        pip.value = isInPictureInPictureMode
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        intent.inviteOrNull()?.let { invite.value = it }
        intent.focusOrNull()?.let { open.value = it }
        intent.sharedOrNull()?.let { (application as IrChad).shared.value = it }
    }
}

private fun Intent.focusOrNull(): Focus? {
    val network = getStringExtra(Notifier.EXTRA_NETWORK) ?: return null
    return Focus(network, getStringExtra(Notifier.EXTRA_TARGET))
}

/** Text or files shared from another app. */
@Suppress("DEPRECATION")
private fun Intent.sharedOrNull(): Shared? {
    val uris = when (action) {
        Intent.ACTION_SEND -> listOfNotNull(getParcelableExtra<Uri>(Intent.EXTRA_STREAM))
        Intent.ACTION_SEND_MULTIPLE -> getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM).orEmpty()
        else -> return null
    }
    val text = getStringExtra(Intent.EXTRA_TEXT).orEmpty()
    return Shared(text, uris).takeIf { text.isNotBlank() || uris.isNotEmpty() }
}

private fun Intent.inviteOrNull(): String? =
    data?.takeIf { action == Intent.ACTION_VIEW && it.scheme == "irchad" }?.toString()

/** Material You: the wallpaper's palette (D41). */
@Composable
fun IrChadTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val ctx = LocalContext.current
    val colors = when {
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
            if (dark) dynamicDarkColorScheme(ctx) else dynamicLightColorScheme(ctx)
        dark -> darkColorScheme()
        else -> lightColorScheme()
    }
    MaterialTheme(colorScheme = colors, content = content)
}
