// SPDX-License-Identifier: AGPL-3.0-or-later
//! Leaving soon (what the server will delete, and keeping it) and, for admins, who keeps what
//! forever with the overlaps.

use i18n::{tr, trn};
use keep_service::{
    AdminUser, CountdownRequest, ForeverStats, KeepSeerrUser, LeavingItem, Protection, Rights,
    ServerStatus, VoteKind, VoteRequest,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;

use crate::profile::{Avatar, name_with_media};
use crate::state;
use crate::ui::{ErrorText, Loading};

fn date(ts: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts as f64 * 1000.0));
    String::from(d.to_locale_date_string(i18n::language(), &wasm_bindgen::JsValue::UNDEFINED))
}

fn status(i: &LeavingItem) -> String {
    match i.protection {
        Protection::Forever => tr!("Kept forever"),
        Protection::UntilWatched => tr!("Kept until watched"),
        Protection::Extended => i
            .effective_deletion_date
            .map(|d| tr!("Kept until {date}", date = date(d)))
            .unwrap_or_default(),
        Protection::None if !i.collection_active => tr!("Not scheduled yet"),
        Protection::None => i
            .deletion_date
            .map(|d| tr!("Leaves {date}", date = date(d)))
            .unwrap_or_default(),
    }
}

/// The home's "Leaving soon" row: what is still to be decided (not already kept), soonest
/// first, from the last copy at once. Nothing without Maintainerr behind the AlwyWatch server.
#[component]
pub fn LeavingRail() -> impl IntoView {
    let c = state::ctx();
    let on = move || c.keep.get().is_some() && c.me.get().is_some_and(|m| m.services.leaving_soon);
    let items = RwSignal::new(state::load::<Vec<LeavingItem>>(state::LEAVING).unwrap_or_default());
    Effect::new(move |_| {
        if !on() {
            return;
        }
        spawn_local(async move {
            let Some(k) = state::ctx().keep_client() else {
                return;
            };
            if let Ok(l) = k.leaving_soon().await {
                state::save(state::LEAVING, &l);
                items.try_set(l);
            }
        });
    });
    move || {
        let acc = state::ctx().keep.get().map(|k| k.account)?;
        let shown: Vec<LeavingItem> = items
            .get()
            .into_iter()
            // Only what is still mine to review; nothing left: no row.
            .filter(keep_service::needs_review)
            .take(15)
            .collect();
        (on() && !shown.is_empty()).then(|| view! {
            <section class="rail">
                <div class="rail-head leaving-head">
                    <div>
                        <h2>{tr!("Leaving soon")}</h2>
                        <p class="muted small">{tr!("Keep what you still want to watch")}</p>
                    </div>
                    <A href="/leaving/sort" attr:class="button primary small">{tr!("Swipe them!")}</A>
                    <A href="/leaving" attr:class="see-all">{tr!("See all ›")}</A>
                </div>
                <crate::ui::RailRow>
                    {shown.into_iter().map(|i| {
                        let href = i.server_item_id.as_ref().map_or_else(|| "/leaving".to_owned(), |id| format!("/item/{acc}/{id}"));
                        view! {
                            <A href=href attr:class="card">
                                <div class="art">
                                    {i.poster_url.clone().map(|p| view! { <img src=p loading="lazy" alt="" /> })}
                                    {short_when(&i).map(|w| view! { <span class="when">{w}</span> })}
                                </div>
                                <div class="title">{i.title.clone()}</div>
                                <div class="sub">{i.year.map(|y| y.to_string()).unwrap_or_default()}</div>
                            </A>
                        }
                    }).collect_view()}
                </crate::ui::RailRow>
            </section>
        })
    }
}

/// The badge on a Leaving soon poster: "No date" while nothing is scheduled, else how soon.
/// When it goes, on its poster; nothing without a date (a paused rule): a row of "No date"
/// badges read as broken.
fn short_when(i: &LeavingItem) -> Option<String> {
    let now = (js_sys::Date::now() / 1000.0) as i64;
    match i.effective_deletion_date.or(i.deletion_date) {
        Some(d) if i.collection_active && d > now => {
            let days = (d - now) / 86_400;
            Some(match days {
                0 => tr!("Today"),
                1 => tr!("Tomorrow"),
                n => trn!("{n} day", "{n} days", n),
            })
        }
        _ => None,
    }
}

fn no_keep() -> String {
    tr!(
        "This only works with a login on one of this site's servers. Add one in Settings → Servers → Add a server."
    )
}

fn mine(i: &LeavingItem) -> Option<String> {
    i.my_vote.as_ref().map(|v| match v.kind {
        VoteKind::KeepForever => tr!("You keep it forever"),
        VoteKind::KeepUntilWatched => tr!("You keep it until you watch it"),
        VoteKind::KeepDays => tr!("You extended it"),
        VoteKind::DontCare => tr!("You're fine with it going"),
    })
}

#[component]
pub fn Leaving() -> impl IntoView {
    let items = RwSignal::new(
        state::load::<Vec<LeavingItem>>(state::LEAVING).map(keep_service::leaving_or_kept),
    );
    let error = RwSignal::new(None::<String>);
    let filter = RwSignal::new("all");
    // A long list (a thousand titles is common): a first screenful, more on request.
    let limit = RwSignal::new(60usize);
    // Again after a link or an unlink (my votes are the account's).
    let revision = state::ctx().links.revision;
    Effect::new(move |_| {
        revision.track();
        spawn_local(async move {
            let Some(k) = state::ctx().keep_client() else {
                return error.set(Some(no_keep()));
            };
            match k.leaving_soon().await {
                Ok(l) => {
                    state::save(state::LEAVING, &l);
                    items.try_set(Some(keep_service::leaving_or_kept(l)));
                    // A failure before (offline, say) is over.
                    error.try_set(None);
                }
                Err(e) => {
                    error.try_set(Some(state::plain_keep(&e)));
                }
            }
        });
    });
    let shown = move || {
        items
            .get()
            .unwrap_or_default()
            .into_iter()
            .filter(|i| match filter.get() {
                "undecided" => i.my_vote.is_none(),
                "mine" => i
                    .my_vote
                    .as_ref()
                    .is_some_and(|v| v.kind != VoteKind::DontCare),
                _ => true,
            })
            .collect::<Vec<_>>()
    };
    view! {
        <h1>{tr!("Leaving soon")}</h1>
        <p class="muted">{tr!("What the server will delete, soonest first. Keep what you want to watch or love.")}</p>
        <div class="actions">
            <A href="/leaving/sort" attr:class="button primary">{tr!("Swipe them!")}</A>
            <Show when=move || state::ctx().keep.get().is_some_and(|k| k.is_admin)>
                <A href="/settings/admin" attr:class="button">{tr!("Admin")}</A>
            </Show>
        </div>
        <div class="chips">
            {[("all", tr!("All")), ("undecided", tr!("Not decided")), ("mine", tr!("Kept by me"))].map(|(k, l)| view! {
                <button class="chip" class:selected=move || filter.get() == k on:click=move |_| filter.set(k)>{l}</button>
            }).collect_view()}
        </div>
        {move || error.get().map(|e| view! { <ErrorText text=e /> })}
        {move || (items.with(Option::is_none) && error.with(Option::is_none)).then(|| view! { <Loading /> })}
        {move || (items.with(|i| i.as_ref().is_some_and(Vec::is_empty))).then(|| view! {
            <p class="muted">{tr!("Nothing is leaving. Nothing to do.")}</p>
        })}
        // Keyed rows: a vote updates its own row, the others (and the focus) stay put.
        <div class="leaving">
            <For each=move || shown().into_iter().take(limit.get()) key=|i| i.key.clone() let(i)>
                <LeavingRow item=i items=items />
            </For>
        </div>
        {move || {
            let total = shown().len();
            (total > limit.get()).then(|| view! {
                <button class="more" on:click=move |_| limit.update(|l| *l += 120)>
                    {tr!("More ({shown} of {total} shown)", shown = limit.get(), total = total)}
                </button>
            })
        }}
    }
}

