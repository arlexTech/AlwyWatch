// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.media.Item
import be.alwyapps.watch.core.seerr.SeerrResult
import be.alwyapps.watch.ui.Field
import be.alwyapps.watch.ui.IconAction
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.LocalAccount
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.PageTitle
import be.alwyapps.watch.ui.PosterCard
import be.alwyapps.watch.ui.Route
import be.alwyapps.watch.ui.dpadEscape
import be.alwyapps.watch.ui.rememberFocus
import kotlinx.coroutines.delay

/**
 * One search for everything: your libraries first, then (with Seerr) what you could request.
 * Search as you type (debounced); works with the TV on-screen keyboard and voice input.
 */
@Composable
fun SearchScreen() {
    val core = LocalCore.current
    val account = LocalAccount.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    val chips = remember { FocusRequester() }
    // Back from a result: the same search, filter, results and focus (screens are disposed by
    // the navigator). Opened anew (menu, top bar): an empty search.
    val back = remember { nav.returning }
    if (!back) remember { SearchMemory.clear() }
    var query by rememberSaveable { mutableStateOf(SearchMemory.query) }
    LaunchedEffect(query) { SearchMemory.query = query }
    // Library results with the account they come from (all servers when shown together).
    var results by remember { mutableStateOf(SearchMemory.results) }
    var requestable by remember { mutableStateOf(SearchMemory.requestable) }
    LaunchedEffect(results, requestable) {
        SearchMemory.results = results
        SearchMemory.requestable = requestable
    }
    var error by remember { mutableStateOf<String?>(null) }
    var seerrError by remember { mutableStateOf<String?>(null) }
    // All (default), on my servers only, or only what is not on them.
    var filter by rememberSaveable { mutableStateOf(SearchMemory.filter) }
    LaunchedEffect(filter) { SearchMemory.filter = filter }
    val grid = rememberLazyGridState(SearchMemory.scroll.first, SearchMemory.scroll.second)
    DisposableEffect(Unit) { onDispose { SearchMemory.scroll = grid.firstVisibleItemIndex to grid.firstVisibleItemScrollOffset } }
    // Voice input, listened to here (the system's own voice screen floats over the page and,
    // on Google TV, doesn't hand its text back). The audio goes to the device's recognition
    // service; the microphone permission is asked on first use.
    val ctx = androidx.compose.ui.platform.LocalContext.current
    var voice by remember { mutableStateOf(android.speech.SpeechRecognizer.isRecognitionAvailable(ctx)) }
    var listening by remember { mutableStateOf(false) }
    var voiceNote by remember { mutableStateOf<String?>(null) }
    val recognizer = remember { mutableStateOf<android.speech.SpeechRecognizer?>(null) }
    DisposableEffect(Unit) { onDispose { recognizer.value?.destroy() } }
    val startListening = {
        val r =
            recognizer.value ?: android.speech.SpeechRecognizer.createSpeechRecognizer(ctx).also { created ->
                created.setRecognitionListener(
                    object : android.speech.RecognitionListener {
                        fun text(b: android.os.Bundle?) =
                            b
                                ?.getStringArrayList(android.speech.SpeechRecognizer.RESULTS_RECOGNITION)
                                ?.firstOrNull()
                                ?.takeIf { it.isNotBlank() }

                        override fun onReadyForSpeech(params: android.os.Bundle?) {
                            listening = true
                        }

                        override fun onPartialResults(partialResults: android.os.Bundle?) {
                            text(partialResults)?.let { query = it }
                        }

                        override fun onResults(results: android.os.Bundle?) {
                            listening = false
                            text(results)?.let { query = it }
                        }

                        override fun onError(error: Int) {
                            listening = false
                            voiceNote =
                                when (error) {
                                    android.speech.SpeechRecognizer.ERROR_NO_MATCH,
                                    android.speech.SpeechRecognizer.ERROR_SPEECH_TIMEOUT,
                                    -> "Didn't catch that. Try again."
                                    android.speech.SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS -> "Voice search needs the microphone permission."
                                    android.speech.SpeechRecognizer.ERROR_NETWORK,
                                    android.speech.SpeechRecognizer.ERROR_NETWORK_TIMEOUT,
                                    -> "Voice search needs the internet."
                                    else -> "Voice search isn't working right now."
                                }
                            android.util.Log.i("AlwySearch", "voice error $error")
                        }

                        override fun onBeginningOfSpeech() {}

                        override fun onRmsChanged(rmsdB: Float) {}

                        override fun onBufferReceived(buffer: ByteArray?) {}

                        override fun onEndOfSpeech() {}

                        override fun onEvent(
                            eventType: Int,
                            params: android.os.Bundle?,
                        ) {}
                    },
                )
                recognizer.value = created
            }
        voiceNote = null
        listening = true
        runCatching {
            r.startListening(
                android.content.Intent(android.speech.RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
                    putExtra(android.speech.RecognizerIntent.EXTRA_LANGUAGE_MODEL, android.speech.RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
                    putExtra(android.speech.RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
                    putExtra(android.speech.RecognizerIntent.EXTRA_CALLING_PACKAGE, ctx.packageName)
                },
            )
        }.onFailure {
            listening = false
            voice = false
        }
    }
    val askMic =
        androidx.activity.compose.rememberLauncherForActivityResult(
            androidx.activity.result.contract.ActivityResultContracts
                .RequestPermission(),
        ) { granted ->
            if (granted) startListening() else voiceNote = "Voice search needs the microphone permission."
        }
    val seerrOn = remember { core.seerrActive() }
    val focus = remember { FocusRequester() }
    val clear = remember { FocusRequester() }
    val mic = remember { FocusRequester() }
    // Keyboard straight away only for a new search, not when coming back to results.
    LaunchedEffect(Unit) { if (query.isEmpty()) runCatching { focus.requestFocus() } }
    // Watched state changed from a result's menu: search again so the marks follow.
    val changed = be.alwyapps.watch.ui.ItemsChanged.version
    LaunchedEffect(query, changed) {
        if (query.length < 2) {
            results = emptyList()
            requestable = emptyList()
            return@LaunchedEffect
        }
        delay(300)
        val accounts = runCatching { core.accounts() }.getOrDefault(emptyList()).map { it.id }
        val together = accounts.size > 1 && runCatching { core.getSetting(COMBINED_KEY) }.getOrNull() != "0"
        val found = mutableListOf<Pair<String, Item>>()
        var failed: String? = null
        // Preferred server first: its copy wins when a title is on several.
        for (a in if (together) preferredFirst(core, accounts) else listOf(account)) {
            try {
                core.search(a, query, 60u).forEach { found += a to it }
            } catch (e: CoreException) {
                failed = e.message
            }
        }
        // One per title across servers.
        results = found.distinctBy { (_, i) -> i.sameAs() }
        error = failed.takeIf { found.isEmpty() }
        if (seerrOn) {
            try {
                // Everything that exists, minus what the library results already show.
                val have = results.mapNotNull { (_, i) -> i.providerIds.tmdb }.toSet()
                val norm = { t: String -> t.lowercase().filter { it.isLetterOrDigit() } }
                val names = results.map { (_, i) -> norm(i.title) }.toSet()
                requestable = core.seerrSearch(query).filter { it.tmdbId.toString() !in have && norm(it.title) !in names }.take(30)
                seerrError = null
            } catch (e: CoreException) {
                requestable = emptyList()
                seerrError = e.message
            }
        }
    }
    LazyVerticalGrid(
        if (d.isTv) GridCells.FixedSize(d.posterWidth) else GridCells.Adaptive(d.posterWidth + 8.dp),
        Modifier.fillMaxSize(),
        state = grid,
        contentPadding = PaddingValues(horizontal = d.gutter, vertical = 16.dp),
        horizontalArrangement = Arrangement.spacedBy(d.railSpacing),
        verticalArrangement = Arrangement.spacedBy(20.dp),
    ) {
        item(span = { GridItemSpan(maxLineSpan) }) {
            Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
                PageTitle("Search")
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    // ✕ sits over the field's end but is not part of it: inside the text field
                    // (its trailing icon) the D-pad could never reach it, → went to the mic.
                    val hasClear = query.isNotEmpty()
                    Box(Modifier.weight(1f), contentAlignment = Alignment.CenterEnd) {
                        Field(
                            query,
                            { query = it },
                            // Down goes to the chosen filter (else the chip nearest the field's middle).
                            Modifier
                                .fillMaxWidth()
                                .focusRequester(focus)
                                .focusProperties {
                                    if (query.length >= 2) down = chips
                                    if (hasClear) right = clear
                                }.dpadEscape(),
                            placeholder = { Text(if (listening) "Listening… say a title" else "Movies, shows, episodes…") },
                            singleLine = true,
                            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                            leadingIcon = Icons.Search,
                            pill = true,
                            selectAllOnEdit = true,
                            // Room for ✕, so the text never runs under it.
                            trailing = if (hasClear) ({ Spacer(Modifier.size(if (d.isTv) 56.dp else 50.dp)) }) else null,
                        )
                        if (hasClear) {
                            IconAction(
                                Icons.Close,
                                "Clear",
                                {
                                    query = ""
                                    runCatching { focus.requestFocus() }
                                },
                                Modifier
                                    .padding(end = 6.dp)
                                    .focusRequester(clear)
                                    .focusProperties {
                                        left = focus
                                        if (voice) right = mic
                                        if (query.length >= 2) down = chips
                                    },
                            )
                        }
                    }
                    if (voice) {
                        IconAction(Icons.Mic, if (listening) "Listening…" else "Voice search", {
                            when {
                                listening -> {
                                    recognizer.value?.stopListening()
                                    listening = false
                                }
                                androidx.core.content.ContextCompat
                                    .checkSelfPermission(ctx, android.Manifest.permission.RECORD_AUDIO) ==
                                    android.content.pm.PackageManager.PERMISSION_GRANTED -> startListening()
                                else -> askMic.launch(android.Manifest.permission.RECORD_AUDIO)
                            }
                        }, Modifier.focusRequester(mic).focusProperties { if (hasClear) left = clear }, active = listening)
                    }
                }
                voiceNote?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = be.alwyapps.watch.ui.Muted) }
            }
        }
        if (query.length >= 2) {
            item(span = { GridItemSpan(maxLineSpan) }) {
                Row(
                    Modifier.horizontalScroll(androidx.compose.foundation.rememberScrollState()),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    for ((key, label) in listOf("all" to "All", "mine" to "On my servers", "other" to "Not on my servers")) {
                        be.alwyapps.watch.ui
                            .ActionButton(
                                label,
                                { filter = key },
                                (if (filter == key) Modifier.focusRequester(chips) else Modifier).rememberFocus("search", "chip.$key"),
                                selected = filter == key,
                            )
                    }
                }
            }
        }
        error?.let { item(span = { GridItemSpan(maxLineSpan) }) { Text(it, color = Muted) } }
        val showMine = filter != "other"
        val showOther = filter != "mine"
        if (showMine && results.isNotEmpty()) {
            item(span = { GridItemSpan(maxLineSpan) }) { Text("On your servers", style = MaterialTheme.typography.titleMedium) }
        }
        if (showMine) {
            items(results, key = { (_, i) -> i.id }) { (a, i) ->
                androidx.compose.runtime.CompositionLocalProvider(
                    be.alwyapps.watch.ui.LocalItemOwners provides mapOf(i.id to a),
                    be.alwyapps.watch.ui.LocalServerBadges provides (results.map { it.first }.distinct().size > 1),
                ) {
                    PosterCard(
                        i,
                        d.posterWidth,
                        onClick = { nav.push(Route.Detail(i.id, account = a)) },
                        cardModifier = Modifier.rememberFocus("search", "lib|${i.id}"),
                        showTitle = true,
                    )
                }
            }
        }
        if (showOther && query.length >= 2 && !seerrOn) {
            item(span = { GridItemSpan(maxLineSpan) }) {
                Text(
                    "To also find what is not on your servers (and request it), connect requests in Settings.",
                    Modifier.padding(top = 8.dp),
                    color = Muted,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
        seerrError?.takeIf { showOther }?.let { msg ->
            item(span = { GridItemSpan(maxLineSpan) }) { Text("Requests unavailable: $msg", color = Muted, style = MaterialTheme.typography.bodySmall) }
        }
        if (showOther && requestable.isNotEmpty()) {
            item(span = { GridItemSpan(maxLineSpan) }) {
                Column(Modifier.padding(top = 12.dp)) {
                    Text("Not on your servers", style = MaterialTheme.typography.titleMedium)
                    Text("Open one to see it and request it", style = MaterialTheme.typography.bodySmall, color = Muted)
                }
            }
            items(requestable, key = { "seerr-${it.mediaType}-${it.tmdbId}" }) { r ->
                SeerrPoster(r, Modifier.rememberFocus("search", "seerr|${r.mediaType}|${r.tmdbId}")) {
                    nav.push(
                        Route.SeerrTitle(
                            if (r.mediaType ==
                                be.alwyapps.watch.core.seerr.SeerrMediaType.TV
                            ) {
                                "tv"
                            } else {
                                "movie"
                            },
                            r.tmdbId,
                        ),
                    )
                }
            }
        }
        if (query.length >= 2 && (!showMine || results.isEmpty()) && (!showOther || requestable.isEmpty()) && error == null) {
            item(span = { GridItemSpan(maxLineSpan) }) { Text("No results for \u201c$query\u201d.", color = Muted) }
        }
    }
}

/** The last search, kept for the process lifetime. */
object SearchMemory {
    var query = ""
    var filter = "all"
    var results: List<Pair<String, Item>> = emptyList()
    var requestable: List<SeerrResult> = emptyList()
    var scroll = 0 to 0

    /** A new search: nothing of the last one, focus memory included. */
    fun clear() {
        query = ""
        filter = "all"
        results = emptyList()
        requestable = emptyList()
        scroll = 0 to 0
        be.alwyapps.watch.ui.FocusMemory
            .forget("search")
    }
}
