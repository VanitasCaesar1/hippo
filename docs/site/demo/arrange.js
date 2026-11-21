/* Arranging Brain in the live demo: a port of hippo_view::night::map's
   arrangement, packer and link router (docs/designs/brain-arrange.md).
   Each block (a folder, or a note read in every session) is moved with its
   own cards, words and marks from the default layout that hippo wrote into
   data.js; the links are routed again. docs/site/check-demo-render.mjs
   checks this port against a map Rust arranged. Nothing is saved. */
(function (root) {
  'use strict';
  var CARD_H = 112, GAP = 12, FRAME_PAD = 14, FRAME_HEAD = 40, FRAME_GAP = 48,
    COL_GAP = 112, CARD_W = 340, COL_W = CARD_W + 2 * FRAME_PAD, OUT_GAP = 72,
    OUT_BELOW = 0.7, OUT_FIT = 0.69, MIN_ZOOM = 0.15, MAX_ZOOM = 3, FIT_PAD = 28;

  function colX(c) { return c * (COL_W + COL_GAP); }
  function right(r) { return r.x + r.w; }
  function bottom(r) { return r.y + r.h; }
  function gapSpan(g) { var r = colX(g) - 6; return [r - (COL_GAP - 12), r]; }
  function itemKey(it) { return it.note != null ? 'n' + it.note : 'f' + it.folder; }
  // Rust's derived order of Item: every note before every folder.
  function itemCmp(a, b) {
    var ka = a.note != null ? 0 : 1, kb = b.note != null ? 0 : 1;
    if (ka !== kb) return ka - kb;
    return (a.note != null ? a.note : a.folder) - (b.note != null ? b.note : b.folder);
  }
  function pinned(M, i) { var f = M.notes[i].flags; return !!(f.entry || f.required); }
  function folderOf(M, i) {
    for (var f = 0; f < M.folders.length; f++) if (M.folders[f].notes.indexOf(i) >= 0) return f;
    return -1;
  }
  /* The block a card stands for, by key. */
  function blockKey(M, it) {
    if (it.folder != null) return 'folder:' + M.folders[it.folder].name;
    if (pinned(M, it.note)) return 'note:' + M.notes[it.note].path;
    var f = folderOf(M, it.note);
    return f >= 0 ? 'folder:' + M.folders[f].name : 'note:' + M.notes[it.note].path;
  }
  function allKeys(arr) { return [].concat.apply([], arr.columns); }

  function settle(M, arr) {
    var known = {}, seen = {};
    allKeys(M.default_arrangement).forEach(function (k) { known[k] = 1; });
    var cols = arr.columns.map(function (c) {
      return c.filter(function (k) { if (!known[k] || seen[k]) return false; seen[k] = 1; return true; });
    });
    M.default_arrangement.columns.forEach(function (col, dc) {
      col.forEach(function (k) {
        if (seen[k]) return;
        while (cols.length <= dc) cols.push([]);
        cols[dc].push(k); seen[k] = 1;
      });
    });
    return { columns: cols.filter(function (c) { return c.length; }) };
  }
  function find(arr, key) {
    for (var c = 0; c < arr.columns.length; c++) {
      var r = arr.columns[c].indexOf(key);
      if (r >= 0) return [c, r];
    }
    return null;
  }
  function moveTo(arr, key, col, at) {
    if (!find(arr, key)) return arr;
    var cols = arr.columns.map(function (c) { return c.filter(function (k) { return k !== key; }); });
    if (col >= cols.length) cols.push([key]);
    else cols[col].splice(Math.min(at, cols[col].length), 0, key);
    return { columns: cols.filter(function (c) { return c.length; }) };
  }
  function nudge(arr, key, dir) {
    var p = find(arr, key);
    if (!p) return arr;
    var c = p[0], r = p[1];
    if (dir === 'left' && c > 0) return moveTo(arr, key, c - 1, r);
    if (dir === 'right' && !(c + 1 === arr.columns.length && arr.columns[c].length === 1)) return moveTo(arr, key, c + 1, r);
    if (dir === 'up' && r > 0) return moveTo(arr, key, c, r - 1);
    if (dir === 'down' && r + 1 < arr.columns[c].length) return moveTo(arr, key, c, r + 1);
    return arr;
  }
  function same(a, b) { return JSON.stringify(a.columns) === JSON.stringify(b.columns); }

  /* Each block of the default layout at one level, drawn from (0, 0). */
  function blocksOf(M, L) {
    var lay = M[L], out = {};
    function get(k) { return out[k] || (out[k] = { frames: [], cards: [], labels: [], marks: [], span: 1, h: 0, pinned: k.indexOf('note:') === 0, ox: 0, oy: 0 }); }
    lay.frames.forEach(function (fr) {
      var b = get(blockKey(M, { folder: fr[0] })), r = fr[1];
      b.ox = r.x; b.oy = r.y; b.h = r.h;
      b.span = Math.round((r.w + COL_GAP) / (COL_W + COL_GAP));
      b.frames.push(fr);
    });
    lay.cards.forEach(function (c) {
      var k = blockKey(M, c.item), b = get(k);
      if (c.item.folder != null || b.pinned) {
        b.ox = c.rect.x - (L !== 'out' && b.pinned ? FRAME_PAD : 0); b.oy = c.rect.y; b.h = c.rect.h;
      }
      b.cards.push(c);
    });
    lay.labels.forEach(function (l) { get(blockKey(M, l.item)).labels.push(l); });
    lay.marks.forEach(function (m) { get(blockKey(M, m.item)).marks.push(m); });
    return out;
  }
  function mv(r, dx, dy) { return { x: r.x + dx, y: r.y + dy, w: r.w, h: r.h }; }

  /* Places one level's blocks from a settled arrangement (no lines). */
  function place(M, L, arr, blocks) {
    var cols = arr.columns.map(function (c) { return c.slice().reverse(); });
    var width = 1;
    cols.forEach(function (col, c) { col.forEach(function (k) { width = Math.max(width, c + (L === 'out' ? 1 : blocks[k].span)); }); });
    var foot = []; for (var i = 0; i < width; i++) foot.push(null);
    function gap(prev, me) { return L === 'out' ? OUT_GAP : (prev && me ? GAP : FRAME_GAP); }
    var frames = [], cards = [], labels = [], marks = [];
    for (;;) {
      var best = null;
      cols.forEach(function (col, c) {
        if (!col.length) return;
        var d = blocks[col[col.length - 1]], span = L === 'out' ? 1 : d.span, top = 0;
        // A note read in every session that starts a column sits as low as
        // a frame's first cards (Mid and In).
        var first = d.pinned && L !== 'out' ? FRAME_HEAD : 0;
        for (var cc = c; cc < c + span; cc++) top = Math.max(top, foot[cc] ? foot[cc][0] + gap(foot[cc][1], d.pinned) : first);
        if (!best || top < best[1] - 0.5) best = [c, top];
      });
      if (!best) break;
      var c = best[0], top = best[1], d = blocks[cols[c].pop()];
      var dx = colX(c) - d.ox, dy = top - d.oy, span = L === 'out' ? 1 : d.span;
      for (var cc = c; cc < c + span; cc++) foot[cc] = [top + d.h, d.pinned];
      d.frames.forEach(function (f) { frames.push([f[0], mv(f[1], dx, dy)]); });
      d.cards.forEach(function (x) { cards.push({ item: x.item, rect: mv(x.rect, dx, dy) }); });
      d.labels.forEach(function (x) { labels.push({ item: x.item, kind: x.kind, rect: mv(x.rect, dx, dy), lines: x.lines, size: x.size }); });
      d.marks.forEach(function (x) { marks.push({ item: x.item, x: x.x + dx, y: x.y + dy, d: x.d, tone: x.tone, row: x.row }); });
    }
    return { frames: frames, cards: cards, labels: labels, marks: marks };
  }
  function size(p) {
    var w = 0, h = 0;
    p.cards.forEach(function (x) { w = Math.max(w, right(x.rect)); h = Math.max(h, bottom(x.rect)); });
    p.frames.forEach(function (f) { w = Math.max(w, right(f[1])); h = Math.max(h, bottom(f[1])); });
    return [w + FRAME_PAD, h];
  }

  function pack(M, L, arr, blocks) {
    var p = place(M, L, arr, blocks), wh = size(p), lk = links(M, L, p.frames, p.cards);
    return { width: wh[0], height: wh[1], frames: p.frames, cards: p.cards, focus: lk.focus, badges: lk.badges, hovered: lk.hovered, hovered_badges: lk.hovered_badges, labels: p.labels, marks: p.marks };
  }

  /* Each linked card's lines (Rust: links): one trunk each way, right
     (with its own column) and left. */
  function links(M, L, frames, cards) {
    var rectOf = {};
    cards.forEach(function (x) { rectOf[itemKey(x.item)] = x.rect; });
    function lift(i) {
      if (L !== 'out') return { note: i };
      var f = folderOf(M, i);
      return f >= 0 ? { folder: f } : { note: i };
    }
    // Each pair: its links, and whether they run from its first card to
    // its second (fwd), the other way (bwd), or both.
    var pairs = [], index = {};
    M.edges.forEach(function (e) {
      var a = lift(e[0]), b = lift(e[1]), ka = itemKey(a), kb = itemKey(b);
      if (ka === kb || !rectOf[ka] || !rectOf[kb]) return;
      var rev = index[kb + '|' + ka] != null, key = rev ? kb + '|' + ka : ka + '|' + kb;
      if (index[key] == null) { index[key] = pairs.length; pairs.push({ a: a, b: b, n: 0, fwd: false, bwd: false }); }
      var pr = pairs[index[key]];
      pr.n++;
      if (rev) pr.bwd = true; else pr.fwd = true;
    });
    pairs.sort(function (p, q) { return itemCmp(p.a, q.a) || itemCmp(p.b, q.b); });
    var heads = frames.map(function (f) { return { x: f[1].x, y: f[1].y, w: f[1].w, h: FRAME_HEAD }; });
    var items = {}, list = [];
    pairs.forEach(function (p) { [p.a, p.b].forEach(function (it) { var k = itemKey(it); if (!items[k]) { items[k] = 1; list.push(it); } }); });
    list.sort(itemCmp);
    var focus = [], badges = [], hovered = [], hoveredBadges = [];
    list.forEach(function (f) {
      var kf = itemKey(f), ca = colOf(rectOf[kf].x), right = [], left = [];
      pairs.forEach(function (p) {
        var t = itemKey(p.a) === kf ? [p.b, p.n, p.fwd, p.bwd] : itemKey(p.b) === kf ? [p.a, p.n, p.bwd, p.fwd] : null;
        if (!t) return;
        (colOf(rectOf[itemKey(t[0])].x) >= ca ? right : left).push(t);
      });
      var weight = function (g) { return g.reduce(function (s, x) { return s + x[1]; }, 0); };
      var groups = [right, left].filter(function (g) { return g.length; })
        .map(function (g, i) { return [g, i]; })
        .sort(function (x, y) { return weight(y[0]) - weight(x[0]) || x[1] - y[1]; })
        .map(function (x) { return x[0]; });
      var wants = groups.map(function (g, k) { return { a: f, to: g, trunk: k }; });
      var r = route(wants, rectOf, heads, L === 'out', false);
      focus.push([f, r.lines]);
      if (r.badges.length) badges.push([f, r.badges]);
      var h = route(wants, rectOf, heads, L === 'out', true);
      hovered.push([f, h.lines]);
      if (h.badges.length) hoveredBadges.push([f, h.badges]);
    });
    return { focus: focus, badges: badges, hovered: hovered, hovered_badges: hoveredBadges };
  }

  function colOf(x) { return Math.max(0, Math.floor(x / (COL_W + COL_GAP))); }

  /* Rust: route. Trunks keep a clearance from cards, name bands and each
     other, turn only in gaps, and branch beside each column they reach. */
  function route(wants, rectOf, heads, out, hovered) {
    var clear = out ? 20 : 12, spread = out ? 24 : 14;
    // Hovered lines run beside the selection's (Rust: route's shifts).
    var laneShift = !hovered ? 0 : out ? 20 : 14, chanShift = !hovered ? 0 : out ? 10 : 6, rootShift = !hovered ? 0 : out ? 14 : 8;
    function centre(r) { return out ? r.y + r.h / 2 : r.y + Math.min(r.h, CARD_H) / 2; }
    // A left edge's lines meet it a little above the middle, a right
    // edge's a little below: no two heads back to back across a gutter.
    function port(r, rt) { var d = out ? r.h * 0.12 : 10; return centre(r) + (rt ? d : -d); }
    var paths = [];
    wants.forEach(function (w) {
      var ra = rectOf[itemKey(w.a)];
      if (!ra) return;
      var to = [];
      w.to.forEach(function (t) { var r = rectOf[itemKey(t[0])]; if (r) to.push([t[0], r, t[1], t[2], t[3]]); });
      if (!to.length) return;
      var ca = colOf(ra.x);
      paths.push({ a: w.a, ra: ra, to: to, right: to.every(function (t) { return colOf(t[1].x) >= ca; }), trunk: w.trunk, ya: centre(ra), yb: to.map(function (t) { return centre(t[1]); }), stops: [], lanes: [] });
    });
    function entersRight(p, j) { return !p.right || colOf(p.to[j][1].x) === colOf(p.ra.x); }
    var ends = {}, order = [], MAXJ = 1e9;
    function end(key, item, e) {
      if (!ends[key]) { ends[key] = { item: item, list: [] }; order.push(key); }
      ends[key].list.push(e);
    }
    paths.forEach(function (p, k) {
      var mean = p.to.reduce(function (s, t) { return s + centre(t[1]); }, 0) / p.to.length;
      end(itemKey(p.a) + (p.right ? 'R' : 'L'), p.a, [mean, k, MAXJ]);
      p.to.forEach(function (t, j) { end(itemKey(t[0]) + (entersRight(p, j) ? 'R' : 'L'), t[0], [centre(p.ra), k, j]); });
    });
    order.forEach(function (key) {
      var en = ends[key], r = rectOf[itemKey(en.item)];
      if (!r) return;
      var list = en.list.sort(function (x, y) { return x[0] - y[0] || x[1] - y[1] || x[2] - y[2]; }), n = list.length;
      var lo = r.y + 10, hi = bottom(r) - 10;
      var step = n <= 1 ? 0 : Math.min(spread, (hi - lo) / (n - 1));
      var half = step * Math.max(0, n - 1) / 2;
      var own = list.some(function (x) { return x[2] === MAXJ; });
      var at = own ? centre(r) + rootShift : port(r, key.charAt(key.length - 1) === 'R');
      var mid = Math.min(Math.max(at, lo + half), Math.max(hi - half, lo + half));
      list.forEach(function (x, k) { var y = mid - half + k * step; if (x[2] === MAXJ) paths[x[1]].ya = y; else paths[x[1]].yb[x[2]] = y; });
    });
    // A line alone at both ends between neighbouring columns runs straight
    // when its ends are within 16 points.
    function alone(key) { return ends[key] && ends[key].list.length === 1; }
    paths.forEach(function (p) {
      if (p.to.length !== 1) return;
      var t = p.to[0][0], r = p.to[0][1], ca = colOf(p.ra.x), tc = colOf(r.x);
      var near = tc === ca || tc === ca + 1 || tc + 1 === ca, er = !p.right || tc === ca;
      if (!near || !alone(itemKey(p.a) + (p.right ? 'R' : 'L')) || !alone(itemKey(t) + (er ? 'R' : 'L'))) return;
      var ya = p.ya, yb = p.yb[0];
      if (Math.abs(ya - yb) < 16) {
        if (ya > r.y + 10 && ya < bottom(r) - 10) p.yb[0] = ya;
        else if (yb > p.ra.y + 10 && yb < bottom(p.ra) - 10) p.ya = yb;
      }
    });
    var blocked = {};
    function block(c) { return blocked[c] || (blocked[c] = []); }
    Object.keys(rectOf).map(function (k) { return rectOf[k]; }).concat(heads).forEach(function (r) {
      for (var c = colOf(r.x); c <= colOf(right(r) - 1); c++) block(c).push([r.y, bottom(r)]);
    });
    function ok(no, y) { return !no.some(function (s) { return s[0] - clear < y && y < s[1] + clear; }); }
    function pick(no, y, want) {
      no.sort(function (a, b) { return a[0] - b[0]; });
      var free = [], from = -Infinity;
      no.forEach(function (s) { if (from > -Infinity && s[0] - from >= 2 * clear) free.push([from, s[0]]); from = Math.max(from, s[1]); });
      var best = null;
      function consider(p) {
        var cost = Math.abs(p - y) + Math.abs(p - want), near = Math.abs(p - want);
        if (!best || cost < best[0] - 1e-3 || (cost < best[0] + 1e-3 && near < best[1] - 1e-3)) best = [cost, near, p];
      }
      free.forEach(function (s) { consider((s[0] + s[1]) / 2 + Math.min(chanShift, (s[1] - s[0]) / 2 - clear)); });
      consider(Math.max(want, Math.max(from, 0) + clear * 1.8) + chanShift);
      return best ? best[2] : want;
    }
    paths.forEach(function (p) {
      var ca = colOf(p.ra.x), tcol = p.to.map(function (t) { return colOf(t[1].x); }), gaps = [], g, far;
      if (p.right) {
        far = Math.max(Math.max.apply(null, tcol), ca + 1);
        for (g = ca + 1; g <= far; g++) gaps.push([g, g === ca + 1 ? [g, ca] : [g], g < far ? g : null]);
      } else {
        far = Math.min.apply(null, tcol);
        for (g = ca; g >= far + 1; g--) gaps.push([g, [g - 1], g - 1 > far ? g - 1 : null]);
      }
      var y = p.ya;
      gaps.forEach(function (gp) {
        var js = [];
        tcol.forEach(function (t, j) { if (gp[1].indexOf(t) >= 0) js.push(j); });
        var to = y, c = gp[2];
        if (c != null) {
          var past = [];
          tcol.forEach(function (t, j) { if (p.right ? t > c : t < c) past.push(j); });
          var want = past.reduce(function (s, j) { return s + p.yb[j]; }, 0) / Math.max(1, past.length);
          if (!ok(block(c), y)) to = pick(block(c).slice(), y, want);
          block(c).push([to, to]);
        }
        if (js.length || to !== y) p.stops.push([gp[0], y, js, to]);
        y = to;
      });
      p.lanes = p.stops.map(function () { return 0; });
    });
    var byGap = {};
    paths.forEach(function (p, k) {
      p.stops.forEach(function (st, s) {
        var lo = Infinity, hi = -Infinity;
        st[2].map(function (j) { return p.yb[j]; }).concat([st[1], st[3]]).forEach(function (v) { lo = Math.min(lo, v); hi = Math.max(hi, v); });
        (byGap[st[0]] = byGap[st[0]] || []).push([k, s, hi - lo]);
      });
    });
    var laneStep = out ? 24 : 16;
    Object.keys(byGap).forEach(function (g) {
      var members = byGap[g].sort(function (x, y) { return y[2] - x[2] || x[0] - y[0] || x[1] - y[1]; });
      var sp = gapSpan(+g), n = members.length;
      var step = n <= 1 ? 0 : Math.min(laneStep, (sp[1] - sp[0]) / (n - 1));
      var start = (sp[0] + sp[1]) / 2 - step * Math.max(0, n - 1) / 2;
      members.forEach(function (m, k) { paths[m[0]].lanes[m[1]] = Math.min(start + k * step + laneShift, sp[1]); });
    });
    var lines = [], badges = [];
    paths.forEach(function (p) {
      var a0 = p.right ? right(p.ra) : p.ra.x, trunk = [[a0, p.ya]];
      // One badge per trunk, on its root (Rust: route).
      var n = p.to.reduce(function (a, t) { return a + t[2]; }, 0);
      if (n >= 2 && p.lanes.length) {
        var room = Math.min(Math.abs(p.lanes[0] - a0), COL_GAP / 2);
        badges.push({ x0: a0, x1: a0 + Math.sign(p.lanes[0] - a0) * room, y: p.ya, links: n, trunk: p.trunk, to: p.to.map(function (t) { return t[0]; }) });
      }
      p.stops.forEach(function (st, s) {
        var x = p.lanes[s];
        st[2].forEach(function (j) {
          var t = p.to[j], b0 = entersRight(p, j) ? right(t[1]) : t[1].x;
          var pts = straight(trunk.concat([[x, st[1]], [x, p.yb[j]], [b0, p.yb[j]]]));
          var x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
          pts.forEach(function (q) { x0 = Math.min(x0, q[0]); y0 = Math.min(y0, q[1]); x1 = Math.max(x1, q[0]); y1 = Math.max(y1, q[1]); });
          lines.push({ from: p.a, to: t[0], points: pts, bounds: { x: x0 - 6, y: y0 - 6, w: x1 - x0 + 12, h: y1 - y0 + 12 }, links: t[2], trunk: p.trunk, out: t[3], back: t[4] });
        });
        if (st[3] !== st[1]) { trunk.push([x, st[1]]); trunk.push([x, st[3]]); }
      });
    });
    return { lines: lines, badges: badges };
  }

  /* Rust: straight. Only corners and ends. */
  function straight(pts) {
    var v = [];
    pts.forEach(function (q) {
      var l = v[v.length - 1];
      if (l && Math.abs(l[0] - q[0]) < 0.01 && Math.abs(l[1] - q[1]) < 0.01) return;
      if (v.length >= 2) {
        var a = v[v.length - 2], b = v[v.length - 1];
        var level = Math.abs(a[1] - b[1]) < 0.01 && Math.abs(b[1] - q[1]) < 0.01;
        var plumb = Math.abs(a[0] - b[0]) < 0.01 && Math.abs(b[0] - q[0]) < 0.01;
        if (level || plumb) v.pop();
      }
      v.push(q);
    });
    return v;
  }

  /* A card's lines and badges as selected or as hovered (Rust: Layout::set). */
  function set(lay, focus, hovered) {
    if (!focus) return [[], []];
    var fk = itemKey(focus), lines = [], bs = [];
    (hovered ? lay.hovered : lay.focus || []).forEach(function (f) { if (itemKey(f[0]) === fk) lines = f[1]; });
    (hovered ? lay.hovered_badges : lay.badges || []).forEach(function (f) { if (itemKey(f[0]) === fk) bs = f[1]; });
    return [lines, bs];
  }
  /* The lines of `focus` under map point (x, y) at zoom z (Rust:
     Layout::lines_at): a badge stands for its trunk, else the nearest
     run's lines. */
  function linesAt(lay, focus, hovered, x, y, tol, z, st) {
    var sb = set(lay, focus, hovered), lines = sb[0], bs = sb[1], r = st.badge / Math.max(0.01, z);
    for (var i = 0; i < bs.length; i++) {
      var b = bs[i], c = badgeAt(st, b, z);
      if (c && Math.abs(c[0] - x) <= badgeHalf(st, b.links) / z && Math.abs(c[1] - y) <= r) {
        var to = b.to.map(itemKey);
        return lines.filter(function (l) { return l.trunk === b.trunk && to.indexOf(itemKey(l.to)) >= 0; });
      }
    }
    var hit = [];
    lines.forEach(function (l) {
      var d = Infinity;
      for (var k = 1; k < l.points.length; k++) {
        var a = l.points[k - 1], c2 = l.points[k];
        var px = Math.min(Math.max(x, Math.min(a[0], c2[0])), Math.max(a[0], c2[0]));
        var py = Math.min(Math.max(y, Math.min(a[1], c2[1])), Math.max(a[1], c2[1]));
        d = Math.min(d, Math.hypot(px - x, py - y));
      }
      if (d <= tol) hit.push([d, l]);
    });
    var best = hit.reduce(function (m, h) { return Math.min(m, h[0]); }, Infinity);
    return hit.filter(function (h) { return h[0] <= best + 0.5; }).map(function (h) { return h[1]; });
  }
  /* Every end of `lines` that meets a card (Rust: ends): a head where
     links arrive, else a dot; lit only at the targets of `hot` lines. */
  function ends(lines, hot) {
    var out = [];
    function key(p) { return Math.round(p[0] * 2) + '|' + Math.round(p[1] * 2); }
    function put(e) {
      for (var i = 0; i < out.length; i++) {
        if (key(out[i].tip) === key(e.tip)) {
          out[i].head = out[i].head || e.head;
          out[i].lit = out[i].lit || e.lit;
          if (e.head) out[i].from = e.from;
          return;
        }
      }
      out.push(e);
    }
    lines.forEach(function (l) {
      var n = l.points.length;
      if (n < 2) return;
      var lit = hot.some(function (h) { return h[0] === itemKey(l.from) && h[1] === itemKey(l.to); });
      put({ tip: l.points[n - 1], from: l.points[n - 2], head: !!l.out, lit: lit });
      put({ tip: l.points[0], from: l.points[1], head: !!l.back, lit: false });
    });
    return out;
  }
  /* The links lines stand for, once per card they reach (Rust: links_of). */
  function linksOf(lines) {
    var seen = {}, n = 0;
    lines.forEach(function (l) { var k = itemKey(l.to); if (!seen[k]) { seen[k] = 1; n += l.links; } });
    return n;
  }
  /* One step in or out of the style's zoom table (Rust: Style::step). */
  function step(st, z, dir, least) {
    var s = st.steps, next = null, i;
    if (dir > 0) { for (i = 0; i < s.length; i++) if (s[i] > z + 0.005) { next = s[i]; break; } if (next == null) next = MAX_ZOOM; }
    else { for (i = s.length - 1; i >= 0; i--) if (s[i] < z - 0.005) { next = s[i]; break; } if (next == null) next = least; }
    return Math.min(Math.max(next, Math.min(least, MAX_ZOOM)), MAX_ZOOM);
  }
  function badgeHalf(st, n) { return Math.max(st.badge, String(Math.max(1, n)).length * 3.4 + 5.5); }
  /* Where a badge is drawn at zoom z (Rust: Style::badge_at), or null. */
  function badgeAt(st, b, z) {
    var need = (badgeHalf(st, b.links) + st.badge_room) / Math.max(0.01, z), len = Math.abs(b.x1 - b.x0);
    if (len < need) return null;
    return [b.x0 + Math.sign(b.x1 - b.x0) * need, b.y];
  }

  /* The map's three layouts from `arr`, settled first. */
  function layout(M, arr) {
    var a = settle(M, arr), res = { arrangement: a };
    ['out', 'mid', 'in'].forEach(function (L) { res[L] = pack(M, L, a, blocksOf(M, L)); });
    return res;
  }

  /* `level` laid out from `arr` (Rust: preview). */
  function preview(M, arr, L) { return pack(M, L, settle(M, arr), blocksOf(M, L)); }

  /* The notes in every session, the entry note first (Rust: pinned_of). */
  function pinnedOf(M) {
    var p = [];
    M.notes.forEach(function (n, i) { if (n.flags.entry || n.flags.required) p.push(i); });
    p.sort(function (a, b) {
      var ea = M.notes[a].flags.entry ? 0 : 1, eb = M.notes[b].flags.entry ? 0 : 1;
      if (ea !== eb) return ea - eb;
      var ta = M.notes[a].title, tb = M.notes[b].title;
      return ta < tb ? -1 : ta > tb ? 1 : 0;
    });
    return p;
  }
  /* Hippo's layout in `cols` columns (Rust: default_arrangement_in). */
  function defaultIn(M, cols, mid) {
    cols = Math.max(1, cols);
    var columns = [], heights = [];
    for (var c = 0; c < cols; c++) { columns.push([]); heights.push(0); }
    var pinned = pinnedOf(M);
    pinned.forEach(function (i) { columns[0].push('note:' + M.notes[i].path); });
    if (pinned.length) heights[0] = pinned.length * (CARD_H + GAP) - GAP + FRAME_GAP;
    // The notes in every session keep the first column to themselves.
    var first = pinned.length && cols >= 2 ? 1 : 0;
    M.folders.forEach(function (f) {
      var key = 'folder:' + f.name, span = mid[key].span, per = Math.ceil(f.notes.length / span);
      span = Math.max(1, Math.min(span, cols - first));
      var best = [first, Infinity];
      for (var c2 = first; c2 <= cols - span; c2++) {
        var top = 0;
        for (var k = c2; k < c2 + span; k++) top = Math.max(top, heights[k]);
        if (top < best[1] - 0.5) best = [c2, top];
      }
      var h = FRAME_HEAD + per * (CARD_H + GAP) - GAP + FRAME_PAD;
      for (var k2 = best[0]; k2 < best[0] + span; k2++) heights[k2] = best[1] + h + FRAME_GAP;
      columns[best[0]].push(key);
    });
    return { columns: columns.filter(function (x) { return x.length; }) };
  }
  function fitZoom(wh, w, h) {
    return Math.min(Math.max(1, w - 2 * FIT_PAD) / Math.max(1, wh[0]), Math.max(1, h - 2 * FIT_PAD) / Math.max(1, wh[1]));
  }
  /* The arrangement that shows the whole map largest in a w × h view
     (Rust: view_arrangement). */
  function viewArrangement(M, w, h) {
    var out = blocksOf(M, 'out'), mid = blocksOf(M, 'mid'), n = allKeys(M.default_arrangement).length, best = null;
    for (var k = 1; k <= Math.max(1, n); k++) {
      var arr = defaultIn(M, k, mid);
      var zm = fitZoom(size(place(M, 'mid', arr, mid)), w, h), atMid = zm >= OUT_BELOW;
      var z = atMid ? zm : fitZoom(size(place(M, 'out', arr, out)), w, h);
      if (!best || (atMid && !best[0]) || (atMid === best[0] && z > best[1] + 1e-4)) best = [atMid, z, arr];
    }
    return best ? best[2] : M.default_arrangement;
  }
  /* The zoom and the map point at the view's top left that show the whole
     map (Rust: NotesMap::fit). */
  function fit(lay, w, h) {
    var midW = fitZoom([lay.mid.width, lay.mid.height], w, h) >= OUT_BELOW;
    var l = midW ? lay.mid : lay.out, most = midW ? 1 : OUT_FIT;
    var z = Math.max(MIN_ZOOM, Math.min(most, fitZoom([l.width, l.height], w, h)));
    return [z, [-Math.max((w / z - l.width) / 2, FIT_PAD / z), -Math.max((h / z - l.height) / 2, FIT_PAD / z)]];
  }

  /* The block whose handle is under map point (x, y) of level L. */
  function handleAt(M, lay, L, x, y) {
    function inR(r) { return x >= r.x && x < right(r) && y >= r.y && y < bottom(r); }
    for (var i = lay.cards.length - 1; i >= 0; i--) {
      var c = lay.cards[i];
      if (!inR(c.rect)) continue;
      if (L === 'out') return blockKey(M, c.item);
      return c.item.note != null && pinned(M, c.item.note) ? blockKey(M, c.item) : null;
    }
    for (var j = 0; j < lay.frames.length; j++) {
      var r = lay.frames[j][1];
      if (inR(r) && y < r.y + FRAME_HEAD) return blockKey(M, { folder: lay.frames[j][0] });
    }
    return null;
  }
  /* A block's outline in a layout. */
  function blockRect(M, lay, key) {
    for (var j = 0; j < lay.frames.length; j++) if (blockKey(M, { folder: lay.frames[j][0] }) === key) return lay.frames[j][1];
    for (var i = 0; i < lay.cards.length; i++) {
      var it = lay.cards[i].item;
      if ((it.folder != null || pinned(M, it.note)) && blockKey(M, it) === key) return lay.cards[i].rect;
    }
    return null;
  }
  /* Where `key` lands dropped at (x, y): [column, place, line y, [x0, x1],
     a new column at the right]. */
  function dropAt(M, lay, arr, key, x, y) {
    var n = arr.columns.length;
    var col = Math.min(n, Math.max(0, Math.floor((x + COL_GAP / 2) / (COL_W + COL_GAP))));
    var others = (arr.columns[col] || []).filter(function (k) { return k !== key; })
      .map(function (k) { return blockRect(M, lay, k); }).filter(Boolean);
    var at = others.filter(function (r) { return r.y + r.h / 2 < y; }).length;
    var ly = !others.length ? 0 : at === 0 ? others[0].y - FRAME_GAP / 2 : bottom(others[at - 1]) + FRAME_GAP / 2;
    return [col, at, ly, [colX(col), colX(col) + COL_W], col === n];
  }

  root.HippoArrange = {
    layout: layout, settle: settle, moveTo: moveTo, nudge: nudge, same: same, find: find,
    blockKey: blockKey, handleAt: handleAt, blockRect: blockRect, dropAt: dropAt,
    preview: preview, viewArrangement: viewArrangement, fit: fit,
    linesAt: linesAt, linksOf: linksOf, step: step, badgeHalf: badgeHalf, badgeAt: badgeAt, set: set, ends: ends, itemKey: itemKey
  };
})(typeof window !== 'undefined' ? window : globalThis);
