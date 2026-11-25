// SPDX-License-Identifier: AGPL-3.0-or-later
//! AlwyWatch in the browser: the same Jellyfin/Plex/Seerr/keep clients as the apps (compiled
//! to wasm32), a Leptos UI, and the browser's own video player. Served by alwywatch-server.

mod account;
mod admin;
mod browse;
mod dashboard;
mod extra;
mod icons;
mod keep;
mod lang;
mod links;
mod passkey;
mod player;
mod prefs;
mod profile;
mod profiles;
mod request;
mod setup;
mod signin;
mod state;
mod sync;
mod together;
mod tracks;
mod ui;

use i18n::tr;
use leptos::prelude::*;

use leptos_router::components::{A, Route, Router, Routes};
use leptos_router::path;

fn main() {
    show_crashes();
    lang::init();
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
        text.set_text_content(Some(&tr!("Something went wrong.")));
        link.set_text_content(Some(&tr!("Reload")));
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
            // Settings and servers from the other devices (quietly: Settings → Servers says
            // what happened when asked).
            let _ = sync::sync().await;
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
    // A server without a media server yet: its setup page, whatever the address.
    let setup = setup::needed();
    view! {
        <Show when=move || !setup.get() fallback=|| view! { <setup::Setup /> }>
        <Router>
            <Routes fallback=|| view! { <Shell><h1>{tr!("Not found")}</h1><A href="/" attr:class="button primary">{tr!("Home")}</A></Shell> }>
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
                // Admin is a section of Settings now.
                <Route path=path!("/admin") view=|| view! { <MovedTo to="/settings/admin" /> } />
                <Route path=path!("/dashboard") view=|| view! { <Shell><dashboard::Dashboard /></Shell> } />
                <Route path=path!("/admin/forever") view=|| view! { <Shell><keep::Forever /></Shell> } />
                <Route path=path!("/settings") view=|| view! { <Shell><signin::Settings /></Shell> } />
                <Route path=path!("/settings/:section") view=|| view! { <Shell><signin::Settings /></Shell> } />
                <Route path=path!("/settings/:section/:sub") view=|| view! { <Shell><signin::Settings /></Shell> } />
                // Pocket ID comes back to /account?link=… and /account?error=… (alwywatch-server).
                <Route path=path!("/account") view=|| view! { <MovedTo to="/settings/account/password" /> } />
                <Route path=path!("/account/signed-in") view=|| view! { <Shell><account::SignedInPage /></Shell> } />
                <Route path=path!("/join") view=|| view! { <Shell><account::Join /></Shell> } />
                <Route path=path!("/join/:code") view=|| view! { <Shell><account::Join /></Shell> } />
                <Route path=path!("/q/:code") view=|| view! { <Shell><account::Approve /></Shell> } />
                <Route path=path!("/forgot") view=|| view! { <Shell><account::Forgot /></Shell> } />
                <Route path=path!("/reset/:token") view=|| view! { <Shell><account::Reset /></Shell> } />
            </Routes>
        </Router>
        </Show>
    }
}

/// An old address: the same page at its new one, with its query (`?link=`, `?error=`).
#[component]
fn MovedTo(to: &'static str) -> impl IntoView {
    let search = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .unwrap_or_default();
    let path = format!("{to}{search}");
    view! {
        <leptos_router::components::Redirect path=path options=leptos_router::NavigateOptions { replace: true, ..Default::default() } />
    }
}

