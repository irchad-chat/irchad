package party.gentoo.irchad

import android.content.Context
import android.graphics.ImageDecoder
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import androidx.compose.material3.TextButton
import android.graphics.drawable.Animatable
import android.graphics.drawable.Drawable
import android.util.LruCache
import android.widget.ImageView
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.rememberTransformableState
import androidx.compose.foundation.gestures.transformable
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.ui.Alignment
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.staggeredgrid.LazyVerticalStaggeredGrid
import androidx.compose.foundation.lazy.staggeredgrid.StaggeredGridCells
import androidx.compose.foundation.lazy.staggeredgrid.items
import androidx.compose.foundation.lazy.staggeredgrid.rememberLazyStaggeredGridState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.PrimaryTabRow
import androidx.compose.material3.Surface
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import party.gentoo.irchad.core.GifPage
import party.gentoo.irchad.core.Link
import party.gentoo.irchad.core.LinkKind
import party.gentoo.irchad.core.LinkPreview
import party.gentoo.irchad.core.Message
import party.gentoo.irchad.core.emojis
import java.nio.ByteBuffer

/** Decoded images are scaled to fit this many pixels a side. */
private const val MAX_PIXELS = 800
private const val MAX_IMAGES = 3
/** Larger sources are refused, not scaled: sampling still reads every pixel. */
private const val MAX_SIDE = 8192
/** Animated sources decode every frame at full size while they play. */
private const val MAX_ANIMATED = 2048L * 2048
/** What decoded images may hold in memory, all together. */
private const val CACHE_BYTES = 64 shl 20

/**
 * Images and link previews for the chat. The core fetches them (own uploads
 * directly, the rest through chadd, D25), and decoding is bounded (D14).
 */
class Media(private val app: IrChad) {
    /** When images and link previews load without a tap (D25). */
    enum class AutoLoad { ALWAYS, UNMETERED, OFF }

    private val prefs = app.getSharedPreferences("settings", Context.MODE_PRIVATE)
    private var saved by mutableStateOf(
        runCatching { AutoLoad.valueOf(prefs.getString("auto_load", null)!!) }.getOrDefault(AutoLoad.ALWAYS),
    )
    var autoLoad: AutoLoad
        get() = saved
        set(a) {
            saved = a
            prefs.edit().putString("auto_load", a.name).apply()
        }

    private var savedGuests by mutableStateOf(prefs.getBoolean("guest_media", true))

    /** Images and previews in guests' messages; off, their links stay plain links. */
    var guests: Boolean
        get() = savedGuests
        set(g) {
            savedGuests = g
            prefs.edit().putBoolean("guest_media", g).apply()
        }

    // Wi-Fi or Ethernet, as Android counts it; false until the first callback.
    private var unmetered by mutableStateOf(false)

