// SPDX-License-Identifier: AGPL-3.0-or-later
//! Home, libraries, title pages and search. Pages show the last copy kept in the browser at
//! once, then refresh it (never a blank page for data we already had).

use i18n::tr;
use std::collections::HashSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use leptos_router::hooks::{use_params_map, use_query_map};
use media_api::{
    AnyServer, ImageKind, Item, ItemKind, ItemQuery, LibraryKind, MediaServer, RailNote, SortBy,
};
use seerr::{Availability, SeerrMediaType, SeerrResult};

use crate::state::{self, fmt_time};
use crate::ui::{Card, ErrorText, Grid, Loading, Rail};

/// Rows of the home: title, (account, item) cards, 16:9 cards, notes of "recently added" show
/// cards (what is new, what opening them shows).
/// Title, cards (account, item), landscape, notes, where its "See all" goes.
type Rails = Vec<(String, Vec<(String, Item)>, bool, Vec<RailNote>, String)>;

/// Cards in a "recently added" row, and entries asked of a shows library (they come episode
/// by episode and are grouped per show, like the apps).
const LATEST_CARDS: u32 = 20;
const LATEST_SHOWS_FETCH: u32 = 100;

/// One per title across servers (the first server's copy wins).
fn same_as(i: &Item) -> String {
    let p = &i.provider_ids;
    match (&p.tmdb, &p.imdb, &p.tvdb) {
        (Some(t), _, _) => format!("{:?}:tmdb:{t}", i.kind),
        (_, Some(t), _) => format!("{:?}:imdb:{t}", i.kind),
        (_, _, Some(t)) => format!("{:?}:tvdb:{t}", i.kind),
        _ => format!(
            "{:?}:{}:{}",
            i.kind,
            i.title.to_lowercase(),
            i.year.unwrap_or(0)
        ),
    }
}

fn dedupe(list: Vec<(String, Item)>) -> Vec<(String, Item)> {
    let mut seen = HashSet::new();
    list.into_iter()
        .filter(|(_, i)| seen.insert(same_as(i)))
        .collect()
}

/// A library chip on the home page: account, library id, title, server name.
type LibLink = (String, String, String, String);

/// One server's part of the home page.
struct ServerHome {
    resume: Vec<(String, Item)>,
    next: Vec<(String, Item)>,
    latest: Rails,
    libs: Vec<LibLink>,
    /// The server answered (else its part is left as it was).
    ok: bool,
}

async fn load_server(c: &state::Ctx, acc: String, srv: AnyServer) -> ServerHome {
    let ((resume, next), libs) = state::join2(
        state::join2(srv.resume(16), srv.next_up(16)),
        srv.libraries(),
    )
    .await;
    if let Err(e) = &resume {
        c.failed(&acc, e);
    }
    let ok = resume.is_ok() || libs.is_ok();
    let libs: Vec<_> = libs
        .unwrap_or_default()
        .into_iter()
        .filter(|l| l.kind != LibraryKind::Other)
        .collect();
    let latest = state::join_all(libs.iter().map(|l| {
        let fetch = if l.kind == LibraryKind::Shows {
            LATEST_SHOWS_FETCH
        } else {
            LATEST_CARDS
        };
        srv.latest(&l.id, fetch)
    }))
    .await;
    let tag = |l: Vec<Item>| l.into_iter().map(|i| (acc.clone(), i)).collect::<Vec<_>>();
    let server = srv.session().server_name.clone();
    ServerHome {
        resume: tag(resume.unwrap_or_default()),
        next: tag(next.unwrap_or_default()),
        latest: libs
            .iter()
            .zip(latest)
            .filter_map(|(lib, l)| {
                let r = media_api::recently_added(l.ok()?, LATEST_CARDS as usize);
                Some((
                    tr!("Recently added · {library}", library = lib.title),
                    tag(r.items),
                    false,
                    r.notes,
                    format!("/library/{acc}/{}?sort=added", lib.id),
                ))
            })
            .collect(),
        libs: libs
            .into_iter()
            .map(|l| (acc.clone(), l.id, l.title, server.clone()))
            .collect(),
        ok,
    }
}

