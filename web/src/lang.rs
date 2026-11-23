// SPDX-License-Identifier: AGPL-3.0-or-later
//! The site's language (docs/i18n.md): the choice in Settings, kept in this browser, or the
//! browser's own languages. Text goes through `i18n::tr!`, from the catalogs every app shares.

use leptos::prelude::*;

const KEY: &str = "aw.language";

/// The choice in Settings: "" follows the browser.
fn choice() -> String {
    crate::state::load::<String>(KEY).unwrap_or_default()
}

/// Selects the language before the app is drawn.
pub fn init() {
    let chosen = choice();
    let code = if chosen.is_empty() {
        let langs: Vec<String> = web_sys::window()
            .map(|w| {
                w.navigator()
                    .languages()
                    .iter()
                    .filter_map(|v| v.as_string())
                    .collect()
            })
            .unwrap_or_default();
        i18n::best(langs.iter().map(String::as_str)).to_owned()
    } else {
        chosen
    };
    i18n::set_language(&code);
    if let Some(html) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = html.set_attribute("lang", &code);
    }
}

#[component]
pub fn LanguageSetting() -> impl IntoView {
    let current = choice();
    let change = move |ev: leptos::ev::Event| {
        let v = event_target_value(&ev);
        if v.is_empty() {
            crate::state::remove(KEY);
        } else {
            crate::state::save(KEY, &v);
        }
        // Every page again, in the new language.
        if let Some(w) = web_sys::window() {
            let _ = w.location().reload();
        }
    };
    view! {
        <section class="group">
            <h2>{i18n::tr!("Language")}</h2>
            <div class="row">
                <span>{i18n::tr!("Language")}</span>
                <select class="chip" aria-label=i18n::tr!("Language") prop:value=current on:change=change>
                    <option value="">{i18n::tr!("Same as the browser")}</option>
                    {i18n::languages().into_iter().map(|(code, name)| view! { <option value=code>{name}</option> }).collect_view()}
                </select>
            </div>
            <p class="muted small">{i18n::tr!("Kept in this browser.")}</p>
        </section>
    }
}