    init {
        app.getSystemService(ConnectivityManager::class.java).registerDefaultNetworkCallback(
            object : ConnectivityManager.NetworkCallback() {
                override fun onCapabilitiesChanged(n: Network, caps: NetworkCapabilities) {
                    unmetered = caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)
                }

                override fun onLost(n: Network) {
                    unmetered = false
                }
            },
        )
    }

    /** Whether images and previews load without a tap right now. */
    val auto: Boolean
        get() = when (autoLoad) {
            AutoLoad.ALWAYS -> true
            AutoLoad.UNMETERED -> unmetered
            AutoLoad.OFF -> false
        }

    sealed interface Img {
        data object Loading : Img
        /** `bytes`: what it holds in memory, roughly. */
        class Ready(val drawable: Drawable, val bytes: Int) : Img
        data object Failed : Img
    }

    sealed interface Prev {
        data object Loading : Prev
        class Ready(val p: LinkPreview) : Prev
        data object Failed : Prev
    }

    /** Bumped when something arrives: read the caches again. */
    val tick = MutableStateFlow(0L)
    val gifs = MutableSharedFlow<Triple<String, String?, GifPage>>(extraBufferCapacity = 8)

    // Keyed by "network url": what's fetchable differs per network.
    private val images = object : LruCache<String, Img>(CACHE_BYTES) {
        override fun sizeOf(key: String, value: Img) = (value as? Img.Ready)?.bytes ?: 1
    }
    private val previews = LruCache<String, Prev>(128)
    private val decoder = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    /** `null` when it isn't loaded and `ask` is off. */
    fun image(network: String, url: String, ask: Boolean): Img? = synchronized(images) {
        images.get("$network $url") ?: if (!ask) null else Img.Loading.also {
            images.put("$network $url", it)
            app.client.media(network, url)
        }
    }

    /** The network's icon (`Network.icon`), always fetched, like desktop's. */
    fun icon(network: String, url: String): Img = synchronized(images) {
        images.get("$network $url") ?: Img.Loading.also {
            images.put("$network $url", it)
            app.client.icon(network)
        }
    }

    /** `null` when it isn't loaded and `ask` is off. */
    fun preview(network: String, url: String, ask: Boolean): Prev? = synchronized(previews) {
        previews.get("$network $url") ?: if (!ask) null else Prev.Loading.also {
            previews.put("$network $url", it)
            app.client.preview(network, url)
        }
    }

    internal fun gotPreview(network: String, url: String, p: LinkPreview?) {
        synchronized(previews) { previews.put("$network $url", p?.let { Prev.Ready(it) } ?: Prev.Failed) }
        tick.value += 1
    }

    internal fun gotMedia(network: String, url: String, bytes: ByteArray?) {
        decoder.launch {
            val img = bytes?.let(::decode) ?: Img.Failed
            synchronized(images) { images.put("$network $url", img) }
            tick.value += 1
        }
    }

    // Server-sent bytes (D14): only the formats desktop and chadd take, bounded
    // in size, and any failure only loses the image.
    private fun decode(bytes: ByteArray): Img.Ready? = if (!isImage(bytes)) null else try {
        var animated = false
        val d = ImageDecoder.decodeDrawable(ImageDecoder.createSource(ByteBuffer.wrap(bytes))) { d, info, _ ->
            val (w, h) = info.size.width to info.size.height
            animated = info.isAnimated
            if (w > MAX_SIDE || h > MAX_SIDE || animated && w.toLong() * h > MAX_ANIMATED) {
                throw java.io.IOException("too big")
            }
            val big = maxOf(w, h)
            if (big > MAX_PIXELS) d.setTargetSize(maxOf(1, w * MAX_PIXELS / big), maxOf(1, h * MAX_PIXELS / big))
        }
        // An animated drawable keeps its source and decodes frames from it.
        val held = d.intrinsicWidth * d.intrinsicHeight * 4 * (if (animated) 2 else 1)
        Img.Ready(d, held + if (animated) bytes.size else 0)
    } catch (e: Exception) {
        null
    } catch (e: OutOfMemoryError) {
        null
    }
}

/** PNG, JPEG, GIF or WebP by magic bytes. */
internal fun isImage(b: ByteArray): Boolean {
    fun at(i: Int, vararg m: Int) = b.size >= i + m.size && m.indices.all { b[i + it] == m[it].toByte() }
    return at(0, 0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A) || at(0, 0xFF, 0xD8, 0xFF) ||
        at(0, 0x47, 0x49, 0x46, 0x38) || at(0, 0x52, 0x49, 0x46, 0x46) && at(8, 0x57, 0x45, 0x42, 0x50)
}

/**
 * A fetched image, scaled to fit `maxW` × `maxH`, or cropped to fill
 * `modifier` with `fill`; a placeholder while loading.
 */
@Composable
fun RemoteImage(
    app: IrChad, network: String, url: String, maxW: Dp, maxH: Dp,
    modifier: Modifier = Modifier, fill: Boolean = false, ask: Boolean = app.media.auto,
    onOpen: ((Drawable) -> Unit)? = null,
) {
    val tick by app.media.tick.collectAsState()
    var tapped by remember(network, url) { mutableStateOf(false) }
    val img = remember(tick, network, url, ask || tapped) { app.media.image(network, url, ask || tapped) }
    val shape = RoundedCornerShape(8.dp)
    when (img) {
        null -> TextButton(onClick = { tapped = true }, modifier = modifier) { Text("Show image") }
        Media.Img.Loading -> if (!fill) Box(
            modifier.size(maxW.coerceAtMost(160.dp), 90.dp).clip(shape)
                .background(MaterialTheme.colorScheme.surfaceVariant),
        )
        Media.Img.Failed -> {}
        is Media.Img.Ready -> {
            val d = img.drawable
            val px = with(LocalDensity.current) { 1.dp.toPx() }
            val (w, h) = d.intrinsicWidth.coerceAtLeast(1) / px to d.intrinsicHeight.coerceAtLeast(1) / px
            val scale = minOf(1f, maxW.value / w, maxH.value / h)
            AndroidView(
                factory = {
                    ImageView(it).apply {
                        scaleType = if (fill) ImageView.ScaleType.CENTER_CROP else ImageView.ScaleType.FIT_CENTER
                    }
                },
                // shortcut: a drawable animates in the last view it was set on, so the
                // same GIF twice on screen animates once; decode per view if that matters.
                update = {
                    if (it.drawable !== d) {
                        it.setImageDrawable(d)
                        (d as? Animatable)?.start()
                    }
                    // The view takes the touches, so the click goes on it.
                    it.setOnClickListener(onOpen?.let { open -> { open(d) } })
                    it.isClickable = onOpen != null
                },
                modifier = (if (fill) modifier else modifier.size((w * scale).dp, (h * scale).dp)).clip(shape),
            )
        }
    }
}