/// Every server at once. None when none of them answered (offline: keep what we had).
async fn load_home(c: &state::Ctx) -> Option<(Rails, Vec<LibLink>)> {
    let parts = state::join_all(
        c.servers()
            .into_iter()
            .map(|(acc, srv)| load_server(c, acc, srv)),
    )
    .await;
    if !parts.iter().any(|p| p.ok) {
        return None;
    }
    let mut resume = Vec::new();
    let mut next = Vec::new();
    let mut latest: Rails = Vec::new();
    let mut libs = Vec::new();
    for p in parts {
        resume.extend(p.resume);
        next.extend(p.next);
        latest.extend(p.latest);
        libs.extend(p.libs);
    }
    resume.sort_by_key(|(_, i)| std::cmp::Reverse(i.user.last_played_at.unwrap_or(0)));
    // One row, like the apps: what is in progress, then the next episode of the shows that
    // have nothing in progress (the servers' "next up" repeats the in-progress ones).
    // A watched title reopened for a moment is not "in progress" (same rule as the apps:
    // under two minutes or 5 %).
    resume.retain(|(_, i)| {
        !(i.user.played && i.user.position_ms < (i.runtime_ms.unwrap_or(0) / 20).max(120_000))
    });
    let mut watching = dedupe(resume);
    let started: HashSet<String> = watching
        .iter()
        .flat_map(|(_, i)| [Some(i.id.clone()), i.series_id.clone()])
        .flatten()
        .collect();
    watching.extend(dedupe(next).into_iter().filter(|(_, i)| {
        !started.contains(&i.id) && i.series_id.as_ref().is_none_or(|s| !started.contains(s))
    }));
    // Posters, as in the apps (the show's name on an episode).
    let mut rails: Rails = vec![(
        tr!("Continue watching"),
        watching,
        false,
        vec![],
        "/continue".into(),
    )];
    rails.extend(latest);
    Some((rails, libs))
}

#[component]
pub fn Home() -> impl IntoView {
    let c = state::ctx();
    // The last copy at once, then the servers' (one request per server at the same time).
    let rails = RwSignal::new(state::load::<Rails>(state::HOME));
    let libs = RwSignal::new(state::load::<Vec<LibLink>>(state::LIBS).unwrap_or_default());
    let offline = RwSignal::new(false);
    // Only the latest load counts (an account added or removed meanwhile starts another).
    let seq = StoredValue::new(0u32);
    Effect::new(move |_| {
        c.accounts.track();
        let c = c.clone();
        seq.update_value(|s| *s += 1);
        let mine = seq.get_value();
        spawn_local(async move {
            let res = load_home(&c).await;
            if seq.try_get_value() != Some(mine) {
                return;
            }
            match res {
                Some((fresh, l)) => {
                    state::save(state::HOME, &fresh);
                    state::save(state::LIBS, &l);
                    offline.set(false);
                    rails.set(Some(fresh));
                    libs.set(l);
                }
                None => {
                    offline.set(true);
                    rails.update(|r| {
                        r.get_or_insert_with(Vec::new);
                    });
                }
            }
        });
    });
    view! {
        <div class="chips">
            {move || libs.get().into_iter().map(|(acc, id, title, server)| view! {
                <A href=format!("/library/{acc}/{id}") attr:class="chip" attr:title=server>{title}</A>
            }).collect_view()}
        </div>
        {move || offline.get().then(|| view! {
            <ErrorText text=tr!("Can't reach your servers right now. Showing what was here last time.") />
        })}
        {move || match rails.get() {
            None => view! { <Loading /> }.into_any(),
            Some(r) if r.iter().all(|(_, items, _, _, _)| items.is_empty()) && !offline.get() => view! {
                <p class="muted">{tr!("Nothing here yet. Your libraries are above.")}</p>
            }.into_any(),
            // Leaving soon right after "Continue watching", as in the apps.
            Some(r) => r.into_iter().enumerate().map(|(n, (t, items, wide, notes, link))| view! {
                <Rail title=t items=items wide=wide notes=notes see_all=link />
                {(n == 0).then(|| view! { <crate::keep::LeavingRail /> })}
            }).collect_view().into_any(),
        }}
    }
}

thread_local! {
    /// The library page left last: its address, titles, total and scroll position.
    #[allow(clippy::type_complexity)]
    static LIBRARY_MEMO: std::cell::RefCell<Option<(String, Vec<(String, Item)>, Option<u32>, f64)>> =
        const { std::cell::RefCell::new(None) };
}

