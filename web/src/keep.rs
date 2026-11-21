// SPDX-License-Identifier: AGPL-3.0-or-later
//! Leaving soon (what the server will delete, and keeping it) and, for admins, who keeps what
//! forever with the overlaps.

use keep_service::{ForeverStats, LeavingItem, Protection, VoteKind, VoteRequest};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::state;
use crate::ui::{ErrorText, Loading};

fn date(ts: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts as f64 * 1000.0));
    String::from(d.to_locale_date_string("default", &wasm_bindgen::JsValue::UNDEFINED))
}

fn status(i: &LeavingItem) -> String {
    match i.protection {
        Protection::Forever => "Kept forever".into(),
        Protection::UntilWatched => "Kept until watched".into(),
        Protection::Extended => i
            .effective_deletion_date
            .map(|d| format!("Kept until {}", date(d)))
            .unwrap_or_default(),
        Protection::None if !i.collection_active => "Not scheduled yet".into(),
        Protection::None => i
            .deletion_date
            .map(|d| format!("Leaves {}", date(d)))
            .unwrap_or_default(),
    }
}

fn mine(i: &LeavingItem) -> Option<&'static str> {
    i.my_vote.as_ref().map(|v| match v.kind {
        VoteKind::KeepForever => "You keep it forever",
        VoteKind::KeepUntilWatched => "You keep it until you watch it",
        VoteKind::KeepDays => "You extended it",
        VoteKind::DontCare => "You don't mind",
    })
}

