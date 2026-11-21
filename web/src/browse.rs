// SPDX-License-Identifier: AGPL-3.0-or-later
//! Home, libraries, title pages and search. Pages show the last copy kept in the browser at
//! once, then refresh it (never a blank page for data we already had).

use std::collections::HashSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use leptos_router::hooks::{use_params_map, use_query_map};
use media_api::{ImageKind, Item, ItemKind, ItemQuery, LibraryKind, MediaServer};
use seerr::{Availability, SeerrMediaType, SeerrResult};

use crate::state::{self, fmt_time};
use crate::ui::{Card, ErrorText, Grid, Loading, Rail};

type Rails = Vec<(String, Vec<(String, Item)>, bool)>;

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

async fn load_home(c: &state::Ctx) -> Rails {
    let mut resume = Vec::new();
    let mut next = Vec::new();
    let mut latest: Rails = Vec::new();
    for (acc, srv) in c.servers() {
        if let Ok(l) = srv.resume(16).await {
            resume.extend(l.into_iter().map(|i| (acc.clone(), i)));
        }
        if let Ok(l) = srv.next_up(16).await {
            next.extend(l.into_iter().map(|i| (acc.clone(), i)));
        }
        for lib in srv.libraries().await.unwrap_or_default() {
            if lib.kind == LibraryKind::Other {
                continue;
            }
            if let Ok(l) = srv.latest(&lib.id, 20).await {
                let name = format!("Recently added · {}", lib.title);
                latest.push((
                    name,
                    l.into_iter().map(|i| (acc.clone(), i)).collect(),
                    false,
                ));
            }
        }
    }
    resume.sort_by_key(|(_, i)| std::cmp::Reverse(i.user.last_played_at.unwrap_or(0)));
    let mut rails: Rails = vec![
        ("Continue watching".into(), dedupe(resume), true),
        ("Next up".into(), dedupe(next), true),
    ];
    rails.extend(latest);
    rails
}

#[component]
pub fn Home() -> impl IntoView {
    let c = state::ctx();
    let rails = RwSignal::new(state::load::<Rails>("aw.home"));
    let c2 = c.clone();
    Effect::new(move |_| {
        c2.accounts.track();
        let c = c2.clone();
        spawn_local(async move {
            let fresh = load_home(&c).await;
            state::save("aw.home", &fresh);
            rails.set(Some(fresh));
        });
    });
    let libs = LocalResource::new(move || {
        let c = c.clone();
        c.accounts.track();
        async move {
            let mut out = Vec::new();
            for (acc, srv) in c.servers() {
                for l in srv.libraries().await.unwrap_or_default() {
                    if l.kind != LibraryKind::Other {
                        out.push((
                            acc.clone(),
                            l.id,
                            l.title,
                            srv.session().server_name.clone(),
                        ));
                    }
                }
            }
            out
        }
    });
    view! {
        <div class="chips">
            {move || libs.get().unwrap_or_default().into_iter().map(|(acc, id, title, server)| view! {
                <A href=format!("/library/{acc}/{id}") attr:class="chip" attr:title=server>{title}</A>
            }).collect_view()}
        </div>
        {move || match rails.get() {
            None => view! { <Loading /> }.into_any(),
            Some(r) => r.into_iter().map(|(t, items, wide)| view! { <Rail title=t items=items wide=wide /> }).collect_view().into_any(),
        }}
    }
}