#[component]
pub fn Library() -> impl IntoView {
    let params = use_params_map();
    let acc = move || params.read().get("acc").unwrap_or_default();
    let id = move || params.read().get("id").unwrap_or_default();
    let items = RwSignal::new(Vec::<(String, Item)>::new());
    let total = RwSignal::new(None::<u32>);
    let error = RwSignal::new(None::<String>);
    // The view is in the address (Back from a title comes back to it, a link keeps it); the
    // order last picked is remembered per browser for libraries opened without one.
    let query = use_query_map();
    let sort = Memo::new(move |_| {
        query
            .read()
            .get("sort")
            .or_else(|| state::load::<String>("aw.lib.sort"))
            .unwrap_or_else(|| "title".into())
    });
    let genre = Memo::new(move |_| query.read().get("genre").unwrap_or_default());
    let unwatched = Memo::new(move |_| query.read().get("unwatched").as_deref() == Some("1"));
    let navigate = leptos_router::hooks::use_navigate();
    let set_view = move |sort: String, genre: String, unwatched: bool| {
        let mut q = format!("?sort={}", net::percent_encode(&sort));
        if !genre.is_empty() {
            q += &format!("&genre={}", net::percent_encode(&genre));
        }
        if unwatched {
            q += "&unwatched=1";
        }
        navigate(
            &q,
            leptos_router::NavigateOptions {
                replace: true,
                scroll: false,
                ..Default::default()
            },
        );
    };
    let (set1, set2, set3) = (set_view.clone(), set_view.clone(), set_view);
    // Back from a title: what was loaded ("More" included) and where the page was.
    let view_key = move || {
        format!(
            "{}/{}|{}|{}|{}",
            acc(),
            id(),
            sort.get(),
            genre.get(),
            unwatched.get()
        )
    };
    let key = untrack(view_key);
    let cur = StoredValue::new(key.clone());
    let restored = LIBRARY_MEMO
        .with(|m| m.borrow_mut().take())
        .filter(|(k, ..)| *k == key);
    let skip_first = StoredValue::new(restored.is_some());
    if let Some((_, list, t, y)) = restored {
        items.set(list);
        total.set(t);
        spawn_local(async move {
            state::sleep(0).await;
            if let Some(w) = web_sys::window() {
                w.scroll_to_with_x_and_y(0.0, y);
            }
        });
    }
    on_cleanup(move || {
        let y = web_sys::window()
            .and_then(|w| w.scroll_y().ok())
            .unwrap_or(0.0);
        if let (Some(list), Some(t), Some(k)) = (
            items.try_get_untracked(),
            total.try_get_untracked(),
            cur.try_get_value(),
        ) {
            LIBRARY_MEMO.with(|m| *m.borrow_mut() = Some((k, list, t, y)));
        }
    });
    // Only the latest load counts (a quick change of sort or filter overtakes the last one).
    let seq = StoredValue::new(0u32);
    let genres = LocalResource::new(move || {
        let (acc, id) = (acc(), id());
        async move {
            let srv = state::ctx().server(&acc)?;
            srv.genres(&id).await.ok()
        }
    });
    let load = move |start: u32| {
        let c = state::ctx();
        let (acc, id) = (acc(), id());
        let mut q = ItemQuery::library(id, start, 60);
        (q.sort_by, q.descending) = match sort.get_untracked().as_str() {
            "added" => (SortBy::DateAdded, true),
            "released" => (SortBy::ReleaseDate, true),
            "rating" => (SortBy::Rating, true),
            "random" => (SortBy::Random, false),
            _ => (SortBy::Title, false),
        };
        q.unplayed_only = unwatched.get_untracked();
        q.genre_id = Some(genre.get_untracked()).filter(|g| !g.is_empty());
        seq.update_value(|s| *s += 1);
        let mine = seq.get_value();
        error.set(None);
        spawn_local(async move {
            let Some(srv) = c.server(&acc) else {
                total.try_set(Some(0));
                return error.set(Some(tr!("This account isn't signed in any more.")));
            };
            let res = srv.items(&q).await;
            if seq.try_get_value() != Some(mine) {
                return;
            }
            match res {
                Ok(p) => {
                    total.set(Some(p.total));
                    if start == 0 {
                        items.set(Vec::new());
                    }
                    items.update(|v| v.extend(p.items.into_iter().map(|i| (acc.clone(), i))));
                }
                Err(e) => {
                    c.failed(&acc, &e);
                    // Not loading any more: what's shown stays, with the reason.
                    total.update(|t| {
                        t.get_or_insert(0);
                    });
                    error.set(Some(state::plain(&e)));
                }
            }
        });
    };
    Effect::new(move |prev: Option<(String, String)>| {
        // Another library (the page stays, only its address changes): not the last library's
        // titles while it loads.
        let here = (acc(), id());
        if prev.as_ref().is_some_and(|p| *p != here) {
            items.set(Vec::new());
        }
        cur.set_value(view_key());
        if skip_first.get_value() {
            skip_first.set_value(false);
            return here;
        }
        total.set(None);
        load(0);
        here
    });
    view! {
        <div class="chips">
            <select class="chip" aria-label=tr!("Sort by") prop:value=move || sort.get() on:change=move |ev| {
                let v = event_target_value(&ev);
                state::save("aw.lib.sort", &v);
                set1(v, genre.get_untracked(), unwatched.get_untracked());
            }>
                <option value="title">{tr!("A to Z")}</option>
                <option value="added">{tr!("Recently added")}</option>
                <option value="released">{tr!("Newest releases")}</option>
                <option value="rating">{tr!("Best rated")}</option>
                <option value="random">{tr!("Random")}</option>
            </select>
            <select class="chip" aria-label=tr!("Genre") prop:value=move || genre.get() on:change=move |ev| set2(sort.get_untracked(), event_target_value(&ev), unwatched.get_untracked())>
                <option value="">{tr!("All genres")}</option>
                {move || genres.get().flatten().unwrap_or_default().into_iter().map(|g| view! {
                    <option value=g.id>{g.name}</option>
                }).collect_view()}
            </select>
            <button class="chip" class:selected=move || unwatched.get() on:click=move |_| set3(sort.get_untracked(), genre.get_untracked(), !unwatched.get_untracked())>{tr!("Not watched")}</button>
        </div>
        {move || total.get().is_none().then(|| view! { <Loading /> })}
        {move || (total.get() == Some(0) && error.with(Option::is_none)).then(|| view! { <p class="muted">{tr!("Nothing matches.")}</p> })}
        {move || view! { <Grid items=items.get() /> }}
        {move || {
            let n = items.with(|v| v.len() as u32);
            total.get().filter(|&t| t > n).map(|_| view! {
                <button class="more" on:click=move |_| load(n)>{tr!("More")}</button>
            })
        }}
        {move || error.get().map(|e| view! { <ErrorText text=e /> })}
    }
}

