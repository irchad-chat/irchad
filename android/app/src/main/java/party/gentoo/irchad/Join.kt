package party.gentoo.irchad

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import party.gentoo.irchad.core.Network
import party.gentoo.irchad.core.Server
import party.gentoo.irchad.core.parseInvite
import party.gentoo.irchad.core.welcomeCards

/** Discord's blurple. */
private val Blurple = Color(0xFF5865F2)

/** "Sign in with Discord" in Discord's colours, with Phosphor's logo. */
@Composable
fun DiscordButton(onClick: () -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true) {
    Button(
        onClick = onClick,
        enabled = enabled,
        modifier = modifier,
        colors = ButtonDefaults.buttonColors(containerColor = Blurple, contentColor = Color.White),
    ) {
        Icon(painterResource(R.drawable.ic_discord), contentDescription = null, Modifier.size(20.dp))
        Spacer(Modifier.width(8.dp))
        Text("Sign in with Discord")
    }
}

private enum class Mode { Login, Register, Guest }

/**
 * The server's address first (an invite link answers that by itself), then
 * logging in, with signing up and being a guest as the quieter choices the
 * server offers. Closes itself once the network shows up; `onCancel` is null
 * when there's nothing to go back to.
 */
@Composable
fun JoinScreen(
    app: IrChad,
    invite: String?,
    nets: List<Network>,
    onDone: (origin: String, channel: String?) -> Unit,
    onCancel: (() -> Unit)?,
) {
    var link by rememberSaveable { mutableStateOf(invite ?: "irchad.chat") }
    // What the server offers, once it answered: the sign-in step.
    var server by remember { mutableStateOf<Server?>(null) }
    var mode by rememberSaveable { mutableStateOf(Mode.Login) }
    var account by rememberSaveable { mutableStateOf("") }
    var password by rememberSaveable { mutableStateOf("") }
    var email by rememberSaveable { mutableStateOf("") }
    // The invite sent to the core, while waiting for the outcome.
    var busy by rememberSaveable { mutableStateOf<String?>(null) }
    var error by rememberSaveable { mutableStateOf<String?>(null) }
    // `busy` is a Discord sign-in waiting on the browser (D96).
    var discord by rememberSaveable { mutableStateOf(false) }

    val discover = { inv: String ->
        error = null
        server = null
        busy = inv
        app.client.discover(inv)
    }
    // A link names its server: go straight to asking it.
    LaunchedEffect(invite) {
        if (invite != null) {
            link = invite
            discover(invite.trim())
        }
    }
    LaunchedEffect(Unit) {
        app.joinFailures.collect { (inv, e) ->
            if (inv == busy) {
                busy = null
                discord = false
                error = e
            }
        }
    }
    LaunchedEffect(Unit) {
        app.discovered.collect { (inv, s) ->
            if (inv == busy && !discord) {
                busy = null
                link = inv
                server = s
            }
        }
    }
    // Done once the network is up, or waiting on the sign-up email's code;
    // a failed join shows up in `joinFailures` instead.
    LaunchedEffect(nets) {
        val inv = busy?.let { parseInvite(it) } ?: return@LaunchedEffect
        val n = nets.firstOrNull { it.network == inv.origin } ?: return@LaunchedEffect
        if (n.status == null || n.needsCode) onDone(inv.origin, inv.channel)
    }

    Surface(Modifier.fillMaxSize()) {
        Column(
            Modifier
                .safeDrawingPadding()
                .imePadding()
                .verticalScroll(rememberScrollState())
                .padding(24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Spacer(Modifier.size(24.dp))
            val s = server
            if (s == null) {
                Text("Join a network", style = MaterialTheme.typography.headlineMedium)
                Text(
                    "Paste the invite link or server address someone sent you. You'll log in or sign up on the next step.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                OutlinedTextField(
                    link, { link = it },
                    label = { Text("Server address") },
                    singleLine = true,
                    enabled = busy == null,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                    modifier = Modifier.fillMaxWidth(),
                )
                error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                Button(
                    onClick = { discover(link.trim()) },
                    enabled = link.isNotBlank() && busy == null,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    if (busy != null) {
                        CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                    } else {
                        Text("Continue")
                    }
                }
            } else {
                // A choice the server doesn't offer.
                if ((mode == Mode.Register && !s.signUp) || (mode == Mode.Guest && !s.guests)) mode = Mode.Login
                val guest = mode == Mode.Guest
                Text(s.name, style = MaterialTheme.typography.headlineMedium)
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        s.origin.substringAfter("://"),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    TextButton(
                        onClick = {
                            // The password doesn't follow to another server.
                            server = null
                            password = ""
                            error = null
                        },
                        enabled = busy == null,
                    ) { Text("Change") }
                }
                OutlinedTextField(
                    account, { account = it },
                    label = { Text(if (guest) "Nickname" else "Username") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                if (!guest) {
                    OutlinedTextField(
                        password, { password = it },
                        label = { Text("Password") },
                        singleLine = true,
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
                if (mode == Mode.Register) {
                    OutlinedTextField(
                        email, { email = it },
                        label = { Text("Email (if the network asks for one)") },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Email),
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
                if (guest && !s.classic) {
                    Text(
                        "Guests miss what's said while they're away, and anyone can take the nickname.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                if (discord && busy != null) {
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        Text("Finish signing in in your browser.", Modifier.weight(1f))
                        TextButton(onClick = {
                            app.client.discordCancel()
                            busy = null
                            discord = false
                        }) { Text("Cancel") }
                    }
                } else {
                    Button(
                        onClick = {
                            error = null
                            busy = link
                            when (mode) {
                                Mode.Login -> app.client.join(link, account.trim(), password)
                                Mode.Register -> app.client.register(link, account.trim(), password, email.trim())
                                Mode.Guest -> app.client.joinGuest(link, account.trim())
                            }
                        },
                        enabled = busy == null && account.isNotBlank() && (guest || password.isNotEmpty()),
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        if (busy != null) {
                            CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                        } else {
                            Text(
                                when (mode) {
                                    Mode.Login -> "Log in"
                                    Mode.Register -> "Sign up"
                                    Mode.Guest -> "Continue as guest"
                                },
                            )
                        }
                    }
                    if (s.discord && !guest) {
                        DiscordButton(
                            onClick = {
                                error = null
                                busy = link
                                discord = true
                                app.client.discordStart(link)
                            },
                            enabled = busy == null,
                            modifier = Modifier.fillMaxWidth(),
                        )
                    }
                    // The other ways in, quieter than the button; guest quietest.
                    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
                        if (mode != Mode.Login) {
                            Ask("Have an account?", "Log in", busy == null) { mode = Mode.Login; error = null }
                        }
                        if (mode != Mode.Register && s.signUp) {
                            Ask("No account yet?", "Sign up", busy == null) { mode = Mode.Register; error = null }
                        }
                        if (!guest && s.guests) {
                            TextButton(
                                onClick = { mode = Mode.Guest; error = null },
                                enabled = busy == null,
                                colors = ButtonDefaults.textButtonColors(
                                    contentColor = MaterialTheme.colorScheme.onSurfaceVariant,
                                ),
                            ) { Text("Continue as guest", style = MaterialTheme.typography.bodySmall) }
                        }
                    }
                }
            }
            if (onCancel != null) {
                TextButton(onClick = onCancel, Modifier.align(Alignment.End)) { Text("Cancel") }
            }
        }
    }
}

/** A muted question and the link that answers it. */
@Composable
private fun Ask(question: String, link: String, enabled: Boolean, onClick: () -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(
            question,
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        TextButton(onClick = onClick, enabled = enabled) { Text(link) }
    }
}

/**
 * First launch: the core's cards, one at a time, then `onDone` (the join
 * screen). Skipping is `onDone` too.
 */
@Composable
fun WelcomeScreen(onDone: () -> Unit) {
    val cards = remember { welcomeCards() }
    var at by rememberSaveable { mutableIntStateOf(0) }
    val card = cards.getOrNull(at) ?: return onDone()
    val last = at == cards.lastIndex
    Surface(Modifier.fillMaxSize()) {
        Column(
            Modifier
                .safeDrawingPadding()
                .verticalScroll(rememberScrollState())
                .padding(32.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Spacer(Modifier.size(48.dp))
            Text(card.title, style = MaterialTheme.typography.headlineMedium, textAlign = TextAlign.Center)
            Text(
                card.body,
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
            Text(
                cards.indices.joinToString(" ") { if (it == at) "●" else "○" },
                Modifier.semantics { contentDescription = "Step ${at + 1} of ${cards.size}" },
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Button(onClick = { if (last) onDone() else at++ }, modifier = Modifier.fillMaxWidth()) {
                Text(if (last) "Get started" else "Next")
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (at > 0) TextButton(onClick = { at-- }) { Text("Back") }
                if (!last) {
                    TextButton(
                        onClick = onDone,
                        colors = ButtonDefaults.textButtonColors(
                            contentColor = MaterialTheme.colorScheme.onSurfaceVariant,
                        ),
                    ) { Text("Skip", style = MaterialTheme.typography.bodySmall) }
                }
            }
        }
    }
}
