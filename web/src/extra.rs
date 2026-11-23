// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the title page and the menu offer beyond browsing: watched state, My list, the cast,
//! and the My list page.

use i18n::tr;
use leptos::prelude::*;
use leptos::task::spawn_local;
use media_api::{Item, ItemKind, MediaServer, Person, ServerKind};

use crate::state;
use crate::ui::{ErrorText, Grid, Loading};

/// "Watched" and "My list" toggles of a title (movies, shows, episodes).
#[component]
pub fn ItemActions(acc: String, item: Item) -> impl IntoView {
    let played = RwSignal::new(item.user.played);
    let listed = RwSignal::new(item.user.favorite);
    let error = RwSignal::new(None::<String>);
    let id = item.id.clone();
    let (a1, i1) = (acc.clone(), id.clone());
    let a2 = acc.clone();
    let toggle_played = move |_| {
        let (acc, id, want) = (a1.clone(), i1.clone(), !played.get_untracked());
        error.set(None);
        spawn_local(async move {
            let Some(srv) = state::ctx().server(&acc) else {
                return;
            };
            match srv.set_played(&id, want).await {
                Ok(()) => {
                    played.set(want);
                    // The home rails kept in the browser are now out of date.
                    state::remove(state::HOME);
                }
                Err(e) => error.set(Some(state::plain(&e))),
            }
        });
    };
    // Plex keeps the list on plex.tv, under the account's own token (kept at Plex sign-in;
    // accounts signed in before that have none: no list button rather than a failing one).
    let c = state::ctx();
    let plex = c.session(&acc).is_some_and(|s| s.kind == ServerKind::Plex);
    let plex_tv = plex.then(|| c.plex_tv_token(&acc)).flatten();
    let pt = plex_tv.clone();
    let toggle_listed = move |_| {
        let (acc, id, want, pt) = (acc.clone(), id.clone(), !listed.get_untracked(), pt.clone());
        error.set(None);
        spawn_local(async move {
            let Some(srv) = state::ctx().server(&acc) else {
                return;
            };
            match srv.set_watchlisted(&id, want, pt.as_deref()).await {
                Ok(()) => listed.set(want),
                Err(e) => error.set(Some(state::plain(&e))),
            }
        });
    };
    let can_list =
        matches!(item.kind, ItemKind::Movie | ItemKind::Series) && (!plex || plex_tv.is_some());
    // A show: every episode at once, both ways (a partly watched show can go either way).
    let show = item.kind == ItemKind::Series;
    let (show_acc, show_id, show_title) = (a2, item.id.clone(), item.title.clone());
    view! {
        {show.then(|| view! {
            <MarkAll acc=show_acc.clone() id=show_id.clone() noun="all episodes" what=show_title.clone() watched=item.user.played />
        })}
        <div class="actions">
            {(!show).then(|| view! {
                <button on:click=toggle_played.clone()>
                    {move || if played.get() { tr!("✓ Watched · mark unwatched") } else { tr!("Mark watched") }}
                </button>
            })}
            {can_list.then(|| view! {
                <button on:click=toggle_listed>
                    {move || if listed.get() { tr!("★ In my list · remove") } else { tr!("☆ Add to my list") }}
                </button>
            })}
        </div>
        {move || error.get().map(|e| view! { <ErrorText text=e /> })}
    }
}

/// The cast and crew of a title, as a row of faces.
#[component]
pub fn Cast(acc: String, id: String) -> impl IntoView {
    let people = LocalResource::new(move || {
        let (acc, id) = (acc.clone(), id.clone());
        async move {
            let srv = state::ctx().server(&acc)?;
            let session = srv.session().clone();
            let list: Vec<Person> = srv.people(&id).await.ok()?;
            Some(
                list.into_iter()
                    .take(20)
                    .map(|p| {
                        let photo = p
                            .photo_url
                            .as_deref()
                            .map(|u| state::with_token(u, &session));
                        (p.name, p.role, photo)
                    })
                    .collect::<Vec<_>>(),
            )
        }
    });
    view! {
        {move || people.get().flatten().filter(|l| !l.is_empty()).map(|list| view! {
            <h2>{tr!("Cast")}</h2>
            <div class="rail-row">
                {list.into_iter().map(|(name, role, photo)| view! {
                    <div class="person-card">
                        <div class="face">{photo.map(|p| view! { <img src=p loading="lazy" alt="" /> })}</div>
                        <div class="title">{name}</div>
                        <div class="sub">{role.unwrap_or_default()}</div>
                    </div>
                }).collect_view()}
            </div>
        })}
    }
}