#[component]
pub fn Detail() -> impl IntoView {
    let params = use_params_map();
    // Marking episodes watched or unwatched reloads the seasons and episodes shown.
    provide_context(crate::extra::Refresh(RwSignal::new(0)));
    let acc = Memo::new(move |_| params.read().get("acc").unwrap_or_default());
    let id = Memo::new(move |_| params.read().get("id").unwrap_or_default());
    let item = LocalResource::new(move || {
        let (acc, id) = (acc.get(), id.get());
        async move {
            let c = state::ctx();
            let srv = c
                .server(&acc)
                .ok_or_else(|| tr!("This account isn't signed in any more."))?;
            match srv.item(&id).await {
                Ok(i) => Ok((srv, i)),
                Err(e) => {
                    c.failed(&acc, &e);
                    Err(state::plain(&e))
                }
            }
        }
    });
    // A season is shown on its show's page, that season picked (the show, its other seasons
    // and their episodes in one place, as in the apps).
    let navigate = leptos_router::hooks::use_navigate();
    Effect::new(move |_| {
        if let Some(Ok((_, i))) = item.get()
            && i.kind == ItemKind::Season
            && let Some(show) = &i.series_id
        {
            navigate(
                &format!("/item/{}/{show}?season={}", acc.get_untracked(), i.id),
                leptos_router::NavigateOptions {
                    replace: true,
                    ..Default::default()
                },
            );
        }
    });
    view! {
        {move || match item.get() {
            None => view! { <Loading /> }.into_any(),
            Some(Err(e)) => view! { <ErrorText text=e.to_string() /> }.into_any(),
            Some(Ok((_, i))) if i.kind == ItemKind::Season && i.series_id.is_some() => view! { <Loading /> }.into_any(),
            Some(Ok((srv, i))) => {
                let acc = acc.get();
                let backdrop = state::image(&srv, &i, &[ImageKind::Backdrop, ImageKind::Thumb], 960);
                let poster = state::image(&srv, &i, &[ImageKind::Primary, ImageKind::SeriesPrimary], 240);
                let meta = [
                    i.year.map(|y| y.to_string()),
                    i.runtime_ms.map(|r| tr!("{minutes} min", minutes = r / 60_000)),
                    i.official_rating.clone(),
                    i.community_rating.map(|r| format!("★ {r:.1}")),
                    (!i.genres.is_empty()).then(|| i.genres.join(", ")),
                ].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                let playable = matches!(i.kind, ItemKind::Movie | ItemKind::Episode);
                let pos = i.user.position_ms;
                let base = format!("/play/{acc}/{}", i.id);
                // Where an episode or a season belongs: the show, then the season, as links.
                let show_href = i.series_id.as_ref().map(|sid| format!("/item/{acc}/{sid}"));
                let heading = match i.kind {
                    ItemKind::Episode | ItemKind::Season => {
                        let show = i.series_title.clone().unwrap_or_default();
                        let season = (i.kind == ItemKind::Episode).then(|| {
                            let label = tr!("Season {number}", number = i.season_number.unwrap_or(0));
                            match (&show_href, &i.season_id) {
                                (Some(h), Some(sid)) => view! { " › "<A href=format!("{h}?season={sid}")>{label}</A> }.into_any(),
                                _ => view! { " › "{label} }.into_any(),
                            }
                        });
                        let episode = i.episode_number.filter(|_| i.kind == ItemKind::Episode)
                            .map(|n| format!(" › {}", tr!("Episode {number}", number = n)));
                        Some(view! {
                            <nav class="crumbs" aria-label=tr!("Show")>
                                {match show_href.clone() {
                                    Some(h) => view! { <A href=h>{show}</A> }.into_any(),
                                    None => view! { <span>{show}</span> }.into_any(),
                                }}
                                {season}{episode}
                            </nav>
                        })
                    }
                    _ => None,
                };
                view! {
                    <div class="hero" style=backdrop.map(|b| format!("background-image:{}", state::css_url(&b))).unwrap_or_default()>
                        <div class="hero-inner">
                            {poster.map(|p| view! { <img class="hero-poster" src=p alt="" /> })}
                            <div>
                                {heading}
                                <h1>{i.title.clone()}</h1>
                                <p class="muted">{meta}</p>
                                {playable.then(|| view! {
                                    <div class="actions">
                                        {if pos > 0 {
                                            view! {
                                                <A href=format!("{base}?t={pos}") attr:class="button primary">{tr!("Resume from {time}", time = fmt_time(pos))}</A>
                                                <A href=format!("{base}?t=0") attr:class="button">{tr!("From start")}</A>
                                            }.into_any()
                                        } else {
                                            view! { <A href=format!("{base}?t=0") attr:class="button primary">{tr!("Play")}</A> }.into_any()
                                        }}
                                        {state::ctx().keep.get_untracked().is_some().then(|| view! {
                                            <crate::together::TogetherButton srv=srv.clone() item=i.clone() />
                                        })}
                                    </div>
                                })}
                                {(i.kind == ItemKind::Series).then(|| view! { <ShowPlay acc=acc.clone() series=i.id.clone() /> })}
                                {(i.kind != ItemKind::Season).then(|| view! {
                                    <crate::extra::ItemActions acc=acc.clone() item=i.clone() />
                                })}
                                {(i.kind == ItemKind::Season).then(|| view! {
                                    <crate::extra::MarkAll acc=acc.clone() id=i.id.clone() noun="season"
                                        what=tr!("{season} of {show}", season = i.title, show = i.series_title.clone().unwrap_or_default()) watched=i.user.played />
                                })}
                                <p class="overview">{i.overview.clone().unwrap_or_default()}</p>
                                {(i.kind == ItemKind::Movie).then(|| view! {
                                    <SagaLink tmdb=i.provider_ids.tmdb.as_deref().and_then(|t| t.parse().ok()) />
                                })}
                                <crate::keep::KeepPanel item=i.clone() />
                            </div>
                        </div>
                    </div>
                    {(i.kind == ItemKind::Series).then(|| view! { <Seasons acc=acc.clone() series=i.id.clone() show=i.title.clone()
                        tmdb=i.provider_ids.tmdb.as_deref().and_then(|t| t.parse().ok()) /> })}
                    {(i.kind == ItemKind::Season).then(|| view! {
                        <Episodes acc=acc.clone() series=i.series_id.clone().unwrap_or_default() season=i.id.clone() />
                    })}
                    // The rest of the season under an episode, this one marked.
                    {(i.kind == ItemKind::Episode).then(|| i.series_id.clone().zip(i.season_id.clone())).flatten().map(|(series, season)| view! {
                        <h2>{tr!("Season {number}", number = i.season_number.unwrap_or(0))}</h2>
                        <Episodes acc=acc.clone() series=series season=season current=i.id.clone() />
                    })}
                    {matches!(i.kind, ItemKind::Movie | ItemKind::Series).then(|| view! {
                        <crate::extra::Cast acc=acc.clone() id=i.id.clone() />
                    })}
                }.into_any()
            }
        }}
    }
}

