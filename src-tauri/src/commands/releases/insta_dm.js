// Runs in the Instagram window of the app, in the user's own session. Sends
// ONE validated message to ONE account, then hands the outcome back through a
// navigation the app refuses. Reloads never send twice (sessionStorage).
(function () {
  if (window.top !== window || window.__piersDm) return;
  if (!/(^|\.)instagram\.com$/.test(location.hostname)) return;
  window.__piersDm = true;
  var TASK = __TASK__; // { id, handle, text }
  var OUT = 'https://pierscrm.invalid/bp';
  var KEY = 'piersDm:' + TASK.id;
  function out(kind, data) {
    location.href = OUT + '?k=ig_' + kind + '#' + encodeURIComponent(JSON.stringify(data || {}));
  }
  function sleep(ms) { return new Promise(function (r) { setTimeout(r, ms); }); }
  function rnd(a, b) { return a + Math.floor(Math.random() * (b - a)); }
  function state() { try { return sessionStorage.getItem(KEY) || ''; } catch (e) { return ''; } }
  function setState(v) { try { sessionStorage.setItem(KEY, v); } catch (e) {} }
  function visible(el) { return !!el && el.getClientRects().length > 0; }
  function byText(words) {
    var els = document.querySelectorAll('div[role="button"], button, a[role="link"], a[role="button"]');
    for (var i = 0; i < els.length; i++) {
      var t = (els[i].textContent || '').trim().toLowerCase();
      if (visible(els[i]) && words.indexOf(t) >= 0) return els[i];
    }
    return null;
  }
  function loginWall() {
    // (A signed-out visitor still sees public profiles, with links to the sign-in page.)
    return location.pathname.indexOf('/accounts/') === 0 || !!document.querySelector('input[name="username"]')
      || !!document.querySelector('a[href*="/accounts/login"], a[href*="/accounts/emailsignup"]');
  }
  function box() {
    var b = document.querySelector('div[contenteditable="true"][role="textbox"]');
    return visible(b) ? b : null;
  }
  function onProfile() {
    return location.pathname.replace(/\//g, '').toLowerCase() === TASK.handle.toLowerCase();
  }
  function pageSays(parts) {
    var t = (document.body && document.body.innerText || '').toLowerCase();
    for (var i = 0; i < parts.length; i++) if (t.indexOf(parts[i]) >= 0) return true;
    return false;
  }
  function squeeze(s) { return (s || '').replace(/\s+/g, ' ').trim(); }

  async function run() {
    await sleep(rnd(2500, 4000));
    // Not signed in: say so once, then wait for the user to sign in here.
    var told = false;
    for (var w = 0; loginWall(); w++) {
      if (!told) { told = true; out('login', {}); }
      if (w > 600) return;
      await sleep(1000);
    }
    if (told) await sleep(3000);
    if (state() === 'typed' || state() === 'done') {
      // The page was reloaded after the message was typed: never type it twice.
      out(pageSays([squeeze(TASK.text).slice(0, 40).toLowerCase()]) ? 'sent' : 'failed', { reason: 'Page rechargée pendant l\'envoi, à vérifier dans Instagram.' });
      return;
    }
    if (!onProfile() && location.pathname.indexOf('/direct/t/') !== 0) {
      if (state() === 'nav') { out('failed', { reason: 'Profil inaccessible.' }); return; }
      setState('nav');
      location.href = 'https://www.instagram.com/' + encodeURIComponent(TASK.handle) + '/';
      return;
    }
    if (onProfile()) {
      var btn = null;
      for (var i = 0; i < 30 && !btn; i++) {
        if (pageSays(["this page isn't available", "cette page n'est pas disponible", "page isn’t available", "n’est pas disponible"])) {
          out('failed', { reason: 'Compte introuvable.' });
          return;
        }
        btn = byText(['message', 'contacter', 'écrire', 'envoyer un message', 'send message']);
        if (!btn) await sleep(500);
      }
      if (!btn) { out('failed', { reason: 'Pas de bouton Message sur ce profil (compte privé ou messages fermés).' }); return; }
      await sleep(rnd(900, 2200));
      btn.click();
    }
    var b = null;
    for (var j = 0; j < 50 && !b; j++) {
      var later = byText(['not now', 'plus tard', 'pas maintenant']);
      if (later) later.click();
      b = box();
      if (!b) await sleep(500);
    }
    if (!b) { out('failed', { reason: 'La conversation ne s\'est pas ouverte.' }); return; }
    await sleep(rnd(1200, 2600));
    b.focus();
    setState('typed');
    document.execCommand('insertText', false, TASK.text);
    await sleep(rnd(900, 1800));
    var head = squeeze(TASK.text).slice(0, 24);
    if (squeeze(b.textContent).indexOf(head) < 0) { out('failed', { reason: 'Instagram a refusé la saisie du message.' }); return; }
    ['keydown', 'keypress', 'keyup'].forEach(function (type) {
      b.dispatchEvent(new KeyboardEvent(type, { key: 'Enter', code: 'Enter', keyCode: 13, which: 13, bubbles: true, cancelable: true }));
    });
    await sleep(1500);
    if (squeeze(b.textContent).indexOf(head) >= 0) {
      var send = byText(['send', 'envoyer']);
      if (send) send.click();
    }
    for (var k = 0; k < 20; k++) {
      await sleep(500);
      var cur = box();
      if ((!cur || squeeze(cur.textContent).indexOf(head) < 0) && pageSays([head.toLowerCase()])) {
        setState('done');
        await sleep(1200);
        out('sent', {});
        return;
      }
    }
    out('failed', { reason: 'Envoi non confirmé, à vérifier dans Instagram.' });
  }
  run().catch(function (e) { out('failed', { reason: 'Erreur de la page : ' + (e && e.message || e) }); });
})();
