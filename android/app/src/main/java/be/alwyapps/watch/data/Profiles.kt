// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.data

import android.content.Context
import android.util.Base64
import org.json.JSONArray
import org.json.JSONObject
import java.security.SecureRandom
import javax.crypto.SecretKeyFactory
import javax.crypto.spec.PBEKeySpec

/** The first profile's name until it takes its account's (or is renamed). */
const val UNNAMED = "Me"

/** A person on this device: their own accounts, AlwyWatch server, Seerr and settings. */
data class Profile(
    val id: String,
    val name: String,
    val hasPin: Boolean,
    /** No name of its own yet: it takes its account's (a new one shows as "New profile"). */
    val unnamed: Boolean = false,
)

/**
 * The device's profiles. "default" is the profile that existed before profiles (its data stays
 * where it was). A PIN is stored as a salted PBKDF2 hash: it keeps people out of each other's
 * profile on a shared device; tokens themselves stay encrypted by the Android Keystore.
 */
class ProfileStore(
    private val context: Context,
) {
    private val prefs = context.getSharedPreferences("profiles", Context.MODE_PRIVATE)

    private fun raw(): JSONArray =
        prefs.getString("list", null)?.let { runCatching { JSONArray(it) }.getOrNull() }
            ?: JSONArray().put(JSONObject().put("id", DEFAULT).put("name", UNNAMED))

    private fun save(a: JSONArray) {
        prefs.edit().putString("list", a.toString()).commit()
    }

    @Synchronized
    fun list(): List<Profile> {
        val a = raw()
        return (0 until a.length()).map { i ->
            val o = a.getJSONObject(i)
            val id = o.getString("id")
            val stored = o.getString("name")
            val unnamed = stored == UNNAMED
            // Only the first profile is "Me": an added one is "New profile" until it has a login.
            val name =
                if (unnamed && id != DEFAULT) {
                    be.alwyapps.watch.ui
                        .tr("New profile")
                } else {
                    stored
                }
            Profile(id, name, o.has("pin"), unnamed)
        }
    }

    fun current(): String = prefs.getString("current", null)?.takeIf { id -> list().any { it.id == id } } ?: DEFAULT

    @Synchronized
    fun setCurrent(id: String) {
        prefs.edit().putString("current", id).commit()
    }

    @Synchronized
    fun add(name: String): String {
        val id =
            java.util.UUID
                .randomUUID()
                .toString()
                .take(8)
        save(raw().put(JSONObject().put("id", id).put("name", name.trim().ifEmpty { "Profile" })))
        return id
    }

    @Synchronized
    fun rename(
        id: String,
        name: String,
    ) = edit(id) { it.put("name", name.trim().ifEmpty { "Profile" }) }

    /**
     * Added profiles left before any login (no token stored): removed with their few files, so
     * starting to add a profile and going back leaves nothing behind. Not [keep] (the one this
     * process runs).
     */
    @Synchronized
    fun pruneEmpty(keep: String) {
        for (p in list()) {
            if (p.id == DEFAULT || p.id == keep || !p.unnamed) continue
            val tokens = "tokens-${p.id}"
            if (context.getSharedPreferences(tokens, Context.MODE_PRIVATE).all.isNotEmpty()) continue
            java.io.File(context.filesDir, "profiles/${p.id}").deleteRecursively()
            context.deleteSharedPreferences(tokens)
            remove(p.id)
        }
    }

    @Synchronized
    fun remove(id: String) {
        if (id == DEFAULT) return
        val a = raw()
        val out = JSONArray()
        for (i in 0 until a.length()) if (a.getJSONObject(i).getString("id") != id) out.put(a.getJSONObject(i))
        save(out)
    }

    /** Sets (or with null, removes) a profile's PIN. */
    @Synchronized
    fun setPin(
        id: String,
        pin: String?,
    ) = edit { o ->
        if (o.getString("id") != id) return@edit o
        if (pin == null) {
            o.remove("pin")
        } else {
            val salt = ByteArray(16).also { SecureRandom().nextBytes(it) }
            o.put("pin", JSONObject().put("salt", b64(salt)).put("hash", b64(hash(pin, salt))))
        }
        o
    }

    fun checkPin(
        id: String,
        pin: String,
    ): Boolean {
        val a = raw()
        for (i in 0 until a.length()) {
            val o = a.getJSONObject(i)
            if (o.getString("id") != id) continue
            val p = o.optJSONObject("pin") ?: return true
            val salt = Base64.decode(p.getString("salt"), Base64.NO_WRAP)
            return java.security.MessageDigest.isEqual(hash(pin, salt), Base64.decode(p.getString("hash"), Base64.NO_WRAP))
        }
        return false
    }

    /** Several profiles: open the last one used at launch instead of asking who's watching. */
    var autoSignIn: Boolean
        get() = prefs.getBoolean("auto_sign_in", false)
        set(v) {
            prefs.edit().putBoolean("auto_sign_in", v).apply()
        }

    /**
     * The last AlwyWatch address a profile of this device used: "Add a profile" offers it (each
     * profile's own settings live in its own database).
     */
    var lastAlwyWatchUrl: String?
        get() = prefs.getString("alwywatch_url", null)
        set(v) {
            prefs.edit().putString("alwywatch_url", v).apply()
        }

    /** Asked once at the next start (after a switch whose PIN was already checked). */
    var unlockedOnce: String?
        get() = prefs.getString("unlocked_once", null)
        set(v) {
            prefs.edit().putString("unlocked_once", v).commit()
        }

    private fun edit(
        id: String,
        f: (JSONObject) -> JSONObject,
    ) = edit { o -> if (o.getString("id") == id) f(o) else o }

    private fun edit(f: (JSONObject) -> JSONObject) {
        val a = raw()
        val out = JSONArray()
        for (i in 0 until a.length()) out.put(f(a.getJSONObject(i)))
        save(out)
    }

    private fun hash(
        pin: String,
        salt: ByteArray,
    ): ByteArray =
        SecretKeyFactory
            .getInstance("PBKDF2WithHmacSHA256")
            .generateSecret(PBEKeySpec(pin.toCharArray(), salt, 20_000, 256))
            .encoded

    private fun b64(b: ByteArray) = Base64.encodeToString(b, Base64.NO_WRAP)

    companion object {
        const val DEFAULT = "default"
    }
}
