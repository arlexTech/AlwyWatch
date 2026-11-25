// SPDX-License-Identifier: AGPL-3.0-or-later
//! Titles that are not (or not fully) on the servers, through Seerr: their page with the
//! request button (all or some seasons of a show), and the Requests page (mine, everyone's).

use i18n::{tr, trn};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;
use net::Error;
use seerr::{
    Availability, RequestStatus, SeasonState, SeerrDetails, SeerrMediaType, SeerrRequest,
    SeerrSeasonInfo, season_plan, seasons_requested_message,
};

use crate::state;
use crate::ui::{ErrorText, Loading};
use media_api::{ItemKind, MediaServer};
use std::cell::RefCell;
use std::collections::HashMap;

/// TMDB id → item id, with when it was listed (ms).
type TmdbIndex = (f64, HashMap<String, String>);

thread_local! {
    /// Each server's movies or shows by TMDB id ("acc/movie" → tmdb → item id), with when.
    static TMDB_INDEX: RefCell<HashMap<String, TmdbIndex>> =
        RefCell::new(HashMap::new());
}

/// The library pages of these titles (`kind/tmdb` → `/item/acc/id`) on the signed-in servers:
/// a request opens the playable page when the title is there. Indexes are kept ten minutes.
async fn library_hrefs(titles: Vec<(SeerrMediaType, i64)>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut kinds: Vec<SeerrMediaType> = titles.iter().map(|t| t.0).collect();
    kinds.dedup();
    for (acc, srv) in state::ctx().servers() {
        for &kind in [SeerrMediaType::Movie, SeerrMediaType::Tv]
            .iter()
            .filter(|k| kinds.contains(k))
        {
            let (item_kind, tag) = match kind {
                SeerrMediaType::Movie => (ItemKind::Movie, "movie"),
                SeerrMediaType::Tv => (ItemKind::Series, "show"),
            };
            let key = format!("{acc}/{tag}");
            let fresh = TMDB_INDEX.with_borrow(|m| {
                m.get(&key)
                    .filter(|(at, _)| js_sys::Date::now() - at < 600_000.0)
                    .is_some()
            });
            if !fresh {
                let Ok(items) = srv.provider_index(item_kind).await else {
                    continue;
                };
                let index = items
                    .into_iter()
                    .filter_map(|i| Some((i.provider_ids.tmdb?, i.id)))
                    .collect();
                TMDB_INDEX.with_borrow_mut(|m| m.insert(key.clone(), (js_sys::Date::now(), index)));
            }
            TMDB_INDEX.with_borrow(|m| {
                let Some((_, index)) = m.get(&key) else {
                    return;
                };
                for (k, tmdb) in titles.iter().filter(|t| t.0 == kind) {
                    let place = format!("{k:?}/{tmdb}");
                    if let (false, Some(id)) =
                        (out.contains_key(&place), index.get(&tmdb.to_string()))
                    {
                        out.insert(place, format!("/item/{acc}/{id}"));
                    }
                }
            });
        }
    }
    out
}

fn no_seerr() -> String {
    tr!("Requests need an account on this AlwyWatch server: sign in with one in Settings.")
}

/// Settings → Requests: whether this site's AlwyWatch server takes requests, and as whom.
/// (The web only requests through it: no request-site address of its own.)
#[component]
pub fn RequestsSettings() -> impl IntoView {
    let c = state::ctx();
    let (keep, account, me) = (c.keep, c.account, c.me);
    let seerr_name = RwSignal::new(None::<String>);
    if let Some(s) = c.seerr()
        && me.with_untracked(|m| m.as_ref().is_none_or(|m| m.services.requests))
    {
        spawn_local(async move {
            if let Ok(u) = s.me().await {
                seerr_name.try_set(Some(u.display_name));
            }
        });
    }
    view! {
        <section class="group">
            <h2>{tr!("Requests")}</h2>
            {move || {
                let takes = me.with(|m| m.as_ref().is_none_or(|m| m.services.requests));
                let text = if keep.get().is_none() {
                    if account.get().is_some() {
                        tr!("Requests need a Jellyfin or Plex login on this site's servers. Add one: Servers → Add a server.")
                    } else {
                        tr!("Requests go through your AlwyWatch server. Connect to it in Account.")
                    }
                } else if !takes {
                    tr!("Your AlwyWatch server doesn't take requests. Ask its owner.")
                } else {
                    match seerr_name.get() {
                        Some(name) => tr!("Through your AlwyWatch server, as {name}", name = name),
                        None => tr!("Through your AlwyWatch server"),
                    }
                };
                view! { <p>{text}</p> }
            }}
        </section>
    }
}

