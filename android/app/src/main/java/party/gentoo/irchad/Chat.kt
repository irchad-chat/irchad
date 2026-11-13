package party.gentoo.irchad

import android.Manifest
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Checkbox
import androidx.compose.material3.ListItem
import androidx.compose.material3.SuggestionChip
import androidx.compose.ui.unit.Dp
import io.livekit.android.room.participant.ConnectionQuality
import party.gentoo.irchad.core.Device
import party.gentoo.irchad.core.Reaction
import android.content.pm.PackageManager
import android.os.Build
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material.icons.filled.Call
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.FilledIconToggleButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.runtime.key
import android.content.pm.ActivityInfo
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.rememberTransformableState
import androidx.compose.foundation.gestures.transformable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withStyle
import party.gentoo.irchad.core.Span
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import io.livekit.android.renderer.TextureViewRenderer
import livekit.org.webrtc.RendererCommon
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material.icons.filled.Menu
import androidx.compose.material.icons.filled.Person
import androidx.compose.material3.AlertDialog
import party.gentoo.irchad.core.Sas
import party.gentoo.irchad.core.SasStep
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.NavigationDrawerItem
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.material.icons.filled.Face
import androidx.compose.material.icons.filled.Settings
import androidx.compose.foundation.text.InlineTextContent
import androidx.compose.foundation.text.appendInlineContent
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.Placeholder
import androidx.compose.ui.text.PlaceholderVerticalAlign
import androidx.compose.ui.unit.em
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import party.gentoo.irchad.core.Buffer
import party.gentoo.irchad.core.Link
import party.gentoo.irchad.core.Lock
import party.gentoo.irchad.core.Message
import party.gentoo.irchad.core.Network
import java.text.DateFormat
import java.util.Date

/** A new header after this much quiet, even from the same person. */
private const val GROUP_GAP_MS = 5 * 60 * 1000L

/** A network (and buffer) to show. Compared by identity, so showing it again works. */
class Focus(val network: String, val target: String?)

