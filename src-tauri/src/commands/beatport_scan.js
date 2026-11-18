(function () {
  if (window.top !== window || window.__piersScan) return;
  if (location.hostname !== 'www.beatport.com') return;
  window.__piersScan = true;
  var TASK = __TASK__;
  var OUT = 'https://pierscrm.invalid/bp';
  function send(kind, data) {
    location.href = OUT + '?k=' + kind + '#' + encodeURIComponent(JSON.stringify(data));
  }
  function sleep(ms) { return new Promise(function (r) { setTimeout(r, ms); }); }
  function queries(doc) {
    var el = doc.getElementById('__NEXT_DATA__');
    if (!el) return null;
    try { return JSON.parse(el.textContent).props.pageProps.dehydratedState.queries || []; } catch (e) { return null; }
  }
  function track(t, i) {
    var l = (t.release && t.release.label) || {};
    return {
      id: t.id, name: (t.name || '').trim(), mix: t.mix_name || '', slug: t.slug || '', pos: i + 1,
      artists: (t.artists || []).map(function (a) { return a.name; }),
      label: { id: l.id || 0, name: l.name || '', slug: l.slug || '' }
    };
  }
  function tracksOf(qs, want) {
    for (var i = 0; i < qs.length; i++) {
      var d = qs[i].state && qs[i].state.data;
      var key = JSON.stringify(qs[i].queryKey || '');
      if (d && Array.isArray(d.results) && d.results.length && d.results[0].artists && (!want || key.indexOf(want) >= 0)) {
        return d.results.map(track);
      }
    }
    return null;
  }
  async function ready() {
    for (var i = 0; i < 360; i++) {
      if (document.getElementById('__NEXT_DATA__')) return true;
      await sleep(500);
    }
    return false;
  }
  async function charts() {
    var read = 0, missing = [];
    for (var i = 0; i < TASK.charts.length; i++) {
      var c = TASK.charts[i], tracks = null;
      try {
        var r = await fetch(c.path, { credentials: 'include' });
        if (r.ok) {
          var qs = queries(new DOMParser().parseFromString(await r.text(), 'text/html'));
          tracks = qs && tracksOf(qs, null);
        }
      } catch (e) {}
      if (tracks) { read++; send('chart', { key: c.key, tracks: tracks }); } else { missing.push(c.key); }
      await sleep(1500);
    }
    send('done', { read: read, missing: missing });
  }
  async function label() {
    var qs = queries(document) || [], bio = '';
    for (var i = 0; i < qs.length; i++) {
      var d = qs[i].state && qs[i].state.data;
      if (d && d.id === TASK.id && typeof d.bio === 'string') bio = d.bio;
    }
    send('label', { id: TASK.id, bio: bio, tracks: tracksOf(qs, 'top-10') || [] });
  }
  ready().then(function (ok) {
    if (!ok) return send('done', { read: 0, missing: ['timeout'] });
    return TASK.mode === 'label' ? label() : charts();
  });
})();