/// A show's Play: the episode in progress (the home's Continue watching), else the first one
/// not watched, else the first.
#[component]
fn ShowPlay(acc: String, series: String) -> impl IntoView {
    let a = acc.clone();
    let next = LocalResource::new(move || {
        let (acc, series) = (a.clone(), series.clone());
        async move {
            let started = state::load::<Rails>(state::HOME).and_then(|r| {
                r.into_iter()
                    .find(|(_, _, _, _, l)| l == "/continue")?
                    .1
                    .into_iter()
                    .find(|(x, i)| *x == acc && i.series_id.as_deref() == Some(series.as_str()))
            });
            if let Some((_, e)) = started {
                return Some(e);
            }
            let srv = state::ctx().server(&acc)?;
            let seasons = srv.seasons(&series).await.ok()?;
            // Specials (season 0) last.
            let mut order: Vec<_> = seasons.iter().collect();
            order.sort_by_key(|s| (s.season_number == Some(0), s.season_number));
            let mut first = None;
            for s in order
                .into_iter()
                .filter(|s| !s.user.played)
                .chain(seasons.first())
            {
                let eps = srv.episodes(&series, &s.id).await.ok()?;
                if first.is_none() {
                    first = eps.first().cloned();
                }
                if let Some(e) = eps.into_iter().find(|e| !e.user.played) {
                    return Some(e);
                }
            }
            first
        }
    });
    move || {
        let e = next.get().flatten()?;
        let (s, n) = (e.season_number.unwrap_or(0), e.episode_number.unwrap_or(0));
        let pos = e.user.position_ms;
        let label = if pos > 0 {
            tr!("Resume S{season} E{episode}", season = s, episode = n)
        } else {
            tr!("Play S{season} E{episode}", season = s, episode = n)
        };
        Some(view! {
            <div class="actions">
                <A href=format!("/play/{acc}/{}?t={pos}", e.id) attr:class="button primary">{label}</A>
            </div>
        })
    }
}

