package party.gentoo.irchad

import android.app.PictureInPictureParams
import android.content.Intent
import android.content.res.Configuration
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

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        if (savedInstanceState == null) invite.value = intent.inviteOrNull()
        val app = application as IrChad
        setContent {
            IrChadTheme {
                Root(app, invite.value, pip.value) { invite.value = null }
            }
        }
    }

    /** Leaving the app while watching a screen share keeps it in a small window. */
    override fun onUserLeaveHint() {
        super.onUserLeaveHint()
        if ((application as IrChad).calls.state.value?.watching != null) {
            enterPictureInPictureMode(PictureInPictureParams.Builder().setAspectRatio(Rational(16, 9)).build())
        }
    }

    override fun onPictureInPictureModeChanged(isInPictureInPictureMode: Boolean, newConfig: Configuration) {
        super.onPictureInPictureModeChanged(isInPictureInPictureMode, newConfig)
        pip.value = isInPictureInPictureMode
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        intent.inviteOrNull()?.let { invite.value = it }
    }
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
