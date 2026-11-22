// SPDX-License-Identifier: AGPL-3.0-or-later
//! Leaving soon (what the server will delete, and keeping it) and, for admins, who keeps what
//! forever with the overlaps.

use keep_service::{
    AdminUser, CountdownRequest, ForeverStats, LeavingItem, Protection, Rights, SeerrUser,
    VoteKind, VoteRequest,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;

use crate::profile::{Avatar, name_with_media};
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

const NO_KEEP: &str =
    "This needs an account on this AlwyWatch server: sign in with one in Settings.";

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
    let items = RwSignal::new(state::load::<Vec<LeavingItem>>(state::LEAVING));
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
                return error.set(Some(NO_KEEP.into()));
            };
            match k.leaving_soon().await {
                Ok(l) => {
                    state::save(state::LEAVING, &l);
                    items.try_set(Some(l));
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
        <h1>"Leaving soon"</h1>
        <p class="muted">"What the server will delete. Keep what you want to watch or keep."</p>
        <div class="actions">
            <A href="/leaving/sort" attr:class="button primary">"Sort them out"</A>
            <Show when=move || state::ctx().keep.get().is_some_and(|k| k.is_admin)>
                <A href="/admin" attr:class="button">"Admin"</A>
            </Show>
        </div>
        <div class="chips">
            {[("all", "All"), ("undecided", "Not decided"), ("mine", "Kept by me")].map(|(k, l)| view! {
                <button class="chip" class:selected=move || filter.get() == k on:click=move |_| filter.set(k)>{l}</button>
            }).collect_view()}
        </div>
        {move || error.get().map(|e| view! { <ErrorText text=e /> })}
        {move || (items.with(Option::is_none) && error.with(Option::is_none)).then(|| view! { <Loading /> })}
        {move || (items.with(|i| i.as_ref().is_some_and(Vec::is_empty))).then(|| view! {
            <p class="muted">"Nothing is leaving. Nothing to do."</p>
        })}
        <div class="leaving">
            {move || shown().into_iter().take(limit.get()).map(|i| view! { <LeavingRow item=i items=items /> }).collect_view()}
        </div>
        {move || {
            let total = shown().len();
            (total > limit.get()).then(|| view! {
                <button class="more" on:click=move |_| limit.update(|l| *l += 120)>
                    {format!("More ({} of {total} shown)", limit.get())}
                </button>
            })
        }}
    }
}