/// A vote's result into the full list kept for Leaving soon (the page shows a filtered copy:
/// saving that one would drop titles).
fn remember_vote(updated: &LeavingItem) {
    if let Some(mut l) = state::load::<Vec<LeavingItem>>(state::LEAVING) {
        match l.iter_mut().find(|x| x.key == updated.key) {
            Some(x) => *x = updated.clone(),
            None => l.push(updated.clone()),
        }
        state::save(state::LEAVING, &l);
    }
}

#[component]
fn LeavingRow(item: LeavingItem, items: RwSignal<Option<Vec<LeavingItem>>>) -> impl IntoView {
    // This row's current state: a vote changes its texts, not the row (the focus stays).
    let key = item.key.clone();
    let now = Memo::new(move |_| {
        items.with(|l| {
            l.as_ref()
                .and_then(|l| l.iter().find(|x| x.key == key).cloned())
        })
    });
    let on_voted = Callback::new(move |updated: LeavingItem| {
        remember_vote(&updated);
        items.update(|l| {
            if let Some(x) = l
                .as_mut()
                .and_then(|l| l.iter_mut().find(|x| x.key == updated.key))
            {
                *x = updated;
            }
        })
    });
    let year = item.year.map(|y| y.to_string());
    // To the title's page when the server knows it there (as the home's row).
    let href = state::ctx()
        .keep
        .get_untracked()
        .map(|k| k.account)
        .zip(item.server_item_id.clone())
        .map(|(acc, id)| format!("/item/{acc}/{id}"));
    let poster = item
        .poster_url
        .clone()
        .map(|p| view! { <img src=p loading="lazy" alt="" /> });
    let title = view! { <strong>{item.title.clone()}</strong> };
    let (poster, title) = match href {
        Some(h) => (
            view! { <A href=h.clone() attr:tabindex="-1" attr:aria-hidden="true">{poster}</A> }
                .into_any(),
            view! { <A href=h attr:class="leaving-title">{title}</A> }.into_any(),
        ),
        None => (poster.into_any(), title.into_any()),
    };
    view! {
        <div class="leaving-row">
            {poster}
            <div class="leaving-info">
                {title}
                <span class="muted">{year}" · "{move || now.with(|i| i.as_ref().map(status))}</span>
                {move || now.with(|i| {
                    let keepers = i.as_ref().map_or(0, |i| i.counts.forever + i.counts.until_watched + i.counts.days);
                    (keepers > 0).then(|| view! { <span class="muted small">{trn!("{n} keeps it", "{n} keep it", keepers)}</span> })
                })}
                {move || now.with(|i| i.as_ref().and_then(mine)).map(|m| view! { <span class="accent small">{m}</span> })}
                <KeepChoices item=item.clone() on_voted=on_voted />
            </div>
        </div>
    }
}

/// The keep choices of a Leaving soon title (its row, and the title page's keep panel).
#[component]
fn KeepChoices(item: LeavingItem, on_voted: Callback<LeavingItem>) -> impl IntoView {
    let key = item.key.clone();
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    // "Remove my choice" follows the votes made here (the row isn't rebuilt).
    let voted = RwSignal::new(item.my_vote.is_some());
    let vote = move |req: Option<VoteRequest>| {
        let key = key.clone();
        let Some(k) = state::ctx().keep_client() else {
            return error.set(Some(no_keep()));
        };
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let res = match req {
                Some(r) => k.vote(&key, &r).await,
                None => k.unvote(&key).await,
            };
            match res {
                Ok(updated) => {
                    voted.try_set(updated.my_vote.is_some());
                    // The page may have gone meanwhile.
                    let _ = on_voted.try_run(updated);
                }
                Err(e) => {
                    error.try_set(Some(state::plain_keep(&e)));
                }
            }
            busy.try_set(false);
        });
    };
    let req = |kind, days| VoteRequest {
        kind,
        days,
        reminder_days: None,
        reason: None,
    };
    let (v1, v2, v3, v4, v5, v6) = (
        vote.clone(),
        vote.clone(),
        vote.clone(),
        vote.clone(),
        vote.clone(),
        vote,
    );
    view! {
        <div class="actions">
            <button class="small" disabled=move || busy.get() on:click=move |_| v1(Some(req(VoteKind::KeepUntilWatched, None)))>{tr!("Until I watch it")}</button>
            <button class="small" disabled=move || busy.get() on:click=move |_| v3(Some(req(VoteKind::KeepDays, Some(30))))>{tr!("Keep 30 days")}</button>
            <button class="small" disabled=move || busy.get() on:click=move |_| v6(Some(req(VoteKind::KeepDays, Some(7))))>{tr!("Keep 7 days")}</button>
            <button class="small" disabled=move || busy.get() on:click=move |_| v2(Some(req(VoteKind::KeepForever, None)))>{tr!("Keep forever")}</button>
            <button class="small" disabled=move || busy.get() on:click=move |_| v4(Some(req(VoteKind::DontCare, None)))>{tr!("Fine to delete")}</button>
            {move || voted.get().then(|| {
                let v5 = v5.clone();
                view! {
                    <button class="small subtle" disabled=move || busy.get() on:click=move |_| v5(None)>{tr!("Remove my choice")}</button>
                }
            })}
        </div>
        <p class="muted small">{tr!("Everyone's choices count and the longest one wins. “Fine to delete”: it only goes if nobody else keeps it.")}</p>
        {move || error.get().map(|e| view! { <ErrorText text=e /> })}
    }
}

