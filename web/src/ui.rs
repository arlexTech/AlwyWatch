// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared pieces: poster cards, rails, grids, loading and error states.

use i18n::tr;
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
        ItemKind::Episode => tr!(
            "{series} · S{season} E{episode}",
            series = item.series_title.clone().unwrap_or_default(),
            season = item.season_number.unwrap_or(0),
            episode = item.episode_number.unwrap_or(0)
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
    // An episode as a poster: the show's name, and where it is (as in the apps).
    let as_show = !wide && note.is_none() && item.kind == ItemKind::Episode;
    let title = if as_show {
        item.series_title
            .clone()
            .unwrap_or_else(|| item.title.clone())
    } else {
        item.title.clone()
    };
    // Under an episode shown as its show: the episode's own title too.
    let sub2 = (as_show && item.title != title).then(|| item.title.clone());
    let (sub, href) = match note {
        Some(n) => {
            let season = n
                .open_season_id
                .filter(|_| n.open_id == n.item_id)
                .map(|s| format!("?season={s}"))
                .unwrap_or_default();
            (n.label, format!("/item/{acc}/{}{season}", n.open_id))
        }
        None if as_show => (
            tr!(
                "S{season} · E{episode}",
                season = item.season_number.unwrap_or(0),
                episode = item.episode_number.unwrap_or(0)
            ),
            item_href(&acc, &item),
        ),
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
            {sub2.map(|t| view! { <div class="sub">{t}</div> })}
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
                    {(!see_all.is_empty()).then(|| view! { <A href=see_all attr:class="see-all">{tr!("See all ›")}</A> })}
                </div>
                <RailRow>
                    {items.into_iter().map(|(acc, i)| {
                        let note = notes.iter().find(|n| n.item_id == i.id).cloned();
                        view! { <Card acc=acc item=i wide=wide note=note /> }
                    }).collect_view()}
                </RailRow>
            </section>
        }
    })
}

/// A row that scrolls sideways: with a mouse, arrows on its sides move it a page at a time (no
/// scroll bar, as in the desktop apps); touch screens swipe it. An arrow shows only when there
/// is more that way.
#[component]
pub fn RailRow(children: Children) -> impl IntoView {
    let row = NodeRef::<leptos::html::Div>::new();
    let (more_left, more_right) = (RwSignal::new(false), RwSignal::new(false));
    let update = move || {
        // Also from a timer: the row may be gone by then (home rows are redrawn when they refresh).
        if let Some(r) = row.try_get_untracked().flatten() {
            let (x, max) = (r.scroll_left(), r.scroll_width() - r.client_width());
            more_left.try_set(x > 4);
            more_right.try_set(x < max - 4);
        }
    };
    // Once laid out (images and fonts change the widths a little later too).
    Effect::new(move |_| {
        if row.get().is_some() {
            update();
            leptos::task::spawn_local(async move {
                state::sleep(400).await;
                update();
            });
        }
    });
    let by = move |dir: i32| {
        if let Some(r) = row.try_get_untracked().flatten() {
            // A page, minus a card's worth so the last one seen stays in view.
            let step = (f64::from(r.client_width()) * 0.85) as i32;
            r.set_scroll_left(r.scroll_left() + dir * step);
        }
    };
    view! {
        <div class="rail-wrap">
            <button class="rail-arrow prev" class:shown=move || more_left.get() tabindex="-1"
                aria-label=tr!("Previous") on:click=move |_| by(-1)>"‹"</button>
            <div class="rail-row" node_ref=row on:scroll=move |_| update()>{children()}</div>
            <button class="rail-arrow next" class:shown=move || more_right.get() tabindex="-1"
                aria-label=tr!("Next") on:click=move |_| by(1)>"›"</button>
        </div>
    }
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

/// A translated sentence around a value shown apart (in bold, a link…): the text before and
/// after `key` (e.g. "{name}"), so translators keep the whole sentence and its word order.
pub fn around(text: &str, key: &str) -> (String, String) {
    match text.split_once(key) {
        Some((before, after)) => (before.to_owned(), after.to_owned()),
        None => (text.to_owned(), String::new()),
    }
}
