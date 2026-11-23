// SPDX-License-Identifier: AGPL-3.0-or-later
// An old browser can't even load the module: say so instead of spinning forever.
if (!('noModule' in HTMLScriptElement.prototype) || typeof WebAssembly !== 'object') {
  document.getElementById('boot').innerHTML = '<p>This browser is too old for AlwyWatch. Use a recent Firefox, Chrome, Edge or Safari.</p>';
}
