// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the title page and the menu offer beyond browsing: watched state, My list, the cast,
//! and the pages My list and Requests.

use leptos::prelude::*;
use leptos::task::spawn_local;
use media_api::{Item, ItemKind, MediaServer, Person};
use seerr::{Availability, RequestStatus, SeerrMediaType, SeerrRequest};

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
        spawn_local(async move {
            let Some(srv) = state::ctx().server(&acc) else {
                return;
            };
            match srv.set_played(&id, want).await {
                Ok(()) => {
                    played.set(want);
                    // The home rails kept in the browser are now out of date.
                    state::remove("aw.home");
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };
    let toggle_listed = move |_| {
        let (acc, id, want) = (acc.clone(), id.clone(), !listed.get_untracked());
        spawn_local(async move {
            let Some(srv) = state::ctx().server(&acc) else {
                return;
            };
            match srv.set_watchlisted(&id, want, None).await {
                Ok(()) => listed.set(want),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };
    let can_list = matches!(item.kind, ItemKind::Movie | ItemKind::Series);
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
        let mut all = Vec::new();
        for (acc, srv) in state::ctx().servers() {
            if let Ok(l) = srv.watchlist(None).await {
                all.extend(l.into_iter().map(|i| (acc.clone(), i)));
            }
        }
        all
    });
    view! {
        <h1>"My list"</h1>
        {move || match items.get() {
            None => view! { <Loading /> }.into_any(),
            Some(l) if l.is_empty() => view! {
                <p class="muted">"Nothing yet. Open a movie or a show and add it to your list."</p>
            }.into_any(),
            Some(l) => view! { <Grid items=l /> }.into_any(),
        }}
    }
}

fn request_state(r: &SeerrRequest) -> &'static str {
    match (r.status, r.availability) {
        (_, Availability::Available) => "Available",
        (_, Availability::PartiallyAvailable) => "Partly available",
        (RequestStatus::Declined, _) => "Declined",
        (RequestStatus::Failed, _) => "Failed",
        (RequestStatus::Pending, _) => "Waiting for approval",
        (_, Availability::Processing) => "Downloading",
        _ => "Approved",
    }
}

/// What the user asked for through Seerr, newest first.
#[component]
pub fn Requests() -> impl IntoView {
    let list = LocalResource::new(move || async move {
        let s = state::ctx()
            .seerr()
            .ok_or("Requests need an account on this AlwyWatch server.".to_string())?;
        s.requests(50, false).await.map_err(|e| e.to_string())
    });
    view! {
        <h1>"Requests"</h1>
        <p class="muted">"Search for a title that isn't on your servers to request it."</p>
        {move || match list.get() {
            None => view! { <Loading /> }.into_any(),
            Some(Err(e)) => view! { <ErrorText text=e /> }.into_any(),
            Some(Ok(l)) if l.is_empty() => view! { <p class="muted">"You haven't requested anything yet."</p> }.into_any(),
            Some(Ok(l)) => view! {
                <div class="leaving">
                    {l.into_iter().map(|r| {
                        let state = request_state(&r);
                        let kind = if r.media_type == SeerrMediaType::Tv { "Show" } else { "Movie" };
                        let day = r.created_at.chars().take(10).collect::<String>();
                        view! {
                            <div class="leaving-row">
                                {r.poster_url.clone().map(|p| view! { <img src=p loading="lazy" alt="" /> })}
                                <div class="leaving-info">
                                    <strong>{r.title.clone().unwrap_or_else(|| "Untitled".into())}</strong>
                                    <span class="muted">{kind}" · requested "{day}</span>
                                    <span class="accent small">{state}</span>
                                </div>
                            </div>
                        }
                    }).collect_view()}
                </div>
            }.into_any(),
        }}
    }
}
