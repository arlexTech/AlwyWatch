// SPDX-License-Identifier: AGPL-3.0-or-later
//! AlwyWatch in the browser: the same Jellyfin/Plex/Seerr/keep clients as the apps (compiled
//! to wasm32), a Leptos UI, and the browser's own video player. Served by alwywatch-server.

mod account;
mod browse;
mod extra;
mod keep;
mod links;
mod passkey;
mod player;
mod prefs;
mod profile;
mod profiles;
mod request;
mod signin;
mod state;
mod together;
mod ui;

use leptos::prelude::*;
use leptos_router::components::{A, Route, Router, Routes};
use leptos_router::path;

fn main() {
    show_crashes();
    // The page's own "Loading…" (shown while the app downloads) makes way for the app.
    if let Some(boot) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("boot"))
    {
        boot.remove();
    }
    leptos::mount::mount_to_body(App);
}

/// A crash leaves a dead page behind: say so, with a way out, instead.
fn show_crashes() {
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&info.to_string().into());
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        if doc.get_element_by_id("crash").is_some() {
            return;
        }
        let (Ok(box_), Ok(text), Ok(link), Some(body)) = (
            doc.create_element("div"),
            doc.create_element("p"),
            doc.create_element("a"),
            doc.body(),
        ) else {
            return;
        };
        box_.set_id("crash");
        let _ = box_.set_attribute("role", "alert");
        text.set_text_content(Some("Something went wrong."));
        link.set_text_content(Some("Reload"));
        let here = doc
            .location()
            .and_then(|l| l.href().ok())
            .unwrap_or_else(|| "/".into());
        let _ = link.set_attribute("href", &here);
        let _ = link.set_attribute("class", "button primary");
        let _ = box_.append_child(&text);
        let _ = box_.append_child(&link);
        let _ = body.append_child(&box_);
    }));
}

#[component]
fn App() -> impl IntoView {
    let c = state::install();
    provide_context(c.clone());
    {
        let c = c.clone();
        leptos::task::spawn_local(async move {
            account::check(c.clone()).await;
            signin::check_keep(c).await;
            // "Are these both you?" once for two accounts of one person.
            links::refresh(true).await;
        });
    }
    // One invitation long poll while an account is linked to the AlwyWatch server (restarted
    // only when the linked account changes, stopped on sign-out).
    let keep = c.keep;
    let linked = Memo::new(move |_| keep.with(|k| k.as_ref().map(|k| k.account.clone())));
    let invites = c.invites;
    Effect::new(move |_| match linked.get() {
        Some(_) => invites.run(),
        None => invites.stop(),
    });
    // The room seat this tab had before a reload.
    c.room.restore();
    // Room notes go away after a few seconds.
    let room = c.room;
    Effect::new(move |_| {
        let Some(n) = room.note.get() else { return };
        leptos::task::spawn_local(async move {
            state::sleep(5000).await;
            if room.note.try_get_untracked().flatten().as_deref() == Some(n.as_str()) {
                room.note.try_set(None);
            }
        });
    });
    view! {
        <Router>
            <Routes fallback=|| view! { <main class="page"><h1>"Not found"</h1></main> }>
                <Route path=path!("/play/:acc/:id") view=player::Player />
                <Route path=path!("/") view=|| view! { <Shell><browse::Home /></Shell> } />
                <Route path=path!("/signin") view=|| view! { <Shell><signin::SignIn /></Shell> } />
                <Route path=path!("/library/:acc/:id") view=|| view! { <Shell><browse::Library /></Shell> } />
                <Route path=path!("/item/:acc/:id") view=|| view! { <Shell><browse::Detail /></Shell> } />
                <Route path=path!("/search") view=|| view! { <Shell><browse::Search /></Shell> } />
                <Route path=path!("/list") view=|| view! { <Shell><extra::MyList /></Shell> } />
                <Route path=path!("/continue") view=|| view! { <Shell><browse::ContinueWatching /></Shell> } />
                <Route path=path!("/requests") view=|| view! { <Shell><request::Requests /></Shell> } />
                <Route path=path!("/request/:kind/:tmdb") view=|| view! { <Shell><request::SeerrTitle /></Shell> } />
                <Route path=path!("/saga/:id") view=|| view! { <Shell><request::Saga /></Shell> } />
                <Route path=path!("/together") view=|| view! { <Shell><together::Together /></Shell> } />
                <Route path=path!("/together/:code") view=|| view! { <Shell><together::RoomPage /></Shell> } />
                <Route path=path!("/leaving") view=|| view! { <Shell><keep::Leaving /></Shell> } />
                <Route path=path!("/leaving/sort") view=|| view! { <Shell><keep::Triage /></Shell> } />
                <Route path=path!("/admin") view=|| view! { <Shell><keep::Admin /></Shell> } />
                <Route path=path!("/admin/forever") view=|| view! { <Shell><keep::Forever /></Shell> } />
                <Route path=path!("/settings") view=|| view! { <Shell><signin::Settings /></Shell> } />
                <Route path=path!("/settings/:section") view=|| view! { <Shell><signin::Settings /></Shell> } />
                <Route path=path!("/account") view=|| view! { <Shell><account::AccountPage /></Shell> } />
                <Route path=path!("/account/signed-in") view=|| view! { <Shell><account::SignedInPage /></Shell> } />
                <Route path=path!("/join") view=|| view! { <Shell><account::Join /></Shell> } />
                <Route path=path!("/join/:code") view=|| view! { <Shell><account::Join /></Shell> } />
                <Route path=path!("/q/:code") view=|| view! { <Shell><account::Approve /></Shell> } />
            </Routes>
        </Router>
    }
}