/** Images inline, videos and files as cards, and a preview of the first page. */
@Composable
fun MessageLinks(app: IrChad, network: String, m: Message) {
    if (m.notice || m.redacted || m.undecryptable || m.links.isEmpty()) return
    if (!app.media.guests && m.account == null && !m.own) return
    val pages = m.links.filter { it.kind == LinkKind.PAGE }.take(1)
    val images = m.links.filter { it.kind == LinkKind.IMAGE }.take(MAX_IMAGES)
    var open by remember { mutableStateOf<Drawable?>(null) }
    open?.let { ImageViewer(it) { open = null } }
    Column(Modifier.padding(top = 4.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        for (l in images) RemoteImage(app, network, l.url, 280.dp, 220.dp, onOpen = { open = it })
        for (l in m.links.filter { it.kind == LinkKind.VIDEO || it.kind == LinkKind.FILE }) FileCard(l)
        for (l in pages) LinkPreviewCard(app, network, l.url)
    }
}

/**
 * An image over everything: pinch to zoom, drag to pan, double-tap to reset.
 */
// shortcut: shows the inline decode (800 px a side); decode the bytes again
// larger, like desktop's viewer, if that looks soft on big screens.
@Composable
fun ImageViewer(d: Drawable, onDismiss: () -> Unit) {
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        var scale by remember { mutableStateOf(1f) }
        var offset by remember { mutableStateOf(Offset.Zero) }
        val zoom = rememberTransformableState { z, pan, _ ->
            scale = (scale * z).coerceIn(1f, 8f)
            offset = if (scale == 1f) Offset.Zero else offset + pan
        }
        Box(
            Modifier.fillMaxSize().background(Color.Black)
                .pointerInput(Unit) {
                    detectTapGestures(onDoubleTap = { scale = 1f; offset = Offset.Zero }, onTap = { if (scale == 1f) onDismiss() })
                }
                .transformable(zoom),
        ) {
            AndroidView(
                factory = { ImageView(it).apply { scaleType = ImageView.ScaleType.FIT_CENTER } },
                update = {
                    if (it.drawable !== d) {
                        it.setImageDrawable(d)
                        (d as? Animatable)?.start()
                    }
                },
                modifier = Modifier.fillMaxSize().graphicsLayer {
                    scaleX = scale
                    scaleY = scale
                    translationX = offset.x
                    translationY = offset.y
                },
            )
            IconButton(onClick = onDismiss, modifier = Modifier.align(Alignment.TopEnd).statusBarsPadding().padding(8.dp)) {
                Icon(Icons.Default.Close, "Close", tint = Color.White)
            }
        }
    }
}

/** There's no player: a video or file opens outside. */
@Composable
private fun FileCard(l: Link) {
    val uri = LocalUriHandler.current
    val name = l.url.substringBefore('?').substringAfterLast('/').ifEmpty { l.url }
    Surface(
        shape = RoundedCornerShape(8.dp),
        color = MaterialTheme.colorScheme.surfaceVariant,
        modifier = Modifier.clickable { uri.openUri(l.url) },
    ) {
        Text(
            (if (l.kind == LinkKind.VIDEO) "▶  " else "📄  ") + name,
            color = MaterialTheme.colorScheme.primary, maxLines = 1, overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(horizontal = 12.dp, vertical = 10.dp),
        )
    }
}

