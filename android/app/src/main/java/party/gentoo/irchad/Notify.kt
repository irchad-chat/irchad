package party.gentoo.irchad

import android.Manifest
import android.app.Activity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.graphics.Bitmap
import android.graphics.Canvas
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
import androidx.core.content.LocusIdCompat
import androidx.core.content.pm.ShortcutInfoCompat
import androidx.core.content.pm.ShortcutManagerCompat
import androidx.core.graphics.drawable.IconCompat
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

    /** The main activity is on screen: no notifications then. */
    @Volatile var visible = false
        private set
    /** Activities on screen, bubbles included: the connections stay up while any is. */
    @Volatile private var started = 0
    /** Conversations (by [tag]) an expanded bubble shows: no notifications for them. */
    private val reading = ConcurrentHashMap.newKeySet<String>()
    private val pause = Runnable { if (started == 0 && mode != Background.SOCKET) app.client.background(true) }
    /** Bumped when an activity resumes: a distributor may have been installed. */
    var resumes by mutableIntStateOf(0)

    /** `null` until the user picks, or a distributor being there picks for them. */
    var mode by mutableStateOf(
        runCatching { Background.valueOf(prefs.getString("background", null)!!) }.getOrNull(),
    )
        private set

    /** The push app pushes come through; our own package means Google (D84). */
    var distributor by mutableStateOf(UnifiedPush.getSavedDistributor(app))
        private set

    init {
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL, "Messages", NotificationManager.IMPORTANCE_HIGH),
        )
    }

    /** The core just started; maybe for a push, with nothing on screen. */
    fun started() {
        if (started == 0) main.postDelayed(pause, PAUSE_MS)
    }

    /**
     * The app came to the front, or left it. Leaving drops the connections
     * after a minute (D39), unless the user picked "Stay connected".
     * [inApp] is false for a bubble: other conversations still notify then.
     */
    fun foreground(on: Boolean, inApp: Boolean = true) {
        // Stop and start interleave when switching between the app and a bubble.
        started = (started + if (on) 1 else -1).coerceAtLeast(0)
        if (inApp) visible = on
        main.removeCallbacks(pause)
        if (!on) {
            if (started == 0) main.postDelayed(pause, PAUSE_MS)
            return
        }
        app.client.background(false)
        keepConnected()
        distributor = UnifiedPush.getSavedDistributor(app)
        // The distributor was uninstalled: pushes stopped, so ask again.
        if (mode == Background.PUSH && !hasDistributor()) {
            mode = null
            prefs.edit().remove("background").apply()
            app.notices.tryEmit("Your push app is gone, so notifications stopped.")
        }
    }

    /** Our own package counts: the embedded FCM distributor, when Play services is there (D84). */
    fun hasDistributor() = UnifiedPush.getDistributors(app).isNotEmpty()

    /**
     * Picking push without [via] lets the connector pick a push app, which may
     * ask, so it needs [activity]. With [via], pushes go through that one.
     */
    fun choose(m: Background, activity: Activity, via: String? = null) {
        val was = mode
        mode = m
        prefs.edit().putString("background", m.name).apply()
        if (was == Background.PUSH && m != Background.PUSH) {
            // Unregisters every network; ergo drops a subscription that 404s.
            UnifiedPush.removeDistributor(app)
            vapids.keys.forEach { app.client.push(it, null) }
            distributor = null
        }
        if (m == Background.PUSH && via != null && via != distributor) {
            // The new endpoints replace the old ones at ergo (Net::set_push).
            UnifiedPush.saveDistributor(app, via)
            distributor = via
            vapids.forEach { (n, v) -> register(n, v) }
        } else if (m == Background.PUSH && via == null && was != Background.PUSH) {
            UnifiedPush.tryUseCurrentOrDefaultDistributor(activity) { ok ->
                distributor = UnifiedPush.getSavedDistributor(app)
                if (ok) vapids.forEach { (n, v) -> register(n, v) } else app.notices.tryEmit("No push app answered.")
            }
        }
        if (m == Background.SOCKET) keepConnected() else app.stopService(Intent(app, KeepService::class.java))
        if (started == 0 && m != Background.SOCKET) main.postDelayed(pause, PAUSE_MS)
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

    /** A bubble showing [network]/[target] expanded ([on]) or collapsed. */
    fun reading(network: String, target: String, on: Boolean) {
        if (on) reading.add(tag(network, target)) else reading.remove(tag(network, target))
    }

    /** The conversation was read: out of the shade, but a bubble of it stays (cancelling would close it). */
    fun dismiss(network: String, target: String) {
        val tag = tag(network, target)
        val old = nm.activeNotifications.firstOrNull { it.tag == tag && it.id == ID }?.notification ?: return
        val style = NotificationCompat.MessagingStyle.extractMessagingStyleFromNotification(old)
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R || old.flags and Notification.FLAG_BUBBLE == 0 || style == null) {
            nm.cancel(tag, ID)
            return
        }
        nm.notify(tag, ID, build(network, target, style, read = true))
    }

    /** Forgets [network]'s conversations: notifications, bubbles and shortcuts. */
    fun forget(network: String) {
        val prefix = "$network\n"
        nm.activeNotifications.filter { it.tag?.startsWith(prefix) == true }.forEach { nm.cancel(it.tag, it.id) }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return
        val ids = ShortcutManagerCompat.getShortcuts(
            app, ShortcutManagerCompat.FLAG_MATCH_DYNAMIC or ShortcutManagerCompat.FLAG_MATCH_CACHED,
        ).map { it.id }.filter { it.startsWith(prefix) }
        ShortcutManagerCompat.removeLongLivedShortcuts(app, ids)
    }

    private fun show(network: String, target: String, from: String, text: String, channel: Boolean) {
        val tag = tag(network, target)
        if (visible || tag in reading || !nm.areNotificationsEnabled()) return
        // Add to what's already showing for this conversation, unless that was read.
        val old = nm.activeNotifications.firstOrNull { it.tag == tag && it.id == ID }?.notification
            ?.takeUnless { Build.VERSION.SDK_INT >= Build.VERSION_CODES.R && it.bubbleMetadata?.isNotificationSuppressed == true }
        val style = old?.let { NotificationCompat.MessagingStyle.extractMessagingStyleFromNotification(it) }
            ?: NotificationCompat.MessagingStyle(Person.Builder().setName("You").build())
        if (channel) style.setConversationTitle(target).setGroupConversation(true)
        style.addMessage(text, System.currentTimeMillis(), person(network, from))
        // Keep it short: a busy channel shouldn't grow one without end.
        while (style.messages.size > 8) style.messages.removeAt(0)
        nm.notify(tag, ID, build(network, target, style, read = false))
    }

    /**
     * Opens [network]/[target] in an expanded bubble, from the app. When bubbles
     * are off for it, shows the setting instead: only the user can allow them.
     */
    fun bubble(network: String, target: String, channel: Boolean) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return
        val tag = tag(network, target)
        // The conversation's own setting, once it has notified and the user picked it.
        val picked = nm.getNotificationChannel(CHANNEL, tag)?.canBubble() == true
        val allowed = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            nm.bubblePreference == NotificationManager.BUBBLE_PREFERENCE_ALL ||
                nm.bubblePreference == NotificationManager.BUBBLE_PREFERENCE_SELECTED && picked
        } else {
            @Suppress("DEPRECATION")
            nm.areBubblesAllowed() || picked
        }
        if (!allowed) {
            app.notices.tryEmit("Let IrChad's conversations bubble, then try again.")
            app.startActivity(
                Intent(android.provider.Settings.ACTION_APP_NOTIFICATION_BUBBLE_SETTINGS)
                    .putExtra(android.provider.Settings.EXTRA_APP_PACKAGE, app.packageName)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
            return
        }
        val style = NotificationCompat.MessagingStyle(Person.Builder().setName("You").build())
        if (channel) style.setConversationTitle(target).setGroupConversation(true)
        app.client.messages(network, target).lastOrNull()?.let {
            style.addMessage(it.text, it.time, if (it.own) null else person(network, it.nick))
        }
        nm.notify(tag, ID, build(network, target, style, read = true, expand = true))
    }

    private fun person(network: String, nick: String) =
        Person.Builder().setName(nick).setIcon(avatar(network, nick)).build()

    /** [nick]'s picture if it's loaded; otherwise it starts loading, for the next notification. */
    private fun avatar(network: String, nick: String): IconCompat? {
        val url = runCatching { app.client.profile(network, nick).avatar }.getOrNull() ?: return null
        return picture(app.media.image(network, url, app.media.auto))
    }

    /** A loaded image as a square icon, cropped to fill like the app's avatars. */
    private fun picture(img: Media.Img?): IconCompat? = (img as? Media.Img.Ready)?.let {
        runCatching {
            val d = it.forView(ICON_PX)
            val scale = ICON_PX / minOf(d.intrinsicWidth, d.intrinsicHeight).coerceAtLeast(1).toFloat()
            val w = (d.intrinsicWidth.coerceAtLeast(1) * scale).toInt()
            val h = (d.intrinsicHeight.coerceAtLeast(1) * scale).toInt()
            val bmp = Bitmap.createBitmap(ICON_PX, ICON_PX, Bitmap.Config.ARGB_8888)
            d.setBounds((ICON_PX - w) / 2, (ICON_PX - h) / 2, (ICON_PX + w) / 2, (ICON_PX + h) / 2)
            d.draw(Canvas(bmp))
            IconCompat.createWithBitmap(bmp)
        }.getOrNull()
    }

    /** A conversation notification; on API 30+ with a shortcut, so it can bubble. */
    private fun build(
        network: String, target: String, style: NotificationCompat.MessagingStyle, read: Boolean, expand: Boolean = false,
    ): Notification {
        val tag = tag(network, target)
        fun focus(c: Class<*>) = Intent(app, c)
            .setAction(Intent.ACTION_VIEW)
            .setIdentifier(tag)
            .putExtra(EXTRA_NETWORK, network)
            .putExtra(EXTRA_TARGET, target)
        val open = PendingIntent.getActivity(
            app, tag.hashCode(), focus(MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val b = NotificationCompat.Builder(app, CHANNEL)
            .setSmallIcon(R.drawable.ic_chat)
            .setStyle(style)
            .setContentIntent(open)
            .setAutoCancel(true)
            .setSilent(read)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            // The person for a DM, the network's icon for a channel.
            val icon = (
                if (!style.isGroupConversation) avatar(network, target)
                else runCatching { app.client.networks().firstOrNull { it.network == network }?.icon }.getOrNull()
                    ?.let { picture(app.media.icon(network, it)) }
                ) ?: IconCompat.createWithResource(app, R.mipmap.ic_launcher)
            // A long-lived shortcut makes it a conversation, which the user can bubble.
            val shortcut = ShortcutInfoCompat.Builder(app, tag)
                .setShortLabel(target)
                .setIcon(icon)
                .setIntent(focus(MainActivity::class.java))
                .setLongLived(true)
                .setLocusId(LocusIdCompat(tag))
                .apply { if (!style.isGroupConversation) setPerson(Person.Builder().setName(target).setKey(tag).setIcon(icon).build()) }
                .build()
            ShortcutManagerCompat.pushDynamicShortcut(app, shortcut)
            // The system fills in the bubble's intent, so it must be mutable.
            val bubble = PendingIntent.getActivity(
                app, tag.hashCode(), focus(BubbleActivity::class.java),
                PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
            b.setShortcutInfo(shortcut).setBubbleMetadata(
                NotificationCompat.BubbleMetadata.Builder(bubble, icon)
                    .setDesiredHeight(600)
                    .setSuppressNotification(read)
                    .setAutoExpandBubble(expand)
                    .build(),
            )
        }
        return b.build()
    }

    private fun tag(network: String, target: String) = "$network\n${target.lowercase()}"

    companion object {
        const val EXTRA_NETWORK = "party.gentoo.irchad.NETWORK"
        const val EXTRA_TARGET = "party.gentoo.irchad.TARGET"
        private const val CHANNEL = "messages"
        private const val ID = 3
        /** Notification and shortcut pictures, in pixels. */
        private const val ICON_PX = 192
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

private data class Choice(val mode: Background, val label: String, val sub: String, val via: String? = null)

/** The three ways (D39), push once per push app (D84), for onboarding and the settings dialog. */
@Composable
fun BackgroundChoices(app: IrChad) {
    val n = app.notifier
    val ctx = LocalContext.current
    // In a dialog LocalContext is a ContextThemeWrapper, not the Activity.
    val activity = LocalActivity.current!!
    val pushApps = remember(n.resumes) { UnifiedPush.getDistributors(ctx) }
    val has = pushApps.isNotEmpty()
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {}
    fun pick(m: Background, via: String?) {
        n.choose(m, activity, via)
        if (m != Background.OFF && Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(ctx, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            ask.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }
    Column {
        // One push row per push app; our own package is the embedded FCM one (D84).
        val push = pushApps.map { pkg ->
            val name = if (pkg == ctx.packageName) "Google Play services" else runCatching {
                ctx.packageManager.run { getApplicationLabel(getApplicationInfo(pkg, 0)).toString() }
            }.getOrDefault(pkg)
            Choice(Background.PUSH, "Push", "Through $name", pkg)
        }.ifEmpty { listOf(Choice(Background.PUSH, "Push", "Needs a UnifiedPush app such as ntfy")) }
        for ((m, label, sub, via) in push + listOf(
            Choice(Background.SOCKET, "Stay connected", "Shows a permanent notification, and uses some battery"),
            Choice(Background.OFF, "Off", "Only while IrChad is open"),
        ).filter { it.mode != Background.SOCKET || BuildConfig.STAY_CONNECTED }) {
            val enabled = m != Background.PUSH || has
            val selected = n.mode == m && (via == null || via == n.distributor)
            Row(
                Modifier.fillMaxWidth()
                    .selectable(selected, enabled = enabled, role = Role.RadioButton) { pick(m, via) }
                    .padding(vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                RadioButton(selected = selected, onClick = null, enabled = enabled)
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