#[component]
fn LeavingRow(item: LeavingItem, items: RwSignal<Option<Vec<LeavingItem>>>) -> impl IntoView {
    let key = item.key.clone();
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let vote = move |req: Option<VoteRequest>| {
        let key = key.clone();
        let Some(k) = state::ctx().keep_client() else {
            return error.set(Some(NO_KEEP.into()));
        };
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let res = match req {
                Some(r) => k.vote(&key, &r).await,
                None => k.unvote(&key).await,
            };
            match res {
                Ok(updated) => items.update(|l| {
                    if let Some(l) = l {
                        if let Some(x) = l.iter_mut().find(|x| x.key == updated.key) {
                            *x = updated;
                        }
                        state::save(state::LEAVING, l);
                    }
                }),
                Err(e) => error.set(Some(state::plain_keep(&e))),
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
                {move || error.get().map(|e| view! { <ErrorText text=e /> })}
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
            return stats.set(Some(Err(NO_KEEP.into())));
        };
        stats.set(Some(k.admin_forever().await.map_err(|e| admin_error(&e))));
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
                    <p class="muted">{format!("{} titles · {} kept by several people · {} by one person", s.titles, s.shared, s.titles.saturating_sub(s.shared))}</p>
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
                                    <div class="row">
                                        <span class="person-head">
                                            <Avatar user_key=p.user_key.clone() name=p.name.clone() avatar=p.avatar.clone() size=32 />
                                            <strong>{name_with_media(&p.name, &p.media_name)}</strong>
                                        </span>
                                        <span>{p.count}</span>
                                    </div>
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
        return ("Kept".into(), false);
    }
    if paused(i, now) {
        return ("Not scheduled".into(), false);
    }
    let Some(d) = i.effective_deletion_date else {
        return ("Soon".into(), false);
    };
    let days = (d - now).div_euclid(DAY);
    match days {
        ..0 => ("Overdue".into(), true),
        0 => ("Today".into(), true),
        1 => ("1 day".into(), true),
        n => (format!("{n} days"), n <= 7),
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

/// "Sort them out": one undecided title at a time, decided with a button or an arrow key.
#[component]
pub fn Triage() -> impl IntoView {
    let queue = RwSignal::new(None::<Vec<LeavingItem>>);
    let error = RwSignal::new(None::<String>);
    let done = RwSignal::new(0u32);
    spawn_local(async move {
        let Some(k) = state::ctx().keep_client() else {
            return error.set(Some(NO_KEEP.into()));
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
            return error.set(Some(NO_KEEP.into()));
        };
        // At once to the next one; put back if the server says no.
        queue.update(|q| {
            if let Some(q) = q {
                q.remove(0);
            }
        });
        done.update(|d| *d += 1);
        error.set(None);
        spawn_local(async move {
            match k.vote(&item.key, &r).await {
                // The list kept for Leaving soon follows.
                Ok(updated) => {
                    if let Some(mut l) = state::load::<Vec<LeavingItem>>(state::LEAVING) {
                        if let Some(x) = l.iter_mut().find(|x| x.key == updated.key) {
                            *x = updated;
                        }
                        state::save(state::LEAVING, &l);
                    }
                }
                Err(e) => {
                    error.set(Some(format!(
                        "{} wasn't saved: {}",
                        item.title,
                        state::plain_keep(&e)
                    )));
                    queue.update(|q| {
                        if let Some(q) = q
                            && !q.iter().any(|x| x.key == item.key)
                        {
                            q.insert(0, item);
                        }
                    });
                    done.update(|d| *d = d.saturating_sub(1));
                }
            }
        });
    };
    let until = move || vote(vote_req(VoteKind::KeepUntilWatched, None, Some(7), None));
    let forever = move || vote(vote_req(VoteKind::KeepForever, None, None, Some("love it")));
    let days = move || vote(vote_req(VoteKind::KeepDays, Some(30), None, None));
    let dont = move || vote(vote_req(VoteKind::DontCare, None, None, None));
    let on_key = move |ev: web_sys::KeyboardEvent| {
        match ev.key().as_str() {
            "ArrowLeft" => dont(),
            "ArrowRight" => until(),
            "ArrowUp" => forever(),
            _ => return,
        }
        ev.prevent_default();
    };
    let card = NodeRef::<leptos::html::Div>::new();
    // Keyboard first: the card has the focus, so the arrows decide at once.
    Effect::new(move |_| {
        queue.track();
        if let Some(c) = card.get() {
            let _ = c.focus();
        }
    });
    view! {
        <div class="triage">
            <h1>"Sort them out"</h1>
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
                        <p class="muted">{if n > 0 { format!("All done, thanks! {n} sorted.") } else { "Nothing to sort right now.".to_string() }}</p>
                        <A href="/leaving" attr:class="button">"Back to Leaving soon"</A>
                    }.into_any()
                }
                Some(q) => {
                    let item = q[0].clone();
                    let (label, urgent) = short_label(&item, now_s());
                    let c = &item.counts;
                    let keepers = c.forever + c.until_watched + c.days;
                    view! {
                        <p class="muted small">{format!("{} left · ← don't care   → until I watch it   ↑ forever", q.len())}</p>
                        <div class="triage-body">
                            <div class="triage-card" tabindex="0" node_ref=card on:keydown=on_key>
                                {item.poster_url.clone().map(|p| view! { <img src=p alt="" /> })}
                            </div>
                            <div class="triage-info">
                                <h2>{item.title.clone()}</h2>
                                <p>
                                    {item.year.map(|y| format!("{y} "))}
                                    <span class="state" class:urgent=urgent>{label}</span>
                                </p>
                                {(keepers > 0).then(|| view! {
                                    <p class="muted small">{format!("{keepers} {} it", if keepers > 1 { "people keep" } else { "person keeps" })}</p>
                                })}
                                <div class="actions">
                                    <button class="primary" on:click=move |_| until()>"Until I watch it"</button>
                                    <button on:click=move |_| forever()>"♥ Forever"</button>
                                    <button on:click=move |_| days()>"Keep 30 days"</button>
                                    <button on:click=move |_| dont()>"✕ Don't care"</button>
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
fn admin_error(e: &net::Error) -> String {
    match e {
        net::Error::Unauthorized => "Only admins of this AlwyWatch server can see this.".into(),
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
const COUNTDOWN_WARNING: &str = "Every item is already past its date. Before activating a collection in Maintainerr, restart its countdown so people get the full delay to vote. This changes deletion dates in Maintainerr.";

/// A countdown waiting for its confirmation: collection, its title, stagger days.
type Armed = (i64, String, u32);

/// Admins: people's rights, and the collection countdown (behind a confirmation).
#[component]
pub fn Admin() -> impl IntoView {
    let users = RwSignal::new(None::<Result<Vec<AdminUser>, String>>);
    let seerr_users = RwSignal::new(Vec::<SeerrUser>::new());
    let message = RwSignal::new(None::<String>);
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
            return users.set(Some(Err(NO_KEEP.into())));
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
                        .unwrap_or_else(|| format!("Collection {id}")),
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
            return message.set(Some(NO_KEEP.into()));
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
            message.set(Some(match r {
                Ok(r) => format!("{title}: {} restarted, {} skipped", r.reset, r.skipped),
                Err(e) => format!("{title}: {}", admin_error(&e)),
            }));
            busy.set(false);
            armed.set(None);
        });
    };
    view! {
        <h1>"Admin"</h1>
        <p class="muted">"Leaving soon: countdown and people's rights"</p>
        <section class="group">
            <h2>"Kept forever"</h2>
            <A href="/admin/forever" attr:class="row link-row">
                <div><strong>"Who keeps what forever"</strong>
                    <div class="muted small">"Per person, and how much of it someone else keeps too"</div></div>
                <span>"›"</span>
            </A>
        </section>
        {move || message.get().map(|m| view! { <p class="accent">{m}</p> })}
        <section class="group">
            <h2>"Start countdown"</h2>
            {move || {
                let list = collections();
                if list.is_empty() {
                    return view! { <p class="muted">"No collection has items leaving."</p> }.into_any();
                }
                list.into_iter().map(|(id, title)| view! {
                    <div class="row wrap">
                        <span>{title.clone()}</span>
                        <div class="actions tight">
                            {[0u32, 14, 30].map(|stagger| {
                                let title = title.clone();
                                let label = if stagger == 0 { "Today".to_string() } else { format!("Over {stagger} days") };
                                view! { <button class="small" on:click=move |_| armed.set(Some((id, title.clone(), stagger)))>{label}</button> }
                            }).collect_view()}
                        </div>
                    </div>
                }).collect_view().into_any()
            }}
            <p class="muted small">{COUNTDOWN_WARNING}" You'll be asked to confirm."</p>
        </section>
        <section class="group">
            <h2>"People"</h2>
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
                                <strong>{name_with_media(&u.name, &u.media_name)}{u.is_admin.then_some(" · admin")}</strong>
                            </div>
                            <div class="muted small">{format!("{server} · {} kept forever", u.forever_count)}</div>
                            <div class="actions tight">
                                <button class="small" class:selected=!r.can_keep_forever on:click=move |_| {
                                    set_rights(k1.clone(), Rights { can_keep_forever: !r1.can_keep_forever, ..r1.clone() })
                                }>{if r.can_keep_forever { "Forever allowed" } else { "Forever blocked" }}</button>
                                <button class="small" on:click=move |_| {
                                    set_rights(k2.clone(), Rights { max_forever: cycle(&[None, Some(5), Some(10), Some(25)], r2.max_forever), ..r2.clone() })
                                }>{format!("Max forever: {}", limit(r.max_forever))}</button>
                                <button class="small" on:click=move |_| {
                                    set_rights(k3.clone(), Rights { max_keep_days: cycle(&[None, Some(7), Some(30), Some(90)], r3.max_keep_days), ..r3.clone() })
                                }>{format!("Max days: {}", limit(r.max_keep_days))}</button>
                                {move || (!seerr_users.with(Vec::is_empty)).then(|| {
                                    let k4 = k4.clone();
                                    view! {
                                        <select class="chip small" aria-label="Requests as"
                                            prop:value=linked.map(|i| i.to_string()).unwrap_or_default()
                                            on:change=move |ev| link(k4.clone(), event_target_value(&ev).parse().ok())>
                                            <option value="">"Requests as: automatic"</option>
                                            {seerr_users.get().into_iter().map(|s| view! {
                                                <option value=s.id.to_string() selected=linked == Some(s.id)>{format!("Requests as: {}", s.name)}</option>
                                            }).collect_view()}
                                        </select>
                                    }
                                })}
                            </div>
                        </div>
                    }
                }).collect_view().into_any(),
            }}
            <p class="muted small">"Everyone can keep forever by default. Click a value to change it."</p>
        </section>
        {move || armed.get().map(|(_, title, stagger)| view! {
            <div class="dialog-back" on:click=move |_| if !busy.get_untracked() { armed.set(None) }
                on:keydown=move |ev| if ev.key() == "Escape" && !busy.get_untracked() { armed.set(None) }>
                <div class="dialog" role="alertdialog" aria-modal="true" aria-labelledby="countdown-title" on:click=|ev| ev.stop_propagation()>
                    <h2 id="countdown-title">{format!("Restart the countdown of {title}?")}</h2>
                    <p>{COUNTDOWN_WARNING}</p>
                    <p class="muted">{if stagger == 0 {
                        "Every item of this collection gets its full delay starting today.".to_string()
                    } else {
                        format!("New deletion dates are spread over {stagger} days.")
                    }}</p>
                    <div class="actions">
                        <button node_ref=cancel disabled=move || busy.get() on:click=move |_| armed.set(None)>"Cancel"</button>
                        <button class="danger" disabled=move || busy.get() on:click=confirm>
                            {move || if busy.get() { "Changing dates…" } else { "Change deletion dates" }}
                        </button>
                    </div>
                </div>
            </div>
        })}
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
            ("Overdue".into(), true)
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
    fn limits_cycle_and_wrap() {
        let opts = [None, Some(7), Some(30), Some(90)];
        assert_eq!(cycle(&opts, None), Some(7));
        assert_eq!(cycle(&opts, Some(90)), None);
        // An odd value set elsewhere starts the cycle over.
        assert_eq!(cycle(&opts, Some(12)), None);
    }
}