@Composable
private fun LinkPreviewCard(app: IrChad, network: String, url: String) {
    val tick by app.media.tick.collectAsState()
    var tapped by remember(network, url) { mutableStateOf(false) }
    val ask = app.media.auto || tapped
    val got = remember(tick, network, url, ask) { app.media.preview(network, url, ask) }
    if (got == null) {
        TextButton(onClick = { tapped = true }) { Text("Show link preview") }
        return
    }
    val p = (got as? Media.Prev.Ready)?.p ?: return
    if (p.title == null && p.description == null && p.image == null) return
    val uri = LocalUriHandler.current
    Row(
        Modifier
            .widthIn(max = 320.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .clickable { uri.openUri(url) },
    ) {
        Box(Modifier.size(4.dp, 1.dp).background(MaterialTheme.colorScheme.primary))
        Column(Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            p.site?.let {
                Text(it, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            p.title?.let {
                Text(it, fontWeight = FontWeight.SemiBold, color = MaterialTheme.colorScheme.primary, maxLines = 2,
                    overflow = TextOverflow.Ellipsis)
            }
            p.description?.let {
                Text(it, style = MaterialTheme.typography.bodySmall, maxLines = 3, overflow = TextOverflow.Ellipsis)
            }
            p.image?.let { RemoteImage(app, network, it, 300.dp, 160.dp, Modifier.padding(top = 4.dp), ask = ask) }
        }
    }
}

/**
 * Emoji, and GIFs when `provider` is set (the network has search). Emoji
 * stay open for more; a GIF closes it. `gifQuery` opens on the GIFs, already
 * searching (desktop's `/gif`).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun Picker(
    app: IrChad,
    network: String,
    provider: String?,
    onEmoji: (String) -> Unit,
    onGif: (String) -> Unit,
    onDismiss: () -> Unit,
    gifQuery: String? = null,
) {
    var tab by rememberSaveable { mutableStateOf(if (gifQuery != null) 1 else 0) }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(Modifier.navigationBarsPadding().fillMaxWidth().height(480.dp)) {
            if (provider != null) {
                PrimaryTabRow(selectedTabIndex = tab) {
                    Tab(tab == 0, { tab = 0 }, text = { Text("Emoji") })
                    Tab(tab == 1, { tab = 1 }, text = { Text("GIFs") })
                }
            }
            if (tab == 1 && provider != null) GifTab(app, network, provider, gifQuery.orEmpty(), onGif) else EmojiTab(onEmoji)
        }
    }
}

@Composable
private fun ColumnScope.EmojiTab(onEmoji: (String) -> Unit) {
    var query by rememberSaveable { mutableStateOf("") }
    val list = remember(query) { emojis(query) }
    OutlinedTextField(
        query, { query = it }, singleLine = true, placeholder = { Text("Find an emoji") },
        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp),
    )
    LazyVerticalGrid(GridCells.Adaptive(48.dp), Modifier.weight(1f).padding(horizontal = 8.dp)) {
        items(list) { e ->
            Box(
                Modifier.size(48.dp).clip(RoundedCornerShape(8.dp)).clickable { onEmoji(e) },
                contentAlignment = androidx.compose.ui.Alignment.Center,
            ) { Text(e, style = MaterialTheme.typography.headlineSmall) }
        }
    }
}

@Composable
private fun ColumnScope.GifTab(app: IrChad, network: String, provider: String, initial: String, onGif: (String) -> Unit) {
    var query by rememberSaveable { mutableStateOf(initial) }
    val gifs = remember { mutableStateListOf<party.gentoo.irchad.core.Gif>() }
    var next by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    // The `pos` asked for and not answered yet; "" for the first page.
    var asked by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(query) {
        delay(300)
        gifs.clear()
        next = null
        error = null
        asked = ""
        app.client.gifs(network, query.trim(), null)
    }
    LaunchedEffect(query) {
        app.media.gifs.collect { (q, pos, page) ->
            if (q != query.trim() || (pos ?: "") != asked) return@collect
            asked = null
            gifs.addAll(page.gifs)
            next = page.next
            error = page.error
        }
    }
    val grid = rememberLazyStaggeredGridState()
    val nearEnd by remember {
        derivedStateOf { (grid.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0) >= grid.layoutInfo.totalItemsCount - 6 }
    }
    LaunchedEffect(nearEnd, next, asked) {
        val pos = next
        if (nearEnd && pos != null && asked == null && gifs.isNotEmpty()) {
            asked = pos
            app.client.gifs(network, query.trim(), pos)
        }
    }

    OutlinedTextField(
        query, { query = it }, singleLine = true, placeholder = { Text("Search GIFs") },
        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp),
    )
    error?.let { Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(horizontal = 16.dp)) }
    LazyVerticalStaggeredGrid(
        StaggeredGridCells.Fixed(2), Modifier.weight(1f).padding(horizontal = 8.dp), state = grid,
        verticalItemSpacing = 4.dp, horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        items(gifs, key = { it.url }) { g ->
            val ratio = if (g.width > 0u && g.height > 0u) g.width.toFloat() / g.height.toFloat() else 1f
            Box(
                Modifier.fillMaxWidth().aspectRatio(ratio.coerceIn(0.4f, 3f)).clip(RoundedCornerShape(6.dp))
                    .background(MaterialTheme.colorScheme.surfaceVariant).clickable { onGif(g.url) },
            ) { RemoteImage(app, network, g.preview, 0.dp, 0.dp, Modifier.matchParentSize(), fill = true, ask = true) }
        }
    }
    Text(
        "Powered by $provider", style = MaterialTheme.typography.labelSmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(12.dp),
    )
}