#[component]
pub fn Leaving() -> impl IntoView {
    let items = RwSignal::new(state::load::<Vec<LeavingItem>>("aw.leaving"));
    let error = RwSignal::new(None::<String>);
    let filter = RwSignal::new("all");
    spawn_local(async move {
        let Some(k) = state::ctx().keep_client() else {
            return;
        };
        match k.leaving_soon().await {
            Ok(l) => {
                state::save("aw.leaving", &l);
                items.set(Some(l));
            }
            Err(e) => error.set(Some(e.to_string())),
        }
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
        <h1>"Leaving soon"</h1>
        <p class="muted">"What the server will delete. Keep what you want to watch or keep."</p>
        <div class="chips">
            {[("all", "All"), ("undecided", "Not decided"), ("mine", "Kept by me")].map(|(k, l)| view! {
                <button class="chip" class:selected=move || filter.get() == k on:click=move |_| filter.set(k)>{l}</button>
            }).collect_view()}
        </div>
        {move || error.get().map(|e| view! { <ErrorText text=e /> })}
        {move || items.get().is_none().then(|| view! { <Loading /> })}
        <div class="leaving">
            {move || shown().into_iter().map(|i| view! { <LeavingRow item=i items=items /> }).collect_view()}
        </div>
    }
}

#[component]
fn LeavingRow(item: LeavingItem, items: RwSignal<Option<Vec<LeavingItem>>>) -> impl IntoView {
    let key = item.key.clone();
    let busy = RwSignal::new(false);
    let vote = move |req: Option<VoteRequest>| {
        let key = key.clone();
        busy.set(true);
        spawn_local(async move {
            let Some(k) = state::ctx().keep_client() else {
                return;
            };
            let res = match req {
                Some(r) => k.vote(&key, &r).await,
                None => k.unvote(&key).await,
            };
            if let Ok(updated) = res {
                items.update(|l| {
                    if let Some(l) = l {
                        if let Some(x) = l.iter_mut().find(|x| x.key == updated.key) {
                            *x = updated;
                        }
                        state::save("aw.leaving", l);
                    }
                });
            }
            busy.set(false);
        });
    };
    let req = |kind, days| VoteRequest {
        kind,
        days,
        reminder_days: None,
        reason: None,
    };
    let (v1, v2, v3, v4, v5) = (vote.clone(), vote.clone(), vote.clone(), vote.clone(), vote);
    let keepers = item.counts.forever + item.counts.until_watched + item.counts.days;
    view! {
        <div class="leaving-row">
            {item.poster_url.clone().map(|p| view! { <img src=p loading="lazy" alt="" /> })}
            <div class="leaving-info">
                <strong>{item.title.clone()}</strong>
                <span class="muted">{item.year.map(|y| y.to_string())}" · "{status(&item)}</span>
                {(keepers > 0).then(|| view! { <span class="muted small">{format!("{keepers} keep it")}</span> })}
                {mine(&item).map(|m| view! { <span class="accent small">{m}</span> })}
                <div class="actions">
                    <button class="small" disabled=move || busy.get() on:click=move |_| v1(Some(req(VoteKind::KeepUntilWatched, None)))>"Until I watch it"</button>
                    <button class="small" disabled=move || busy.get() on:click=move |_| v2(Some(req(VoteKind::KeepForever, None)))>"Forever"</button>
                    <button class="small" disabled=move || busy.get() on:click=move |_| v3(Some(req(VoteKind::KeepDays, Some(30))))>"30 more days"</button>
                    <button class="small" disabled=move || busy.get() on:click=move |_| v4(Some(req(VoteKind::DontCare, None)))>"Don't care"</button>
                    {item.my_vote.is_some().then(|| view! {
                        <button class="small subtle" disabled=move || busy.get() on:click=move |_| v5(None)>"Undo"</button>
                    })}
                </div>
            </div>
        </div>
    }
}

#[component]
pub fn Forever() -> impl IntoView {
    let stats = RwSignal::new(None::<Result<ForeverStats, String>>);
    let person = RwSignal::new(None::<String>);
    let spread = RwSignal::new("all");
    spawn_local(async move {
        let Some(k) = state::ctx().keep_client() else {
            return;
        };
        stats.set(Some(k.admin_forever().await.map_err(|e| e.to_string())));
    });
    view! {
        <h1>"Kept forever"</h1>
        {move || match stats.get() {
            None => view! { <Loading /> }.into_any(),
            Some(Err(e)) => view! { <ErrorText text=e /> }.into_any(),
            Some(Ok(s)) if s.people.is_empty() => view! { <p class="muted">"Nobody keeps anything forever yet."</p> }.into_any(),
            Some(Ok(s)) => {
                let names: std::collections::HashMap<String, String> = s.people.iter().map(|p| (p.user_key.clone(), p.name.clone())).collect();
                let name = move |k: &str| names.get(k).cloned().unwrap_or_else(|| k.to_owned());
                let n2 = name.clone();
                let items = s.items.clone();
                view! {
                    <p class="muted">{format!("{} titles · {} kept by several people · {} by one person", s.titles, s.shared, s.titles - s.shared)}</p>
                    <section class="group">
                        <h2>"People"</h2>
                        {s.people.into_iter().map(|p| {
                            let k = p.user_key.clone();
                            let k2 = p.user_key.clone();
                            let overlaps = p.overlaps.iter().take(3).map(|o| format!("{} ({})", o.name, o.both)).collect::<Vec<_>>().join(", ");
                            let total = p.count.max(1) as f32;
                            view! {
                                <button class="person" class:selected=move || person.get().as_deref() == Some(k.as_str())
                                    on:click=move |_| person.update(|x| *x = if x.as_deref() == Some(k2.as_str()) { None } else { Some(k2.clone()) })>
                                    <div class="row"><strong>{p.name.clone()}</strong><span>{p.count}</span></div>
                                    <div class="muted small">{format!("{} shared · {} only them", p.shared, p.alone)}
                                        {(!overlaps.is_empty()).then(|| format!(" · most with {overlaps}"))}</div>
                                    <div class="split"><div class="shared" style=format!("width:{}%", p.shared as f32 / total * 100.0) /></div>
                                </button>
                            }
                        }).collect_view()}
                    </section>
                    <div class="chips">
                        {[("all", "All"), ("shared", "Kept by several"), ("alone", "Kept by one person")].map(|(k, l)| view! {
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
