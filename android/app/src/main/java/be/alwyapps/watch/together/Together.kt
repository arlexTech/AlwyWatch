// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.together

import android.os.SystemClock
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.keep.TogetherJoined
import be.alwyapps.watch.core.keep.TogetherMedia
import be.alwyapps.watch.core.keep.TogetherMember
import be.alwyapps.watch.core.keep.TogetherRoom
import be.alwyapps.watch.core.keep.TogetherRooms
import be.alwyapps.watch.core.keep.TogetherState
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/** "Show · S1 · E2", or the movie's title. */
fun TogetherMedia.label(): String = seriesTitle?.let { "$it · S${season ?: 1} · E${episode ?: 1}" } ?: title

/** One entry per person (a user on two devices has two seats), with how many devices. */
fun List<TogetherMember>.people(): List<Pair<TogetherMember, Int>> = groupBy { it.userKey.ifEmpty { it.id } }.values.map { it.first() to it.size }

/** Plain words for what went wrong with a watch together or profile call. */
fun friendly(e: CoreException): String {
    val status =
        (e as? CoreException.Server)
            ?.message
            ?.substringAfter("HTTP ", "")
            ?.takeWhile { it.isDigit() }
            ?.toIntOrNull()
    return when {
        e is CoreException.Network -> "Can't reach your AlwyWatch server right now."
        e is CoreException.NotFound -> "This room has closed."
        status == 429 -> "Too many changes. Try again in a few minutes."
        status == 409 -> "This room is full."
        status == 413 -> "This picture is too big."
        status == 403 -> "Only the host can do that."
        else -> e.message ?: "Something went wrong."
    }
}

/**
 * The watch together room this device sits in (one at a time), shared by the Room screen and
 * the player so the seat never goes silent between them. Its `wait` loop is the seat's
 * heartbeat (a seat silent for a minute is dropped): it runs while something shows the room.
 */
object TogetherSession {
    data class Seat(
        val code: String,
        val memberId: String,
    )

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    var seat by mutableStateOf<Seat?>(null)
        private set

    /** The room as last heard (null before the first answer). */
    var state by mutableStateOf<TogetherState?>(null)
        private set

    /** When [state] arrived (elapsed realtime), for [be.alwyapps.watch.core.togetherPosition]. */
    var receivedAt = 0L
        private set

    /** Why the room went away under us ("This room has closed."), until someone reads it. */
    var ended by mutableStateOf<String?>(null)

    /** Where the host starts playback (their resume point when they opened the room). */
    var startAt = 0L

    private val answers = MutableSharedFlow<TogetherState>(extraBufferCapacity = 16)

    /** Every answer of the room (also the unchanged ones every ~25 s: they correct drift). */
    val updates: SharedFlow<TogetherState> = answers

    private var core: AlwyWatch? = null
    private var users = 0
    private var loop: Job? = null
    private var stopping: Job? = null

    /** A room just joined or created: this device's seat. */
    fun begin(
        core: AlwyWatch,
        joined: TogetherJoined,
        startAt: Long = 0L,
    ) {
        this.core = core
        if (seat?.code != joined.state.code) {
            loop?.cancel()
            loop = null
            state = null
        }
        seat = Seat(joined.state.code, joined.memberId)
        this.startAt = startAt
        ended = null
        accept(joined.state, force = true)
        if (users > 0) ensureLoop()
    }

    /** The seat the player was opened with (the process was restarted meanwhile). */
    fun adopt(
        core: AlwyWatch,
        code: String,
        memberId: String,
    ) {
        this.core = core
        if (seat?.code == code) return
        seat = Seat(code, memberId)
        state = null
        ended = null
    }

    /** A newer state of this room (from `wait`, `update`, `invite`…). */
    fun accept(
        s: TogetherState,
        force: Boolean = false,
    ) {
        if (seat?.code != s.code) return
        val old = state
        if (force || old == null || s.seq >= old.seq) {
            state = s
            receivedAt = SystemClock.elapsedRealtime()
        }
    }

    fun isHost(): Boolean {
        val s = state ?: return false
        val me = seat?.memberId ?: return false
        return be.alwyapps.watch.core
            .togetherIsHost(s, me)
    }

    /** Something shows the room (Room screen, player): keep the seat alive. */
    fun attach(core: AlwyWatch) {
        this.core = core
        users++
        stopping?.cancel()
        stopping = null
        ensureLoop()
    }

    /** It no longer does. A short grace bridges the Room screen handing over to the player. */
    fun detach() {
        users = (users - 1).coerceAtLeast(0)
        if (users > 0) return
        stopping?.cancel()
        stopping =
            scope.launch {
                delay(3_000)
                if (users == 0) {
                    loop?.cancel()
                    loop = null
                }
            }
    }

    /** Leaves for good (a new invitation is needed to come back). */
    fun leave() {
        val s = seat ?: return
        val c = core
        TogetherInbox.forget(s.code)
        clear()
        // Not tied to a screen: the call outlives whatever closed.
        if (c != null) background.launch { runCatching { c.togetherLeave(s.code, s.memberId) } }
    }

    private val background = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    private fun clear() {
        loop?.cancel()
        loop = null
        seat = null
        state = null
    }

