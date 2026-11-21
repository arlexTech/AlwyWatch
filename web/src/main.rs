// SPDX-License-Identifier: AGPL-3.0-or-later
//! AlwyWatch in the browser: the same Jellyfin/Plex/Seerr/keep clients as the apps (compiled
//! to wasm32), a Leptos UI, and the browser's own video player. Served by alwywatch-server.

mod browse;
mod keep;
mod player;
mod signin;
mod state;
mod ui;

use leptos::prelude::*;
use leptos_router::components::{A, Route, Router, Routes};
use leptos_router::path;

use state::Ctx;

fn main() {
    leptos::mount::mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    provide_context(Ctx::new());
    view! {
        <Router>
            <Routes fallback=|| view! { <main class="page"><h1>"Not found"</h1></main> }>
                <Route path=path!("/play/:acc/:id") view=player::Player />
                <Route path=path!("/") view=|| view! { <Shell><browse::Home /></Shell> } />
                <Route path=path!("/signin") view=|| view! { <Shell><signin::SignIn /></Shell> } />
                <Route path=path!("/library/:acc/:id") view=|| view! { <Shell><browse::Library /></Shell> } />
                <Route path=path!("/item/:acc/:id") view=|| view! { <Shell><browse::Detail /></Shell> } />
                <Route path=path!("/search") view=|| view! { <Shell><browse::Search /></Shell> } />
                <Route path=path!("/leaving") view=|| view! { <Shell><keep::Leaving /></Shell> } />
                <Route path=path!("/admin/forever") view=|| view! { <Shell><keep::Forever /></Shell> } />
                <Route path=path!("/settings") view=|| view! { <Shell><signin::Settings /></Shell> } />
            </Routes>
        </Router>
    }
}

/// Side menu + page. Without any account, every page leads to signing in.
#[component]
fn Shell(children: ChildrenFn) -> impl IntoView {
    let c = state::ctx();
    let signed_in = move || !c.accounts.get().is_empty();
    let keep_on = move || c.keep.get().is_some();
    let admin = move || c.keep.get().is_some_and(|k| k.is_admin);
    view! {
        <div class="shell">
            <nav class="menu">
                <A href="/" attr:class="brand">"AlwyWatch"</A>
                <Show when=signed_in>
                    <A href="/">"Home"</A>
                    <A href="/search">"Search"</A>
                    <Show when=keep_on><A href="/leaving">"Leaving soon"</A></Show>
                    <Show when=admin><A href="/admin/forever">"Kept forever"</A></Show>
                </Show>
                <A href="/settings">"Settings"</A>
            </nav>
            <main class="page">
                {move || if signed_in() || matches!(current_path().as_str(), "/signin" | "/settings") {
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