/// "Kept forever", "Kept until 2 people have watched it", "Leaves 3 Dec 2026"… (as Android).
fn keep_policy(e: &LeavingItem, now: i64) -> String {
    match e.protection {
        Protection::Forever => tr!("Kept forever"),
        Protection::UntilWatched => match e.counts.until_watched {
            ..=1 => tr!("Kept until the person who asked has watched it"),
            n => trn!(
                "Kept until {n} person has watched it",
                "Kept until {n} people have watched it",
                n
            ),
        },
        _ if paused(e, now) => tr!("On the deletion list"),
        _ => status(e),
    }
}

/// Which list it is in and why nothing happens yet, plus who keeps it (`date` formats a day).
fn keep_detail(e: &LeavingItem, now: i64, date: impl Fn(i64) -> String) -> String {
    let mut parts = vec![];
    // (A title taken out of its list by a keep still remembers which one it was in.)
    if let Some(t) = e.collection_title.as_ref().filter(|_| e.listed) {
        parts.push(tr!("In “{list}”", list = t));
    }
    if e.listed && e.protection == Protection::None && !e.collection_active {
        parts.push(tr!("deletion isn't turned on yet"));
    }
    if let Some(d) = e.effective_deletion_date
        && e.listed
        && !paused(e, now)
    {
        parts.push(tr!("date {date}", date = date(d)));
    }
    let c = &e.counts;
    let keepers: Vec<String> = [
        (c.forever > 0).then(|| tr!("{n} forever", n = c.forever)),
        (c.until_watched > 0).then(|| tr!("{n} until watched", n = c.until_watched)),
        (c.days > 0).then(|| tr!("{n} for a while", n = c.days)),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !keepers.is_empty() {
        parts.push(tr!("kept by {list}", list = keepers.join(", ")));
    }
    let s = parts.join(" · ");
    let mut chars = s.chars();
    match chars.next() {
        Some(f) => f.to_uppercase().chain(chars).collect(),
        None => s,
    }
}

/// "3 Dec 2026" in the browser's language.
fn medium_date(ts: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts as f64 * 1000.0));
    let o = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&o, &"dateStyle".into(), &"medium".into());
    String::from(d.to_locale_date_string(i18n::language(), &o))
}

/// Under "Not scheduled for deletion": a title can be kept ahead of any list.
fn unplanned() -> String {
    tr!(
        "The server doesn't plan to delete this. You can still say you want it kept, in case it does later."
    )
}

/// A movie or show page: where it stands in Leaving soon, my choice, the keep choices on
/// demand. Shown once the list has been loaded (kept in the browser), then refreshed.
#[component]
pub fn KeepPanel(item: media_api::Item) -> impl IntoView {
    let kind = match item.kind {
        media_api::ItemKind::Movie => "movie",
        media_api::ItemKind::Series => "show",
        _ => return ().into_any(),
    };
    let c = state::ctx();
    // Not without the AlwyWatch server, nor when Maintainerr doesn't answer it.
    let maintainerr = c.me.get_untracked().is_none_or(|m| m.services.leaving_soon);
    if c.keep.get_untracked().is_none() || !maintainerr {
        return ().into_any();
    }
    let list = RwSignal::new(state::load::<Vec<LeavingItem>>(state::LEAVING));
    spawn_local(async move {
        let Some(k) = state::ctx().keep_client() else {
            return;
        };
        if let Ok(l) = k.leaving_soon().await {
            state::save(state::LEAVING, &l);
            list.try_set(Some(l));
        }
    });
    let choosing = RwSignal::new(false);
    let on_voted = Callback::new(move |updated: LeavingItem| {
        list.update(|l| {
            if let Some(l) = l {
                match l.iter_mut().find(|x| x.key == updated.key) {
                    Some(x) => *x = updated,
                    // A title in no deletion list, kept from here.
                    None => l.push(updated),
                }
                state::save(state::LEAVING, l);
            }
        });
        choosing.set(false);
    });
    // In no list: "Keep it" asks the server for the title's entry first.
    let looking = RwSignal::new(false);
    let lookup_error = RwSignal::new(None::<String>);
    let title = keep_service::TitleLookup {
        item_id: item.id.clone(),
        kind: kind.into(),
        title: item.title.clone(),
        tmdb_id: item.provider_ids.tmdb.clone(),
        tvdb_id: item.provider_ids.tvdb.clone(),
    };
    let look_up = move || {
        if looking.get_untracked() {
            return;
        }
        let Some(k) = state::ctx().keep_client() else {
            return lookup_error.set(Some(no_keep()));
        };
        looking.set(true);
        let title = title.clone();
        spawn_local(async move {
            match k.keep_entry(&title).await {
                Ok(e) => {
                    lookup_error.try_set(None);
                    list.try_update(|l| {
                        if let Some(l) = l
                            && !l.iter().any(|x| x.key == e.key)
                        {
                            l.push(e);
                        }
                    });
                    choosing.try_set(true);
                }
                Err(net::Error::NotFound) => {
                    lookup_error.try_set(Some(tr!("This title is never deleted automatically.")));
                }
                Err(e) => {
                    lookup_error.try_set(Some(state::plain_keep(&e)));
                }
            }
            looking.try_set(false);
        });
    };
    let (id, tmdb, tvdb) = (
        item.id.clone(),
        item.provider_ids.tmdb.clone(),
        item.provider_ids.tvdb.clone(),
    );
    let entry = Memo::new(move |_| {
        list.with(|l| {
            l.as_ref().map(|l| {
                keep_service::leaving_entry(l, &id, kind, tmdb.as_deref(), tvdb.as_deref()).cloned()
            })
        })
    });
    view! {
        {move || entry.get().map(|e| {
            let now = now_s();
            let Some(e) = e else {
                let look_up = look_up.clone();
                return view! {
                    <section class="keep-panel">
                        <h3>{tr!("Not scheduled for deletion")}</h3>
                        <p class="muted small">{unplanned()}</p>
                        <p>{tr!("You haven't chosen anything")}</p>
                        <div class="actions">
                            <button class="small" disabled=move || looking.get() on:click=move |_| look_up()>
                                {move || if looking.get() { tr!("Looking it up…") } else { tr!("Keep it") }}
                            </button>
                        </div>
                        {move || lookup_error.get().map(|m| view! { <p class="error small">{m}</p> })}
                    </section>
                }.into_any();
            };
            let detail = keep_detail(&e, now, medium_date);
            let voted = e.my_vote.is_some();
            // In no list and nobody keeps it: nothing is planned for it.
            let planned = e.listed || e.protection != Protection::None;
            view! {
                <section class="keep-panel" class:listed=planned>
                    <h3>{if planned { keep_policy(&e, now) } else { tr!("Not scheduled for deletion") }}</h3>
                    {(!planned).then(|| view! { <p class="muted small">{unplanned()}</p> })}
                    {(!detail.is_empty()).then(|| view! { <p class="muted small">{detail}</p> })}
                    <p>{mine(&e).unwrap_or_else(|| tr!("You haven't chosen anything"))}</p>
                    {move || if choosing.get() {
                        view! { <KeepChoices item=e.clone() on_voted=on_voted /> }.into_any()
                    } else {
                        view! {
                            <div class="actions">
                                <button class="small" on:click=move |_| choosing.set(true)>
                                    {if voted { tr!("Change my choice") } else { tr!("Keep it") }}

                                </button>
                            </div>
                        }.into_any()
                    }}
                </section>
            }.into_any()
        })}
    }
    .into_any()
}

