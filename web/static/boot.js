// SPDX-License-Identifier: AGPL-3.0-or-later
// Starts the app (a file of its own: the page allows no inline script).
import init from '/alwywatch_web.js';
init({ module_or_path: '/alwywatch_web_bg.wasm' }).catch((e) => {
  console.error(e);
  const b = document.getElementById('boot');
  // The words, in the page's language, come from compat.js (which runs first).
  const t = window.awText || {
    failed: 'AlwyWatch couldn\'t start. Check your connection, then <a class="button primary" href="">reload</a>.',
    tooOld: 'If it keeps happening, this browser may be too old.',
  };
  if (b) b.innerHTML = '<p>' + t.failed + '</p><p class="muted small">' + t.tooOld + '</p>';
});
