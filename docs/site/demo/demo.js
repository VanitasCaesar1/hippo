/* The live demo: Morning and Brain from Hippo.app, drawn in the browser from
   window.HIPPO_DEMO (docs/site/demo/data.js, generated from the synthetic demo
   home by docs/site/demo-data.sh). Nothing here writes anywhere: Keep, Dismiss
   and Undo change only this page. Brain zooms as the app does: the layout comes
   with the data, one per zoom level, and crossing a level keeps the card under
   the pointer there. */
(function () {
  'use strict';
  var D = window.HIPPO_DEMO;
  var app = document.getElementById('app');
  if (!D || !app) return;

  var OUT_BELOW = 0.55, IN_ABOVE = 1.6, MIN_ZOOM = 0.15, MAX_ZOOM = 3.0;

  function esc(s) {
    return String(s == null ? '' : s).replace(/[&<>"']/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
  }
  var P = {
    morning: '<path d="M1.5 12.5h13M4.5 12.5a3.5 3.5 0 0 1 7 0M8 5.2V3.4M3.4 7.9 2.2 6.7M12.6 7.9l1.2-1.2"/>',
    brain: '<circle cx="3.6" cy="4" r="1.7"/><circle cx="12.4" cy="5.6" r="1.7"/><circle cx="7.2" cy="12" r="1.7"/><path d="M5.3 4.3l5.4 1M4.3 5.6l2.2 4.8M11.3 6.9 8.3 10.7"/>',
    agents: '<path d="M2 4h6.5M2 8h12M2 12h9"/>',
    guard: '<path d="M8 1.8l5 1.9v3.9c0 3.1-2 5.4-5 6.6-3-1.2-5-3.5-5-6.6V3.7z"/>',
    note: '<path d="M4 1.8h5.3l3.2 3.2v9.2H4zM9.3 1.8V5h3.2M6.2 8.3h3.9M6.2 10.9h2.6"/>',
    folder: '<path d="M1.8 4.2h4.3l1.3 1.6h6.8v7.4H1.8z"/>',
    check: '<path d="M3.2 8.4l3.1 3.1 6.5-6.9"/>',
    x: '<path d="M4.2 4.2l7.6 7.6M11.8 4.2l-7.6 7.6"/>',
    undo: '<path d="M5.6 3.2l-3 3 3 3M2.9 6.2h6.3a3.6 3.6 0 0 1 0 7.2H6.6"/>',
    said: '<path d="M2.2 3h11.6v7.4H7.6L4.6 13v-2.6H2.2z"/>',
    receipt: '<path d="M3.5 1.8h9v12.4l-1.5-1-1.5 1-1.5-1-1.5 1-1.5-1-1.5 1zM6 5.5h4M6 8.2h4"/>',
    up: '<path d="M8 13V3.2M3.8 7.2 8 3l4.2 4.2"/>',
    sun: '<circle cx="8" cy="8" r="2.8"/><path d="M8 1.6v1.6M8 12.8v1.6M1.6 8h1.6M12.8 8h1.6M3.5 3.5l1.1 1.1M11.4 11.4l1.1 1.1M12.5 3.5l-1.1 1.1M4.6 11.4l-1.1 1.1"/>',
    moon: '<path d="M13 9.6A5.6 5.6 0 0 1 6.4 3a5.6 5.6 0 1 0 6.6 6.6z"/>',
    start: '<path d="M2.5 13.5v-11M2.5 3.2h9.3l-2 2.8 2 2.8H2.5"/>',
    right: '<path d="M3 8h10M9 4l4 4-4 4"/>'
  };
  function ic(n, s) {
    s = s || 16;
    return '<svg width="' + s + '" height="' + s + '" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' + (P[n] || '') + '</svg>';
  }
  var MARK = '<svg viewBox="0 0 32 32" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M5 20c0-6 5-10 11-10s11 4 11 10c0 3-2 5-5 5H10c-3 0-5-2-5-5z"/><circle cx="11" cy="17" r="1"/><circle cx="21" cy="17" r="1"/><path d="M9 10 7 6M23 10l2-4"/></svg>';
  function badge(a) {
    return a ? '<span class="badge ' + esc(a.code) + '">' + esc(a.short) + '</span>' : '';
  }
  function chip(words, tone) {
    var dot = tone === 'you' || tone === 'ok' || tone === 'bad';
    return '<span class="chip ' + (tone || '') + (dot ? ' dot' : '') + '">' + esc(words) + '</span>';
  }

  var S = { view: location.hash === '#brain' ? 'brain' : 'morning', rules: {}, undone: {}, sel: D.selected, rule: null, toast: '', zoom: 1, pan: [0, 0], fitted: false };
  try {
    var t = localStorage.getItem('hippo-demo-theme');
    if (t) document.documentElement.setAttribute('data-theme', t);
  } catch (e) { /* no storage */ }
  function dark() {
    var t = document.documentElement.getAttribute('data-theme');
    if (t) return t === 'dark';
    return !window.matchMedia('(prefers-color-scheme: light)').matches;
  }

  /* ---- the shell ---- */
  function top() {
    var waiting = D.morning.rules.filter(function (r) { return !S.rules[r.id]; }).length;
    var tab = function (k, name, n) {
      var on = S.view === k;
      return '<button data-view="' + k + '"' + (on ? ' aria-current="page"' : '') + '>' + ic(k) + '<span class="name">' + name + '</span>' + (n ? '<span class="count you">' + n + '</span>' : '') + '</button>';
    };
    var off = function (k, name) {
      return '<button disabled title="In the app">' + ic(k) + '<span class="name">' + name + '</span></button>';
    };
    return '<header class="top"><div class="brand">' + MARK + '<b>hippo</b><span class="ws">tern</span></div>' +
      '<nav class="nav" aria-label="Views">' + tab('morning', 'Morning', waiting) + tab('brain', 'Brain', 0) + off('agents', 'Agents') + off('guard', 'Guard') + '</nav>' +
      '<div class="right"><span class="idx"><i></i>Indexed ' + esc(D.counts.indexed || '') + '</span>' +
      '<button class="icon-btn" data-act="theme" aria-label="Switch theme">' + ic(dark() ? 'sun' : 'moon') + '</button></div></header>';
  }
  function ask() {
    var a = D.counts.ask;
    return '<div class="ask" aria-hidden="true"><span style="flex:1">Ask about Tern\'s memory…</span>' +
      (a ? '<span class="who">' + (a.known === false ? '' : badge(a.agent)) + esc(a.label) + '</span>' : '') +
      '<span class="send">' + ic('up', 18) + '</span></div>';
  }

  /* ---- Morning ---- */
  function stripHtml() {
    var s = D.strip, t0 = Date.parse(s.start), t1 = Date.parse(s.now);
    var x = function (iso) { return Math.max(0, Math.min(1, (Date.parse(iso) - t0) / Math.max(1, t1 - t0))) * 100; };
    var peak = Math.max(4, Math.max.apply(null, s.beats.concat([0])));
    var bars = s.beats.map(function (v, i) {
      var h = v ? Math.max(4, Math.sqrt(v / peak) * 100) : 0;
      return '<i' + (s.corrections.indexOf(i) >= 0 ? ' class="c"' : '') + ' style="height:' + h.toFixed(1) + '%"></i>';
    }).join('');
    var n0 = x(s.night_start), n1 = x(s.night_end);
    var pass = s.pass_start ? '<span class="pass" style="left:' + x(s.pass_start).toFixed(2) + '%"></span>' : '';
    return '<div class="strip"><span class="nightzone" style="left:' + n0.toFixed(2) + '%;width:' + Math.max(0.5, n1 - n0).toFixed(2) + '%"></span>' +
      '<span class="bars">' + bars + '</span>' + pass + '<span class="now"></span></div>' +
      '<div class="strip-labels"><span style="left:0"><b>Day</b>' + esc(s.day) + '</span>' +
      '<span style="left:' + Math.min(n0, 70).toFixed(1) + '%;color:var(--night)"><b>Night</b>' + esc(s.night) + '</span>' +
      '<span style="right:0"><b>Morning</b><span class="mono">' + esc(s.morning) + '</span></span></div>';
  }
  function said(e) {
    if (e.said) return '<span class="said clamp2">' + esc(e.said) + '</span>';
    return '<span class="failed clamp2">' + esc(e.failed) + '</span>';
  }
  function evCard(e, id) {
    return '<div class="card ev" data-ev="' + id + '"><div class="row">' + badge(e.agent) +
      '<span class="clamp1" style="flex:1;color:var(--muted)">' + esc(e.session_title || e.session.slice(0, 8)) + '</span><span class="mono" style="color:var(--faint)">' + esc(e.when) + '</span></div>' +
      '<div class="row" style="align-items:flex-start;margin-top:6px"><span style="color:var(--faint)">' + (e.said ? 'you' : '') + '</span>' + said(e) + '</div></div>';
  }
  function ruleCard(r) {
    var st = S.rules[r.id];
    var note = (r.note_label || '').split('/').pop();
    var actions = st
      ? '<span style="color:var(--muted)">' + (st === 'kept' ? 'Kept: in the app this writes the rule into ' + esc(note) + ', with a receipt.' : 'Dismissed: it comes back only if it recurs in 3 more sessions.') + '</span>'
      : '<button class="btn go" data-act="keep" data-id="' + r.id + '">Keep</button><button class="btn second" data-act="edit" data-id="' + r.id + '">Edit</button><button class="btn quiet" data-act="dismiss" data-id="' + r.id + '">Dismiss</button><span class="clamp1" style="flex:1;text-align:right;color:var(--faint)">into ' + esc(note) + '</span>';
    return '<div class="card rule' + (S.rule === r.id ? ' sel' : '') + (st ? ' gone' : '') + '" data-rule="' + r.id + '"><div class="row"><span class="tile you">' + ic(r.kind === 'failure' ? 'x' : 'said') + '</span>' +
      '<span class="clamp1" style="flex:1;color:var(--muted)">' + esc(r.from) + '</span>' + (r.agrees ? chip('second model agrees', 'ok') : chip('not reviewed')) + '</div>' +
      '<p class="text clamp2">' + esc(r.rule) + '</p><div class="row">' + actions + '</div>' +
      '<div class="evs narrow" style="margin-top:12px">' + r.evidence.slice(0, 3).map(function (e) { return '<div class="row" style="align-items:flex-start">' + badge(e.agent) + '<span class="mono" style="color:var(--faint)">' + esc(e.when) + '</span>' + said(e) + '</div>'; }).join('') + '</div></div>';
  }
  function fixCard(f, rejected) {
    var undone = S.undone[f.proposal] || f.reverted;
    var c = rejected ? chip('rejected', 'bad') : undone ? chip('undone') : chip('fixed last night', 'ok');
    var diff = '<div class="diff">' + (f.was ? '<div class="row"><span class="was">' + esc(f.was) + '</span>' + ic('right', 14) + '</div>' : '') + '<div><span class="now">' + esc(f.now) + '</span></div></div>';
    var foot = rejected
      ? '<p class="quote bad">' + esc(f.reason) + '</p>'
      : '<div class="row"><span style="color:var(--faint)">' + ic('receipt', 14) + '</span><span class="mono clamp1" style="flex:1;color:var(--faint)">' + esc((f.receipt || '').slice(0, 10)) + '…</span>' +
        (f.undo && !undone ? '<button class="btn second" data-act="undo" data-id="' + f.proposal + '">' + ic('undo', 14) + 'Undo</button>' : '') + '</div>';
    return '<div class="card fix"><div class="row"><span class="tile ' + (rejected ? 'bad' : 'ok') + '">' + ic(rejected ? 'x' : 'check') + '</span><span class="title clamp1">' + esc(f.title) + '</span>' + c + '</div>' + diff + foot + '</div>';
  }
  function morning() {
    var m = D.morning, f = m.figures;
    var fig = function (n, w, color) { return '<div class="card figure"><div class="n" style="color:' + (n ? color : 'var(--text)') + '">' + n + '</div><div class="w">' + w + '</div></div>'; };
    var groups = m.rules.map(function (r) {
      return '<div class="group"><div class="evs wide">' + r.evidence.slice(0, 4).map(function (e, k) { return evCard(e, r.id + '-' + k); }).join('') + '</div>' + ruleCard(r) + '</div>';
    }).join('');
    var waiting = m.rules.filter(function (r) { return !S.rules[r.id]; }).length;
    return '<p class="kicker">' + esc(m.kicker) + '</p><h1>' + esc(m.headline) + '</h1><p class="lead">' + esc(m.lead) + '</p>' +
      '<div class="figures">' + fig(f.rules_waiting, 'rules waiting for you', 'var(--you)') + fig(f.notes_fixed, 'notes fixed and confirmed', 'var(--ok)') +
      fig(f.fixes_rejected, 'fixes rejected by the second model', 'var(--bad)') + fig(f.notes_checked, 'notes checked against what happened', 'var(--text)') + '</div>' +
      stripHtml() +
      '<div class="flow"><svg class="wires"></svg><div class="col"><div class="group" style="margin-bottom:10px"><p class="label evs wide">Yesterday, in your sessions</p><p class="label">Waiting for you <span class="count you">' + waiting + '</span></p></div>' + groups + '</div>' +
      '<div class="col"><p class="label">Applied overnight <span class="count">' + m.applied.length + '</span></p>' + m.applied.map(function (x) { return fixCard(x, false); }).join('') +
      (m.rejected.length ? '<p class="label" style="margin-top:14px">Rejected by the second model <span class="count">' + m.rejected.length + '</span></p>' + m.rejected.map(function (x) { return fixCard(x, true); }).join('') : '') + '</div></div>';
  }
  function startPanel() {
    var b = D.brief, p = b.parts, reaches = b.reaches || [];
    var row = function (title, right) { return '<div class="row-note">' + ic('note', 15) + '<span class="t clamp1">' + esc(title) + '</span><span style="color:var(--faint);font-size:13px">' + esc(right) + '</span></div>'; };
    var kb = function (n) { return n < 1000 ? n + ' B' : (n / 1024).toFixed(1) + ' KB'; };
    return '<div class="start"><div class="row"><span class="tile night">' + ic('start') + '</span><span style="color:var(--muted)">Start block</span></div>' +
      '<h2 style="margin-top:14px">Your agents now start with</h2><p style="color:var(--muted);margin-top:10px">Before its first prompt, every new session gets the notes its task needs: about ' + kb(p.bytes.total) + ' of a ' + kb(p.budget) + ' budget, not the whole pile.</p>' +
      '<div class="row-note" style="margin-top:14px"><span style="color:var(--faint)">For the task</span><span class="t clamp1">' + esc(b.brief.task) + '</span></div>' +
      (p.required.length ? '<p class="group-label">Always</p>' + p.required.map(function (x) { return row(x.title, x.reason); }).join('') : '') +
      (p.picked.length ? '<p class="group-label">Picked for this task</p>' + p.picked.slice(0, 4).map(function (x) { return row(x.title, x.reason); }).join('') : '') +
      (p.waiting.length ? '<p class="group-label">Not yet: waiting for you</p>' + p.waiting.map(function (w) { return '<p class="bullet hollow">' + esc(w) + '</p>'; }).join('') : '') +
      '<p class="group-label">Reaches</p>' + (reaches.length ? reaches.map(function (r) { return '<div class="row-note">' + badge(r.agent) + '<span class="t">' + esc(r.agent.name) + '</span><span style="color:var(--faint)">' + esc(r.how) + '</span></div>'; }).join('') : '<p style="color:var(--muted)">Claude Code at session start, once hippo hooks install adds its hook. (The demo home installs none.)</p>') + '</div>';
  }
  function drawWires() {
    var flow = app.querySelector('.flow'), svg = flow && flow.querySelector('svg.wires');
    if (!svg || window.innerWidth < 900) return;
    var box = flow.getBoundingClientRect(), out = '';
    D.morning.rules.forEach(function (r) {
      var rc = app.querySelector('[data-rule="' + r.id + '"]');
      if (!rc) return;
      var rb = rc.getBoundingClientRect(), n = 0, evs = app.querySelectorAll('[data-ev^="' + r.id + '-"]');
      evs.forEach(function (ev, k) {
        var eb = ev.getBoundingClientRect();
        if (!eb.width) return;
        var spread = evs.length > 1 ? (k - (evs.length - 1) / 2) * 10 : 0;
        var x0 = eb.right - box.left, y0 = eb.top + eb.height / 2 - box.top;
        var x1 = rb.left - box.left, y1 = rb.top + rb.height / 2 + spread - box.top;
        var xt = x0 + (x1 - x0) * 0.45;
        var col = S.rules[r.id] ? 'var(--line2)' : 'var(--you-line)';
        out += '<polyline fill="none" stroke="' + col + '" stroke-width="1.5" points="' + [x0, y0, xt, y0, xt, y1, x1, y1].join(' ') + '"/>' +
          '<circle cx="' + x0 + '" cy="' + y0 + '" r="3" fill="var(--canvas)" stroke="' + col + '" stroke-width="1.5"/><circle cx="' + x1 + '" cy="' + y1 + '" r="3" fill="var(--canvas)" stroke="' + col + '" stroke-width="1.5"/>';
        n++;
      });
    });
    svg.setAttribute('width', box.width);
    svg.setAttribute('height', box.height);
    svg.innerHTML = out;
  }

  /* ---- Brain ---- */
  var M = D.map;
  function level(z) { return z < OUT_BELOW ? 'out' : z > IN_ABOVE ? 'in' : 'mid'; }
  function key(item) { return item.note != null ? 'n' + item.note : 'f' + item.folder; }
  function pinned(i) { var f = M.notes[i].flags; return f.entry || f.required; }
  function sameItem(item, to) {
    if (item.note != null && to === 'out' && !pinned(item.note)) {
      for (var fi = 0; fi < M.folders.length; fi++) if (M.folders[fi].notes.indexOf(item.note) >= 0) return { folder: fi };
    }
    return item;
  }
  function cardAt(lay, x, y) {
    for (var k = lay.cards.length - 1; k >= 0; k--) {
      var r = lay.cards[k].rect;
      if (x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h) return lay.cards[k];
    }
    return null;
  }
  function cardOf(lay, item) {
    var k = key(item);
    for (var i = 0; i < lay.cards.length; i++) if (key(lay.cards[i].item) === k) return lay.cards[i];
    return null;
  }
  function anchor(from, to, x, y) {
    var a = cardAt(M[from], x, y);
    if (!a) return [x, y];
    var b = cardOf(M[to], sameItem(a.item, to));
    if (!b) return [x, y];
    var fx = (x - a.rect.x) / a.rect.w, fy = (y - a.rect.y) / a.rect.h;
    return [b.rect.x + fx * b.rect.w, b.rect.y + fy * b.rect.h];
  }
  function selIndex() { for (var i = 0; i < M.notes.length; i++) if (M.notes[i].path === S.sel) return i; return -1; }
  function neighbours(i) {
    var n = {};
    M.edges.forEach(function (e) { if (e[0] === i) n[e[1]] = 1; if (e[1] === i) n[e[0]] = 1; });
    return n;
  }
  function worldHtml(L) {
    var lay = M[L], sel = selIndex(), near = sel >= 0 ? neighbours(sel) : {};
    var h = '<svg class="links' + (sel >= 0 ? ' dim' : '') + '" width="' + lay.width + '" height="' + lay.height + '">' + lay.lines.map(function (l) {
      var on = sel >= 0 && ((l.from.note === sel) || (l.to.note === sel));
      return '<polyline class="' + (on ? 'on' : '') + '" points="' + l.points.map(function (p) { return p[0] + ',' + p[1]; }).join(' ') + '"/>';
    }).join('') + '</svg>';
    lay.frames.forEach(function (fr) {
      var f = M.folders[fr[0]], r = fr[1];
      h += '<div class="frame" style="left:' + r.x + 'px;top:' + r.y + 'px;width:' + r.w + 'px;height:' + r.h + 'px"><div class="fh">' + esc(f.name || 'Notes') + '<span>' + f.notes.length + (f.notes.length === 1 ? ' note' : ' notes') + '</span></div></div>';
    });
    lay.cards.forEach(function (c) {
      var r = c.rect, st = 'left:' + r.x + 'px;top:' + r.y + 'px;width:' + r.w + 'px;height:' + r.h + 'px';
      if (c.item.folder != null) {
        var f = M.folders[c.item.folder];
        var bits = [f.notes.length + (f.notes.length === 1 ? ' note' : ' notes')];
        if (f.fixed) bits.push(f.fixed + ' fixed');
        if (f.waiting) bits.push(f.waiting + ' waiting');
        if (f.rejected) bits.push(f.rejected + ' rejected');
        if (f.stale) bits.push(f.stale + ' stale');
        h += '<div class="folder" data-folder="' + c.item.folder + '" style="' + st + '"><div class="t">' + esc(f.name || 'Notes') + '</div><div class="s">' + bits.join(' · ') + '</div></div>';
        return;
      }
      var i = c.item.note, n = M.notes[i], fl = n.flags;
      var ch = fl.rejected ? chip('fix rejected', 'bad') : fl.waiting ? chip(fl.waiting === 1 ? 'rule waiting' : fl.waiting + ' rules waiting', 'you') : fl.fixed ? chip('fixed last night', 'ok') : fl.stale_days != null ? chip('stale, ' + fl.stale_days + ' days') : (fl.required || fl.entry) ? chip('every session', 'night') : '';
      var inner = '';
      if (L === 'in') {
        inner = '<div class="in">' + n.pending.map(function (p) { return '<p class="bullet hollow">' + esc(p) + '</p>'; }).join('') +
          n.evidence.map(function (e) { return '<p><span class="said">' + esc(e) + '</span></p>'; }).join('') +
          n.rules.map(function (p) { return '<p class="bullet">' + esc(p) + '</p>'; }).join('') + '</div>';
      }
      h += '<div class="note' + (i === sel ? ' sel' : near[i] ? ' near' : '') + '" data-note="' + i + '" style="' + st + '"><div class="row">' + ic(fl.entry ? 'start' : 'note') + '<span class="t clamp1">' + esc(n.title) + '</span></div>' +
        '<p class="s clamp1">' + esc(n.summary) + '</p><div class="row">' + ch + chip(n.reads_week ? 'read ' + n.reads_week + '× this week' : 'not read this week') + '</div>' + inner + '</div>';
    });
    return h;
  }
  var drawnLevel = null;
  function placeWorld(force) {
    var wrap = app.querySelector('.mapwrap');
    if (!wrap) return;
    var world = wrap.querySelector('.world'), L = level(S.zoom);
    if (force || drawnLevel !== L) {
      world.innerHTML = worldHtml(L);
      world.style.width = M[L].width + 'px';
      world.style.height = M[L].height + 'px';
      drawnLevel = L;
    }
    world.style.transform = 'translate(' + (-S.pan[0] * S.zoom) + 'px,' + (-S.pan[1] * S.zoom) + 'px) scale(' + S.zoom + ')';
    var pct = wrap.querySelector('[data-act="fit"]');
    if (pct) pct.textContent = Math.round(S.zoom * 100) + '%';
  }
  function fit() {
    var wrap = app.querySelector('.mapwrap');
    if (!wrap) return;
    var w = wrap.clientWidth, h = wrap.clientHeight, pad = 28;
    var z = Math.max(MIN_ZOOM, Math.min(1, (w - 2 * pad) / M.mid.width, (h - 2 * pad) / M.mid.height));
    if (M.notes.length <= 80 && z < OUT_BELOW + 0.05) z = OUT_BELOW + 0.1;
    S.zoom = z;
    S.pan = [-Math.max(pad / z, (w / z - M.mid.width) / 2), -pad / z];
    S.fitted = true;
    placeWorld(true);
  }
  function zoomAt(z, px, py) {
    z = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, z));
    var from = level(S.zoom), to = level(z);
    var m = [S.pan[0] + px / S.zoom, S.pan[1] + py / S.zoom];
    if (from !== to) m = anchor(from, to, m[0], m[1]);
    S.zoom = z;
    S.pan = [m[0] - px / z, m[1] - py / z];
    placeWorld(false);
  }
  function brain() {
    return '<p class="kicker">' + esc(M.kicker) + '</p><h1>' + esc(M.headline) + '</h1><p class="lead">' + esc(M.lead) + '</p>' +
      '<div class="mapwrap card" style="padding:0;margin-top:18px"><div class="world"></div><div class="zoombar"><button data-act="zout" aria-label="Zoom out">−</button><button data-act="fit" aria-label="Fit">100%</button><button data-act="zin" aria-label="Zoom in">+</button></div></div>' +
      '<div class="boardwrap"></div>';
  }
  function boardHtml() {
    var sel = selIndex(), near = sel >= 0 ? neighbours(sel) : {};
    var card = function (i) {
      var n = M.notes[i];
      return '<div class="card' + (i === sel ? ' sel' : near[i] ? ' near' : '') + '" data-note="' + i + '"><div class="row">' + ic('note') + '<b class="clamp1">' + esc(n.title) + '</b></div><p class="clamp2" style="color:var(--muted);margin:8px 0">' + esc(n.summary) + '</p>' + (near[i] ? chip('linked', 'night') : '') + chip(n.reads_week ? 'read ' + n.reads_week + '× this week' : 'not read this week') + '</div>';
    };
    var h = '<div class="board">' + M.notes.map(function (n, i) { return pinned(i) ? card(i) : ''; }).join('') + '</div>';
    M.folders.forEach(function (f) {
      h += '<p class="label" style="font-size:17px;color:var(--text)">' + ic('folder') + esc(f.name || 'Notes') + ' <span style="color:var(--faint);font-size:14px">' + f.notes.length + '</span></p><div class="board">' + f.notes.map(card).join('') + '</div>';
    });
    return h;
  }
  function notePanel() {
    var p = D.notes[S.sel];
    if (!p) return '<p style="color:var(--muted)">Pick a note on the map.</p>';
    var ch = p.chip ? chip(p.chip[0], p.chip[1]) : '';
    var h = '<div class="row"><span class="tile">' + ic('note') + '</span><span style="color:var(--muted)">' + esc(p.folder || 'Notes') + '</span></div>' +
      '<h2 style="margin-top:14px">' + esc(p.title) + '</h2><p style="color:var(--muted);margin-top:10px">' + esc(p.summary) + '</p>' +
      '<div class="row" style="margin-top:12px;flex-wrap:wrap">' + ch + chip(p.reads_week ? 'read ' + p.reads_week + '× this week' : 'not read this week') + '</div>' +
      '<div class="stats"><div><b>' + p.reads_week + '</b><span>reads this week</span></div><div><b>' + p.rules_kept + '</b><span>rules kept</span></div><div><b>' + p.changes + '</b><span>changes</span></div></div>';
    if (p.rules.length) h += '<p class="group-label">Rules</p>' + p.rules.map(function (r) { return '<p class="bullet">' + esc(r.rule) + ' <span style="color:var(--faint)">' + esc(r.kept) + '</span></p>'; }).join('');
    p.pending.forEach(function (r) {
      var st = S.rules[r.id];
      h += '<div class="pending"><p>' + esc(r.rule) + '</p><div class="row" style="margin-top:10px">' + (st ? '<span style="color:var(--muted)">' + (st === 'kept' ? 'Kept' : 'Dismissed') + '</span>' : '<button class="btn go" data-act="keep" data-id="' + r.id + '">Keep</button><button class="btn quiet" data-act="dismiss" data-id="' + r.id + '">Dismiss</button>') + '</div></div>';
      if (r.evidence.length) h += '<p class="group-label">Evidence: what you said</p>' + r.evidence.slice(0, 4).map(function (e) { return '<div style="margin-bottom:10px"><div class="row">' + badge(e.agent) + '<span class="mono" style="color:var(--faint)">' + esc(e.when) + '</span></div><span class="said">' + esc(e.said) + '</span></div>'; }).join('');
    });
    var links = function (label, l) { return l.length ? '<p class="group-label">' + label + '</p>' + l.slice(0, 6).map(function (x) { return '<div class="row-note">' + ic('note', 14) + '<span class="t clamp1">' + esc(x[1]) + '</span></div>'; }).join('') : ''; };
    h += '<div class="start">' + links('Links to', p.links_out) + links('Linked from', p.links_in) + '</div>';
    if (p.receipts.length) h += '<p class="group-label">Receipts: every change, with an undo</p>' + p.receipts.map(function (r) { return '<div class="row-note" style="height:auto;padding:10px 12px"><span class="t">' + esc(r.what) + '<br><span style="color:var(--faint);font-size:12.5px">' + esc(r.when) + '</span></span></div>'; }).join('');
    return h;
  }

  /* ---- render and events ---- */
  function render() {
    var narrow = window.innerWidth < 900;
    var main = S.view === 'brain' ? brain() : morning();
    var panel = S.view === 'brain' ? notePanel() : startPanel();
    app.innerHTML = '<div class="banner">A replica of Hippo.app\'s Morning and Brain, drawn from hippo\'s own output for a synthetic project. Nothing you click is saved. <a href="index.html">Back to hippo</a></div>' +
      top() + '<div class="stage"><main class="main">' + main + '</main><aside class="panel" aria-label="Details">' + panel + '</aside></div>' + ask() +
      (S.toast ? '<div class="toast" role="status">' + esc(S.toast) + '</div>' : '');
    if (S.view === 'brain') {
      var wrap = app.querySelector('.mapwrap'), board = app.querySelector('.boardwrap');
      if (narrow) { wrap.style.display = 'none'; board.innerHTML = boardHtml(); } else {
        drawnLevel = null;
        if (!S.fitted) fit(); else placeWorld(true);
        bindMap(wrap);
      }
    } else {
      drawWires();
    }
  }
  function toast(t) {
    S.toast = t;
    render();
    setTimeout(function () { if (S.toast === t) { S.toast = ''; render(); } }, 2600);
  }
  function bindMap(wrap) {
    var drag = null;
    var pt = function (e) { var b = wrap.getBoundingClientRect(); return [e.clientX - b.left, e.clientY - b.top]; };
    wrap.addEventListener('wheel', function (e) {
      e.preventDefault();
      var p = pt(e);
      if (e.ctrlKey || e.metaKey) zoomAt(S.zoom * Math.exp(-e.deltaY * 0.01), p[0], p[1]);
      else { S.pan = [S.pan[0] + e.deltaX / S.zoom, S.pan[1] + e.deltaY / S.zoom]; placeWorld(false); }
    }, { passive: false });
    wrap.addEventListener('pointerdown', function (e) {
      if (e.target.closest('.zoombar')) return;
      drag = { p: pt(e), pan: S.pan.slice(), moved: false, id: e.pointerId };
    });
    wrap.addEventListener('pointermove', function (e) {
      if (!drag) return;
      var p = pt(e), dx = p[0] - drag.p[0], dy = p[1] - drag.p[1];
      if (Math.abs(dx) + Math.abs(dy) > 3) {
        if (!drag.moved) { wrap.setPointerCapture(drag.id); wrap.classList.add('drag'); }
        drag.moved = true;
        S.pan = [drag.pan[0] - dx / S.zoom, drag.pan[1] - dy / S.zoom];
        placeWorld(false);
      }
    });
    wrap.addEventListener('pointerup', function (e) {
      var d = drag;
      drag = null;
      wrap.classList.remove('drag');
      if (!d || d.moved) return;
      var note = e.target.closest('[data-note]'), folder = e.target.closest('[data-folder]');
      if (note) { S.sel = M.notes[+note.getAttribute('data-note')].path; render(); }
      else if (folder) zoomInto({ folder: +folder.getAttribute('data-folder') });
    });
    wrap.addEventListener('dblclick', function (e) {
      var note = e.target.closest('[data-note]');
      if (note) zoomInto({ note: +note.getAttribute('data-note') });
    });
  }
  function zoomInto(item) {
    var wrap = app.querySelector('.mapwrap'), w = wrap.clientWidth, h = wrap.clientHeight;
    var L = item.folder != null ? 'mid' : 'in', target = item.folder != null ? 1 : IN_ABOVE + 0.2;
    var r = null;
    if (item.folder != null) M.mid.frames.forEach(function (f) { if (f[0] === item.folder) r = f[1]; });
    else { var c = cardOf(M[L], item); r = c && c.rect; }
    if (!r) return;
    var z = item.folder != null ? Math.max(OUT_BELOW + 0.05, Math.min(target, (w - 80) / r.w, (h - 80) / r.h)) : target;
    S.zoom = z;
    S.pan = [r.x + r.w / 2 - w / 2 / z, r.y + r.h / 2 - h / 2 / z];
    placeWorld(false);
  }
  app.addEventListener('click', function (e) {
    var b = e.target.closest('[data-act],[data-view],[data-rule],[data-note]');
    if (!b) return;
    var act = b.getAttribute('data-act'), id = b.getAttribute('data-id');
    if (b.hasAttribute('data-view')) { S.view = b.getAttribute('data-view'); history.replaceState(null, '', '#' + S.view); window.scrollTo(0, 0); render(); return; }
    if (act === 'theme') {
      var t = dark() ? 'light' : 'dark';
      document.documentElement.setAttribute('data-theme', t);
      try { localStorage.setItem('hippo-demo-theme', t); } catch (err) { /* no storage */ }
      render();
    } else if (act === 'keep') { S.rules[id] = 'kept'; toast('Kept. In the app: hippo lessons keep, a receipt, and hippo revert undoes it.'); }
    else if (act === 'dismiss') { S.rules[id] = 'dismissed'; toast('Dismissed. It comes back only if it recurs in 3 more sessions.'); }
    else if (act === 'edit') { toast('In the app, Edit opens the rule\'s words before Keep writes it.'); }
    else if (act === 'undo') { S.undone[id] = true; toast('Undone. In the app: hippo revert, from the change\'s snapshot.'); }
    else if (act === 'zin' || act === 'zout') { var wrap = app.querySelector('.mapwrap'); zoomAt(S.zoom * (act === 'zin' ? 1.25 : 0.8), wrap.clientWidth / 2, wrap.clientHeight / 2); }
    else if (act === 'fit') { fit(); }
    else if (b.hasAttribute('data-rule')) { S.rule = b.getAttribute('data-rule'); render(); }
    else if (b.hasAttribute('data-note') && S.view === 'brain' && window.innerWidth < 900) { S.sel = M.notes[+b.getAttribute('data-note')].path; render(); }
  });
  var last = window.innerWidth < 900;
  window.addEventListener('resize', function () {
    var now = window.innerWidth < 900;
    if (now !== last) { last = now; render(); } else if (S.view === 'morning') drawWires();
  });
  render();
})();