@Composable
fun Root(app: IrChad, invite: String?, open: Focus?, pip: Boolean, inviteTaken: () -> Unit) {
    val tick by app.version.collectAsState()
    val call by app.calls.state.collectAsState()
    val nets = remember(tick) { app.client.networks() }
    var adding by rememberSaveable { mutableStateOf(false) }
    // The network (and channel) a finished join should show.
    var focus by remember { mutableStateOf<Focus?>(null) }
    LaunchedEffect(open) { if (open != null) focus = open }
    val ctx = LocalContext.current
    LaunchedEffect(Unit) {
        app.notices.collect { Toast.makeText(ctx, it, Toast.LENGTH_LONG).show() }
    }
    val verify by app.verify.collectAsState()
    verify?.let { VerifyDialog(app, it) }
    val reported by app.reported.collectAsState()
    if (reported) {
        AlertDialog(
            onDismissRequest = { app.reported.value = false },
            title = { Text("Report sent") },
            text = { Text("Thanks for letting us know. The moderators of this server will look into it.") },
            confirmButton = { TextButton(onClick = { app.reported.value = false }) { Text("OK") } },
        )
    }
    // The core lists saved networks shortly after start; give it a moment
    // before deciding there are none. A join in progress is listed too, so
    // the join screen stays up by `adding`, not by the list being empty.
    var settled by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) {
        kotlinx.coroutines.delay(400)
        settled = true
    }
    LaunchedEffect(settled, nets.isEmpty()) { if (settled && nets.isEmpty()) adding = true }
    val close = { adding = false; inviteTaken() }
    when {
        invite != null || adding -> JoinScreen(
            app, invite, nets,
            onDone = { origin, channel -> focus = Focus(origin, channel); close() },
            onCancel = if (nets.isEmpty()) null else close,
        )
        nets.isEmpty() -> Surface(Modifier.fillMaxSize()) {}
        call?.watching != null && pip -> ScreenViewer(app, call!!, pip = true)
        else -> Box(Modifier.fillMaxSize()) {
            ChatScreen(app, tick, nets, focus, call) { adding = true }
            BackgroundOnboarding(app)
            SoundboardSheet(app)
            val shared by app.shared.collectAsState()
            shared?.let { sh ->
                ShareSheet(app, sh, nets, onSent = { focus = it; app.shared.value = null }) { app.shared.value = null }
            }
            call?.takeIf { it.watching != null }?.let { ScreenViewer(app, it, pip = false) }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ChatScreen(
    app: IrChad,
    tick: Long,
    nets: List<Network>,
    focus: Focus?,
    call: CallState?,
    onAdd: () -> Unit,
) {
    val c = app.client
    var current by rememberSaveable { mutableStateOf(nets.first().network) }
    val net = nets.firstOrNull { it.network == current } ?: nets.first()
    val buffers = remember(tick, net.network) { c.buffers(net.network) }
    // Selected buffer per network, by name as the server spells it.
    val selected = remember { mutableStateMapOf<String, String>() }
    LaunchedEffect(focus) {
        val f = focus ?: return@LaunchedEffect
        current = f.network
        if (f.target != null) selected[f.network] = f.target
    }
    val target = selected[net.network]?.takeIf { s -> buffers.any { it.target.equals(s, true) } }
        ?: buffers.firstOrNull()?.target
    val buf = buffers.firstOrNull { it.target.equals(target, true) }

    val drawer = rememberDrawerState(DrawerValue.Closed)
    val scope = rememberCoroutineScope()
    var members by remember { mutableStateOf(false) }
    var dialog by remember { mutableStateOf<String?>(null) }
    val joinVoice = rememberVoiceJoin(app)
    // The message being answered, or edited, in the buffer on screen.
    var replyTo by remember(net.network, buf?.target) { mutableStateOf<Message?>(null) }
    var editing by remember(net.network, buf?.target) { mutableStateOf<Message?>(null) }

    BackHandler(drawer.isOpen) { scope.launch { drawer.close() } }

    ModalNavigationDrawer(
        drawerState = drawer,
        drawerContent = {
            ModalDrawerSheet {
                Drawer(
                    app, tick, nets, net, buffers, buf?.target,
                    onNet = { current = it },
                    onBuffer = {
                        selected[net.network] = it
                        scope.launch { drawer.close() }
                    },
                    onAdd = onAdd,
                    onDialog = { dialog = it },
                )
            }
        },
    ) {
        Scaffold(
            topBar = {
                TopAppBar(
                    navigationIcon = {
                        IconButton(onClick = { scope.launch { drawer.open() } }) {
                            BadgedBox(badge = {
                                val others = nets.filter { it.network != net.network }.sumOf { it.mentions.toInt() } +
                                    buffers.filter { it.target != buf?.target }.sumOf { it.mentions.toInt() }
                                if (others > 0) Badge { Text("$others") }
                            }) { Icon(Icons.Default.Menu, "Channels") }
                        }
                    },
                    title = {
                        Column {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text(buf?.target ?: net.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                if (buf != null && buf.encryption != Lock.NONE && buf.encryption != Lock.OFF) {
                                    Icon(
                                        Icons.Default.Lock, "Encrypted",
                                        Modifier.padding(start = 6.dp).size(16.dp),
                                        tint = if (buf.encryption == Lock.CHANGED) MaterialTheme.colorScheme.error
                                        else MaterialTheme.colorScheme.primary,
                                    )
                                }
                            }
                            val sub = buf?.topic?.takeIf { it.isNotBlank() } ?: net.name
                            Text(
                                sub, style = MaterialTheme.typography.bodySmall, maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    },
                    actions = {
                        if (buf != null && !buf.dm && buf.voiceAllowed && !net.guest &&
                            !buf.target.equals(net.voice, true)
                        ) {
                            IconButton(onClick = { joinVoice(net.network, buf.target) }) {
                                Icon(Icons.Default.Call, "Join voice")
                            }
                        }
                        if (buf != null && buf.dm && net.status == null &&
                            (buf.encryption == Lock.ON || buf.encryption == Lock.CHANGED)
                        ) {
                            TextButton(onClick = { c.verification(net.network, buf.target, SasStep.START) }) {
                                Text("Verify")
                            }
                        }
                        if (buf != null && !buf.dm) {
                            IconButton(onClick = { members = true }) { Icon(Icons.Default.Person, "Members") }
                        }
                        if (buf != null) {
                            var more by remember { mutableStateOf(false) }
                            Box {
                                IconButton(onClick = { more = true }) { Icon(Icons.Default.MoreVert, "More") }
                                DropdownMenu(expanded = more, onDismissRequest = { more = false }) {
                                    if (buf.dm && buf.encryption != Lock.NONE && buf.encryption != Lock.OFF) {
                                        DropdownMenuItem(text = { Text("Devices") }, onClick = {
                                            more = false
                                            dialog = "devices"
                                        })
                                    }
                                    if (buf.dm && c.reportTo(net.network) != null) {
                                        DropdownMenuItem(text = { Text("Report") }, onClick = {
                                            more = false
                                            dialog = "report"
                                        })
                                    }
                                    if (buf.dm) {
                                        DropdownMenuItem(text = { Text("Block") }, onClick = {
                                            more = false
                                            c.block(net.network, buf.target, null)
                                            c.closeDm(net.network, buf.target)
                                        })
                                    }
                                    DropdownMenuItem(
                                        text = { Text(if (buf.dm) "Close conversation" else "Leave channel") },
                                        onClick = {
                                            more = false
                                            if (buf.dm) c.closeDm(net.network, buf.target) else c.part(net.network, buf.target)
                                        },
                                    )
                                }
                            }
                        }
                    },
                )
            },
        ) { pad ->
            Column(
                Modifier
                    .padding(pad)
                    .fillMaxSize()
                    .imePadding(),
            ) {
                StatusBanner(app, net)
                if (buf?.encryption == Lock.CHANGED) KeysChanged(app, net, buf)
                if (buf == null) {
                    Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                        Text("No channels yet.", color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                } else {
                    Log(
                        app, tick, net, buf, Modifier.weight(1f),
                        onReply = { editing = null; replyTo = it },
                        onEdit = { replyTo = null; editing = it },
                    )
                }
                call?.let { VoicePanel(app, nets, it) }
                if (buf != null) {
                    Composer(app, tick, net, buf, replyTo, editing) {
                        replyTo = null
                        editing = null
                    }
                }
            }
        }
    }

    if (members && buf != null) {
        val list = remember(tick, buf.target) { c.memberList(net.network, buf.target) }
        ModalBottomSheet(onDismissRequest = { members = false }) {
            Text(
                "Members" + (list?.let { " · ${it.size}" } ?: ""),
                style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp),
            )
            LazyColumn(Modifier.navigationBarsPadding()) {
                for ((away, title) in listOf(false to "ONLINE", true to "AWAY")) {
                    val group = list.orEmpty().filter { it.away == away }
                    if (group.isEmpty()) continue
                    // '#' can't start a nick, so header keys never collide with member keys.
                    item(key = "#$title") {
                        Text(
                            "$title — ${group.size}",
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(start = 24.dp, end = 24.dp, top = 12.dp, bottom = 4.dp),
                        )
                    }
                    items(group, key = { it.nick }) { m ->
                        Row(
                            Modifier
                                .fillMaxWidth()
                                .clickable {
                                    if (!m.nick.equals(net.nick, true)) {
                                        c.openDm(net.network, m.nick)
                                        selected[net.network] = m.nick
                                        members = false
                                    }
                                }
                                .padding(horizontal = 24.dp, vertical = 10.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            val p = remember(tick, m.nick) { c.profile(net.network, m.nick) }
                            Avatar(app, net.network, m.nick, p.avatar)
                            Spacer(Modifier.width(12.dp))
                            Column {
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    if (m.oper) {
                                        Text("👑", Modifier.semantics { contentDescription = "IRC operator" })
                                        Spacer(Modifier.width(4.dp))
                                    }
                                    if (m.account == null) {
                                        Text("👻", Modifier.semantics { contentDescription = "Guest (not logged in)" })
                                        Spacer(Modifier.width(4.dp))
                                    }
                                    Text(
                                        m.prefix + m.shown,
                                        color = if (m.away) MaterialTheme.colorScheme.onSurfaceVariant
                                        else MaterialTheme.colorScheme.onSurface,
                                    )
                                }
                                p.activity?.let {
                                    Text(
                                        (if (it.startsWith("Listening to")) "🎵 " else "🎮 ") + it,
                                        style = MaterialTheme.typography.bodySmall,
                                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                                        maxLines = 1,
                                        overflow = TextOverflow.Ellipsis,
                                    )
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    when (dialog) {
        "settings" -> SettingsDialog(app) { dialog = null }
        "status" -> StatusDialog(app, net, onProfile = { dialog = "profile" }, onLeave = { dialog = "leave" }) { dialog = null }
        "profile" -> ProfileDialog(app, tick, net) { dialog = null }
        "leave" -> LeaveDialog(app, net) { dialog = null }
        "devices" -> buf?.let { DevicesDialog(app, tick, net, it.target) { dialog = null } } ?: run { dialog = null }
        "report" -> buf?.let { ReportDialog(app, net, it.target, it.target, null) { dialog = null } } ?: run { dialog = null }
        "channel", "dm" -> AddDialog(app, net, dialog!!, onOpen = { selected[net.network] = it }) { dialog = null }
    }
}

/** Join a channel, or open a DM. */
@Composable
private fun AddDialog(app: IrChad, net: Network, kind: String, onOpen: (String) -> Unit, onDismiss: () -> Unit) {
    val c = app.client
    var name by remember { mutableStateOf(if (kind == "channel") "#" else "") }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (kind == "channel") "Join a channel" else "Message someone") },
        text = {
            OutlinedTextField(
                name, { name = it }, singleLine = true,
                label = { Text(if (kind == "channel") "Channel" else "Nickname") },
            )
        },
        confirmButton = {
            TextButton(enabled = name.trim().length > 1 || kind == "dm" && name.isNotBlank(), onClick = {
                val n = name.trim()
                if (kind == "channel") c.joinChannel(net.network, n) else c.openDm(net.network, n)
                onOpen(n)
                onDismiss()
            }) { Text(if (kind == "channel") "Join" else "Open") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** Report `nick` (or their message `msg`) to whoever the network names; `Root` says when it's sent. */
@Composable
private fun ReportDialog(app: IrChad, net: Network, nick: String, target: String?, msg: Message?, onDismiss: () -> Unit) {
    var reason by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (msg != null) "Report message" else "Report $nick") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                msg?.let { Text("\"${it.text}\"", maxLines = 3, overflow = TextOverflow.Ellipsis) }
                Text("This goes privately to the server's moderators.")
                OutlinedTextField(reason, { reason = it }, label = { Text("What's wrong? (optional)") })
            }
        },
        confirmButton = {
            TextButton(onClick = {
                app.client.report(net.network, nick, target, msg?.msgid, msg?.text, reason)
                onDismiss()
            }) { Text("Report") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** Online or away, and the way to the profile and to leaving, like desktop's status menu. */
@Composable
private fun StatusDialog(app: IrChad, net: Network, onProfile: () -> Unit, onLeave: () -> Unit, onDismiss: () -> Unit) {
    val s = app.settings
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Your status and profile") },
        text = {
            Column {
                for ((away, label) in listOf(false to "Online", true to "Away")) {
                    Choice(label, s.away == away) {
                        s.away = away
                        app.checkAway()
                    }
                }
                if (!s.away && app.away) Hint("You're away because you haven't used IrChad for a while.")
                if (!net.guest) TextButton(onClick = onProfile) { Text("Edit profile…") }
                TextButton(onClick = onLeave) { Text("Leave ${net.name}…", color = MaterialTheme.colorScheme.error) }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Done") } },
    )
}

/** Display name, picture, and this account's devices with encrypted DMs. */
@Composable
private fun ProfileDialog(app: IrChad, tick: Long, net: Network, onDismiss: () -> Unit) {
    val c = app.client
    val ctx = LocalContext.current
    val me = remember(tick) { c.profile(net.network, net.nick) }
    var name by remember { mutableStateOf(me.displayName.orEmpty()) }
    var receipts by remember { mutableStateOf(net.receipts) }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
        if (uri != null) upload(app, ctx, net, null, listOf(uri))
    }
    LaunchedEffect(net.network) { c.fetchDevices(net.network, net.nick) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Your profile on ${net.name}") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Avatar(app, net.network, net.nick, me.avatar, 64.dp)
                    Column(Modifier.padding(start = 8.dp)) {
                        if (net.uploads != null) TextButton(onClick = { pick.launch("image/*") }) { Text("Choose a picture…") }
                        if (me.avatar != null) TextButton(onClick = { c.clearAvatar(net.network) }) { Text("Remove picture") }
                    }
                }
                OutlinedTextField(
                    name, { name = it.take(64) }, singleLine = true,
                    label = { Text("Display name") }, placeholder = { Text(net.nick) },
                    modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
                )
                Hint("People still see that you're ${net.nick}.")
                Toggle("Read receipts in DMs", receipts) { receipts = it }
                Hint("Show when you've read a DM, and see when they have. Only between people who both turn this on.")
                val devices = remember(tick) { c.devices(net.network, net.nick) }
                if (devices.isNotEmpty()) {
                    Text("Devices with encrypted DMs", fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 16.dp))
                    Devices(devices) { c.removeDevice(net.network, it) }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = {
                c.setDisplayName(net.network, name.trim())
                if (receipts != net.receipts) c.setReceipts(net.network, receipts)
                onDismiss()
            }) { Text("Save") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** Someone's devices with encrypted DMs, to compare fingerprints by eye. */
@Composable
private fun DevicesDialog(app: IrChad, tick: Long, net: Network, nick: String, onDismiss: () -> Unit) {
    val c = app.client
    LaunchedEffect(net.network, nick) { c.fetchDevices(net.network, nick) }
    val devices = remember(tick, nick) { c.devices(net.network, nick) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("$nick's devices") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                if (devices.isEmpty()) Hint("None yet.") else Devices(devices, null)
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Done") } },
    )
}

/** Each device's name, whether it's verified, and its fingerprint; [onRemove] adds Remove (not on this one). */
@Composable
private fun Devices(devices: List<Device>, onRemove: ((String) -> Unit)?) {
    for (d in devices) {
        Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(if (d.`this`) "This device" else d.id, Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (d.verified) Icon(Icons.Default.CheckCircle, "Verified", Modifier.size(16.dp), tint = MaterialTheme.colorScheme.primary)
            if (onRemove != null && !d.`this`) TextButton(onClick = { onRemove(d.id) }) { Text("Remove") }
        }
        Text(
            d.fingerprint, fontFamily = FontFamily.Monospace,
            style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

/** Forget the network here, as on desktop; optionally delete the account too. */
@Composable
private fun LeaveDialog(app: IrChad, net: Network, onDismiss: () -> Unit) {
    var delete by remember { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Leave ${net.name}?") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("IrChad forgets this network and your saved password for it.")
                Text("This device's encryption keys go too, so it can't read encrypted DMs again.")
                if (delete) {
                    Text("Your account is deleted from the server, and its channels with it. The name stays reserved.")
                } else {
                    Text("Your account stays, so you can join again later.")
                }
                if (!net.guest) {
                    Row(
                        Modifier.fillMaxWidth().toggleable(delete, role = Role.Checkbox) { delete = it },
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Checkbox(checked = delete, onCheckedChange = null)
                        Text("Delete account", Modifier.padding(start = 8.dp))
                    }
                }
            }
        },
        confirmButton = {
            Button(
                onClick = {
                    if (delete) app.client.deleteAccount(net.network) else app.client.remove(net.network)
                    onDismiss()
                },
                colors = ButtonDefaults.buttonColors(
                    containerColor = MaterialTheme.colorScheme.error,
                    contentColor = MaterialTheme.colorScheme.onError,
                ),
            ) { Text(if (delete) "Delete and leave" else "Leave") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun Drawer(
    app: IrChad,
    tick: Long,
    nets: List<Network>,
    net: Network,
    buffers: List<Buffer>,
    selected: String?,
    onNet: (String) -> Unit,
    onBuffer: (String) -> Unit,
    onAdd: () -> Unit,
    onDialog: (String) -> Unit,
) {
    LazyRow(
        Modifier.padding(horizontal = 12.dp, vertical = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        items(nets, key = { it.network }) { n ->
            FilterChip(
                selected = n.network == net.network,
                onClick = { onNet(n.network) },
                leadingIcon = n.icon?.let { url -> { NetworkIcon(app, n.network, url) } },
                label = {
                    Text(n.name)
                    if (n.mentions > 0u) {
                        Spacer(Modifier.width(6.dp))
                        Badge { Text("${n.mentions}") }
                    }
                },
            )
        }
        item {
            IconButton(onClick = onAdd) { Icon(Icons.Default.Add, "Join a network") }
        }
        item {
            IconButton(onClick = { onDialog("settings") }) { Icon(Icons.Default.Settings, "Settings") }
        }
    }
    // Your status and profile, as in desktop's sidebar.
    val me = remember(tick, net.network) { app.client.profile(net.network, net.nick) }
    val shown = me.displayName?.takeIf { it.isNotBlank() && it != net.nick }?.let { "$it (${net.nick})" } ?: net.nick
    ListItem(
        headlineContent = { Text(shown, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        supportingContent = { Text(if (app.away) "Away" else "Online") },
        leadingContent = { Avatar(app, net.network, net.nick, me.avatar) },
        modifier = Modifier
            .clickable(onClickLabel = "Your status and profile") { onDialog("status") }
            .padding(horizontal = 12.dp),
    )
    HorizontalDivider()
    LazyColumn(Modifier.padding(horizontal = 12.dp)) {
        for ((title, dm) in listOf("Channels" to false, "Direct messages" to true)) {
            item(key = title) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        title, style = MaterialTheme.typography.titleSmall,
                        modifier = Modifier.weight(1f).padding(start = 16.dp, top = 12.dp, bottom = 4.dp),
                    )
                    IconButton(onClick = { onDialog(if (dm) "dm" else "channel") }) {
                        Icon(Icons.Default.Add, if (dm) "Message someone" else "Join a channel")
                    }
                }
            }
            items(buffers.filter { it.dm == dm }, key = { "b:" + it.target }) { b ->
                NavigationDrawerItem(
                    label = {
                        Text(
                            b.target,
                            fontWeight = if (b.unread > 0u) FontWeight.Bold else FontWeight.Normal,
                            color = if (dm && b.online == false) MaterialTheme.colorScheme.onSurfaceVariant
                            else Color.Unspecified,
                        )
                    },
                    badge = { if (b.mentions > 0u) Badge { Text("${b.mentions}") } },
                    selected = b.target.equals(selected, true),
                    onClick = { onBuffer(b.target) },
                )
                if (b.voice.isNotEmpty()) {
                    Text(
                        "🔊 " + b.voice.joinToString(", "),
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(start = 32.dp, end = 16.dp, bottom = 4.dp),
                    )
                }
            }
        }
    }
}

/** Connecting, reconnecting, a failed login, or a sign-up waiting on its code. */
@Composable
private fun KeysChanged(app: IrChad, net: Network, buf: Buffer) {
    Surface(color = MaterialTheme.colorScheme.errorContainer) {
        Column(Modifier.fillMaxWidth().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                "${buf.target}'s encryption keys changed. That's normal after they reinstall, " +
                    "but it can also mean someone is listening in. Verify to be sure.",
                style = MaterialTheme.typography.bodyMedium,
            )
            TextButton(onClick = { app.client.acceptKeys(net.network, buf.target) }) {
                Text("Accept the new keys")
            }
        }
    }
}

/** Comparing emoji with someone (SAS). */
@Composable
private fun VerifyDialog(app: IrChad, v: Verify) {
    val step = { s: SasStep -> app.client.verification(v.network, v.nick, s) }
    val close = { app.verify.value = null }
    val finished = v.state is Sas.Done || v.state is Sas.Failed
    AlertDialog(
        onDismissRequest = { if (finished) close() else step(SasStep.CANCEL) },
        title = { Text("Verify ${v.nick}") },
        text = {
            when (val s = v.state) {
                Sas.Incoming -> Text(
                    "${v.nick} wants to make sure your messages to each other are private, by comparing emoji.",
                )
                Sas.Waiting -> Row(verticalAlignment = Alignment.CenterVertically) {
                    CircularProgressIndicator(Modifier.size(20.dp))
                    Spacer(Modifier.width(12.dp))
                    Text("Waiting for ${v.nick}…")
                }
                is Sas.Compare -> Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text(
                        "Check that ${v.nick} sees the same emoji, in the same order. " +
                            "Compare them in person or on a call, not by message.",
                    )
                    // Seven emoji: four on top, three below, so they fit a phone.
                    s.emoji.chunked(4).forEach { row ->
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly) {
                            row.forEach { e ->
                                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                                    Text(e.emoji, style = MaterialTheme.typography.headlineMedium)
                                    Text(e.name, style = MaterialTheme.typography.labelSmall)
                                }
                            }
                        }
                    }
                }
                Sas.Done -> Text("${v.nick}'s device is verified. Nobody is listening in.")
                is Sas.Failed -> Text(s.reason)
            }
        },
        confirmButton = {
            when (v.state) {
                Sas.Incoming -> TextButton(onClick = { step(SasStep.ACCEPT) }) { Text("Start") }
                Sas.Waiting -> TextButton(onClick = { step(SasStep.CANCEL) }) { Text("Cancel") }
                is Sas.Compare -> TextButton(onClick = { step(SasStep.MATCH) }) { Text("They match") }
                Sas.Done -> TextButton(onClick = close) { Text("Done") }
                is Sas.Failed -> TextButton(onClick = close) { Text("Close") }
            }
        },
        dismissButton = {
            when (v.state) {
                Sas.Incoming -> TextButton(onClick = { step(SasStep.CANCEL) }) { Text("Not now") }
                is Sas.Compare -> TextButton(onClick = { step(SasStep.CANCEL) }) { Text("They don't match") }
                else -> {}
            }
        },
    )
}

@Composable
private fun StatusBanner(app: IrChad, net: Network) {
    val status = net.status ?: return
    var input by remember(net.network) { mutableStateOf("") }
    Surface(color = MaterialTheme.colorScheme.secondaryContainer) {
        Column(Modifier.fillMaxWidth().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(status, style = MaterialTheme.typography.bodyMedium)
            if (net.failed || net.needsCode) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    OutlinedTextField(
                        input, { input = it }, singleLine = true,
                        label = { Text(if (net.needsCode) "Code from the email" else "Password") },
                        visualTransformation = if (net.needsCode) androidx.compose.ui.text.input.VisualTransformation.None
                        else PasswordVisualTransformation(),
                        modifier = Modifier.weight(1f),
                    )
                    Spacer(Modifier.width(8.dp))
                    Button(enabled = input.isNotBlank(), onClick = {
                        if (net.needsCode) app.client.verify(net.network, input.trim())
                        else app.client.signIn(net.network, input)
                        input = ""
                    }) { Text(if (net.needsCode) "Verify" else "Sign in") }
                }
            }
        }
    }
}

@Composable
private fun Log(
    app: IrChad, tick: Long, net: Network, buf: Buffer, modifier: Modifier,
    onReply: (Message) -> Unit, onEdit: (Message) -> Unit,
) {
    val c = app.client
    val msgs = remember(tick, net.network, buf.target) { c.messages(net.network, buf.target) }
    val connected = net.status == null
    // Newest at the bottom: the list is reversed so it starts there.
    val list = rememberLazyListState()
    var menu by remember { mutableStateOf<Message?>(null) }
    var reactTo by remember { mutableStateOf<String?>(null) }
    var reporting by remember { mutableStateOf<Message?>(null) }
    val reportTo = remember(tick, net.network) { c.reportTo(net.network) }
    @Suppress("DEPRECATION")
    val clipboard = androidx.compose.ui.platform.LocalClipboardManager.current

    LaunchedEffect(net.network, buf.target) { app.notifier.dismiss(net.network, buf.target) }
    LaunchedEffect(net.network, buf.target, msgs.size, buf.unread) {
        if (buf.unread > 0u) c.markRead(net.network, buf.target)
    }
    LaunchedEffect(net.network, buf.target, connected) {
        if (connected && !buf.dm) c.members(net.network, buf.target)
    }
    val nearTop by remember { derivedStateOf { list.layoutInfo.visibleItemsInfo.lastOrNull()?.index?.let { it >= list.layoutInfo.totalItemsCount - 5 } ?: false } }
    LaunchedEffect(nearTop, msgs.size, buf.historyStart) {
        if (nearTop && !buf.historyStart && connected) c.loadOlder(net.network, buf.target)
    }
    LaunchedEffect(msgs.lastOrNull()?.key) {
        if (list.firstVisibleItemIndex <= 1) list.animateScrollToItem(0)
    }

    val rev = msgs.asReversed()
    // Where each msgid is in `rev`, for reply quotes.
    val at = remember(msgs) { rev.withIndex().filter { it.value.msgid != null }.associate { it.value.msgid!! to it.index } }
    val scope = rememberCoroutineScope()
    // A reply's parent being loaded (`loadAround`), to scroll to once it's in.
    var jump by remember(net.network, buf.target) { mutableStateOf<String?>(null) }
    LaunchedEffect(at, jump) {
        val i = jump?.let { at[it] } ?: return@LaunchedEffect
        jump = null
        list.animateScrollToItem(i)
    }
    Box(modifier.fillMaxWidth()) {
        LazyColumn(state = list, reverseLayout = true, modifier = Modifier.fillMaxSize()) {
            items(rev.size, key = { rev[it].key }) { i ->
                val m = rev[i]
                val prev = rev.getOrNull(i + 1)
                val parent = m.replyTo?.let { id -> at[id]?.let { rev[it] } }
                val head = prev == null || prev.nick != m.nick || m.time - prev.time > GROUP_GAP_MS ||
                    prev.action || m.action || m.replyTo != null
                Box {
                    MessageRow(
                        app, net.network, net.nick, m, head, buf.dm, parent,
                        seenAt = buf.seenAt?.takeIf { m.msgid != null && m.msgid == buf.seen },
                        onLong = { menu = m },
                        onParent = {
                            val id = m.replyTo
                            val i = id?.let { at[it] }
                            when {
                                i != null -> scope.launch { list.animateScrollToItem(i) }
                                id != null && connected -> {
                                    jump = id
                                    c.loadAround(net.network, buf.target, id)
                                }
                            }
                        },
                        onReact = { r ->
                            val id = m.msgid ?: return@MessageRow
                            if (r.nicks.any { it.equals(net.nick, true) }) c.unreact(net.network, buf.target, id, r.emoji)
                            else c.react(net.network, buf.target, id, r.emoji)
                        },
                    )
                    DropdownMenu(expanded = menu?.key == m.key, onDismissRequest = { menu = null }) {
                        if (m.msgid != null && !m.redacted) {
                            DropdownMenuItem(text = { Text("Reply") }, onClick = {
                                onReply(m)
                                menu = null
                            })
                        }
                        for (e in listOf("👍", "❤", "😂", "😮")) {
                            DropdownMenuItem(text = { Text("React $e") }, onClick = {
                                m.msgid?.let { c.react(net.network, buf.target, it, e) }
                                menu = null
                            })
                        }
                        if (m.msgid != null) {
                            DropdownMenuItem(text = { Text("React…") }, onClick = {
                                reactTo = m.msgid
                                menu = null
                            })
                        }
                        if (!m.redacted && m.text.isNotEmpty()) {
                            DropdownMenuItem(text = { Text("Copy text") }, onClick = {
                                clipboard.setText(androidx.compose.ui.text.AnnotatedString(m.text))
                                menu = null
                            })
                        }
                        if (m.own && m.msgid != null && !m.redacted && !m.undecryptable && !m.notice) {
                            DropdownMenuItem(text = { Text("Edit") }, onClick = {
                                onEdit(m)
                                menu = null
                            })
                        }
                        if (m.own && m.msgid != null && !m.redacted) {
                            DropdownMenuItem(text = { Text("Delete") }, onClick = {
                                c.redact(net.network, buf.target, m.msgid!!)
                                menu = null
                            })
                        }
                        if (!m.own && reportTo != null) {
                            DropdownMenuItem(text = { Text("Report") }, onClick = {
                                reporting = m
                                menu = null
                            })
                        }
                        if (!m.own) {
                            DropdownMenuItem(text = { Text("Block ${m.shown}") }, onClick = {
                                c.block(net.network, m.nick, m.account)
                                menu = null
                            })
                        }
                    }
                }
            }
        }
        reporting?.let { m ->
            ReportDialog(app, net, m.nick, buf.target, m) { reporting = null }
        }
        reactTo?.let { id ->
            Picker(app, net.network, null, onEmoji = {
                c.react(net.network, buf.target, id, it)
                reactTo = null
            }, onGif = {}, onDismiss = { reactTo = null })
        }
        if (buf.typing.isNotEmpty()) {
            val who = buf.typing
            Text(
                when (who.size) {
                    1 -> "${who[0]} is typing…"
                    2 -> "${who[0]} and ${who[1]} are typing…"
                    else -> "Several people are typing…"
                },
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier
                    .align(Alignment.BottomStart)
                    .background(MaterialTheme.colorScheme.surface)
                    .padding(horizontal = 16.dp, vertical = 2.dp),
            )
        }
    }
}

@OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
@Composable
private fun MessageRow(
    app: IrChad, network: String, own: String, m: Message, head: Boolean, dm: Boolean, parent: Message?,
    seenAt: Long?, onLong: () -> Unit, onParent: () -> Unit, onReact: (Reaction) -> Unit,
) {
    val time = remember(m.time) { DateFormat.getTimeInstance(DateFormat.SHORT).format(Date(m.time)) }
    val bar = if (m.mention && !dm) MaterialTheme.colorScheme.primary else Color.Transparent
    Row(
        Modifier
            .fillMaxWidth()
            .combinedClickable(onClick = {}, onLongClick = onLong)
            .background(if (m.mention && !dm) MaterialTheme.colorScheme.primary.copy(alpha = 0.08f) else Color.Transparent)
            .padding(start = 0.dp, end = 16.dp, top = if (head) 10.dp else 1.dp, bottom = 1.dp),
    ) {
        Box(Modifier.width(3.dp).size(3.dp, 20.dp).background(bar))
        Spacer(Modifier.width(9.dp))
        if (head) Avatar(app, network, m.nick, m.avatar) else Spacer(Modifier.width(36.dp))
        Spacer(Modifier.width(10.dp))
        Column(Modifier.weight(1f)) {
            if (m.replyTo != null) {
                Text(
                    "↪ " + (parent?.let { p -> "${p.shown}: " + (if (p.redacted) "Message deleted" else p.text) }
                        ?: "Replying to an earlier message"),
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.clickable(onClickLabel = "Jump to the message", onClick = onParent),
                )
            }
            if (head) {
                Row(verticalAlignment = Alignment.Bottom) {
                    Text(m.shown, fontWeight = FontWeight.SemiBold, color = nickColor(m.nick))
                    Spacer(Modifier.width(8.dp))
                    Text(time, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    if (m.encrypted) {
                        Icon(Icons.Default.Lock, "Encrypted", Modifier.padding(start = 4.dp).size(12.dp),
                            tint = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
            val faint = MaterialTheme.colorScheme.onSurfaceVariant
            val inline = emojiInline(
                app, network,
                remember(m.spans, m.reactions) { m.spans.mapNotNull { it.emoji } + m.reactions.mapNotNull { it.image } },
            )
            when {
                m.redacted -> Text("Message deleted", fontStyle = FontStyle.Italic, color = faint)
                m.undecryptable -> Text("Couldn't decrypt this message.", fontStyle = FontStyle.Italic, color = MaterialTheme.colorScheme.error)
                m.action -> Text(
                    styled(m.spans, MaterialTheme.colorScheme.primary, prefix = "* ${m.shown} ", emoji = inline.keys),
                    fontStyle = FontStyle.Italic,
                    inlineContent = inline,
                )
                else -> Text(
                    styled(m.spans, MaterialTheme.colorScheme.primary, emoji = inline.keys),
                    color = if (m.pending) faint else if (m.notice) faint else Color.Unspecified,
                    inlineContent = inline,
                )
            }
            if (m.edited && !m.redacted) Text("(edited)", style = MaterialTheme.typography.labelSmall, color = faint)
            MessageLinks(app, network, m)
            m.failed?.let { Text("Not sent: $it", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.error) }
            if (m.reactions.isNotEmpty()) {
                Row(horizontalArrangement = Arrangement.spacedBy(4.dp), modifier = Modifier.padding(top = 2.dp)) {
                    for (r in m.reactions) {
                        val mine = r.nicks.any { it.equals(own, true) }
                        Text(
                            // A custom emoji's image, or its `:name:` until that has loaded.
                            buildAnnotatedString {
                                val image = r.image
                                if (image != null && image in inline) appendInlineContent(image, r.emoji) else append(r.emoji)
                                append(" ${r.nicks.size}")
                            },
                            inlineContent = inline,
                            style = MaterialTheme.typography.labelMedium,
                            modifier = Modifier
                                .clip(RoundedCornerShape(10.dp))
                                .background(
                                    if (mine) MaterialTheme.colorScheme.primaryContainer
                                    else MaterialTheme.colorScheme.surfaceVariant,
                                )
                                .clickable(
                                    enabled = m.msgid != null,
                                    onClickLabel = if (mine) "Take back ${r.emoji}" else "React ${r.emoji}",
                                ) { onReact(r) }
                                .semantics { contentDescription = "${r.emoji} from ${r.nicks.joinToString(", ")}" }
                                .padding(horizontal = 8.dp, vertical = 2.dp),
                        )
                    }
                }
            }
            if (seenAt != null) {
                val at = remember(seenAt) {
                    DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.SHORT).format(Date(seenAt))
                }
                Text("Seen $at", style = MaterialTheme.typography.labelSmall, color = faint, modifier = Modifier.padding(top = 2.dp))
            }
        }
    }
}

/** Their picture once it's loaded (when images load by themselves), or their initial. */
@Composable
private fun Avatar(app: IrChad, network: String, nick: String, url: String?, size: Dp = 36.dp) {
    val tick by app.media.tick.collectAsState()
    val auto = app.media.auto
    val img = url?.let { remember(tick, network, it, auto) { app.media.image(network, it, auto) } }
    if (url != null && img is Media.Img.Ready) {
        RemoteImage(app, network, url, size, size, Modifier.size(size).clip(CircleShape), fill = true, ask = true)
        return
    }
    Box(
        Modifier
            .size(size)
            .clip(CircleShape)
            .background(nickColor(nick).copy(alpha = 0.25f)),
        contentAlignment = Alignment.Center,
    ) {
        Text(nick.take(1).uppercase(), fontWeight = FontWeight.Bold, color = nickColor(nick))
    }
}

/** A network's icon in its chip, once it's loaded. */
@Composable
private fun NetworkIcon(app: IrChad, network: String, url: String) {
    val tick by app.media.tick.collectAsState()
    val img = remember(tick, network, url) { app.media.icon(network, url) }
    if (img is Media.Img.Ready) {
        RemoteImage(app, network, url, 18.dp, 18.dp, Modifier.size(18.dp), fill = true, ask = true)
    }
}

/** Stable per nick (FNV-1a, same as desktop), readable on both themes. */
@Composable
private fun nickColor(nick: String): Color {
    var h = 2166136261u
    for (b in nick.toByteArray()) h = (h xor b.toUByte().toUInt()) * 16777619u
    val hue = (h % 360u).toFloat()
    val dark = MaterialTheme.colorScheme.surface.luminance() < 0.5f
    return Color.hsv(hue, 0.55f, if (dark) 0.9f else 0.55f)
}

/** What draws each of `urls`, custom emoji's images, in a line of text; only those that have loaded. */
@Composable
private fun emojiInline(app: IrChad, network: String, urls: List<String>): Map<String, InlineTextContent> {
    if (urls.isEmpty()) return emptyMap()
    val tick by app.media.tick.collectAsState()
    val ready = remember(tick, network, urls) {
        urls.filter { app.media.image(network, it, true) is Media.Img.Ready }.toSet()
    }
    return remember(ready, network) {
        ready.associateWith { url ->
            InlineTextContent(Placeholder(1.4.em, 1.4.em, PlaceholderVerticalAlign.Center)) { name ->
                EmojiImage(app, network, url, name, Modifier.fillMaxSize())
            }
        }
    }
}

/**
 * Message text with its IRC formatting, and its links tappable. A custom
 * emoji in `emoji` (`emojiInline`'s keys) is inline content; any other stays
 * its `:name:`.
 */
private fun styled(spans: List<Span>, link: Color, prefix: String = "", emoji: Set<String> = emptySet()) = buildAnnotatedString {
    append(prefix)
    for (sp in spans) {
        val image = sp.emoji
        if (image != null && image in emoji) {
            appendInlineContent(image, sp.text)
            continue
        }
        val deco = listOfNotNull(
            TextDecoration.Underline.takeIf { sp.underline || sp.link },
            TextDecoration.LineThrough.takeIf { sp.strike },
        )
        val style = SpanStyle(
            fontWeight = if (sp.bold) FontWeight.Bold else null,
            fontStyle = if (sp.italic) FontStyle.Italic else null,
            fontFamily = if (sp.mono) FontFamily.Monospace else null,
            textDecoration = if (deco.isEmpty()) null else TextDecoration.combine(deco),
            color = sp.fg?.let { rgb(it) } ?: if (sp.link) link else Color.Unspecified,
            background = sp.bg?.let { rgb(it) } ?: Color.Unspecified,
        )
        val mention = sp.mention
        if (mention != null) {
            // An `@`-mention, with the person's current name (D66).
            withStyle(
                style.copy(
                    fontWeight = FontWeight.SemiBold,
                    color = link,
                    background = link.copy(alpha = 0.15f),
                ),
            ) { append("@$mention") }
        } else if (sp.link) {
            withLink(LinkAnnotation.Url(sp.text, TextLinkStyles(style))) { append(sp.text) }
        } else {
            withStyle(style) { append(sp.text) }
        }
    }
}

private fun rgb(c: UInt) = Color(0xFF000000.toInt() or c.toInt())

private fun Color.luminance(): Float = 0.2126f * red + 0.7152f * green + 0.0722f * blue

@Composable
private fun Composer(
    app: IrChad, tick: Long, net: Network, buf: Buffer, replyTo: Message?, editing: Message?, onDone: () -> Unit,
) {
    val c = app.client
    var draft by rememberSaveable(net.network, buf.target, stateSaver = TextFieldValue.Saver) {
        mutableStateOf(TextFieldValue())
    }
    // Nicks picked from the `@` list, sent as `+chad/mentions` (D66).
    var picked by remember(net.network, buf.target) { mutableStateOf(listOf<String>()) }
    // Editing starts from the message's text, without its formatting (like desktop).
    LaunchedEffect(editing?.key) {
        editing?.let {
            draft = TextFieldValue(it.text, TextRange(it.text.length))
            picked = it.mentions
        }
    }
    val ctx = LocalContext.current
    val attach = rememberLauncherForActivityResult(ActivityResultContracts.GetMultipleContents()) { uris ->
        if (uris.isNotEmpty()) upload(app, ctx, net, buf.target, uris)
    }
    val banner = when {
        editing != null -> "Editing your message"
        replyTo != null -> "Replying to ${replyTo.shown}: ${replyTo.text}"
        else -> null
    }
    if (banner != null) {
        Surface(color = MaterialTheme.colorScheme.surfaceContainerHigh) {
            Row(Modifier.fillMaxWidth().padding(start = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(
                    banner,
                    style = MaterialTheme.typography.labelMedium, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                IconButton(onClick = {
                    if (editing != null) {
                        draft = TextFieldValue()
                        picked = emptyList()
                    }
                    onDone()
                }) { Icon(Icons.Default.Close, if (editing != null) "Cancel edit" else "Cancel reply") }
            }
        }
    }
    var picker by remember { mutableStateOf(false) }
    // What `/gif` asked to search for; `null` opens on emoji.
    var gifQuery by remember { mutableStateOf<String?>(null) }
    if (picker) {
        Picker(
            app, net.network, net.gifs,
            onEmoji = { e ->
                val t = draft.text
                val (a, b) = draft.selection.min to draft.selection.max
                draft = TextFieldValue(t.substring(0, a) + e + t.substring(b), TextRange(a + e.length))
            },
            // Posted as a plain link: every client can open it, and IrChad shows it inline.
            onGif = {
                c.send(net.network, buf.target, it, null, emptyList())
                picker = false
            },
            onDismiss = { picker = false },
            gifQuery = gifQuery,
        )
    }

    // The `@` list: the word before the cursor, while it starts with `@`.
    val cursor = draft.selection.min
    val wordStart = draft.text.lastIndexOfAny(charArrayOf(' ', '\n', '\t'), (cursor - 1).coerceAtLeast(0))
        .let { if (cursor == 0) 0 else it + 1 }
    val word = draft.text.substring(wordStart.coerceAtMost(cursor), cursor)
    val nicks = remember(tick, net.network, buf.target, word) {
        if (word.startsWith("@")) c.nicks(net.network, buf.target, word.drop(1)) else emptyList()
    }
    if (nicks.isNotEmpty()) {
        LazyRow(
            Modifier.fillMaxWidth().padding(horizontal = 8.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            items(nicks) { n ->
                SuggestionChip(onClick = {
                    // What IRC clients highlight on: `nick: ` to start a message, `nick ` elsewhere.
                    val put = if (wordStart == 0) "${n.nick}: " else "${n.nick} "
                    val t = draft.text
                    draft = TextFieldValue(t.substring(0, wordStart) + put + t.substring(cursor), TextRange(wordStart + put.length))
                    if (n.nick !in picked) picked = picked + n.nick
                }, label = { Text(n.shown) })
            }
        }
    }

    fun change(v: TextFieldValue) {
        var next = v
        if (v.text != draft.text) {
            // `:shortcode:` turns into its emoji as it's typed, like desktop.
            val at = v.text.codePointCount(0, v.selection.min.coerceIn(0, v.text.length))
            c.replaceShortcodes(net.network, v.text, at.toUInt())?.let { r ->
                val i = r.text.offsetByCodePoints(0, r.cursor.toInt().coerceAtMost(r.text.codePointCount(0, r.text.length)))
                next = TextFieldValue(r.text, TextRange(i))
            }
            c.typing(net.network, buf.target, next.text.isNotBlank())
            if (next.text.isBlank()) picked = emptyList()
        }
        draft = next
    }

    fun submit() {
        val text = draft.text.trimEnd()
        gifCommand(text)?.let { q ->
            if (net.gifs != null) {
                gifQuery = q
                picker = true
            } else {
                app.notices.tryEmit("This network doesn't have GIFs.")
            }
            draft = TextFieldValue()
            return
        }
        if (text.isBlank()) return
        val id = editing?.msgid
        if (id != null) c.edit(net.network, buf.target, id, text, picked)
        else c.send(net.network, buf.target, text, replyTo?.msgid, picked)
        draft = TextFieldValue()
        picked = emptyList()
        c.typing(net.network, buf.target, false)
        onDone()
    }

    Row(
        Modifier
            .fillMaxWidth()
            .navigationBarsPadding()
            .padding(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (net.uploads != null && editing == null) {
            IconButton(onClick = { attach.launch("*/*") }) { Icon(Icons.Default.Add, "Attach files") }
        }
        IconButton(onClick = {
            gifQuery = null
            picker = true
        }) { Icon(Icons.Default.Face, "Emoji and GIFs") }
        TextField(
            draft, ::change,
            placeholder = { Text("Message ${buf.target}") },
            maxLines = 5,
            shape = RoundedCornerShape(24.dp),
            colors = androidx.compose.material3.TextFieldDefaults.colors(
                focusedIndicatorColor = Color.Transparent,
                unfocusedIndicatorColor = Color.Transparent,
            ),
            modifier = Modifier.weight(1f),
        )
        IconButton(enabled = draft.text.isNotBlank(), onClick = ::submit) {
            Icon(if (editing != null) Icons.Default.Check else Icons.AutoMirrored.Filled.Send, if (editing != null) "Save" else "Send")
        }
    }
}

/** Desktop's `/gif [search]`: the search, or `null` when it's not that. */
internal fun gifCommand(text: String): String? {
    val t = text.trim()
    val cmd = t.substringBefore(' ').substringBefore('\t')
    return if (cmd.equals("/gif", ignoreCase = true)) t.drop(cmd.length).trim() else null
}

/** Joins voice once the mic (and headset, notification) permissions are in. */
@Composable
private fun rememberVoiceJoin(app: IrChad): (String, String) -> Unit {
    val ctx = LocalContext.current
    var pending by remember { mutableStateOf<Pair<String, String>?>(null) }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        val (network, channel) = pending ?: return@rememberLauncherForActivityResult
        pending = null
        if (ContextCompat.checkSelfPermission(ctx, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
            app.client.joinVoice(network, channel)
        } else {
            Toast.makeText(ctx, "Voice needs the microphone.", Toast.LENGTH_LONG).show()
        }
    }
    return { network, channel ->
        val missing = buildList {
            add(Manifest.permission.RECORD_AUDIO)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) add(Manifest.permission.BLUETOOTH_CONNECT)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) add(Manifest.permission.POST_NOTIFICATIONS)
        }.filter { ContextCompat.checkSelfPermission(ctx, it) != PackageManager.PERMISSION_GRANTED }
        if (missing.isEmpty()) {
            app.client.joinVoice(network, channel)
        } else {
            pending = network to channel
            ask.launch(missing.toTypedArray())
        }
    }
}

/** Above the composer while in a call on any network. */
@Composable
private fun VoicePanel(app: IrChad, nets: List<Network>, call: CallState) {
    val net = nets.firstOrNull { it.voice != null } ?: return
    val calls = app.calls
    Surface(color = MaterialTheme.colorScheme.surfaceContainerHigh) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    val ok = call.connected && !call.reconnecting
                    Text(
                        when {
                            !call.connected -> "Connecting…"
                            call.reconnecting -> "Reconnecting…"
                            else -> "Voice connected"
                        },
                        style = MaterialTheme.typography.titleSmall,
                        color = if (ok) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.tertiary,
                    )
                    val quality = when (call.quality) {
                        ConnectionQuality.EXCELLENT -> "Excellent connection"
                        ConnectionQuality.GOOD -> "Good connection"
                        ConnectionQuality.POOR, ConnectionQuality.LOST -> "Poor connection"
                        else -> null
                    }.takeIf { ok }
                    Text(
                        listOfNotNull(net.voice, net.name, quality).joinToString(" · "),
                        style = MaterialTheme.typography.bodySmall,
                        color = if (call.quality == ConnectionQuality.POOR || call.quality == ConnectionQuality.LOST)
                            MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                if (call.outputs.size > 1) {
                    var routes by remember { mutableStateOf(false) }
                    Box {
                        IconButton(onClick = { routes = true }) {
                            Icon(painterResource(R.drawable.ic_speaker), "Speakers: ${call.output?.name ?: "default"}")
                        }
                        DropdownMenu(expanded = routes, onDismissRequest = { routes = false }) {
                            for (d in call.outputs) {
                                DropdownMenuItem(
                                    text = { Text(d.name) },
                                    leadingIcon = if (d == call.output) {
                                        { Icon(Icons.Default.Check, "In use") }
                                    } else null,
                                    onClick = {
                                        calls.output(d)
                                        routes = false
                                    },
                                )
                            }
                        }
                    }
                }
                FilledIconToggleButton(checked = call.muted, onCheckedChange = { calls.setMuted(it) }) {
                    Icon(
                        painterResource(if (call.muted) R.drawable.ic_mic_off else R.drawable.ic_mic),
                        if (call.muted) "Unmute" else "Mute",
                    )
                }
                FilledIconToggleButton(checked = call.deafened, onCheckedChange = { calls.setDeafened(it) }) {
                    Icon(painterResource(R.drawable.ic_headset), if (call.deafened) "Undeafen" else "Deafen")
                }
                IconButton(enabled = call.connected, onClick = { app.sounds.open(net.network) }) {
                    Icon(painterResource(R.drawable.ic_soundboard), "Soundboard")
                }
                FilledIconButton(
                    onClick = { app.client.leaveVoice(net.network) },
                    colors = IconButtonDefaults.filledIconButtonColors(
                        containerColor = MaterialTheme.colorScheme.error,
                        contentColor = MaterialTheme.colorScheme.onError,
                    ),
                ) { Icon(painterResource(R.drawable.ic_call_end), "Leave voice") }
            }
            if (app.settings.pushToTalk && call.connected && !call.muted && !call.deafened) {
                var down by remember { mutableStateOf(false) }
                Surface(
                    color = if (down) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.secondaryContainer,
                    contentColor = if (down) MaterialTheme.colorScheme.onPrimary else MaterialTheme.colorScheme.onSecondaryContainer,
                    shape = RoundedCornerShape(24.dp),
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(vertical = 4.dp)
                        .semantics { contentDescription = "Push to talk: hold to talk" }
                        .pointerInput(Unit) {
                            detectTapGestures(onPress = {
                                down = true
                                calls.mic.held = true
                                try {
                                    tryAwaitRelease()
                                } finally {
                                    calls.mic.held = false
                                    down = false
                                }
                            })
                        },
                ) {
                    Text(
                        if (down) "Talking…" else "Hold to talk",
                        Modifier.padding(vertical = 12.dp),
                        textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                    )
                }
            }
            var volumeOf by remember { mutableStateOf<Person?>(null) }
            LazyRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                items(call.people) { p -> PersonChip(p, onWatch = { calls.watch(p.identity) }, onVolume = { volumeOf = p }) }
            }
            volumeOf?.let { p -> VolumeDialog(app, p) { volumeOf = null } }
        }
    }
}

/**
 * Someone in the call: tap to watch their share, hold for their volume.
 * A chip can't take a long press, so this draws its own.
 */
@OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
@Composable
private fun PersonChip(p: Person, onWatch: () -> Unit, onVolume: () -> Unit) {
    val watchable = p.sharing && !p.local
    Surface(
        shape = RoundedCornerShape(8.dp),
        border = BorderStroke(
            if (p.speaking) 2.dp else 1.dp,
            if (p.speaking) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outlineVariant,
        ),
        modifier = Modifier.combinedClickable(
            enabled = !p.local,
            onClickLabel = if (watchable) "Watch their screen" else null,
            onClick = { if (watchable) onWatch() else onVolume() },
            onLongClickLabel = "Volume",
            onLongClick = onVolume,
        ),
    ) {
        Row(Modifier.height(32.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            if (p.muted) Icon(painterResource(R.drawable.ic_mic_off), "Muted", Modifier.size(16.dp).padding(end = 4.dp))
            Text(p.identity, style = MaterialTheme.typography.labelLarge)
            if (p.sharing) {
                Icon(
                    painterResource(R.drawable.ic_screen), if (watchable) "Watch their screen" else "Sharing",
                    Modifier.padding(start = 4.dp).size(16.dp),
                )
            }
        }
    }
}

/** How loud someone, and their screen share, are for us. */
@Composable
private fun VolumeDialog(app: IrChad, p: Person, onDismiss: () -> Unit) {
    var voice by remember(p.identity) { mutableStateOf(app.settings.volume(p.identity, false)) }
    var screen by remember(p.identity) { mutableStateOf(app.settings.volume(p.identity, true)) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(p.identity) },
        text = {
            Column {
                Percent("Volume", voice) {
                    voice = it
                    app.calls.setVolume(p.identity, false, it)
                }
                if (p.sharing) {
                    Percent("Screen share volume", screen) {
                        screen = it
                        app.calls.setVolume(p.identity, true, it)
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Done") } },
    )
}

/** The soundboard's buttons (D37): a clip goes out to everyone in the call. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SoundboardSheet(app: IrChad) {
    val board by app.sounds.board.collectAsState()
    val b = board ?: return
    ModalBottomSheet(onDismissRequest = { app.sounds.board.value = null }) {
        Column(Modifier.navigationBarsPadding().padding(horizontal = 16.dp, vertical = 8.dp)) {
            Text("Soundboard", style = MaterialTheme.typography.titleMedium)
            when {
                b.error != null -> Text(b.error, color = MaterialTheme.colorScheme.error)
                b.clips == null -> CircularProgressIndicator(Modifier.padding(16.dp))
                b.clips.isEmpty() -> Text("This network has no sounds.", color = MaterialTheme.colorScheme.onSurfaceVariant)
                else -> androidx.compose.foundation.layout.FlowRow(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    modifier = Modifier.padding(vertical = 8.dp),
                ) {
                    for (c in b.clips) AssistChip(onClick = { app.sounds.play(b.network, c.file) }, label = { Text(c.name) })
                }
            }
        }
    }
}

/** Someone's screen share, over everything (or alone, in picture-in-picture). */
@Composable
private fun ScreenViewer(app: IrChad, call: CallState, pip: Boolean) {
    BackHandler(!pip) { app.calls.watch(null) }
    val activity = LocalContext.current as? android.app.Activity
    var landscape by rememberSaveable { mutableStateOf(false) }
    DisposableEffect(landscape) {
        activity?.requestedOrientation = if (landscape) ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
        else ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
        onDispose { activity?.requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED }
    }
    var scale by remember { mutableStateOf(1f) }
    var offset by remember { mutableStateOf(androidx.compose.ui.geometry.Offset.Zero) }
    val zoom = rememberTransformableState { z, pan, _ ->
        scale = (scale * z).coerceIn(1f, 6f)
        offset = if (scale == 1f) androidx.compose.ui.geometry.Offset.Zero else offset + pan
    }
    if (pip) {
        scale = 1f
        offset = androidx.compose.ui.geometry.Offset.Zero
    }
    Box(
        Modifier.fillMaxSize().background(Color.Black)
            .pointerInput(Unit) {
                detectTapGestures(onDoubleTap = { scale = 1f; offset = androidx.compose.ui.geometry.Offset.Zero })
            }
            .transformable(zoom, enabled = !pip),
        contentAlignment = Alignment.Center,
    ) {
        val track = call.screen
        if (track == null) {
            CircularProgressIndicator()
        } else {
            key(track) {
                // The frame's shape, for picture-in-picture.
                val sink = remember(track) {
                    livekit.org.webrtc.VideoSink { f -> app.calls.frame.value = f.rotatedWidth to f.rotatedHeight }
                }
                DisposableEffect(track) {
                    track.addRenderer(sink)
                    onDispose { track.removeRenderer(sink) }
                }
                AndroidView(
                    factory = { ctx ->
                        TextureViewRenderer(ctx).apply {
                            app.calls.room()?.initVideoRenderer(this)
                            setScalingType(RendererCommon.ScalingType.SCALE_ASPECT_FIT)
                            track.addRenderer(this)
                        }
                    },
                    onRelease = {
                        track.removeRenderer(it)
                        it.release()
                    },
                    // Not fillMaxSize: the renderer crops to its view, so let it measure to the frame's shape.
                    // A TextureView follows the layer's transform, which is what makes zoom work.
                    modifier = Modifier.graphicsLayer {
                        scaleX = scale
                        scaleY = scale
                        translationX = offset.x
                        translationY = offset.y
                    },
                )
            }
        }
        if (!pip) {
            Row(
                Modifier.align(Alignment.TopStart).fillMaxWidth().statusBarsPadding().padding(8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    "${call.watching}'s screen", color = Color.White,
                    modifier = Modifier.weight(1f).padding(start = 8.dp),
                )
                IconButton(onClick = { landscape = !landscape }) {
                    Icon(
                        painterResource(R.drawable.ic_rotate),
                        if (landscape) "Back to portrait" else "Turn to landscape", tint = Color.White,
                    )
                }
                IconButton(onClick = { app.calls.watch(null) }) {
                    Icon(Icons.Default.Close, "Stop watching", tint = Color.White)
                }
            }
        }
    }
}