/// A movie's saga (TMDB collection, through Seerr): a link to its page under the overview.
/// Nothing without Seerr or on an error.
#[component]
fn SagaLink(tmdb: Option<i64>) -> impl IntoView {
    let saga = LocalResource::new(move || async move {
        let s = state::ctx().seerr()?;
        s.title(SeerrMediaType::Movie, tmdb?).await.ok()?.collection
    });
    move || {
        saga.get().flatten().map(|c| {
            view! {
                <div class="actions title-links">
                    <A href=crate::request::saga_href(c.id) attr:class="button">{crate::request::saga_label(&c.name)}</A>
                </div>
            }
        })
    }
}

#[component]
fn Seasons(acc: String, series: String, show: String, tmdb: Option<i64>) -> impl IntoView {
    let (a, s) = (acc.clone(), series.clone());
    let refresh = use_context::<crate::extra::Refresh>();
    let seasons = LocalResource::new(move || {
        if let Some(r) = refresh {
            r.0.track();
        }
        let (acc, series) = (a.clone(), s.clone());
        async move {
            let srv = state::ctx().server(&acc)?;
            srv.seasons(&series).await.ok()
        }
    });
    let picked = RwSignal::new(None::<String>);
    // Opened from a "recently added" card: on the season concerned (`?season=`).
    let asked = use_query_map().with_untracked(|q| q.get("season"));
    // The season numbers shown, for "More seasons" (Seerr may follow another media server).
    let have = Signal::derive(move || {
        seasons
            .get()
            .flatten()
            .map(|l| l.iter().filter_map(|s| s.season_number).collect())
            .unwrap_or_default()
    });
    let more_show = show.clone();
    view! {
        {move || matches!(seasons.get(), Some(None)).then(|| view! {
            <ErrorText text=tr!("Couldn't load the seasons.") />
        })}
        <div class="season-row">
        {move || seasons.get().flatten().map(|list| {
            // The first season not fully watched, else the first.
            if picked.get_untracked().is_none() {
                let first = list.iter().find(|s| Some(&s.id) == asked.as_ref())
                    .or_else(|| list.iter().find(|s| !s.user.played)).or(list.first()).map(|s| s.id.clone());
                picked.set(first);
            }
            view! {
                <div class="chips">
                    {list.into_iter().map(|s| {
                        let id = s.id.clone();
                        let id2 = s.id.clone();
                        view! { <button class="chip" class:selected=move || picked.get().as_deref() == Some(id.as_str())
                            on:click=move |_| picked.set(Some(id2.clone()))>{s.title.clone()}</button> }
                    }).collect_view()}
                </div>
            }
        })}
        {tmdb.map(|t| view! { <crate::request::MoreSeasons tmdb=t show=more_show.clone() have=have /> })}
        </div>
        {
            let acc = acc.clone();
            move || {
                let id = picked.get()?;
                let s = seasons.get().flatten()?.into_iter().find(|s| s.id == id)?;
                Some(view! {
                    <crate::extra::MarkAll acc=acc.clone() id=s.id.clone() noun="season"
                        what=tr!("{season} of {show}", season = s.title, show = show) watched=s.user.played />
                })
            }
        }
        {move || picked.get().map(|season| view! { <Episodes acc=acc.clone() series=series.clone() season=season /> })}
    }
}

