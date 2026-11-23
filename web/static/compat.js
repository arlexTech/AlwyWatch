// SPDX-License-Identifier: AGPL-3.0-or-later
// The few words shown before the app runs, in French or English (the app itself translates
// everything else, docs/i18n.md): the language chosen in Settings, else the browser's. Plain
// ES5, since this must work in browsers too old for the app.
var awText = (function () {
  var langs = [];
  try {
    var p = localStorage.getItem('aw.profile');
    var key = !p || p === 'default' ? 'aw.language' : 'aw.p.' + p + '.language';
    var chosen = JSON.parse(localStorage.getItem(key) || '""');
    if (chosen) langs = [chosen];
  } catch (e) {}
  if (!langs.length) langs = navigator.languages || [navigator.language || ''];
  var fr = false;
  for (var i = 0; i < langs.length; i++) {
    var base = String(langs[i]).toLowerCase().split(/[-_.@]/)[0];
    if (base === 'en') break;
    if (base === 'fr') { fr = true; break; }
  }
  if (fr) document.documentElement.lang = 'fr';
  return fr ? {
    loading: 'Chargement d\'AlwyWatch…',
    old: 'Ce navigateur est trop ancien pour AlwyWatch. Utilisez un Firefox, Chrome, Edge ou Safari récent.',
    failed: 'AlwyWatch n\'a pas pu démarrer. Vérifiez votre connexion, puis <a class="button primary" href="">rechargez</a>.',
    tooOld: 'Si cela se reproduit, ce navigateur est peut-être trop ancien.'
  } : {
    loading: 'Loading AlwyWatch…',
    old: 'This browser is too old for AlwyWatch. Use a recent Firefox, Chrome, Edge or Safari.',
    failed: 'AlwyWatch couldn\'t start. Check your connection, then <a class="button primary" href="">reload</a>.',
    tooOld: 'If it keeps happening, this browser may be too old.'
  };
})();
var awBoot = document.getElementById('boot');
// An old browser can't even load the module: say so instead of spinning forever.
if (!('noModule' in HTMLScriptElement.prototype) || typeof WebAssembly !== 'object') {
  awBoot.innerHTML = '<p>' + awText.old + '</p>';
} else {
  var awLoading = awBoot.getElementsByTagName('p')[0];
  if (awLoading) awLoading.textContent = awText.loading;
}