#[component]
pub fn Forever() -> impl IntoView {
    let stats = RwSignal::new(None::<Result<ForeverStats, String>>);
    let person = RwSignal::new(None::<String>);
    let spread = RwSignal::new("all");
    spawn_local(async move {
        let Some(k) = state::ctx().keep_client() else {
            return stats.set(Some(Err(no_keep())));
        };
        stats.set(Some(k.admin_forever().await.map_err(|e| admin_error(&e))));
    });
    view! {
        <h1>{tr!("Kept forever")}</h1>
        {move || match stats.get() {
            None => view! { <Loading /> }.into_any(),
            Some(Err(e)) => view! { <ErrorText text=e /> }.into_any(),
            Some(Ok(s)) if s.people.is_empty() => view! { <p class="muted">{tr!("Nobody keeps anything forever yet.")}</p> }.into_any(),
            Some(Ok(s)) => {
                let names: std::collections::HashMap<String, String> = s.people.iter().map(|p| (p.user_key.clone(), p.name.clone())).collect();
                let name = move |k: &str| names.get(k).cloned().unwrap_or_else(|| k.to_owned());
                let n2 = name.clone();
                let items = s.items.clone();
                view! {
                    <p class="muted">{tr!("{titles} titles · {shared} kept by several people · {alone} by one person", titles = s.titles, shared = s.shared, alone = s.titles.saturating_sub(s.shared))}</p>
                    <section class="group">
                        <h2>{tr!("People")}</h2>
                        {s.people.into_iter().map(|p| {
                            let k = p.user_key.clone();
                            let k2 = p.user_key.clone();
                            let overlaps = p.overlaps.iter().take(3).map(|o| format!("{} ({})", o.name, o.both)).collect::<Vec<_>>().join(", ");
                            let total = p.count.max(1) as f32;
                            view! {
                                <button class="person" class:selected=move || person.get().as_deref() == Some(k.as_str())
                                    on:click=move |_| person.update(|x| *x = if x.as_deref() == Some(k2.as_str()) { None } else { Some(k2.clone()) })>
                                    <div class="row">
                                        <span class="person-head">
                                            <Avatar user_key=p.user_key.clone() name=p.name.clone() avatar=p.avatar.clone() size=32 />
                                            <strong>{name_with_media(&p.name, &p.media_name)}</strong>
                                        </span>
                                        <span>{p.count}</span>
                                    </div>
                                    <div class="muted small">{if overlaps.is_empty() {
                                        tr!("{shared} shared · {alone} only them", shared = p.shared, alone = p.alone)
                                    } else {
                                        tr!("{shared} shared · {alone} only them · most with {people}", shared = p.shared, alone = p.alone, people = overlaps)
                                    }}</div>
                                    <div class="split"><div class="shared" style=format!("width:{}%", p.shared as f32 / total * 100.0) /></div>
                                </button>
                            }
                        }).collect_view()}
                    </section>
                    <div class="chips">
                        {[("all", tr!("All")), ("shared", tr!("Kept by several")), ("alone", tr!("Kept by one person"))].map(|(k, l)| view! {
                            <button class="chip" class:selected=move || spread.get() == k on:click=move |_| spread.set(k)>{l}</button>
                        }).collect_view()}
                    </div>
                    <div class="leaving">
                        {move || items.iter().filter(|t| person.get().is_none_or(|p| t.keepers.contains(&p)))
                            .filter(|t| match spread.get() { "shared" => t.keepers.len() > 1, "alone" => t.keepers.len() == 1, _ => true })
                            .map(|t| {
                                let who = t.keepers.iter().map(|k| n2(k)).collect::<Vec<_>>().join(", ");
                                view! {
                                    <div class="leaving-row">
                                        {t.poster_url.clone().map(|p| view! { <img src=p loading="lazy" alt="" /> })}
                                        <div class="leaving-info">
                                            <strong>{t.title.clone()}{t.year.map(|y| format!(" ({y})"))}</strong>
                                            <span class="muted">{who}</span>
                                        </div>
                                    </div>
                                }
                            }).collect_view()}
                    </div>
                }.into_any()
            }
        }}
    }
}

const DAY: i64 = 86_400;

fn now_s() -> i64 {
    (js_sys::Date::now() / 1000.0) as i64
}

/// True when the date has passed but the owner has not turned deletion on: nothing happens yet.
fn paused(i: &LeavingItem, now: i64) -> bool {
    !i.collection_active
        && i.protection == Protection::None
        && i.effective_deletion_date.unwrap_or(i64::MAX) < now
}

/// The countdown as on the apps' poster badges ("3 days", "Today", "Not scheduled"…), and whether
/// it is urgent.
pub fn short_label(i: &LeavingItem, now: i64) -> (String, bool) {
    if matches!(i.protection, Protection::Forever | Protection::UntilWatched) {
        return (tr!("Kept"), false);
    }
    if paused(i, now) {
        return (tr!("Not scheduled"), false);
    }
    let Some(d) = i.effective_deletion_date else {
        return (tr!("Soon"), false);
    };
    let days = (d - now).div_euclid(DAY);
    match days {
        ..0 => (tr!("Any day now"), true),
        0 => (tr!("Today"), true),
        n => (trn!("{n} day", "{n} days", n), n <= 7),
    }
}

fn vote_req(
    kind: VoteKind,
    days: Option<u32>,
    reminder: Option<u32>,
    reason: Option<&str>,
) -> VoteRequest {
    VoteRequest {
        kind,
        days,
        reminder_days: reminder,
        reason: reason.map(str::to_owned),
    }
}

/// The swipe queue while one of its titles' page is open.
const TRIAGE_BACK: &str = "aw.triage.back";

/// What TRIAGE_BACK holds: the queue, how many are done, the keys decided during the visit.
type Back = (Vec<LeavingItem>, u32, Vec<String>);

