// SPDX-License-Identifier: AGPL-3.0-or-later
// Starts the app (a file of its own: the page allows no inline script).
import init from '/alwywatch_web.js';
init({ module_or_path: '/alwywatch_web_bg.wasm' }).catch((e) => {
  console.error(e);
  const b = document.getElementById('boot');
  if (b) b.innerHTML = '<p>AlwyWatch couldn\'t start. Check your connection, then <a class="button primary" href="">reload</a>.</p>' +
    '<p class="muted small">If it keeps happening, this browser may be too old.</p>';
});
