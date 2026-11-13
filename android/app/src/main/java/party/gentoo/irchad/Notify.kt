package party.gentoo.irchad

import android.Manifest
import android.app.Activity
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import androidx.activity.compose.LocalActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.selection.selectable
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.core.app.NotificationCompat
import androidx.core.app.Person
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import org.unifiedpush.android.connector.FailedReason
import org.unifiedpush.android.connector.UnifiedPush
import org.unifiedpush.android.connector.data.PushEndpoint
import org.unifiedpush.android.connector.data.PushMessage
import party.gentoo.irchad.core.Push
import party.gentoo.irchad.core.PushSubscription
import party.gentoo.irchad.core.readPush
import java.util.concurrent.ConcurrentHashMap

/** How DMs and mentions reach the phone while IrChad is in the background (D39). */
enum class Background { PUSH, SOCKET, OFF }

/** Message notifications: from ergo's pushes, or from the live connection. */
class Notifier(private val app: IrChad) {
    private val prefs = app.getSharedPreferences("settings", Context.MODE_PRIVATE)
    private val nm = app.getSystemService(NotificationManager::class.java)
    private val main = Handler(Looper.getMainLooper())
    /** Networks that can push, with their VAPID keys, since they connected. */
    private val vapids = ConcurrentHashMap<String, String>()

    /** An activity is on screen: no notifications then. */
    @Volatile var visible = false
        private set
    private val pause = Runnable { if (!visible && mode != Background.SOCKET) app.client.background(true) }
    /** Bumped when an activity resumes: a distributor may have been installed. */
    var resumes by mutableIntStateOf(0)

    /** `null` until the user picks, or a distributor being there picks for them. */
    var mode by mutableStateOf(
        runCatching { Background.valueOf(prefs.getString("background", null)!!) }.getOrNull(),
    )
        private set