/// My list: Jellyfin favorites, Plex watchlist titles the server has.
#[component]
pub fn MyList() -> impl IntoView {
    let items = LocalResource::new(move || async move {
        let c = state::ctx();
        let lists = state::join_all(c.servers().into_iter().map(|(acc, srv)| {
            let plex_tv = c.plex_tv_token(&acc);
            async move {
                let l = srv.watchlist(plex_tv.as_deref()).await;
                (acc, l)
            }
        }))
        .await;
        let failed = lists.iter().all(|(_, l)| l.is_err()) && !lists.is_empty();
        let all: Vec<_> = lists
            .into_iter()
            .flat_map(|(acc, l)| {
                l.unwrap_or_default()
                    .into_iter()
                    .map(move |i| (acc.clone(), i))
            })
            .collect();
        (all, failed)
    });
    view! {
        <h1>{tr!("My list")}</h1>
        {move || match items.get() {
            None => view! { <Loading /> }.into_any(),
            Some((_, true)) => view! {
                <ErrorText text=tr!("Can't reach your servers right now.") />
            }.into_any(),
            Some((l, _)) if l.is_empty() => view! {
                <p class="muted">{tr!("Nothing yet. Open a movie or a show and add it to your list.")}</p>
            }.into_any(),
            Some((l, _)) => view! { <Grid items=l /> }.into_any(),
        }}
    }
}

/// Bumped after episodes were marked watched or unwatched: the seasons and episodes shown
/// reload.
#[derive(Clone, Copy)]
pub struct Refresh(pub RwSignal<u32>);

/// Marks every episode of a show or a season (`id`) watched or unwatched. Unwatched forgets
/// where the viewer stopped, so it asks first. `noun`: "all episodes" or "season"; `what`:
/// the show (or "Season 2 of the show"), for the question.
#[component]
pub fn MarkAll(
    acc: String,
    id: String,
    noun: &'static str,
    what: String,
    watched: bool,
) -> impl IntoView {
    let fully = RwSignal::new(watched);
    let asking = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let refresh = use_context::<Refresh>();
    let set = move |played: bool| {
        let (acc, id) = (acc.clone(), id.clone());
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let c = state::ctx();
            let res = match c.server(&acc) {
                Some(srv) => srv.set_played(&id, played).await,
                None => Err(media_api::Error::NotFound),
            };
            busy.try_set(false);
            asking.try_set(false);
            match res {
                Ok(()) => {
                    fully.try_set(played);
                    // The home rows kept in the browser are out of date now.
                    state::remove(state::HOME);
                    if let Some(Refresh(r)) = refresh {
                        r.try_update(|n| *n += 1);
                    }
                }
                Err(e) => {
                    c.failed(&acc, &e);
                    error.try_set(Some(state::plain(&e)));
                }
            }
        });
    };
    let (set1, set2) = (set.clone(), set);
    let cancel = NodeRef::<leptos::html::Button>::new();
    // The question opens with the focus on Cancel.
    Effect::new(move |_| {
        if let Some(b) = cancel.get() {
            let _ = b.focus();
        }
    });
    view! {
        <div class="actions tight">
            {move || (!fully.get()).then(|| {
                let set1 = set1.clone();
                view! {
                    <button class="small" disabled=move || busy.get() on:click=move |_| set1(true)>
                        {if noun == "season" { tr!("Mark season watched") } else { tr!("Mark all episodes watched") }}
                    </button>
                }
            })}
            <button class="small" disabled=move || busy.get() on:click=move |_| asking.set(true)>
                {if noun == "season" { tr!("Mark season unwatched") } else { tr!("Mark all episodes unwatched") }}
            </button>
        </div>
        {move || error.get().map(|e| view! { <ErrorText text=e /> })}
        {move || asking.get().then(|| {
            let set2 = set2.clone();
            let question = tr!("Mark all episodes of {title} unwatched?", title = what);
            view! {
                <div class="dialog-back" on:click=move |_| if !busy.get_untracked() { asking.set(false) }
                    on:keydown=move |ev| if ev.key() == "Escape" && !busy.get_untracked() { asking.set(false) }>
                    <div class="dialog" role="alertdialog" aria-modal="true" aria-labelledby="unwatch-title" on:click=|ev| ev.stop_propagation()>
                        <h2 id="unwatch-title">{question}</h2>
                        <p>{tr!("This forgets where you stopped.")}</p>
                        <div class="actions">
                            <button node_ref=cancel disabled=move || busy.get() on:click=move |_| asking.set(false)>{tr!("Cancel")}</button>
                            <button class="danger" disabled=move || busy.get() on:click=move |_| set2(false)>
                                {move || if busy.get() { tr!("Marking…") } else { tr!("Mark unwatched") }}

                            </button>
                        </div>
                    </div>
                </div>
            }
        })}
    }
}