/// Side menu + page. Without any account, every page leads to signing in.
/// Pages that work without a Jellyfin/Plex account.
fn open_page(path: &str) -> bool {
    matches!(
        path,
        "/signin" | "/settings" | "/account" | "/account/signed-in" | "/join" | "/forgot"
    ) || path.starts_with("/settings/")
        || path.starts_with("/reset/")
        || path.starts_with("/join/")
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
    use icons::Icon;
    state::mark_app_page();
    let navigate_home = leptos_router::hooks::use_navigate();
    // The first path is where the tab landed; any later one is a move inside the app.
    let first = StoredValue::new(None::<String>);
    Effect::new(move |_| {
        let p = current_path();
        match first.get_value() {
            None => first.set_value(Some(p)),
            Some(f) if f != p => MOVED.store(true, std::sync::atomic::Ordering::Relaxed),
            _ => {}
        }
    });
    // The libraries the home found last (one request per server there, none here).
    let libs = StoredValue::new(
        state::load::<Vec<(String, String, String, String)>>(state::LIBS).unwrap_or_default(),
    );
    view! {
        <div class="shell">
            // One bar on every width: the search field in the middle (an icon on phones, where
            // the tab bar at the bottom takes the side menu's place).
            <header class="topbar">
                // Pages under a tab (a title, an episode, the dashboard…): a way back.
                <Show when=move || !top_level(&current_path())>
                    {let navigate_home = navigate_home.clone(); move || { let n = navigate_home.clone(); view! {
                    <button class="iconbtn back-btn" aria-label=tr!("Back") title=tr!("Back") on:click=move |_| {
                        if MOVED.load(std::sync::atomic::Ordering::Relaxed) {
                            let _ = leptos::prelude::window().history().map(|h| h.back());
                        } else {
                            // Opened straight on this page (a link): the home.
                            n("/", Default::default());
                        }
                    }>
                        <Icon path=icons::ARROW_BACK />
                    </button>
                    }}}
                </Show>
                <A href="/" attr:class="logo" attr:aria-label=tr!("Home")>"alwy"<span>"watch"</span></A>
                <Show when=signed_in>
                    <TopSearch />
                    <A href="/search" attr:class="iconbtn search-icon" attr:aria-label=tr!("Search") attr:title=tr!("Search")><Icon path=icons::SEARCH /></A>
                    <A href="/list" attr:class="iconbtn" attr:aria-label=tr!("My list") attr:title=tr!("My list")><Icon path=icons::BOOKMARK /></A>
                </Show>
                <A href="/settings" attr:class="iconbtn" attr:aria-label=tr!("Settings") attr:title=tr!("Settings")><Icon path=icons::SETTINGS /></A>
                <profiles::ProfileMenu />
            </header>
            <div class="body">
                <Show when=signed_in>
                    <nav class="menu" aria-label=tr!("Menu")>
                        <A href="/" exact=true><Icon path=icons::HOME /><span>{tr!("Home")}</span></A>
                        <A href="/list"><Icon path=icons::BOOKMARK /><span>{tr!("My list")}</span></A>
                        <Show when=leaving_on><A href="/leaving"><Icon path=icons::HOURGLASS /><span>{tr!("Leaving soon")}</span></A></Show>
                        <Show when=requests_on><A href="/requests"><Icon path=icons::ADD /><span>{tr!("Requests")}</span></A></Show>
                        <Show when=keep_on><A href="/together"><Icon path=icons::GROUP /><span>{tr!("Together")}</span></A></Show>
                        {libs.with_value(|l| !l.is_empty()).then(|| view! { <p class="menu-head">{tr!("Libraries")}</p> })}
                        {libs.get_value().into_iter().map(|(acc, id, title, server)| view! {
                            <A href=format!("/library/{acc}/{id}") attr:title=server><Icon path=icons::LIBRARY /><span>{title}</span></A>
                        }).collect_view()}
                    </nav>
                </Show>
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
            <Show when=signed_in>
                <nav class="tabbar">
                    <A href="/" exact=true><Icon path=icons::HOME /><span>{tr!("Home")}</span></A>
                    <Show when=leaving_on><A href="/leaving"><Icon path=icons::HOURGLASS /><span>{tr!("Leaving")}</span></A></Show>
                    <Show when=keep_on><A href="/together"><Icon path=icons::GROUP /><span>{tr!("Together")}</span></A></Show>
                    <Show when=requests_on><A href="/requests"><Icon path=icons::ADD /><span>{tr!("Requests")}</span></A></Show>
                </nav>
            </Show>
        </div>
    }
}

/// The search field of the top bar. The address holds the text (`/search?q=`): typing here
/// opens the results page and updates it, Back leaves it, and the results page's own field
/// (phones) is the same text. "/" anywhere puts the cursor in it, Escape leaves it.
#[component]
fn TopSearch() -> impl IntoView {
    let loc = leptos_router::hooks::use_location();
    let query = leptos_router::hooks::use_query_map();
    let navigate = leptos_router::hooks::use_navigate();
    let input = NodeRef::<leptos::html::Input>::new();
    let on_results = move || loc.pathname.get() == "/search";
    // The bar is drawn again on each page: back in the field when it was what opened them.
    Effect::new(move |_| {
        if let Some(i) = input.get()
            && loc.pathname.get_untracked() == "/search"
        {
            let _ = i.focus();
            let n = i.value().len() as u32;
            let _ = i.set_selection_range(n, n);
        }
    });
    let keys = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() != "/" || ev.ctrl_key() || ev.meta_key() || ev.alt_key() {
            return;
        }
        // Not while typing somewhere.
        let typing = leptos::prelude::document()
            .active_element()
            .is_some_and(|e| {
                matches!(e.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT")
                    || e.has_attribute("contenteditable")
            });
        if !typing && let Some(i) = input.get_untracked() {
            ev.prevent_default();
            let _ = i.focus();
        }
    });
    on_cleanup(move || keys.remove());
    view! {
        <form class="topsearch" role="search" on:submit=|ev| ev.prevent_default()>
            <icons::Icon path=icons::SEARCH />
            <input type="search" node_ref=input placeholder=tr!("Movies, shows, episodes…") aria-label=tr!("Search")
                prop:value=move || if on_results() { query.read().get("q").unwrap_or_default() } else { String::new() }
                on:input=move |ev| {
                    let q = event_target_value(&ev);
                    navigate(&format!("/search?q={}", net::percent_encode(&q)), leptos_router::NavigateOptions {
                        replace: loc.pathname.get_untracked() == "/search",
                        scroll: false,
                        ..Default::default()
                    });
                }
                on:keydown=move |ev| if ev.key() == "Escape" && let Some(i) = input.get_untracked() { let _ = i.blur(); } />
        </form>
    }
}

/// The pages the menu and tab bar lead to: no Back button there.
fn top_level(path: &str) -> bool {
    let p = path.trim_end_matches('/');
    matches!(
        p,
        "" | "/list" | "/leaving" | "/requests" | "/together" | "/search" | "/settings" | "/signin"
    ) || p.starts_with("/library/")
}

/// Whether this tab moved between pages of the app since it loaded (then Back goes back).
static MOVED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
                <span>{tr!("{name} on {server} was signed out by the server.", name = s.user_name, server = s.server_name)}</span>
                <button class="small primary" on:click=move |_| {
                    c1.remove_account(&k1);
                    nav("/signin", Default::default());
                }>{tr!("Sign in again")}</button>
                <button class="small subtle" on:click=move |_| c2.expired.set(None)>{tr!("Dismiss")}</button>
            </div>
        })
    }
}
