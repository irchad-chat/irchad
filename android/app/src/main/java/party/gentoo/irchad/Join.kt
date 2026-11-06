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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import party.gentoo.irchad.core.Network
import party.gentoo.irchad.core.parseInvite

private enum class Mode(val label: String) { Login("Log in"), Register("Sign up"), Guest("Guest") }

/**
 * Paste an invite, log in (or sign up, or be a guest), done. Closes itself once
 * the network shows up; `onCancel` is null when there's nothing to go back to.
 */
@Composable
fun JoinScreen(
    app: IrChad,
    invite: String?,
    nets: List<Network>,
    onDone: (origin: String, channel: String?) -> Unit,
    onCancel: (() -> Unit)?,
) {
    var link by rememberSaveable { mutableStateOf(invite ?: "") }
    var mode by rememberSaveable { mutableStateOf(Mode.Login) }
    var account by rememberSaveable { mutableStateOf("") }
    var password by rememberSaveable { mutableStateOf("") }
    var email by rememberSaveable { mutableStateOf("") }
    var busy by rememberSaveable { mutableStateOf<String?>(null) }
    var error by rememberSaveable { mutableStateOf<String?>(null) }

    LaunchedEffect(invite) { if (invite != null) link = invite }
    LaunchedEffect(Unit) {
        app.joinFailures.collect { (inv, e) ->
            if (inv == busy) {
                busy = null
                error = e
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

    val submit = {
        val inv = link.trim()
        error = null
        busy = inv
        when (mode) {
            Mode.Login -> app.client.join(inv, account.trim(), password)
            Mode.Register -> app.client.register(inv, account.trim(), password, email.trim())
            Mode.Guest -> app.client.joinGuest(inv, account.trim())
        }
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
            Text("Join a network", style = MaterialTheme.typography.headlineMedium)
            Text(
                "Paste the server URL someone sent you.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            OutlinedTextField(
                link, { link = it },
                label = { Text("Server URL") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                modifier = Modifier.fillMaxWidth(),
            )
            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                Mode.entries.forEachIndexed { i, m ->
                    SegmentedButton(
                        selected = mode == m,
                        onClick = { mode = m },
                        shape = SegmentedButtonDefaults.itemShape(i, Mode.entries.size),
                    ) { Text(m.label) }
                }
            }
            OutlinedTextField(
                account, { account = it },
                label = { Text(if (mode == Mode.Guest) "Nickname" else "Username") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            if (mode != Mode.Guest) {
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
            error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.End,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (onCancel != null) TextButton(onClick = onCancel) { Text("Cancel") }
                Spacer(Modifier.size(8.dp))
                val ready = link.isNotBlank() && account.isNotBlank() &&
                    (mode == Mode.Guest || password.isNotEmpty())
                Button(onClick = submit, enabled = ready && busy == null) {
                    if (busy != null) {
                        CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                    } else {
                        Text(if (mode == Mode.Register) "Sign up" else "Join")
                    }
                }
            }
        }
    }
}
