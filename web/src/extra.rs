// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the title page and the menu offer beyond browsing: watched state, My list, the cast,
//! and the My list page.

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
    view! {
        <div class="actions">
            <button on:click=toggle_played>
                {move || if played.get() { "✓ Watched · mark unwatched" } else { "Mark watched" }}
            </button>
            {can_list.then(|| view! {
                <button on:click=toggle_listed>
                    {move || if listed.get() { "★ In my list · remove" } else { "☆ Add to my list" }}
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
            <h2>"Cast"</h2>
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
        <h1>"My list"</h1>
        {move || match items.get() {
            None => view! { <Loading /> }.into_any(),
            Some((_, true)) => view! {
                <ErrorText text="Can't reach your servers right now.".to_string() />
            }.into_any(),
            Some((l, _)) if l.is_empty() => view! {
                <p class="muted">"Nothing yet. Open a movie or a show and add it to your list."</p>
            }.into_any(),
            Some((l, _)) => view! { <Grid items=l /> }.into_any(),
        }}
    }
}