/// Where a title's page is.
pub fn title_href(kind: SeerrMediaType, tmdb_id: i64) -> String {
    let kind = match kind {
        SeerrMediaType::Movie => "movie",
        SeerrMediaType::Tv => "tv",
    };
    format!("/request/{kind}/{tmdb_id}")
}

/// Where a saga's page is.
pub fn saga_href(id: i64) -> String {
    format!("/saga/{id}")
}

/// "Saga · Alien", or the name alone when it already says so ("Very Bad Trip - Saga").
pub fn saga_label(name: &str) -> String {
    let lower = name.to_lowercase();
    if lower.contains("saga") || lower.contains("collection") {
        name.to_owned()
    } else {
        tr!("Saga · {name}", name = name)
    }
}

/// A saga (TMDB collection): its films with their state; each opens its page.
#[component]
pub fn Saga() -> impl IntoView {
    let params = use_params_map();
    let id = Memo::new(move |_| {
        params
            .read()
            .get("id")
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or(0)
    });
    let saga = LocalResource::new(move || {
        let id = id.get();
        async move {
            let s = state::ctx().seerr().ok_or_else(no_seerr)?;
            s.collection(id).await.map_err(|e| plain_error(&e))
        }
    });
    view! {
        {move || match saga.get() {
            None => view! { <Loading /> }.into_any(),
            Some(Err(e)) => view! { <ErrorText text=e /> }.into_any(),
            Some(Ok(c)) => view! {
                <h1>{c.name.clone()}</h1>
                <p class="muted">{trn!("{n} film", "{n} films", c.parts.len())}</p>
                {c.overview.clone().map(|o| view! { <p class="overview">{o}</p> })}
                <div class="grid">
                    {c.parts.into_iter().map(|r| view! { <crate::browse::Requestable r=r /> }).collect_view()}
                </div>
            }.into_any(),
        }}
    }
}

/// The state of a title as one label, like the apps ("Requested", "Downloading 42%"…).
pub fn availability_label(a: Availability, progress: Option<f32>) -> Option<String> {
    if let Some(p) = progress {
        return Some(tr!("Downloading {percent}%", percent = (p * 100.0) as u32));
    }
    match a {
        Availability::Available => Some(tr!("Available")),
        Availability::PartiallyAvailable => Some(tr!("Partly available")),
        // Seerr's "processing": approved, not on the server yet.
        Availability::Processing => Some(tr!("Requested")),
        Availability::Pending => Some(tr!("Waiting for approval")),
        Availability::Blocklisted => Some(tr!("Blocked")),
        Availability::Deleted | Availability::Unknown => None,
    }
}

/// Whether a title (or a season) can be asked for.
pub fn requestable(a: Availability) -> bool {
    matches!(a, Availability::Unknown | Availability::Deleted)
}

/// Seerr errors in plain words.
pub fn plain_error(e: &Error) -> String {
    match e {
        Error::Unauthorized => {
            tr!(
                "Your AlwyWatch session has expired or you're not allowed to do this. Sign in again in Settings."
            )
        }
        Error::NotFound => tr!("Seerr doesn't know this title."),
        Error::Network { .. } => tr!("Can't reach your AlwyWatch server right now."),
        Error::Status { status: 403 } => tr!("Seerr doesn't allow you to do this."),
        Error::Status { status: 409 } => tr!("It has already been requested."),
        Error::Status { status } if *status >= 500 => {
            tr!("Seerr had a problem. Try again in a moment.")
        }
        other => other.to_string(),
    }
}