/// Side menu + page. Without any account, every page leads to signing in.
/// Pages that work without a Jellyfin/Plex account.
fn open_page(path: &str) -> bool {
    matches!(
        path,
        "/signin" | "/settings" | "/account" | "/account/signed-in" | "/join"
    ) || path.starts_with("/join/")
        || path.starts_with("/q/")
}

#[component]
fn Shell(children: ChildrenFn) -> impl IntoView {
    let c = state::ctx();
    let (accounts, keep, welcome, link_dialog, me) =
        (c.accounts, c.keep, c.welcome, c.links.dialog, c.me);
    let signed_in = move || !accounts.get().is_empty();
    let keep_on = move || keep.get().is_some();
    // What works behind the AlwyWatch server: Maintainerr (Leaving soon), Seerr (Requests).
    let services = move || me.get().map(|m| m.services).unwrap_or_default();
    let requests_on = move || keep_on() && services().requests;
    let leaving_on = move || keep_on() && services().leaving_soon;
    let admin = move || keep.get().is_some_and(|k| k.is_admin);
    view! {
        <div class="shell">
            <nav class="menu">
                <A href="/" attr:class="brand">"AlwyWatch"</A>
                <profiles::ProfileMenu />
                <Show when=signed_in>
                    <A href="/">"Home"</A>
                    <A href="/search">"Search"</A>
                    <A href="/list">"My list"</A>
                    <Show when=requests_on><A href="/requests">"Requests"</A></Show>
                    <Show when=keep_on><A href="/together">"Together"</A></Show>
                    <Show when=leaving_on><A href="/leaving">"Leaving soon"</A></Show>
                    <Show when=admin><A href="/admin/forever">"Kept forever"</A></Show>
                    <Show when=admin><A href="/admin">"Admin"</A></Show>
                </Show>
                <A href="/settings">"Settings"</A>
            </nav>
            <main class="page">
                <Expired />
                <Show when=keep_on><together::InviteBanner /><together::SeatBanner /></Show>
                {move || (keep_on() && welcome.get()).then(|| view! { <profile::Welcome /> })}
                // After the welcome step; the player is its own page, so never over a video.
                {move || (keep_on() && !welcome.get()).then(|| link_dialog.get()).flatten().map(|(account, prompted)| {
                    view! { <links::LinkDialog account=account prompted=prompted /> }
                })}
                {move || if signed_in() || open_page(&current_path()) {
                    children().into_any()
                } else {
                    view! { <signin::SignIn /> }.into_any()
                }}
            </main>
        </div>
    }
}

fn current_path() -> String {
    leptos_router::hooks::use_location().pathname.get()
}

/// A server refused an account (its session ended): offer to sign in again.
#[component]
fn Expired() -> impl IntoView {
    let c = state::ctx();
    let navigate = leptos_router::hooks::use_navigate();
    move || {
        let key = c.expired.get()?;
        let s = c.session(&key)?;
        let (c1, c2, k1, nav) = (c.clone(), c.clone(), key.clone(), navigate.clone());
        Some(view! {
            <div class="banner" role="alert">
                <span>{format!("{} on {} was signed out by the server.", s.user_name, s.server_name)}</span>
                <button class="small primary" on:click=move |_| {
                    c1.remove_account(&k1);
                    nav("/signin", Default::default());
                }>"Sign in again"</button>
                <button class="small subtle" on:click=move |_| c2.expired.set(None)>"Dismiss"</button>
            </div>
        })
    }
}