#[component]
pub fn Library() -> impl IntoView {
    let params = use_params_map();
    let acc = move || params.read().get("acc").unwrap_or_default();
    let id = move || params.read().get("id").unwrap_or_default();
    let items = RwSignal::new(Vec::<(String, Item)>::new());
    let total = RwSignal::new(None::<u32>);
    let error = RwSignal::new(None::<String>);
    let load = move |start: u32| {
        let c = state::ctx();
        let (acc, id) = (acc(), id());
        spawn_local(async move {
            let Some(srv) = c.server(&acc) else { return };
            match srv.items(&ItemQuery::library(id, start, 60)).await {
                Ok(p) => {
                    total.set(Some(p.total));
                    items.update(|v| v.extend(p.items.into_iter().map(|i| (acc.clone(), i))));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };
    Effect::new(move |_| {
        let _ = (acc(), id());
        items.set(Vec::new());
        total.set(None);
        load(0);
    });
    view! {
        {move || total.get().is_none().then(|| view! { <Loading /> })}
        {move || view! { <Grid items=items.get() /> }}
        {move || {
            let n = items.with(|v| v.len() as u32);
            total.get().filter(|&t| t > n).map(|_| view! {
                <button class="more" on:click=move |_| load(n)>"More"</button>
            })
        }}
        {move || error.get().map(|e| view! { <ErrorText text=e /> })}
    }
}

#[component]
pub fn Detail() -> impl IntoView {
    let params = use_params_map();
    let acc = Memo::new(move |_| params.read().get("acc").unwrap_or_default());
    let id = Memo::new(move |_| params.read().get("id").unwrap_or_default());
    let item = LocalResource::new(move || {
        let (acc, id) = (acc.get(), id.get());
        async move {
            let srv = state::ctx().server(&acc).ok_or("Account not found")?;
            srv.item(&id)
                .await
                .map_err(|e| e.to_string())
                .map(|i| (srv, i))
        }
    });
    view! {
        {move || match item.get() {
            None => view! { <Loading /> }.into_any(),
            Some(Err(e)) => view! { <ErrorText text=e.to_string() /> }.into_any(),
            Some(Ok((srv, i))) => {
                let acc = acc.get();
                let backdrop = state::image(&srv, &i, &[ImageKind::Backdrop, ImageKind::Thumb], 960);
                let poster = state::image(&srv, &i, &[ImageKind::Primary, ImageKind::SeriesPrimary], 240);
                let meta = [
                    i.year.map(|y| y.to_string()),
                    i.runtime_ms.map(|r| format!("{} min", r / 60_000)),
                    i.official_rating.clone(),
                    i.community_rating.map(|r| format!("★ {r:.1}")),
                    (!i.genres.is_empty()).then(|| i.genres.join(", ")),
                ].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                let playable = matches!(i.kind, ItemKind::Movie | ItemKind::Episode);
                let pos = i.user.position_ms;
                let base = format!("/play/{acc}/{}", i.id);
                let heading = match i.kind {
                    ItemKind::Episode => format!("{} · S{} E{}", i.series_title.clone().unwrap_or_default(),
                        i.season_number.unwrap_or(0), i.episode_number.unwrap_or(0)),
                    _ => String::new(),
                };
                view! {
                    <div class="hero" style=backdrop.map(|b| format!("background-image:url('{b}')")).unwrap_or_default()>
                        <div class="hero-inner">
                            {poster.map(|p| view! { <img class="hero-poster" src=p alt="" /> })}
                            <div>
                                <p class="muted">{heading}</p>
                                <h1>{i.title.clone()}</h1>
                                <p class="muted">{meta}</p>
                                {playable.then(|| view! {
                                    <div class="actions">
                                        {if pos > 0 {
                                            view! {
                                                <A href=format!("{base}?t={pos}") attr:class="button primary">{format!("Resume from {}", fmt_time(pos))}</A>
                                                <A href=format!("{base}?t=0") attr:class="button">"From start"</A>
                                            }.into_any()
                                        } else {
                                            view! { <A href=format!("{base}?t=0") attr:class="button primary">"Play"</A> }.into_any()
                                        }}
                                    </div>
                                })}
                                <p class="overview">{i.overview.clone().unwrap_or_default()}</p>
                            </div>
                        </div>
                    </div>
                    {(i.kind == ItemKind::Series).then(|| view! { <Seasons acc=acc.clone() series=i.id.clone() /> })}
                    {(i.kind == ItemKind::Season).then(|| view! {
                        <Episodes acc=acc.clone() series=i.series_id.clone().unwrap_or_default() season=i.id.clone() />
                    })}
                }.into_any()
            }
        }}
    }
}

#[component]
fn Seasons(acc: String, series: String) -> impl IntoView {
    let (a, s) = (acc.clone(), series.clone());
    let seasons = LocalResource::new(move || {
        let (acc, series) = (a.clone(), s.clone());
        async move {
            let srv = state::ctx().server(&acc)?;
            srv.seasons(&series).await.ok()
        }
    });
    let picked = RwSignal::new(None::<String>);
    view! {
        {move || seasons.get().flatten().map(|list| {
            // The first season not fully watched, else the first.
            if picked.get_untracked().is_none() {
                let first = list.iter().find(|s| !s.user.played).or(list.first()).map(|s| s.id.clone());
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
        {move || picked.get().map(|season| view! { <Episodes acc=acc.clone() series=series.clone() season=season /> })}
    }
}

#[component]
fn Episodes(acc: String, series: String, season: String) -> impl IntoView {
    let a = acc.clone();
    let eps = LocalResource::new(move || {
        let (acc, series, season) = (a.clone(), series.clone(), season.clone());
        async move {
            let srv = state::ctx().server(&acc)?;
            srv.episodes(&series, &season).await.ok()
        }
    });
    view! {
        {move || match eps.get() {
            None => view! { <Loading /> }.into_any(),
            Some(None) => view! { <ErrorText text="Couldn't load the episodes.".to_string() /> }.into_any(),
            Some(Some(list)) => view! {
                <div class="episodes">
                    {list.into_iter().map(|e| view! { <Card acc=acc.clone() item=e wide=true /> }).collect_view()}
                </div>
            }.into_any(),
        }}
    }
}

#[component]
pub fn Search() -> impl IntoView {
    let query = use_query_map();
    let text = RwSignal::new(query.read_untracked().get("q").unwrap_or_default());
    let found = RwSignal::new(None::<Vec<(String, Item)>>);
    let others = RwSignal::new(Vec::<SeerrResult>::new());
    let seq = StoredValue::new(0u32);
    Effect::new(move |_| {
        let q = text.get();
        seq.update_value(|s| *s += 1);
        let mine = seq.get_value();
        if q.trim().len() < 2 {
            found.set(None);
            others.set(Vec::new());
            return;
        }
        spawn_local(async move {
            state::sleep(300).await;
            if seq.get_value() != mine {
                return;
            }
            let c = state::ctx();
            let mut all = Vec::new();
            for (acc, srv) in c.servers() {
                if let Ok(l) = srv.search(&q, 40).await {
                    all.extend(l.into_iter().map(|i| (acc.clone(), i)));
                }
            }
            let all = dedupe(all);
            let have: HashSet<String> = all
                .iter()
                .filter_map(|(_, i)| i.provider_ids.tmdb.clone())
                .collect();
            if seq.get_value() != mine {
                return;
            }
            found.set(Some(all));
            if let Some(s) = c.seerr() {
                let list = s.search(&q, 1).await.unwrap_or_default();
                if seq.get_value() == mine {
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
    view! {
        <input class="search" type="search" placeholder="Movies, shows, episodes…" autofocus
            prop:value=move || text.get() on:input=move |ev| text.set(event_target_value(&ev)) />
        {move || found.get().map(|f| if f.is_empty() && others.with(|o| o.is_empty()) {
            view! { <p class="muted">"Nothing found."</p> }.into_any()
        } else {
            view! { {(!f.is_empty()).then(|| view! { <h2>"On your servers"</h2> })}<Grid items=f /> }.into_any()
        })}
        {move || (!others.with(|o| o.is_empty())).then(|| view! {
            <h2>"Not on your servers"</h2>
            <div class="grid">
                {others.get().into_iter().map(|r| view! { <Requestable r=r /> }).collect_view()}
            </div>
        })}
    }
}

#[component]
fn Requestable(r: SeerrResult) -> impl IntoView {
    let status = RwSignal::new(match r.availability {
        Availability::Pending | Availability::Processing => Some("Requested".to_string()),
        Availability::PartiallyAvailable => Some("Partly available".to_string()),
        _ => None,
    });
    let (kind, id) = (r.media_type, r.tmdb_id);
    let request = move |_| {
        status.set(Some("Requesting…".into()));
        spawn_local(async move {
            let Some(s) = state::ctx().seerr() else {
                return;
            };
            status.set(Some(match s.request(kind, id, &[]).await {
                Ok(_) => "Requested".into(),
                Err(e) => format!("Couldn't request: {e}"),
            }));
        });
    };
    view! {
        <div class="card">
            <div class="art">{r.poster_url.clone().map(|p| view! { <img src=p loading="lazy" alt="" /> })}</div>
            <div class="title">{r.title.clone()}</div>
            <div class="sub">{r.year.clone().unwrap_or_default()}{if kind == SeerrMediaType::Tv { " · show" } else { "" }}</div>
            {move || match status.get() {
                Some(s) => view! { <div class="sub">{s}</div> }.into_any(),
                None => view! { <button class="small" on:click=request>"Request"</button> }.into_any(),
            }}
        </div>
    }
}