/// A request's error in plain words. Seerr answers 403 both when the person has no requests
/// left and when they may not request (the bridge passes it on).
pub fn request_error(e: &Error) -> String {
    match e {
        Error::Unauthorized | Error::Status { status: 403 } => {
            tr!(
                "Seerr refused: you may have no requests left, or not be allowed to request. If it keeps happening, sign in again in Settings."
            )
        }
        other => plain_error(other),
    }
}

/// After a request: the Requests page's lists kept in the browser are out of date.
fn forget_requests() {
    state::remove(state::REQUESTS_MINE);
    state::remove(state::REQUESTS_ALL);
}

fn runtime(min: u32) -> String {
    if min >= 60 {
        tr!(
            "{hours} h {minutes} min",
            hours = min / 60,
            minutes = min % 60
        )
    } else {
        tr!("{minutes} min", minutes = min)
    }
}

/// The request button's text for a show: `chosen` seasons picked; none picked asks for the
/// missing ones (every season when nothing of the show is there or asked for yet).
pub fn seasons_button(chosen: usize, has_some: bool) -> String {
    match chosen {
        0 if has_some => tr!("Request the missing seasons"),
        0 => tr!("Request all seasons"),
        n => trn!("Request {n} season", "Request {n} seasons", n),
    }
}

/// A title's page: artwork, details, state, request, cast.
#[component]
pub fn SeerrTitle() -> impl IntoView {
    let params = use_params_map();
    let kind = Memo::new(move |_| match params.read().get("kind").as_deref() {
        Some("tv") => SeerrMediaType::Tv,
        _ => SeerrMediaType::Movie,
    });
    let tmdb = Memo::new(move |_| {
        params
            .read()
            .get("tmdb")
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or(0)
    });
    let reload = RwSignal::new(0u32);
    let message = RwSignal::new(None::<String>);
    let title = LocalResource::new(move || {
        reload.track();
        let (kind, tmdb) = (kind.get(), tmdb.get());
        async move {
            let s = state::ctx().seerr().ok_or_else(no_seerr)?;
            s.title(kind, tmdb).await.map_err(|e| plain_error(&e))
        }
    });
    view! {
        {move || match title.get() {
            None => view! { <Loading /> }.into_any(),
            Some(Err(e)) => view! { <ErrorText text=e /> }.into_any(),
            Some(Ok(t)) => view! { <TitleView t=t message=message reload=reload /> }.into_any(),
        }}
    }
}