    private fun ensureLoop() {
        if (loop?.isActive == true || seat == null) return
        loop =
            scope.launch {
                var failures = 0
                var rejoined = false
                while (isActive) {
                    val c = core ?: break
                    val s = seat ?: break
                    val answer =
                        try {
                            c.togetherWait(s.code, s.memberId, state?.seq ?: 0uL)
                        } catch (e: CoreException) {
                            if (e is CoreException.NotFound) {
                                // A dropped seat may come back once; else the room is gone.
                                if (!rejoined) {
                                    rejoined = true
                                    try {
                                        val j = c.togetherJoin(s.code)
                                        if (seat?.code == s.code) {
                                            seat = Seat(s.code, j.memberId)
                                            accept(j.state, force = true)
                                        }
                                        continue
                                    } catch (_: CoreException.NotFound) {
                                    } catch (_: CoreException) {
                                        delay(3_000)
                                        continue
                                    }
                                }
                                if (seat?.code == s.code) {
                                    TogetherInbox.forget(s.code)
                                    clear()
                                    ended = "This room has closed."
                                }
                                break
                            }
                            failures++
                            delay((failures * 2_000L).coerceAtMost(15_000))
                            continue
                        }
                    failures = 0
                    rejoined = false
                    if (seat?.code != s.code) break
                    accept(answer, force = true)
                    answers.tryEmit(answer)
                }
            }
    }
}

/**
 * My rooms and invitations, kept current by a long poll while the app is in the foreground and
 * an AlwyWatch server is connected. A new invitation becomes [prompt] (shown anywhere in the
 * app) and, while the player is in front, a short note there ([notes]).
 */
object TogetherInbox {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    var rooms by mutableStateOf<TogetherRooms?>(null)
        private set

    /** The last try failed (the list shown is the last known one). */
    var unreachable by mutableStateOf(false)
        private set

    /** An invitation to offer with Join / Not now. */
    var prompt by mutableStateOf<TogetherRoom?>(null)

    /** A room to open in the main app (from the player's "Watch together"). */
    var openRoom by mutableStateOf<String?>(null)

    private val noteFlow = MutableSharedFlow<String>(extraBufferCapacity = 4)
    val notes: SharedFlow<String> = noteFlow

    /** Invitations already seen (no second prompt for them). */
    private val known = mutableSetOf<String>()

    /** Rooms left or declined here: never prompted again. */
    private val gone = mutableSetOf<String>()

    private var started = 0
    private var playerStarted = 0
    private var job: Job? = null
    private var core: AlwyWatch? = null

    /** The app's activities going to the foreground and back (MainActivity, PlayerActivity). */
    fun activityStarted(
        core: AlwyWatch?,
        player: Boolean,
    ) {
        if (core != null) this.core = core
        started++
        if (player) playerStarted++
        ensure()
    }

    fun activityStopped(player: Boolean) {
        started = (started - 1).coerceAtLeast(0)
        if (player) playerStarted = (playerStarted - 1).coerceAtLeast(0)
        if (started == 0) {
            job?.cancel()
            job = null
        }
    }

    /** The core became available after the activity started. */
    fun setCore(core: AlwyWatch) {
        this.core = core
        ensure()
    }

    /** Left or declined: no prompt for it again. */
    fun forget(code: String) {
        gone += code
        if (prompt?.code == code) prompt = null
        rooms?.let { r -> rooms = r.copy(rooms = r.rooms.filter { it.code != code || it.mine }) }
    }

    /** A fresh list (Together page): shown, no prompt for what it lists. */
    suspend fun refresh(core: AlwyWatch) {
        try {
            val r = core.togetherRooms()
            known += r.rooms.filter { it.invited }.map { it.code }
            rooms = r
            unreachable = false
        } catch (_: CoreException) {
            unreachable = true
        }
    }

    private fun ensure() {
        if (started == 0 || job?.isActive == true) return
        job =
            scope.launch {
                var version = 0uL
                var failures = 0
                while (isActive) {
                    val c = core
                    if (c == null) {
                        delay(1_000)
                        continue
                    }
                    if (!runCatching { c.keepActive() }.getOrDefault(false)) {
                        rooms = null
                        prompt = null
                        version = 0uL
                        delay(10_000)
                        continue
                    }
                    val r =
                        try {
                            c.togetherRoomsWait(version)
                        } catch (_: CoreException) {
                            unreachable = true
                            failures++
                            delay((failures * 3_000L).coerceAtMost(30_000))
                            continue
                        }
                    failures = 0
                    unreachable = false
                    version = r.version
                    take(r)
                }
            }
    }

    private fun take(r: TogetherRooms) {
        rooms = r
        val invited = r.rooms.filter { it.invited }
        val fresh = invited.filter { it.code !in known && it.code !in gone }
        known += invited.map { it.code }
        // Withdrawn, or the room closed: no prompt for it any more.
        prompt?.let { p -> if (invited.none { it.code == p.code }) prompt = null }
        val newest = fresh.firstOrNull() ?: return
        if (prompt == null) prompt = newest
        if (playerStarted > 0) noteFlow.tryEmit("${newest.host} invites you to watch ${newest.media.label()}. See Together.")
    }
}
