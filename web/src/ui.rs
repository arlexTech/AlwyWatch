// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared pieces: poster cards, rails, grids, loading and error states.

use leptos::prelude::*;
use leptos_router::components::A;
use media_api::{AnyServer, ImageKind, Item, ItemKind, RailNote};

use crate::state;

/// Where a card leads: playable things to their page too (Play is one click there).
pub fn item_href(acc: &str, item: &Item) -> String {
    format!("/item/{}/{}", acc, item.id)
}

fn subtitle(item: &Item) -> String {
    match item.kind {
        ItemKind::Episode => format!(
            "{} · S{} E{}",
            item.series_title.clone().unwrap_or_default(),
            item.season_number.unwrap_or(0),
            item.episode_number.unwrap_or(0)
        ),
        _ => item.year.map(|y| y.to_string()).unwrap_or_default(),
    }
}

/// A poster (or a 16:9 thumbnail for episodes in "continue watching") with its progress.
#[component]
pub fn Card(
    acc: String,
    item: Item,
    #[prop(optional)] wide: bool,
    // A "recently added" show: what is new (the second line) and what opening it shows.
    #[prop(optional_no_strip)] note: Option<RailNote>,
) -> impl IntoView {
    let c = state::ctx();
    let srv: Option<AnyServer> = c.server(&acc);
    let kinds: &[ImageKind] = if wide {
        &[ImageKind::Thumb, ImageKind::Backdrop, ImageKind::Primary]
    } else if item.kind == ItemKind::Episode {
        &[ImageKind::SeriesPrimary, ImageKind::Primary]
    } else {
        &[ImageKind::Primary, ImageKind::SeriesPrimary]
    };
    let src = srv
        .as_ref()
        .and_then(|s| state::image(s, &item, kinds, if wide { 320 } else { 180 }));
    let progress = item
        .runtime_ms
        .filter(|&r| r > 0 && item.user.position_ms > 0)
        .map(|r| (item.user.position_ms * 100 / r).min(100));
    let title = item.title.clone();
    let (sub, href) = match note {
        Some(n) => {
            let season = n
                .open_season_id
                .filter(|_| n.open_id == n.item_id)
                .map(|s| format!("?season={s}"))
                .unwrap_or_default();
            (n.label, format!("/item/{acc}/{}{season}", n.open_id))
        }
        None => (subtitle(&item), item_href(&acc, &item)),
    };
    view! {
        <A href=href attr:class=if wide { "card wide" } else { "card" }>
            <div class="art">
                {src.map(|s| view! { <img src=s loading="lazy" alt="" /> })}
                {item.user.played.then(|| view! { <span class="badge">"✓"</span> })}
                {progress.map(|p| view! { <div class="bar"><div style=format!("width:{p}%") /></div> })}
            </div>
            <div class="title">{title}</div>
            <div class="sub">{sub}</div>
        </A>
    }
}

/// A horizontal rail of cards with a heading; nothing when empty.
#[component]
pub fn Rail(
    title: String,
    items: Vec<(String, Item)>,
    #[prop(optional)] wide: bool,
    #[prop(optional)] notes: Vec<RailNote>,
    /// Where "See all" goes (none when empty).
    #[prop(optional)]
    see_all: String,
) -> impl IntoView {
    (!items.is_empty()).then(|| {
        view! {
            <section class="rail">
                <div class="rail-head">
                    <h2>{title}</h2>
                    {(!see_all.is_empty()).then(|| view! { <A href=see_all attr:class="see-all">"See all ›"</A> })}
                </div>
                <div class="rail-row">
                    {items.into_iter().map(|(acc, i)| {
                        let note = notes.iter().find(|n| n.item_id == i.id).cloned();
                        view! { <Card acc=acc item=i wide=wide note=note /> }
                    }).collect_view()}
                </div>
            </section>
        }
    })
}

#[component]
pub fn Grid(items: Vec<(String, Item)>) -> impl IntoView {
    view! {
        <div class="grid">
            {items.into_iter().map(|(acc, i)| view! { <Card acc=acc item=i /> }).collect_view()}
        </div>
    }
}

#[component]
pub fn Loading() -> impl IntoView {
    view! { <div class="loading"><div class="spinner" /></div> }
}

#[component]
pub fn ErrorText(text: String) -> impl IntoView {
    view! { <p class="error">{text}</p> }
}