#[component]
fn Episodes(
    acc: String,
    series: String,
    season: String,
    /// The episode whose page this is (marked, and scrolled to).
    #[prop(optional)]
    current: String,
) -> impl IntoView {
    let a = acc.clone();
    let refresh = use_context::<crate::extra::Refresh>();
    let eps = LocalResource::new(move || {
        if let Some(r) = refresh {
            r.0.track();
        }
        let (acc, series, season) = (a.clone(), series.clone(), season.clone());
        async move {
            let srv = state::ctx().server(&acc)?;
            srv.episodes(&series, &season).await.ok()
        }
    });
    view! {
        {move || match eps.get() {
            None => view! { <Loading /> }.into_any(),
            Some(None) => view! { <ErrorText text=tr!("Couldn't load the episodes.") /> }.into_any(),
            Some(Some(list)) => view! {
                <div class="episodes">
                    {list.into_iter().map(|e| {
                        let here = e.id == current;
                        view! { <div class="episode" class:current=here><Card acc=acc.clone() item=e wide=true /></div> }
                    }).collect_view()}
                </div>
            }.into_any(),
        }}
    }
}

#[component]
pub fn Search() -> impl IntoView {
    let query = use_query_map();
    // The address holds the text (the top bar's field writes it too).
    let text = Memo::new(move |_| query.read().get("q").unwrap_or_default());
    let found = RwSignal::new(None::<Vec<(String, Item)>>);
    let others = RwSignal::new(Vec::<SeerrResult>::new());
    let searching = RwSignal::new(false);
    let unreachable = RwSignal::new(0usize);
    let seq = StoredValue::new(0u32);
    let navigate = leptos_router::hooks::use_navigate();
    let input = NodeRef::<leptos::html::Input>::new();
    // `autofocus` only works when the page loads, not on a link inside the app. (Hidden on
    // wide screens, where the top bar's field is the one in use.)
    Effect::new(move |_| {
        if let Some(i) = input.get()
            && i.offset_parent().is_some()
        {
            let _ = i.focus();
        }
    });
    Effect::new(move |_| {
        let q = text.get();
        seq.update_value(|s| *s += 1);
        let mine = seq.get_value();
        if q.trim().len() < 2 {
            found.set(None);
            others.set(Vec::new());
            searching.set(false);
            return;
        }
        spawn_local(async move {
            state::sleep(300).await;
            // Typed on (or left the page) meanwhile.
            if seq.try_get_value() != Some(mine) {
                return;
            }
            let c = state::ctx();
            searching.set(true);
            let found_on = state::join_all(c.servers().into_iter().map(|(acc, srv)| {
                let q = q.clone();
                async move {
                    srv.search(&q, 40)
                        .await
                        .map(|l| l.into_iter().map(|i| (acc.clone(), i)).collect::<Vec<_>>())
                }
            }))
            .await;
            let failed = found_on.iter().filter(|r| r.is_err()).count();
            let all = dedupe(
                found_on
                    .into_iter()
                    .filter_map(Result::ok)
                    .flatten()
                    .collect(),
            );
            let have: HashSet<String> = all
                .iter()
                .filter_map(|(_, i)| i.provider_ids.tmdb.clone())
                .collect();
            if seq.try_get_value() != Some(mine) {
                return;
            }
            found.set(Some(all));
            searching.set(false);
            // "Nothing found" would be wrong when a server didn't answer.
            unreachable.set(failed);
            if let Some(s) = c.seerr() {
                let list = s.search(&q, 1).await.unwrap_or_default();
                if seq.try_get_value() == Some(mine) {
                    others.set(
                        list.into_iter()
                            .filter(|r| {
                                !have.contains(&r.tmdb_id.to_string())
                                    && r.availability != Availability::Available
                            })
                            .collect(),
                    );
                }
            }
        });
    });
    let write = move |q: String| {
        navigate(
            &format!("/search?q={}", net::percent_encode(&q)),
            leptos_router::NavigateOptions {
                replace: true,
                scroll: false,
                ..Default::default()
            },
        )
    };
    view! {
        <input class="search page-search" type="search" placeholder=tr!("Movies, shows, episodes…") aria-label=tr!("Search") node_ref=input
            prop:value=move || text.get() on:input=move |ev| write(event_target_value(&ev)) />
        {move || (text.with(|t| t.trim().len() < 2)).then(|| view! {
            <p class="muted">{tr!("Search your servers, and what you could request.")}</p>
        })}
        {move || searching.get().then(|| view! { <p class="muted">{tr!("Searching…")}</p> })}
        {move || (unreachable.get() > 0 && !searching.get()).then(|| view! {
            <ErrorText text=tr!("Some of your servers didn't answer: their titles are missing here.") />
        })}
        {move || found.get().filter(|_| !searching.get()).map(|f| if f.is_empty() && others.with(|o| o.is_empty()) {
            view! { <p class="muted">{tr!("Nothing found.")}</p> }.into_any()
        } else {
            view! { {(!f.is_empty()).then(|| view! { <h2>{tr!("On your servers")}</h2> })}<Grid items=f /> }.into_any()
        })}
        {move || (!others.with(|o| o.is_empty())).then(|| view! {
            <h2>{tr!("Not on your servers")}</h2>
            <div class="grid">
                {others.get().into_iter().map(|r| view! { <Requestable r=r /> }).collect_view()}
            </div>
        })}
    }
}