#[component]
fn TitleView(
    t: SeerrDetails,
    message: RwSignal<Option<String>>,
    reload: RwSignal<u32>,
) -> impl IntoView {
    let show = t.media_type == SeerrMediaType::Tv;
    let meta = [
        t.year.clone(),
        show.then(|| match t.seasons {
            Some(n) => trn!("{n} season", "{n} seasons", n),
            None => tr!("Show"),
        }),
        t.runtime_min.map(runtime),
        t.rating.map(|r| format!("★ {r:.1}")),
        (!t.genres.is_empty()).then(|| {
            t.genres
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        }),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    // A show: its seasons decide (some can be on the server or asked for, others not).
    let can_request = requestable(t.availability)
        || (show
            && !matches!(
                t.availability,
                Availability::Available | Availability::Blocklisted
            ));
    let label = availability_label(t.availability, t.download_progress);
    let available = t.availability == Availability::Available;
    let cast = t.cast.clone();
    // (Partly) on a server: its library page, where what is there plays.
    let on_server = RwSignal::new(None::<String>);
    if matches!(
        t.availability,
        Availability::PartiallyAvailable | Availability::Available
    ) {
        let (kind, tmdb) = (t.media_type, t.tmdb_id);
        spawn_local(async move {
            let found = library_hrefs(vec![(kind, tmdb)]).await;
            on_server.try_set(found.get(&format!("{kind:?}/{tmdb}")).cloned());
        });
    }
    let saga = t.collection.clone();
    view! {
        <div class="hero" style=t.backdrop_url.clone().map(|b| format!("background-image:{}", state::css_url(&b))).unwrap_or_default()>
            <div class="hero-inner">
                {t.poster_url.clone().map(|p| view! { <img class="hero-poster" src=p alt="" /> })}
                <div>
                    <p class="muted">{match (show, t.availability) {
                        (true, Availability::PartiallyAvailable) => tr!("Show · partly on your servers"),
                        (true, _) => tr!("Show · not on your servers"),
                        _ => tr!("Movie · not on your servers"),

                    }}</p>
                    <h1>{t.title.clone()}</h1>
                    <p class="muted">{meta}</p>
                    {label.map(|l| view! { <span class="state" class:ok=available>{l}</span> })}
                    {move || {
                        let here = on_server.get();
                        (here.is_some() || saga.is_some()).then(|| view! {
                            <div class="actions title-links">
                                {here.map(|h| view! { <A href=h attr:class="button primary">{tr!("Open on your server")}</A> })}
                                {saga.clone().map(|c| view! { <A href=saga_href(c.id) attr:class="button">{saga_label(&c.name)}</A> })}
                            </div>
                        })
                    }}
                    {can_request.then(|| view! { <RequestPanel t=t.clone() message=message reload=reload /> })}
                    {move || message.get().map(|m| view! { <p class="accent">{m}</p> })}
                    {t.tagline.clone().map(|x| view! { <p><strong>{x}</strong></p> })}
                    <p class="overview">{t.overview.clone().unwrap_or_default()}</p>
                </div>
            </div>
        </div>
        {(!cast.is_empty()).then(|| view! {
            <h2>{tr!("Cast")}</h2>
            <crate::ui::RailRow>
                {cast.into_iter().map(|p| view! {
                    <div class="person-card">
                        <div class="face">{p.photo_url.map(|u| view! { <img src=u loading="lazy" alt="" /> })}</div>
                        <div class="title">{p.name}</div>
                        <div class="sub">{p.character.unwrap_or_default()}</div>
                    </div>
                }).collect_view()}
            </crate::ui::RailRow>
        })}
    }
}

/// Request a movie, or all or some seasons of a show (the ones not on the server or asked for).
#[component]
fn RequestPanel(
    t: SeerrDetails,
    message: RwSignal<Option<String>>,
    reload: RwSignal<u32>,
) -> impl IntoView {
    let (kind, id) = (t.media_type, t.tmdb_id);
    let name = t.title.clone();
    let busy = RwSignal::new(false);
    let chosen = RwSignal::new(Vec::<u32>::new());
    let seasons = RwSignal::new(None::<Result<Vec<SeerrSeasonInfo>, String>>);
    if kind == SeerrMediaType::Tv {
        spawn_local(async move {
            let Some(s) = state::ctx().seerr() else {
                return seasons.set(Some(Err(no_seerr())));
            };
            seasons.try_set(Some(s.season_info(id).await.map_err(|e| plain_error(&e))));
        });
    }
    let plan = Memo::new(move |_| {
        seasons.with(|s| match s {
            Some(Ok(info)) => Some(season_plan(info, &[])),
            _ => None,
        })
    });
    let send = move || {
        if busy.get_untracked() {
            return;
        }
        let Some(s) = state::ctx().seerr() else {
            return message.set(Some(no_seerr()));
        };
        busy.set(true);
        let mut numbers = chosen.get_untracked();
        // Nothing picked: the missing seasons (none = every season of a new show).
        if numbers.is_empty()
            && let Some(p) = plan.get_untracked().filter(|p| p.has_some)
        {
            numbers = p.missing;
        }
        numbers.sort_unstable();
        let name = name.clone();
        spawn_local(async move {
            message.set(Some(match s.request(kind, id, &numbers).await {
                Ok(_) => {
                    forget_requests();
                    if numbers.is_empty() {
                        tr!("Requested: {title}", title = name)
                    } else {
                        seasons_requested_message(&numbers)
                    }
                }
                Err(e) => tr!(
                    "Couldn't request {title}: {error}",
                    title = name,
                    error = request_error(&e)
                ),
            }));
            busy.set(false);
            reload.update(|r| *r += 1);
        });
    };
    view! {
        {if kind == SeerrMediaType::Movie {
            view! {
                <div class="actions">
                    <button class="primary" disabled=move || busy.get() on:click=move |_| send()>
                        {move || if busy.get() { tr!("Requesting…") } else { tr!("Request") }}
                    </button>
                </div>
            }.into_any()
        } else {
            view! {
                {move || match (seasons.get(), plan.get()) {
                    (None, _) => view! { <p class="muted">{tr!("Loading seasons…")}</p> }.into_any(),
                    (Some(Err(e)), _) => view! { <ErrorText text=e /> }.into_any(),
                    (_, None) => ().into_any(),
                    (Some(Ok(_)), Some(plan)) => {
                        let send = send.clone();
                        let has_some = plan.has_some;
                        let nothing = plan.missing.is_empty();
                        view! {
                            {if nothing {
                                view! { <p class="muted">{tr!("Every season is on your servers or already requested.")}</p> }.into_any()
                            } else {
                                view! {
                                    <div class="actions">
                                        <button class="primary" disabled=move || busy.get() on:click=move |_| send()>
                                            {move || if busy.get() { tr!("Requesting…") } else { seasons_button(chosen.with(Vec::len), has_some) }}
                                        </button>
                                    </div>
                                }.into_any()
                            }}
                            <div class="chips">
                                {plan.seasons.into_iter().map(|s| {
                                    let n = s.number;
                                    let open = s.state == SeasonState::Requestable;
                                    // On the server or asked for already: said, not selectable.
                                    let text = if open { s.name.clone() } else { format!("{} · {}", s.name, s.label) };
                                    view! {
                                        <button class="chip" disabled=!open
                                            class:selected=move || chosen.with(|c| c.contains(&n))
                                            on:click=move |_| chosen.update(|c| {
                                                if let Some(i) = c.iter().position(|x| *x == n) { c.remove(i); } else { c.push(n); }
                                            })>
                                            {move || if chosen.with(|c| c.contains(&n)) { "✓ " } else { "" }}{text}
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                            {(!nothing).then(|| view! {
                                <p class="muted small">{if has_some {
                                    tr!("Pick seasons to request only those, or none for all the missing ones.")
                                } else {
                                    tr!("Pick seasons to request only those, or none for all of them.")
                                }}</p>
                            })}
                        }.into_any()
                    }
                }}
            }.into_any()
        }}
    }
}

/// "+ More seasons" next to a library show's season chips: only once Seerr says some season
/// can be requested (looked up after the page shows; nothing while it loads or if Seerr can't
/// be reached). `have`: the season numbers the library shows.
#[component]
pub fn MoreSeasons(tmdb: i64, show: String, have: Signal<Vec<u32>>) -> impl IntoView {
    let info = RwSignal::new(None::<Vec<SeerrSeasonInfo>>);
    if let Some(s) = state::ctx().seerr() {
        spawn_local(async move {
            if let Ok(i) = s.season_info(tmdb).await {
                info.try_set(Some(i));
            }
        });
    }
    let plan = Memo::new(move |_| info.with(|i| i.as_ref().map(|i| season_plan(i, &have.get()))));
    let open = RwSignal::new(false);
    let button = NodeRef::<leptos::html::Button>::new();
    let show = StoredValue::new(show);
    let close = move || {
        open.set(false);
        // Back on the button (still there when something is left to request).
        if let Some(b) = button.get_untracked() {
            let _ = b.focus();
        }
    };
    view! {
        {move || plan.with(|p| p.as_ref().is_some_and(|p| !p.missing.is_empty())).then(|| view! {
            <button class="small subtle more-seasons" node_ref=button on:click=move |_| open.set(true)>
                {tr!("+ More seasons")}
            </button>
        })}
        {move || open.get().then(|| view! {
            <SeasonsDialog tmdb=tmdb show=show.get_value() info=info plan=plan on_close=close />
        })}
    }
}

/// The "Request more seasons" dialog: every season with its state; the missing ones can be
/// ticked and requested.
#[component]
fn SeasonsDialog(
    tmdb: i64,
    show: String,
    info: RwSignal<Option<Vec<SeerrSeasonInfo>>>,
    plan: Memo<Option<seerr::SeasonPlan>>,
    on_close: impl Fn() + Clone + Send + Sync + 'static,
) -> impl IntoView {
    let chosen = RwSignal::new(Vec::<u32>::new());
    let busy = RwSignal::new(false);
    let done = RwSignal::new(None::<String>);
    let error = RwSignal::new(None::<String>);
    let send = move || {
        let mut numbers = chosen.get_untracked();
        if busy.get_untracked() || numbers.is_empty() {
            return;
        }
        let Some(s) = state::ctx().seerr() else {
            return error.set(Some(no_seerr()));
        };
        numbers.sort_unstable();
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match s.request(SeerrMediaType::Tv, tmdb, &numbers).await {
                Ok(_) => {
                    forget_requests();
                    done.try_set(Some(seasons_requested_message(&numbers)));
                    chosen.try_set(Vec::new());
                    // The new states (requested, maybe downloading already).
                    if let Ok(i) = s.season_info(tmdb).await {
                        info.try_set(Some(i));
                    }
                }
                Err(e) => {
                    error.try_set(Some(tr!(
                        "Couldn't request them: {error}",
                        error = request_error(&e)
                    )));
                }
            }
            busy.try_set(false);
        });
    };
    // Focus starts on the first season that can be requested.
    let first = NodeRef::<leptos::html::Button>::new();
    Effect::new(move |_| {
        if let Some(b) = first.get() {
            let _ = b.focus();
        }
    });
    let (close1, close2, close3) = (on_close.clone(), on_close.clone(), on_close);
    view! {
        <div class="dialog-back" on:click=move |_| close1()
            on:keydown=move |ev| if ev.key() == "Escape" { close2() }>
            <div class="dialog seasons" role="dialog" aria-modal="true" aria-labelledby="more-seasons-title"
                on:click=|ev| ev.stop_propagation()>
                <h2 id="more-seasons-title">{tr!("Request more seasons")}</h2>
                <p class="muted">{show}</p>
                {move || plan.get().map(|p| {
                    let first_open = p.missing.first().copied();
                    let missing = p.missing.clone();
                    let n = move || chosen.with(Vec::len);
                    view! {
                        <div class="pick-list">
                            {p.seasons.into_iter().map(|s| {
                                let num = s.number;
                                if s.state == SeasonState::Requestable {
                                    let on = move || chosen.with(|c| c.contains(&num));
                                    let r = if Some(num) == first_open {
                                        first
                                    } else {
                                        NodeRef::new()
                                    };
                                    view! {
                                        <button class="pick-row" node_ref=r class:selected=on role="checkbox" aria-checked=move || on().to_string()
                                            on:click=move |_| chosen.update(|c| {
                                                if let Some(i) = c.iter().position(|x| *x == num) { c.remove(i); } else { c.push(num); }
                                            })>
                                            <span class="box" aria-hidden="true">{move || if on() { "✓" } else { "" }}</span>
                                            <span class="pname">{s.name.clone()}</span>
                                            <span class="muted small">{s.label.clone()}</span>
                                        </button>
                                    }
                                    .into_any()
                                } else {
                                    let mark = match s.state {
                                        SeasonState::OnServer | SeasonState::PartlyOnServer => "✓",
                                        SeasonState::Downloading => "↓",
                                        SeasonState::Requested => "…",
                                        _ => "–",
                                    };
                                    view! {
                                        <div class="pick-row static" class:here=s.state == SeasonState::OnServer>
                                            <span class="check" aria-hidden="true">{mark}</span>
                                            <span class="pname">{s.name.clone()}</span>
                                            <span class="muted small">{s.label.clone()}</span>
                                        </div>
                                    }.into_any()
                                }
                            }).collect_view()}
                        </div>
                        {move || done.get().map(|m| view! { <p class="accent">{m}</p> })}
                        {move || error.get().map(|e| view! { <ErrorText text=e /> })}
                        <div class="actions">
                            {(missing.len() > 1).then(|| {
                                let all = missing.clone();
                                view! { <button on:click=move |_| chosen.set(all.clone())>{tr!("Select all missing")}</button> }
                            })}
                            {(!missing.is_empty()).then(|| view! {
                                <button class="primary" disabled=move || busy.get() || n() == 0 on:click=move |_| send()>
                                    {move || match (busy.get(), n()) {
                                        (true, _) => tr!("Requesting…"),
                                        (_, k) => trn!("Request {n} season", "Request {n} seasons", k),
                                    }}
                                </button>
                            })}
                            <button on:click={
                                let c = close3.clone();
                                move |_| c()
                            }>{tr!("Close")}</button>
                        </div>
                    }
                })}
            </div>
        </div>
    }
}

fn request_state(r: &SeerrRequest) -> String {
    match (r.status, r.availability) {
        (_, Availability::Available) | (RequestStatus::Completed, _) => tr!("Available"),
        (_, Availability::PartiallyAvailable) => tr!("Partly available"),
        (RequestStatus::Declined, _) => tr!("Declined"),
        (RequestStatus::Failed, _) => tr!("Failed"),
        (RequestStatus::Pending, _) => tr!("Waiting for approval"),
        (_, Availability::Processing) => tr!("Downloading"),
        _ => tr!("Approved"),
    }
}

use crate::state::REQUESTS_EVERYONE as EVERYONE;

/// A request's date ("2026-11-03T…" from Seerr) in the browser's language, as Leaving soon.
fn request_day(iso: &str) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(iso));
    if d.get_time().is_nan() {
        return iso.chars().take(10).collect();
    }
    String::from(d.to_locale_date_string(i18n::language(), &wasm_bindgen::JsValue::UNDEFINED))
}

/// Seerr requests: mine, or everyone's for the people Seerr lets see them.
#[component]
pub fn Requests() -> impl IntoView {
    let all = RwSignal::new(false);
    // Whether Seerr lets this person see everyone's requests (remembered, then checked).
    let everyone_ok = RwSignal::new(state::load::<bool>(EVERYONE).unwrap_or(false));
    spawn_local(async move {
        let Some(s) = state::ctx().seerr() else {
            return;
        };
        let ok = match s.requests(1, true).await {
            Ok(_) => true,
            Err(Error::Unauthorized | Error::Status { status: 403 }) => false,
            // Can't tell (offline…): keep what we knew.
            Err(_) => return,
        };
        state::save(EVERYONE, &ok);
        everyone_ok.set(ok);
        if !ok {
            all.set(false);
        }
    });
    // The last list of each kind at once (Seerr answers slowly: one call per title), then
    // the fresh one. Only the latest load counts.
    let cache = |all: bool| {
        if all {
            state::REQUESTS_ALL
        } else {
            state::REQUESTS_MINE
        }
    };
    let load = move |all: bool| {
        state::load::<Vec<SeerrRequest>>(cache(all)).map(|l| Ok(seerr::group_requests(l)))
    };
    let list = RwSignal::new(load(false));
    // Where each title is in the libraries: its card opens the page where it plays.
    let places = RwSignal::new(HashMap::<String, String>::new());
    Effect::new(move |_| {
        let titles: Vec<(SeerrMediaType, i64)> = list.with(|l| match l {
            Some(Ok(l)) => l.iter().map(|r| (r.media_type, r.tmdb_id)).collect(),
            _ => Vec::new(),
        });
        if !titles.is_empty() {
            spawn_local(async move {
                let found = library_hrefs(titles).await;
                places.try_update(|p| p.extend(found));
            });
        }
    });
    let seq = StoredValue::new(0u32);
    Effect::new(move |_| {
        let all = all.get();
        seq.update_value(|s| *s += 1);
        let mine = seq.get_value();
        list.set(load(all));
        spawn_local(async move {
            let Some(s) = state::ctx().seerr() else {
                return list.set(Some(Err(no_seerr())));
            };
            let res = s.requests(50, all).await;
            if seq.try_get_value() != Some(mine) {
                return;
            }
            let res = match res {
                Ok(l) => {
                    state::save(cache(all), &l);
                    Ok(l)
                }
                Err(Error::Unauthorized | Error::Status { status: 403 }) if all => {
                    state::save(EVERYONE, &false);
                    everyone_ok.set(false);
                    Err(tr!("Only Seerr managers can see everyone's requests."))
                }
                // Offline: what was shown stays (a refusal still says so).
                Err(Error::Network { .. }) if list.with_untracked(|l| matches!(l, Some(Ok(_)))) => {
                    return;
                }
                Err(Error::NotFound) => {
                    Err(tr!("Requests aren't available on this AlwyWatch server."))
                }
                Err(e) => Err(plain_error(&e)),
            };
            list.set(Some(res));
        });
    });
    view! {
        <h1>{tr!("Requests")}</h1>
        <p class="muted">{tr!("To ask for something new, search for it.")}</p>
        <Show when=move || everyone_ok.get()>
            <div class="chips">
                <button class="chip" class:selected=move || !all.get() on:click=move |_| all.set(false)>{tr!("Mine")}</button>
                <button class="chip" class:selected=move || all.get() on:click=move |_| all.set(true)>{tr!("Everyone")}</button>
            </div>
        </Show>
        {move || match list.get() {
            None => view! { <Loading /> }.into_any(),
            Some(Err(e)) => view! { <ErrorText text=e /> }.into_any(),
            Some(Ok(l)) if l.is_empty() => view! {
                <p class="muted">{if all.get_untracked() { tr!("No requests.") } else { tr!("You haven't requested anything yet.") }}</p>
            }.into_any(),
            Some(Ok(l)) => {
                let who = all.get_untracked();
                view! {
                    <div class="leaving">
                        {l.into_iter().map(|r| {
                            let state = request_state(&r);
                            let show = r.media_type == SeerrMediaType::Tv;
                            let day = request_day(&r.created_at);
                            let by = who && !r.requested_by.is_empty();
                            let line = match (show, by) {
                                (true, true) => tr!("Show · requested {date} by {name}", date = day, name = r.requested_by),
                                (true, false) => tr!("Show · requested {date}", date = day),
                                (false, true) => tr!("Movie · requested {date} by {name}", date = day, name = r.requested_by),
                                (false, false) => tr!("Movie · requested {date}", date = day),
                            };
                            view! {
                                <A href={
                                    let (seerr, place) = (title_href(r.media_type, r.tmdb_id), format!("{:?}/{}", r.media_type, r.tmdb_id));
                                    move || places.with(|p| p.get(&place).cloned()).unwrap_or_else(|| seerr.clone())
                                } attr:class="leaving-row">
                                    {r.poster_url.clone().map(|p| view! { <img src=p loading="lazy" alt="" /> })}
                                    <div class="leaving-info">
                                        <strong>{r.title.clone().unwrap_or_else(|| format!("TMDB {}", r.tmdb_id))}</strong>
                                        <span class="muted">{line}</span>

                                        <span class="accent small">{state}</span>
                                    </div>
                                </A>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_follow_the_apps() {
        assert_eq!(
            availability_label(Availability::Processing, None).as_deref(),
            Some("Requested")
        );
        assert_eq!(
            availability_label(Availability::Unknown, Some(0.42)).as_deref(),
            Some("Downloading 42%")
        );
        assert_eq!(availability_label(Availability::Deleted, None), None);
        assert!(requestable(Availability::Deleted));
        assert!(!requestable(Availability::Pending));
        assert_eq!(seasons_button(0, false), "Request all seasons");
        assert_eq!(seasons_button(0, true), "Request the missing seasons");
        assert_eq!(seasons_button(2, true), "Request 2 seasons");
        assert!(request_error(&Error::Unauthorized).contains("no requests left"));
        assert_eq!(title_href(SeerrMediaType::Tv, 12), "/request/tv/12");
        assert_eq!(saga_href(9), "/saga/9");
        assert_eq!(saga_label("Alien"), "Saga · Alien");
        assert_eq!(saga_label("Harry Potter - Saga"), "Harry Potter - Saga");
        assert_eq!(saga_label("Toy Story Collection"), "Toy Story Collection");
    }
}
