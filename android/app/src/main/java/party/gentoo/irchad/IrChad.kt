package party.gentoo.irchad

import android.app.Application
import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import party.gentoo.irchad.core.Client
import party.gentoo.irchad.core.Clip
import party.gentoo.irchad.core.Credentials
import party.gentoo.irchad.core.GifPage
import party.gentoo.irchad.core.LinkPreview
import party.gentoo.irchad.core.Listener
import party.gentoo.irchad.core.Sas
import party.gentoo.irchad.core.SasStep
import party.gentoo.irchad.core.Secrets
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Owns the core for the life of the process, so it outlives activities. */
class IrChad : Application() {
    /** Bumped on every core event: read the client's snapshots again. */
    val version = MutableStateFlow(0L)
    val notices = MutableSharedFlow<String>(extraBufferCapacity = 16)
    /** (invite, error) for a join that failed. */
    val joinFailures = MutableSharedFlow<Pair<String, String>>(extraBufferCapacity = 4)
    /** The verification on screen, one person at a time. */
    val verify = MutableStateFlow<Verify?>(null)

    val calls by lazy { Calls(this) }
    val media by lazy { Media(this) }
    val notifier by lazy { Notifier(this) }
    val sounds by lazy { Sounds(this) }
    /** Something shared to IrChad from another app, until it's sent somewhere. */
    val shared = MutableStateFlow<Shared?>(null)

    val client: Client by lazy {
        Client(
            filesDir.absolutePath,
            "IrChad ${BuildConfig.VERSION_NAME} (Android)",
            KeystoreSecrets(this),
            calls,
            object : Listener {
                override fun changed() {
                    version.value += 1
                }

                override fun notice(text: String) {
                    notices.tryEmit(text)
                }

                override fun joinFailed(invite: String, error: String) {
                    joinFailures.tryEmit(invite to error)
                }

                override fun preview(network: String, url: String, preview: LinkPreview?) =
                    media.gotPreview(network, url, preview)

                override fun media(network: String, url: String, bytes: ByteArray?) =
                    media.gotMedia(network, url, bytes)

                override fun gifs(network: String, query: String, pos: String?, page: GifPage) {
                    media.gifs.tryEmit(Triple(query, pos, page))
                }

                override fun pushAvailable(network: String, vapid: String) =
                    notifier.pushAvailable(network, vapid)

                override fun notify(network: String, target: String, from: String, text: String, channel: Boolean) =
                    notifier.live(network, target, from, text, channel)

                override fun verification(network: String, nick: String, state: Sas) {
                    // A request from someone else waits behind the one on screen.
                    val cur = verify.value
                    val busy = cur != null && (cur.network != network || !cur.nick.equals(nick, true)) &&
                        cur.state !is Sas.Done && cur.state !is Sas.Failed
                    if (!busy) verify.value = Verify(network, nick, state)
                    else if (state is Sas.Incoming) client.verification(network, nick, SasStep.CANCEL)
                }

                override fun sounds(network: String, clips: List<Clip>, error: String?) =
                    sounds.got(network, clips, error)

                override fun sound(network: String, file: String, bytes: ByteArray?) =
                    sounds.gotBytes(network, file, bytes)
            },
        ).also { notifier.started() }
    }
}

data class Verify(val network: String, val nick: String, val state: Sas)

/**
 * Passwords encrypted with an AES key that never leaves the Android Keystore,
 * stored in private SharedPreferences.
 */
class KeystoreSecrets(context: Context) : Secrets {
    private val prefs = context.getSharedPreferences("secrets", Context.MODE_PRIVATE)

    private fun key(): SecretKey {
        val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (ks.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        gen.init(
            KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .build(),
        )
        return gen.generateKey()
    }

    override fun save(origin: String, account: String, password: String): String? = try {
        val c = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key()) }
        val sealed = c.iv + c.doFinal(password.toByteArray())
        prefs.edit()
            .putString("$origin#account", account)
            .putString("$origin#password", Base64.encodeToString(sealed, Base64.NO_WRAP))
            .apply()
        null
    } catch (e: Exception) {
        "Couldn't save the password: ${e.message}"
    }

    override fun load(origin: String): Credentials? = try {
        val account = prefs.getString("$origin#account", null)
        val sealed = prefs.getString("$origin#password", null)?.let { Base64.decode(it, Base64.NO_WRAP) }
        if (account == null || sealed == null || sealed.size <= IV) {
            null
        } else {
            val c = Cipher.getInstance("AES/GCM/NoPadding")
            c.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, sealed, 0, IV))
            Credentials(account, String(c.doFinal(sealed, IV, sealed.size - IV)))
        }
    } catch (e: Exception) {
        null
    }

    override fun delete(origin: String) {
        prefs.edit().remove("$origin#account").remove("$origin#password").apply()
    }

    private companion object {
        const val ALIAS = "irchad-secrets"
        const val IV = 12
    }
}
