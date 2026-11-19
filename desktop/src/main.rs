// SPDX-License-Identifier: AGPL-3.0-or-later
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;
    window.set_greeting(alwywatch_core::greet("desktop".into()).into());
    window.run()
}
