package party.gentoo.irchad

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material.icons.filled.Call
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.AssistChip
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
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.material.icons.filled.Face
import androidx.compose.material.icons.filled.Settings
import androidx.compose.ui.text.LinkAnnotation
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
    // The message being answered, in the buffer on screen.
    var replyTo by remember(net.network, buf?.target) { mutableStateOf<Message?>(null) }

    BackHandler(drawer.isOpen) { scope.launch { drawer.close() } }

    ModalNavigationDrawer(
        drawerState = drawer,
        drawerContent = {
            ModalDrawerSheet {
                Drawer(
                    nets, net, buffers, buf?.target,
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
                    Log(app, tick, net, buf, Modifier.weight(1f), onReply = { replyTo = it })
                }
                call?.let { VoicePanel(app, nets, it) }
                if (buf != null) Composer(app, net, buf, replyTo) { replyTo = null }
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
                items(list.orEmpty(), key = { it.nick }) { m ->
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
                        Avatar(m.nick)
                        Spacer(Modifier.width(12.dp))
                        Text(
                            m.prefix + m.shown,
                            color = if (m.away) MaterialTheme.colorScheme.onSurfaceVariant
                            else MaterialTheme.colorScheme.onSurface,
                        )
                    }
                }
            }
        }
    }

    if (dialog == "settings") AutoLoadDialog(app) { dialog = null } else dialog?.let { kind ->
        var name by remember { mutableStateOf(if (kind == "channel") "#" else "") }
        AlertDialog(
            onDismissRequest = { dialog = null },
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
                    selected[net.network] = n
                    dialog = null
                }) { Text(if (kind == "channel") "Join" else "Open") }
            },
            dismissButton = { TextButton(onClick = { dialog = null }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun Drawer(
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
private fun Log(app: IrChad, tick: Long, net: Network, buf: Buffer, modifier: Modifier, onReply: (Message) -> Unit) {
    val c = app.client
    val msgs = remember(tick, net.network, buf.target) { c.messages(net.network, buf.target) }
    val connected = net.status == null
    // Newest at the bottom: the list is reversed so it starts there.
    val list = rememberLazyListState()
    var menu by remember { mutableStateOf<Message?>(null) }
    var reactTo by remember { mutableStateOf<String?>(null) }
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
                        app, net.network, m, head, buf.dm, parent, onLong = { menu = m },
                        onParent = { at[m.replyTo]?.let { scope.launch { list.animateScrollToItem(it) } } },
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
                        if (m.own && m.msgid != null && !m.redacted) {
                            DropdownMenuItem(text = { Text("Delete") }, onClick = {
                                c.redact(net.network, buf.target, m.msgid!!)
                                menu = null
                            })
                        }
                    }
                }
            }
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
    app: IrChad, network: String, m: Message, head: Boolean, dm: Boolean, parent: Message?,
    onLong: () -> Unit, onParent: () -> Unit,
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
        if (head) Avatar(m.nick) else Spacer(Modifier.width(36.dp))
        Spacer(Modifier.width(10.dp))
        Column(Modifier.weight(1f)) {
            if (m.replyTo != null) {
                Text(
                    "↪ " + (parent?.let { p -> "${p.shown}: " + (if (p.redacted) "Message deleted" else p.text) }
                        ?: "Replying to an earlier message"),
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1, overflow = TextOverflow.Ellipsis,
                    // shortcut: only jumps to a parent that's loaded; desktop's LoadAround isn't in chad-ffi.
                    modifier = Modifier.clickable(enabled = parent != null, onClick = onParent),
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
            when {
                m.redacted -> Text("Message deleted", fontStyle = FontStyle.Italic, color = faint)
                m.undecryptable -> Text("Couldn't decrypt this message.", fontStyle = FontStyle.Italic, color = MaterialTheme.colorScheme.error)
                m.action -> Text(
                    styled(m.spans, MaterialTheme.colorScheme.primary, prefix = "* ${m.shown} "),
                    fontStyle = FontStyle.Italic,
                )
                else -> Text(
                    styled(m.spans, MaterialTheme.colorScheme.primary),
                    color = if (m.pending) faint else if (m.notice) faint else Color.Unspecified,
                )
            }
            MessageLinks(app, network, m)
            m.failed?.let { Text("Not sent: $it", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.error) }
            if (m.reactions.isNotEmpty()) {
                Row(horizontalArrangement = Arrangement.spacedBy(4.dp), modifier = Modifier.padding(top = 2.dp)) {
                    for (r in m.reactions) {
                        Text(
                            "${r.emoji} ${r.nicks.size}",
                            style = MaterialTheme.typography.labelMedium,
                            modifier = Modifier
                                .clip(RoundedCornerShape(10.dp))
                                .background(MaterialTheme.colorScheme.surfaceVariant)
                                .padding(horizontal = 8.dp, vertical = 2.dp),
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun Avatar(nick: String) {
    Box(
        Modifier
            .size(36.dp)
            .clip(CircleShape)
            .background(nickColor(nick).copy(alpha = 0.25f)),
        contentAlignment = Alignment.Center,
    ) {
        Text(nick.take(1).uppercase(), fontWeight = FontWeight.Bold, color = nickColor(nick))
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

/** Message text with its IRC formatting, and its links tappable. */
private fun styled(spans: List<Span>, link: Color, prefix: String = "") = buildAnnotatedString {
    append(prefix)
    for (sp in spans) {
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
        if (sp.link) {
            withLink(LinkAnnotation.Url(sp.text, TextLinkStyles(style))) { append(sp.text) }
        } else {
            withStyle(style) { append(sp.text) }
        }
    }
}

private fun rgb(c: UInt) = Color(0xFF000000.toInt() or c.toInt())

private fun Color.luminance(): Float = 0.2126f * red + 0.7152f * green + 0.0722f * blue

@Composable
private fun Composer(app: IrChad, net: Network, buf: Buffer, replyTo: Message?, onReplied: () -> Unit) {
    var draft by rememberSaveable(net.network, buf.target, stateSaver = TextFieldValue.Saver) {
        mutableStateOf(TextFieldValue())
    }
    val ctx = LocalContext.current
    val attach = rememberLauncherForActivityResult(ActivityResultContracts.GetMultipleContents()) { uris ->
        if (uris.isNotEmpty()) upload(app, ctx, net, buf.target, uris)
    }
    if (replyTo != null) {
        Surface(color = MaterialTheme.colorScheme.surfaceContainerHigh) {
            Row(Modifier.fillMaxWidth().padding(start = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(
                    "Replying to ${replyTo.shown}: ${replyTo.text}",
                    style = MaterialTheme.typography.labelMedium, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                IconButton(onClick = onReplied) { Icon(Icons.Default.Close, "Don't reply") }
            }
        }
    }
    var picker by remember { mutableStateOf(false) }
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
                app.client.send(net.network, buf.target, it, null)
                picker = false
            },
            onDismiss = { picker = false },
        )
    }
    Row(
        Modifier
            .fillMaxWidth()
            .navigationBarsPadding()
            .padding(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (net.uploads != null) {
            IconButton(onClick = { attach.launch("*/*") }) { Icon(Icons.Default.Add, "Attach files") }
        }
        IconButton(onClick = { picker = true }) { Icon(Icons.Default.Face, "Emoji and GIFs") }
        TextField(
            draft, { draft = it },
            placeholder = { Text("Message ${buf.target}") },
            maxLines = 5,
            shape = RoundedCornerShape(24.dp),
            colors = androidx.compose.material3.TextFieldDefaults.colors(
                focusedIndicatorColor = Color.Transparent,
                unfocusedIndicatorColor = Color.Transparent,
            ),
            modifier = Modifier.weight(1f),
        )
        IconButton(
            enabled = draft.text.isNotBlank(),
            onClick = {
                app.client.send(net.network, buf.target, draft.text.trimEnd(), replyTo?.msgid)
                draft = TextFieldValue()
                onReplied()
            },
        ) { Icon(Icons.AutoMirrored.Filled.Send, "Send") }
    }
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
                    Text(
                        "${net.voice} · ${net.name}",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
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
            LazyRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                items(call.people) { p ->
                    val watchable = p.sharing && !p.local
                    AssistChip(
                        onClick = { if (watchable) calls.watch(p.identity) },
                        label = { Text(p.identity) },
                        leadingIcon = if (p.muted) {
                            { Icon(painterResource(R.drawable.ic_mic_off), "Muted", Modifier.size(16.dp)) }
                        } else null,
                        trailingIcon = if (p.sharing) {
                            { Icon(painterResource(R.drawable.ic_screen), if (watchable) "Watch their screen" else "Sharing", Modifier.size(16.dp)) }
                        } else null,
                        border = BorderStroke(
                            if (p.speaking) 2.dp else 1.dp,
                            if (p.speaking) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outlineVariant,
                        ),
                    )
                }
            }
        }
    }
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