/// A title Seerr knows but the servers don't: opens its page (to request it there).
#[component]
pub fn Requestable(r: SeerrResult) -> impl IntoView {
    let label = crate::request::availability_label(r.availability, r.download_progress);
    let show = r.media_type == SeerrMediaType::Tv;
    view! {
            <A href=crate::request::title_href(r.media_type, r.tmdb_id) attr:class="card">
                <div class="art">
                    {r.poster_url.clone().map(|p| view! { <img src=p loading="lazy" alt="" /> })}
                    {label.map(|l| view! { <span class="state on-art">{l}</span> })}
                </div>
                <div class="title">{r.title.clone()}</div>
                <div class="sub">{r.year.clone().unwrap_or_default()}{if show { format!(" · {}", tr!("show")) } else { String::new() }}
    </div>
            </A>
        }
}

/// Everything in Continue watching (its row's "See all"): the home's last copy at once, then
/// the servers' (which also refreshes the home's copy).
#[component]
pub fn ContinueWatching() -> impl IntoView {
    fn watching(r: &Rails) -> Vec<(String, Item)> {
        r.iter()
            .find(|(_, _, _, _, l)| l == "/continue")
            .map(|(_, items, _, _, _)| items.clone())
            .unwrap_or_default()
    }
    let items = RwSignal::new(state::load::<Rails>(state::HOME).map(|r| watching(&r)));
    spawn_local(async move {
        let c = state::ctx();
        if let Some((fresh, libs)) = load_home(&c).await {
            state::save(state::HOME, &fresh);
            state::save(state::LIBS, &libs);
            items.try_set(Some(watching(&fresh)));
        } else {
            items.try_update(|i| {
                i.get_or_insert_with(Vec::new);
            });
        }
    });
    view! {
        <h1>{tr!("Continue watching")}</h1>
        {move || match items.get() {
            None => view! { <Loading /> }.into_any(),
            Some(l) if l.is_empty() => view! { <p class="muted">{tr!("Nothing in progress.")}</p> }.into_any(),
            Some(l) => view! { <Grid items=l /> }.into_any(),
        }}
    }
}