/// "Sort them out": one undecided title at a time, decided with a button or an arrow key.
#[component]
pub fn Triage() -> impl IntoView {
    // Back from a title's page: the same card, where it was (kept for this tab only).
    let back = state::tab_load::<Back>(TRIAGE_BACK);
    state::tab_save::<Back>(TRIAGE_BACK, None);
    let resumed = back.is_some();
    let (back_queue, back_done, back_sorted) = back.map_or((None, 0, vec![]), |(q, d, s)| {
        // Decided meanwhile (on the title's page, or elsewhere): not asked again.
        let decided: Vec<String> = state::load::<Vec<LeavingItem>>(state::LEAVING)
            .unwrap_or_default()
            .into_iter()
            .filter(|i| i.my_vote.is_some())
            .map(|i| i.key)
            .collect();
        let q = q
            .into_iter()
            .filter(|i| !decided.contains(&i.key))
            .collect();
        (Some(q), d, s)
    });
    let queue = RwSignal::new(back_queue);
    let error = RwSignal::new(None::<String>);
    let done = RwSignal::new(back_done);
    // Decided during this visit: the next batch from the server may not know yet.
    let sorted = RwSignal::new(back_sorted);
    // The next batch, once this one is done (the server hands out 50 at a time).
    let more = RwSignal::new(None::<Vec<LeavingItem>>);
    Effect::new(move |_| {
        let empty = queue.with(|q| q.as_ref().is_some_and(Vec::is_empty));
        if !empty || done.get() == 0 {
            return;
        }
        spawn_local(async move {
            // The last votes reach the server first.
            state::sleep(800).await;
            let Some(k) = state::ctx().keep_client() else {
                return;
            };
            if let Ok(l) = k.triage(50).await {
                let fresh: Vec<LeavingItem> = sorted
                    .with_untracked(|s| l.into_iter().filter(|i| !s.contains(&i.key)).collect());
                more.try_set(Some(fresh));
            }
        });
    });
    spawn_local(async move {
        if resumed {
            return;
        }
        let Some(k) = state::ctx().keep_client() else {
            return error.set(Some(no_keep()));
        };
        match k.triage(50).await {
            Ok(l) => queue.set(Some(l)),
            Err(e) => error.set(Some(state::plain_keep(&e))),
        }
    });
    let vote = move |r: VoteRequest| {
        let Some(item) = queue.with_untracked(|q| q.as_ref().and_then(|q| q.first().cloned()))
        else {
            return;
        };
        let Some(k) = state::ctx().keep_client() else {
            return error.set(Some(no_keep()));
        };
        // At once to the next one; put back if the server says no.
        queue.update(|q| {
            if let Some(q) = q {
                q.remove(0);
            }
        });
        done.update(|d| *d += 1);
        sorted.update(|s| s.push(item.key.clone()));
        error.set(None);
        spawn_local(async move {
            match k.vote(&item.key, &r).await {
                // The list kept for Leaving soon follows.
                Ok(updated) => remember_vote(&updated),
                Err(e) => {
                    error.try_set(Some(tr!(
                        "{title} wasn't saved: {error}",
                        title = item.title,
                        error = state::plain_keep(&e)
                    )));
                    sorted.try_update(|s| s.retain(|k| *k != item.key));
                    // At the end, not under the card being decided now.
                    queue.try_update(|q| {
                        if let Some(q) = q
                            && !q.iter().any(|x| x.key == item.key)
                        {
                            q.push(item);
                        }
                    });
                    done.try_update(|d| *d = d.saturating_sub(1));
                }
            }
        });
    };
    let until = move || vote(vote_req(VoteKind::KeepUntilWatched, None, Some(7), None));
    let forever = move || vote(vote_req(VoteKind::KeepForever, None, None, None));
    let days = move || vote(vote_req(VoteKind::KeepDays, Some(30), None, None));
    let dont = move || vote(vote_req(VoteKind::DontCare, None, None, None));
    let card = NodeRef::<leptos::html::Div>::new();
    let on_key = move |ev: web_sys::KeyboardEvent| {
        match ev.key().as_str() {
            // The title's page (its link inside the card).
            "Enter" => {
                use wasm_bindgen::JsCast;
                if let Some(a) = card
                    .get_untracked()
                    .and_then(|c| c.query_selector("a").ok().flatten())
                    .and_then(|a| a.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    a.click();
                }
            }
            "ArrowLeft" => dont(),
            "ArrowRight" => until(),
            "ArrowUp" => forever(),
            _ => return,
        }
        ev.prevent_default();
    };
    // Keyboard first: the card has the focus, so the arrows decide at once.
    Effect::new(move |_| {
        queue.track();
        if let Some(c) = card.get() {
            let _ = c.focus();
        }
    });
    // Same for "Keep going" when the next batch shows up.
    let keep_going = NodeRef::<leptos::html::Button>::new();
    Effect::new(move |_| {
        if let Some(b) = keep_going.get() {
            let _ = b.focus();
        }
    });
    view! {
        <div class="triage">
            <h1>{tr!("Swipe them!")}</h1>
            {move || {
                let left = queue.with(|q| q.as_ref().map_or(0, Vec::len)) as u32;
                let total = left + done.get();
                (total > 0).then(|| view! {
                    <div class="split triage-bar"><div class="shared" style=format!("width:{}%", done.get() as f32 / total as f32 * 100.0) /></div>
                })
            }}
            {move || error.get().map(|e| view! { <ErrorText text=e /> })}
            {move || match queue.get() {
                None if error.with(Option::is_none) => view! { <Loading /> }.into_any(),
                None => ().into_any(),
                Some(q) if q.is_empty() => {
                    let n = done.get();
                    view! {
                        <p class="muted">{if n > 0 { trn!("All done, thanks! {n} sorted.", "All done, thanks! {n} sorted.", n) } else { tr!("Nothing to sort right now.") }}</p>
                        {move || match more.get().filter(|m| !m.is_empty()) {
                            Some(next) => {
                                let count = next.len();
                                view! {
                                    <div class="actions">
                                        <button class="primary" node_ref=keep_going on:click=move |_| {
                                            more.set(None);
                                            queue.set(Some(next.clone()));
                                        }>{trn!("Keep going ({n} more)", "Keep going ({n} more)", count)}</button>
                                        <A href="/leaving" attr:class="button">{tr!("Done")}</A>
                                    </div>
                                }.into_any()
                            }
                            None => view! { <A href="/leaving" attr:class="button">{tr!("Back to Leaving soon")}</A> }.into_any(),
                        }}
                    }.into_any()
                }
                Some(q) => {
                    let item = q[0].clone();
                    let (label, urgent) = short_label(&item, now_s());
                    let c = &item.counts;
                    let keepers = c.forever + c.until_watched + c.days;
                    view! {
                        <p class="muted small">{trn!("{n} left · ← fine to delete   → keep until I watch it   ↑ keep forever", "{n} left · ← fine to delete   → keep until I watch it   ↑ keep forever", q.len())}</p>
                        <div class="triage-body">
                            <div class="triage-card" tabindex="0" node_ref=card on:keydown=on_key>
                                // A click (Enter) opens the title's page; Back comes back to this card.
                                {match (state::ctx().keep.get_untracked().map(|k| k.account), item.server_item_id.clone()) {
                                    (Some(acc), Some(id)) => {
                                        let remember = move |_| {
                                            let d = done.get_untracked();
                                            let s = sorted.get_untracked();
                                            queue.with_untracked(|q| {
                                                state::tab_save(TRIAGE_BACK, q.as_ref().map(|q| (q.clone(), d, s)).as_ref());
                                            });
                                        };
                                        view! {
                                            <A href=format!("/item/{acc}/{id}") attr:aria-label=item.title.clone() on:click=remember>
                                                {item.poster_url.clone().map(|p| view! { <img src=p alt="" /> })}
                                            </A>
                                        }.into_any()
                                    }
                                    _ => item.poster_url.clone().map(|p| view! { <img src=p alt="" /> }).into_any(),
                                }}
                            </div>
                            <div class="triage-info">
                                <h2>{item.title.clone()}</h2>
                                <p>
                                    {item.year.map(|y| format!("{y} "))}
                                    <span class="state" class:urgent=urgent>{label}</span>
                                </p>
                                {(keepers > 0).then(|| view! {
                                    <p class="muted small">{trn!("{n} person keeps it", "{n} people keep it", keepers)}</p>
                                })}
                                <div class="actions">
                                    <button class="primary" on:click=move |_| until()>{tr!("Until I watch it")}</button>
                                    <button on:click=move |_| forever()>{tr!("♥ Forever")}</button>
                                    <button on:click=move |_| days()>{tr!("Keep 30 days")}</button>
                                    <button on:click=move |_| dont()>{tr!("✕ Fine to delete")}</button>
                                </div>
                            </div>
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

/// Admin calls: a refusal means this account isn't an admin (any more).
pub(crate) fn admin_error(e: &net::Error) -> String {
    match e {
        net::Error::Unauthorized => tr!("Only admins of this AlwyWatch server can see this."),
        other => state::plain_keep(other),
    }
}

/// The next value of a cycling choice ("Max days: ∞ → 7 → 30 → 90 → ∞"), as on the apps.
pub fn cycle(options: &[Option<u32>], current: Option<u32>) -> Option<u32> {
    let at = options.iter().position(|o| *o == current);
    options[at.map_or(0, |i| (i + 1) % options.len())]
}

fn limit(v: Option<u32>) -> String {
    v.map_or("∞".into(), |n| n.to_string())
}

/// Same warning as the apps: this rewrites Maintainerr's deletion dates.
fn countdown_warning() -> String {
    tr!(
        "Every item is already past its date. Before activating a collection in Maintainerr, restart its countdown so people get the full delay to vote. This changes deletion dates in Maintainerr."
    )
}

/// A countdown waiting for its confirmation: collection, its title, stagger days.
type Armed = (i64, String, u32);

/// Settings → Admin: people's rights, the collection countdown (behind a confirmation) and
/// the link to Kept forever.
#[component]
pub fn Admin() -> impl IntoView {
    let users = RwSignal::new(None::<Result<Vec<AdminUser>, String>>);
    let seerr_users = RwSignal::new(Vec::<KeepSeerrUser>::new());
    let message = RwSignal::new(None::<String>);
    // The countdown's result, shown in its own section (next to the buttons).
    let countdown_result = RwSignal::new(None::<String>);
    let armed = RwSignal::new(None::<Armed>);
    let busy = RwSignal::new(false);
    let items = RwSignal::new(state::load::<Vec<LeavingItem>>(state::LEAVING).unwrap_or_default());
    let cancel = NodeRef::<leptos::html::Button>::new();
    // The dialog opens with the focus on Cancel (Enter or Space can't change dates by mistake).
    Effect::new(move |_| {
        if let Some(b) = cancel.get() {
            let _ = b.focus();
        }
    });
    spawn_local(async move {
        let Some(k) = state::ctx().keep_client() else {
            return users.set(Some(Err(no_keep())));
        };
        let ((u, su), l) = state::join2(
            state::join2(k.admin_users(), k.admin_seerr_users()),
            k.leaving_soon(),
        )
        .await;
        users.set(Some(u.map_err(|e| admin_error(&e))));
        seerr_users.set(su.unwrap_or_default());
        if let Ok(l) = l {
            state::save(state::LEAVING, &l);
            items.set(l);
        }
    });
    let collections = move || {
        let mut out: Vec<(i64, String)> = Vec::new();
        for i in items.get() {
            if let Some(id) = i.collection_id
                && !out.iter().any(|(x, _)| *x == id)
            {
                out.push((
                    id,
                    i.collection_title
                        .clone()
                        .unwrap_or_else(|| tr!("Collection {id}", id = id)),
                ));
            }
        }
        out
    };
    let replace = move |nu: AdminUser| {
        users.update(|u| {
            if let Some(Ok(list)) = u
                && let Some(x) = list.iter_mut().find(|x| x.user_key == nu.user_key)
            {
                *x = nu;
            }
        })
    };
    let set_rights = move |user_key: String, r: Rights| {
        spawn_local(async move {
            let Some(k) = state::ctx().keep_client() else {
                return;
            };
            match k.admin_set_rights(&user_key, &r).await {
                Ok(nu) => replace(nu),
                Err(e) => message.set(Some(admin_error(&e))),
            }
        });
    };
    let link = move |user_key: String, seerr: Option<i64>| {
        spawn_local(async move {
            let Some(k) = state::ctx().keep_client() else {
                return;
            };
            match k.admin_link_seerr(&user_key, seerr).await {
                Ok(nu) => replace(nu),
                Err(e) => message.set(Some(admin_error(&e))),
            }
        });
    };
    let confirm = move |_| {
        let Some((id, title, stagger)) = armed.get_untracked() else {
            return;
        };
        if busy.get_untracked() {
            return;
        }
        let Some(k) = state::ctx().keep_client() else {
            armed.set(None);
            return countdown_result.set(Some(no_keep()));
        };
        busy.set(true);
        spawn_local(async move {
            let r = k
                .admin_countdown(
                    id,
                    &CountdownRequest {
                        stagger_days: stagger,
                    },
                )
                .await;
            countdown_result.try_set(Some(match r {
                Ok(r) => tr!(
                    "{title}: {restarted} restarted, {skipped} skipped",
                    title = title,
                    restarted = r.reset,
                    skipped = r.skipped
                ),
                Err(e) => tr!("{title}: {error}", title = title, error = admin_error(&e)),
            }));
            busy.try_set(false);
            armed.try_set(None);
        });
    };
    view! {
        {move || message.get().map(|m| view! { <p class="accent">{m}</p> })}
        <section class="group">
            <h2>{tr!("People")}</h2>
            {move || match users.get() {
                None => view! { <Loading /> }.into_any(),
                Some(Err(e)) => view! { <ErrorText text=e /> }.into_any(),
                Some(Ok(list)) => list.into_iter().map(|u| {
                    let r = u.rights.clone();
                    let key = u.user_key.clone();
                    let (k1, k2, k3, k4) = (key.clone(), key.clone(), key.clone(), key);
                    let (r1, r2, r3) = (r.clone(), r.clone(), r.clone());
                    let server = if u.user_key.starts_with("plex:") { "Plex" } else { "Jellyfin" };
                    let linked = u.seerr_user_id;
                    view! {
                        <div class="person-rights">
                            <div class="person-head">
                                <Avatar user_key=u.user_key.clone() name=u.name.clone() avatar=u.avatar.clone() size=32 />
                                <strong>{name_with_media(&u.name, &u.media_name)}{u.is_admin.then(|| format!(" · {}", tr!("admin")))}</strong>
                            </div>
                            <div class="muted small">{tr!("{server} · {count} kept forever", server = server, count = u.forever_count)}</div>
                            <div class="actions tight">
                                <button class="small" class:selected=!r.can_keep_forever on:click=move |_| {
                                    set_rights(k1.clone(), Rights { can_keep_forever: !r1.can_keep_forever, ..r1.clone() })
                                }>{if r.can_keep_forever { tr!("Forever allowed") } else { tr!("Forever blocked") }}</button>
                                <button class="small" on:click=move |_| {
                                    set_rights(k2.clone(), Rights { max_forever: cycle(&[None, Some(5), Some(10), Some(25)], r2.max_forever), ..r2.clone() })
                                }>{tr!("Max forever: {limit}", limit = limit(r.max_forever))}</button>
                                <button class="small" on:click=move |_| {
                                    set_rights(k3.clone(), Rights { max_keep_days: cycle(&[None, Some(7), Some(30), Some(90)], r3.max_keep_days), ..r3.clone() })
                                }>{tr!("Max days: {limit}", limit = limit(r.max_keep_days))}</button>
                                {move || (!seerr_users.with(Vec::is_empty)).then(|| {
                                    let k4 = k4.clone();
                                    view! {
                                        <select class="chip small" aria-label=tr!("Requests as")
                                            prop:value=linked.map(|i| i.to_string()).unwrap_or_default()
                                            on:change=move |ev| link(k4.clone(), event_target_value(&ev).parse().ok())>
                                            <option value="">{tr!("Requests as: automatic")}</option>
                                            {seerr_users.get().into_iter().map(|s| view! {
                                                <option value=s.id.to_string() selected=linked == Some(s.id)>{tr!("Requests as: {name}", name = s.name)}</option>
                                            }).collect_view()}
                                        </select>
                                    }
                                })}
                            </div>
                        </div>
                    }
                }).collect_view().into_any(),
            }}
            <p class="muted small">{tr!("Everyone can keep forever by default. Click a value to change it.")}</p>
        </section>
        <section class="group">
            <h2>{tr!("Leaving soon countdown")}</h2>
            {move || {
                let list = collections();
                if list.is_empty() {
                    return view! { <p class="muted">{tr!("No collection has items leaving.")}</p> }.into_any();
                }
                list.into_iter().map(|(id, title)| view! {
                    <div class="row wrap">
                        <span>{title.clone()}</span>
                        <div class="actions tight">
                            {[0u32, 14, 30].map(|stagger| {
                                let title = title.clone();
                                let label = if stagger == 0 { tr!("Restart now") } else { trn!("Spread over {n} day", "Spread over {n} days", stagger) };
                                // A new try: the last result no longer applies.
                                view! { <button class="small" on:click=move |_| {
                                    countdown_result.set(None);
                                    armed.set(Some((id, title.clone(), stagger)));
                                }>{label}</button> }
                            }).collect_view()}
                        </div>
                    </div>
                }).collect_view().into_any()
            }}
            {move || countdown_result.get().map(|m| view! { <p class="accent" role="status">{m}</p> })}
            <p class="muted small">{countdown_warning()}" "{tr!("You'll be asked to confirm.")}</p>
        </section>
        <section class="group">
            <h2>{tr!("Kept forever")}</h2>
            <A href="/admin/forever" attr:class="row link-row">
                <div><strong>{tr!("Who keeps what forever")}</strong>
                    <div class="muted small">{tr!("Per person, and how much of it someone else keeps too")}</div></div>
                <span>"›"</span>
            </A>
        </section>
        {move || armed.get().map(|(_, title, stagger)| view! {
            <div class="dialog-back" on:click=move |_| if !busy.get_untracked() { armed.set(None) }
                on:keydown=move |ev| if ev.key() == "Escape" && !busy.get_untracked() { armed.set(None) }>
                <div class="dialog" role="alertdialog" aria-modal="true" aria-labelledby="countdown-title" on:click=|ev| ev.stop_propagation()>
                    <h2 id="countdown-title">{tr!("Restart the countdown of {title}?", title = title)}</h2>
                    <p>{countdown_warning()}</p>
                    <p class="muted">{if stagger == 0 {
                        tr!("Every item of this collection gets its full delay starting today.")
                    } else {
                        trn!("New deletion dates are spread over {n} day.", "New deletion dates are spread over {n} days.", stagger)
                    }}</p>
                    <div class="actions">
                        <button node_ref=cancel disabled=move || busy.get() on:click=move |_| armed.set(None)>{tr!("Cancel")}</button>
                        <button class="danger" disabled=move || busy.get() on:click=confirm>
                            {move || if busy.get() { tr!("Changing dates…") } else { tr!("Change deletion dates") }}

                        </button>
                    </div>
                </div>
            </div>
        })}
    }
}

/// How long ago, roughly ("5 min", "2 h", "3 days").
fn ago(seconds: i64) -> String {
    let s = seconds.max(0);
    if s < 3600 {
        tr!("{n} min", n = (s / 60).max(1))
    } else if s < 2 * 86_400 {
        tr!("{n} h", n = s / 3600)
    } else {
        trn!("{n} day", "{n} days", s / 86_400)
    }
}

/// One line of the server status: what, and its state or what to do; `fix`: the Server
/// settings section that changes it, linked when it is off.
fn status_line(what: String, state: String, ok: bool, fix: &'static str) -> impl IntoView {
    let href = format!("/settings/admin/server#{fix}");
    view! {
        <div class="row">
            <div><strong>{what}</strong>
                <div class="small" class:muted=ok class:accent=!ok>{state}</div></div>
            {(!fix.is_empty()).then(|| view! { <A href=href attr:class="button small subtle">{tr!("Server settings")}</A> })}
        </div>
    }
}

/// The Maintainerr line of the server status.
fn maintainerr_line(st: &ServerStatus, now: i64) -> (String, bool) {
    match st.maintainerr.as_str() {
        "off" => (tr!("Off. Set up Maintainerr in Server settings."), false),
        "starting" => (tr!("Reading Maintainerr for the first time…"), true),
        "ok" => (
            trn!(
                "Maintainerr {version} · read {time} ago · {n} title leaving",
                "Maintainerr {version} · read {time} ago · {n} titles leaving",
                st.leaving_items,
                version = st.maintainerr_version.clone().unwrap_or_default(),
                time = ago(now - st.last_sync.unwrap_or(now)),
            ),
            true,
        ),
        _ => (
            tr!(
                "Can't reach Maintainerr. Check its address and API key in Server settings (Test tells why)."
            ),
            false,
        ),
    }
}

/// Settings → Admin → Server status: what the AlwyWatch server is connected to, each line
/// with what to do when it's off and a link to that part of Server settings.
#[component]
pub fn ServerStatusView() -> impl IntoView {
    let status = RwSignal::new(None::<Result<ServerStatus, String>>);
    spawn_local(async move {
        let Some(k) = state::ctx().keep_client() else {
            return status.set(Some(Err(no_keep())));
        };
        status.set(Some(k.admin_status().await.map_err(|e| admin_error(&e))));
    });
    view! {
        <section class="group">
            <h2>{tr!("Server status")}</h2>
            {move || match status.get() {
                None => view! { <Loading /> }.into_any(),
                Some(Err(e)) => view! { <ErrorText text=e /> }.into_any(),
                Some(Ok(st)) => {
                    let now = (js_sys::Date::now() / 1000.0) as i64;
                    let (maintainerr, maintainerr_ok) = maintainerr_line(&st, now);
                    let on = || tr!("On");
                    view! {
                        {st.dry_run.then(|| view! {
                            <p class="banner" role="status">{tr!("Test mode: nothing is changed in Maintainerr or Seerr. Turn it off in Server settings.")}" "<A href="/settings/admin/server#testmode" attr:class="button small">{tr!("Server settings")}</A></p>
                        })}
                        {status_line(tr!("AlwyWatch server"), st.version.clone(), true, "")}
                        {status_line(tr!("Leaving soon"), maintainerr, maintainerr_ok, if maintainerr_ok { "" } else { "maintainerr" })}
                        {status_line(tr!("Requests"), if st.seerr_bridge {
                            tr!("On: every app requests as each person's Seerr user.")
                        } else {
                            tr!("Off: only direct request-site logins work. Turn it on in Server settings → Seerr.")
                        }, st.seerr_bridge, if st.seerr_bridge { "" } else { "seerr" })}
                        {status_line(tr!("Keep until watched (Jellyfin)"), if st.jellyfin_key {
                            on()
                        } else {
                            tr!("Jellyfin keeps don't end by themselves: AlwyWatch has no API key on Jellyfin. Renew it in Server settings → Media servers.")
                        }, st.jellyfin_key, if st.jellyfin_key { "" } else { "servers" })}
                        {status_line(tr!("Keep until watched (Plex)"), if st.tautulli {
                            on()
                        } else {
                            tr!("Plex keeps don't end by themselves. Set up Tautulli in Server settings.")
                        }, st.tautulli, if st.tautulli { "" } else { "tautulli" })}
                        {status_line(tr!("Webhooks"), if st.webhooks {
                            tr!("On: watched titles are noticed at once.")
                        } else {
                            tr!("Optional: watched titles are noticed within 15 min. For instant, paste the webhook addresses from Server settings.")
                        }, true, if st.webhooks { "" } else { "webhooks" })}
                        {status_line(tr!("Passkeys"), if st.passkeys {
                            on()
                        } else {
                            tr!("Off: set this site's address (https) in Server settings.")
                        }, st.passkeys, if st.passkeys { "" } else { "signup" })}
                        {status_line("Pocket ID".to_owned(), if st.pocketid { on() } else { tr!("Off") }, true, if st.pocketid { "" } else { "pocketid" })}
                        {status_line(tr!("New accounts"), match st.registration.as_str() {
                            "open" => tr!("Anyone can create one"),
                            "invite" => tr!("With an invitation"),
                            _ => tr!("Closed"),
                        }, true, "")}
                    }.into_any()
                }
            }}
            <p class="muted small">{
                let (before, after) = crate::ui::around(&tr!("Change these in {page}."), "{page}");
                view! { {before}<A href="/settings/admin/server" attr:class="accent">{tr!("Server settings")}</A>{after} }
            }</p>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(protection: Protection, active: bool, date: Option<i64>) -> LeavingItem {
        LeavingItem {
            key: "1".into(),
            title: "X".into(),
            year: None,
            kind: "movie".into(),
            poster_url: None,
            tmdb_id: None,
            tvdb_id: None,
            server_item_id: None,
            collection_id: None,
            collection_title: None,
            collection_active: active,
            deletion_date: date,
            effective_deletion_date: date,
            protection,
            my_vote: None,
            counts: Default::default(),
            listed: true,
        }
    }

    #[test]
    fn countdown_labels_match_the_apps() {
        let now = 1_000_000;
        assert_eq!(
            short_label(&item(Protection::Forever, true, None), now).0,
            "Kept"
        );
        assert_eq!(
            short_label(&item(Protection::None, false, Some(now - 10)), now).0,
            "Not scheduled"
        );
        assert_eq!(
            short_label(&item(Protection::None, true, Some(now - 10)), now),
            ("Any day now".into(), true)
        );
        assert_eq!(
            short_label(&item(Protection::None, true, Some(now + 100)), now),
            ("Today".into(), true)
        );
        assert_eq!(
            short_label(&item(Protection::None, true, Some(now + 3 * DAY + 5)), now),
            ("3 days".into(), true)
        );
        assert_eq!(
            short_label(&item(Protection::None, true, Some(now + 9 * DAY)), now),
            ("9 days".into(), false)
        );
    }

    #[test]
    fn title_page_panel_words_like_android() {
        let now = 1_000_000;
        let mut kept = item(Protection::UntilWatched, true, Some(now + 20 * DAY));
        kept.counts.until_watched = 1;
        assert_eq!(
            keep_policy(&kept, now),
            "Kept until the person who asked has watched it"
        );
        kept.counts.until_watched = 3;
        kept.counts.forever = 1;
        kept.collection_title = Some("Old movies".into());
        assert_eq!(
            keep_policy(&kept, now),
            "Kept until 3 people have watched it"
        );
        assert_eq!(
            keep_detail(&kept, now, |_| "D".into()),
            "In “Old movies” · date D · kept by 1 forever, 3 until watched"
        );
        let paused = item(Protection::None, false, Some(now - 10));
        assert_eq!(keep_policy(&paused, now), "On the deletion list");
        assert_eq!(
            keep_detail(&paused, now, |_| "D".into()),
            "Deletion isn't turned on yet"
        );
        assert_eq!(
            keep_policy(&item(Protection::Forever, true, None), now),
            "Kept forever"
        );
    }

    #[test]
    fn limits_cycle_and_wrap() {
        let opts = [None, Some(7), Some(30), Some(90)];
        assert_eq!(cycle(&opts, None), Some(7));
        assert_eq!(cycle(&opts, Some(90)), None);
        // An odd value set elsewhere starts the cycle over.
        assert_eq!(cycle(&opts, Some(12)), None);
    }
}