    init {
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL, "Messages", NotificationManager.IMPORTANCE_HIGH),
        )
    }

    /** The core just started; maybe for a push, with nothing on screen. */
    fun started() {
        if (!visible) main.postDelayed(pause, PAUSE_MS)
    }

    /**
     * The app came to the front, or left it. Leaving drops the connections
     * after a minute (D39), unless the user picked "Stay connected".
     */
    fun foreground(on: Boolean) {
        visible = on
        main.removeCallbacks(pause)
        if (!on) {
            main.postDelayed(pause, PAUSE_MS)
            return
        }
        app.client.background(false)
        keepConnected()
        // The distributor was uninstalled: pushes stopped, so ask again.
        if (mode == Background.PUSH && !hasDistributor()) {
            mode = null
            prefs.edit().remove("background").apply()
            app.notices.tryEmit("Your push app is gone, so notifications stopped.")
        }
    }

    /** Our own package counts: the embedded FCM distributor, when Play services is there (D84). */
    fun hasDistributor() = UnifiedPush.getDistributors(app).isNotEmpty()

    /** Push would go through Google, as no UnifiedPush app is installed. */
    fun onlyEmbedded() = UnifiedPush.getDistributors(app) == listOf(app.packageName)

    /** Picking push may ask which distributor, which needs [activity]. */
    fun choose(m: Background, activity: Activity) {
        val was = mode
        mode = m
        prefs.edit().putString("background", m.name).apply()
        if (was == Background.PUSH && m != Background.PUSH) {
            // Unregisters every network; ergo drops a subscription that 404s.
            UnifiedPush.removeDistributor(app)
            vapids.keys.forEach { app.client.push(it, null) }
        }
        if (m == Background.PUSH && was != Background.PUSH) {
            UnifiedPush.tryUseCurrentOrDefaultDistributor(activity) { ok ->
                if (ok) vapids.forEach { (n, v) -> register(n, v) } else app.notices.tryEmit("No push app answered.")
            }
        }
        if (m == Background.SOCKET) keepConnected() else app.stopService(Intent(app, KeepService::class.java))
        if (!visible && m != Background.SOCKET) main.postDelayed(pause, PAUSE_MS)
    }

    /** Starts [KeepService] if that's the mode; only works while the app is in front. */
    fun keepConnected() {
        if (mode == Background.SOCKET && BuildConfig.STAY_CONNECTED) {
            runCatching { ContextCompat.startForegroundService(app, Intent(app, KeepService::class.java)) }
        }
    }

    fun pushAvailable(network: String, vapid: String) {
        vapids[network] = vapid
        if (mode == Background.PUSH) main.post { register(network, vapid) }
    }

    private fun register(network: String, vapid: String) {
        if (UnifiedPush.getAckDistributor(app) == null && UnifiedPush.getSavedDistributor(app) == null) return
        try {
            UnifiedPush.register(app, network, "IrChad", vapid)
        } catch (e: Exception) {
            // VapidNotValidException: the server's key isn't one (D14).
            Log.w(TAG, "push register for $network: $e")
        }
    }

    /** A decrypted push for [network]: one IRC line from ergo. */
    fun pushed(network: String, line: String) {
        if (app.client.pushBlocked(network, line)) return
        when (val p = readPush(line)) {
            is Push.Message -> if (mode == Background.PUSH && app.settings.notify != NotifyLevel.NEVER) {
                show(network, p.target, p.from, p.text, p.channel)
            }
            is Push.Read -> dismiss(network, p.target)
            null -> {}
        }
    }

    /** A DM or mention (any message, with "Every message") on the live connection. */
    fun live(network: String, target: String, from: String, text: String, channel: Boolean) {
        if (mode == Background.SOCKET && app.settings.notify != NotifyLevel.NEVER) show(network, target, from, text, channel)
    }

    fun dismiss(network: String, target: String) = nm.cancel(tag(network, target), ID)

    private fun show(network: String, target: String, from: String, text: String, channel: Boolean) {
        if (visible || !nm.areNotificationsEnabled()) return
        val tag = tag(network, target)
        // Add to what's already showing for this conversation.
        val old = nm.activeNotifications.firstOrNull { it.tag == tag && it.id == ID }?.notification
        val style = old?.let { NotificationCompat.MessagingStyle.extractMessagingStyleFromNotification(it) }
            ?: NotificationCompat.MessagingStyle(Person.Builder().setName("You").build())
        if (channel) style.setConversationTitle(target).setGroupConversation(true)
        style.addMessage(text, System.currentTimeMillis(), Person.Builder().setName(from).build())
        // Keep it short: a busy channel shouldn't grow one without end.
        while (style.messages.size > 8) style.messages.removeAt(0)
        val open = PendingIntent.getActivity(
            app, tag.hashCode(),
            Intent(app, MainActivity::class.java)
                .putExtra(EXTRA_NETWORK, network)
                .putExtra(EXTRA_TARGET, target),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val n = NotificationCompat.Builder(app, CHANNEL)
            .setSmallIcon(R.drawable.ic_chat)
            .setStyle(style)
            .setContentIntent(open)
            .setAutoCancel(true)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .build()
        nm.notify(tag, ID, n)
    }

    private fun tag(network: String, target: String) = "$network\n${target.lowercase()}"

    companion object {
        const val EXTRA_NETWORK = "party.gentoo.irchad.NETWORK"
        const val EXTRA_TARGET = "party.gentoo.irchad.TARGET"
        private const val CHANNEL = "messages"
        private const val ID = 3
        private const val TAG = "irchad"
        /** How long the app may be away before its connections drop. */
        private const val PAUSE_MS = 60_000L
    }
}

/** UnifiedPush hands us endpoints and decrypted pushes (RFC 8291) here. */
class PushService : org.unifiedpush.android.connector.PushService() {
    private val app get() = application as IrChad

    override fun onNewEndpoint(endpoint: PushEndpoint, instance: String) {
        // Without keys the push would arrive in the clear; ergo needs them anyway.
        val keys = endpoint.pubKeySet ?: return
        app.client.push(instance, PushSubscription(endpoint.url, keys.pubKey, keys.auth))
    }

    override fun onMessage(message: PushMessage, instance: String) {
        if (message.decrypted && message.content.size <= 8192) {
            app.notifier.pushed(instance, message.content.decodeToString())
        }
    }

    override fun onRegistrationFailed(reason: FailedReason, instance: String) {
        Log.w("irchad", "push registration for $instance failed: $reason")
        app.notices.tryEmit("Push notifications didn't turn on ($reason).")
    }

    override fun onUnregistered(instance: String) = app.client.push(instance, null)
}

/** Keeps the process, and so the IRC connections, alive without a distributor (D39). */
class KeepService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val app = application as IrChad
        app.client // The core runs for as long as the process does.
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL, "Staying connected", NotificationManager.IMPORTANCE_MIN),
        )
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE,
        )
        val n = NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_chat)
            .setContentTitle("Connected for notifications")
            .setContentText("Install ntfy to get them without this.")
            .setContentIntent(open)
            .setOngoing(true)
            .setSilent(true)
            .build()
        val type = if (Build.VERSION.SDK_INT >= 34) ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE else 0
        try {
            ServiceCompat.startForeground(this, ID, n, type)
        } catch (e: Exception) {
            // Started from the background, which Android refuses.
            Log.w("irchad", "keep-connected service: $e")
            stopSelf()
        }
        return START_STICKY
    }

    private companion object {
        const val CHANNEL = "keep"
        const val ID = 2
    }
}

/** "Stay connected" survives a reboot (D39); push needs nothing, its app restarts itself. */
class BootReceiver : android.content.BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED) return
        val app = context.applicationContext as IrChad
        if (app.notifier.mode == Background.SOCKET) {
            // Allowed from BOOT_COMPLETED for a specialUse service.
            runCatching { ContextCompat.startForegroundService(app, Intent(app, KeepService::class.java)) }
        }
    }
}

/** The three ways (D39), for onboarding and the settings dialog. */
@Composable
fun BackgroundChoices(app: IrChad) {
    val n = app.notifier
    val ctx = LocalContext.current
    // In a dialog LocalContext is a ContextThemeWrapper, not the Activity.
    val activity = LocalActivity.current!!
    val has = remember(n.resumes) { n.hasDistributor() }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {}
    fun pick(m: Background) {
        n.choose(m, activity)
        if (m != Background.OFF && Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(ctx, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            ask.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }
    Column {
        for ((m, label, sub) in listOf(
            Triple(
                Background.PUSH, "Push",
                when {
                    !has -> "Needs a UnifiedPush app such as ntfy"
                    n.onlyEmbedded() -> "Through Google Play services"
                    else -> "Through your UnifiedPush app, such as ntfy"
                },
            ),
            Triple(Background.SOCKET, "Stay connected", "Shows a permanent notification, and uses some battery"),
            Triple(Background.OFF, "Off", "Only while IrChad is open"),
        ).filter { it.first != Background.SOCKET || BuildConfig.STAY_CONNECTED }) {
            val enabled = m != Background.PUSH || has
            Row(
                Modifier.fillMaxWidth()
                    .selectable(n.mode == m, enabled = enabled, role = Role.RadioButton) { pick(m) }
                    .padding(vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                RadioButton(selected = n.mode == m, onClick = null, enabled = enabled)
                Column(Modifier.padding(start = 12.dp)) {
                    Text(label)
                    Text(sub, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
        if (!has) {
            TextButton(onClick = {
                val ntfy = Intent(Intent.ACTION_VIEW, Uri.parse("market://details?id=io.heckel.ntfy"))
                try {
                    ctx.startActivity(ntfy)
                } catch (e: ActivityNotFoundException) {
                    ctx.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse("https://f-droid.org/packages/io.heckel.ntfy/")))
                }
            }) { Text("Get ntfy") }
        }
    }
}

/** Asked once: push when a distributor is installed, otherwise the user picks (D39). */
@Composable
fun BackgroundOnboarding(app: IrChad) {
    val n = app.notifier
    if (n.mode != null) return
    val activity = LocalActivity.current!!
    val has = remember(n.resumes) { n.hasDistributor() }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {}
    var later by remember { mutableStateOf(false) }
    LaunchedEffect(has) {
        if (has) {
            n.choose(Background.PUSH, activity)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) ask.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }
    if (has || later) return
    AlertDialog(
        onDismissRequest = { later = true },
        title = { Text("Notifications while IrChad is closed") },
        text = {
            Column {
                Text("Install ntfy to get DMs and mentions without a permanent notification, then come back.")
                BackgroundChoices(app)
            }
        },
        confirmButton = { TextButton(onClick = { later = true }) { Text("Later") } },
    )
}
