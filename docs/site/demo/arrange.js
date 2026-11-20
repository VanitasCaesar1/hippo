/* Arranging Brain in the live demo: a port of hippo_view::night::map's
   arrangement, packer and link router (docs/designs/brain-arrange.md).
   Each block (a folder, or a note read in every session) is moved with its
   own cards, words and marks from the default layout that hippo wrote into
   data.js; the links are routed again. docs/site/check-demo-render.mjs
   checks this port against a map Rust arranged. Nothing is saved. */
(function (root) {
  'use strict';
  var CARD_H = 112, GAP = 12, FRAME_PAD = 14, FRAME_HEAD = 40, FRAME_GAP = 24,
    COL_GAP = 72, CARD_W = 340, COL_W = CARD_W + 2 * FRAME_PAD, OUT_GAP = 40,
    LINE_BUDGET = 12, OUT_BELOW = 0.55, OUT_FIT = 0.54, MIN_ZOOM = 0.15, FIT_PAD = 28;

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
        for (var cc = c; cc < c + span; cc++) if (foot[cc]) top = Math.max(top, foot[cc][0] + gap(foot[cc][1], d.pinned));
        if (!best || top < best[1] - 0.5) best = [c, top];
      });
      if (!best) break;
      var c = best[0], top = best[1], d = blocks[cols[c].pop()];
      var dx = colX(c) - d.ox, dy = top - d.oy, span = L === 'out' ? 1 : d.span;
      for (var cc = c; cc < c + span; cc++) foot[cc] = [top + d.h, d.pinned];
      d.frames.forEach(function (f) { frames.push([f[0], mv(f[1], dx, dy)]); });
      d.cards.forEach(function (x) { cards.push({ item: x.item, rect: mv(x.rect, dx, dy) }); });
      d.labels.forEach(function (x) { labels.push({ item: x.item, kind: x.kind, rect: mv(x.rect, dx, dy), lines: x.lines, size: x.size }); });
      d.marks.forEach(function (x) { marks.push({ item: x.item, x: x.x + dx, y: x.y + dy, d: x.d, tone: x.tone }); });
    }
    return { frames: frames, cards: cards, labels: labels, marks: marks };
  }
  function size(p, lines) {
    var w = 0, h = 0;
    p.cards.forEach(function (x) { w = Math.max(w, right(x.rect)); h = Math.max(h, bottom(x.rect)); });
    p.frames.forEach(function (f) { w = Math.max(w, right(f[1])); h = Math.max(h, bottom(f[1])); });
    (lines || []).forEach(function (l) { w = Math.max(w, right(l.bounds)); h = Math.max(h, bottom(l.bounds)); });
    return [w + FRAME_PAD, h];
  }

  function pack(M, L, arr, blocks) {
    var p = place(M, L, arr, blocks), lk = links(M, L, p.frames, p.cards), wh = size(p, lk[0]);
    return { width: wh[0], height: wh[1], frames: p.frames, cards: p.cards, lines: lk[0], focus: lk[1], labels: p.labels, marks: p.marks };
  }

  function kindOf(ra, rb) {
    if (ra.x < right(rb) && rb.x < right(ra)) return 'bracket';
    return ra.x < rb.x ? 'right' : 'left';
  }
  var SIDE = { bracket: 0, right: 1, left: 2 };

  /* One level's lines (Rust: links): every line with at most LINE_BUDGET
     pairs, and each linked card's own, one trunk each way. */
  function links(M, L, frames, cards) {
    var rectOf = {};
    cards.forEach(function (x) { rectOf[itemKey(x.item)] = x.rect; });
    function lift(i) {
      if (L !== 'out') return { note: i };
      var f = folderOf(M, i);
      return f >= 0 ? { folder: f } : { note: i };
    }
    var pairs = [], index = {};
    M.edges.forEach(function (e) {
      var a = lift(e[0]), b = lift(e[1]), ka = itemKey(a), kb = itemKey(b);
      if (ka === kb || !rectOf[ka] || !rectOf[kb]) return;
      var key = index[kb + '|' + ka] != null ? kb + '|' + ka : ka + '|' + kb;
      if (index[key] == null) { index[key] = pairs.length; pairs.push({ a: key === ka + '|' + kb ? a : b, b: key === ka + '|' + kb ? b : a, n: 0 }); }
      pairs[index[key]].n++;
    });
    pairs.sort(function (p, q) { return itemCmp(p.a, q.a) || itemCmp(p.b, q.b); });
    var heads = frames.map(function (f) { return { x: f[1].x, y: f[1].y, w: f[1].w, h: FRAME_HEAD }; });
    var out = L === 'out';
    var lines = pairs.length <= LINE_BUDGET ? route(pairs.map(function (p, k) { return { a: p.a, to: [[p.b, p.n]], trunk: k }; }), rectOf, heads, out) : [];
    var items = {}, list = [];
    pairs.forEach(function (p) { [p.a, p.b].forEach(function (it) { var k = itemKey(it); if (!items[k]) { items[k] = 1; list.push(it); } }); });
    list.sort(itemCmp);
    var focus = [];
    list.forEach(function (f) {
      var kf = itemKey(f), rf = rectOf[kf], groups = [[], [], []];
      pairs.forEach(function (p) {
        var o = itemKey(p.a) === kf ? p.b : itemKey(p.b) === kf ? p.a : null;
        if (!o) return;
        groups[SIDE[kindOf(rf, rectOf[itemKey(o)])]].push([o, p.n]);
      });
      groups = groups.filter(function (g) { return g.length; });
      var weight = function (g) { return g.reduce(function (s, x) { return s + x[1]; }, 0); };
      groups = groups.map(function (g, i) { return [g, i]; }).sort(function (x, y) { return weight(y[0]) - weight(x[0]) || x[1] - y[1]; }).map(function (x) { return x[0]; }).slice(0, LINE_BUDGET);
      focus.push([f, route(groups.map(function (g, k) { return { a: f, to: g, trunk: k }; }), rectOf, heads, out)]);
    });
    return [lines, focus];
  }

  function colOf(x) { return Math.max(0, Math.floor(x / (COL_W + COL_GAP))); }

  /* Rust: route. Lines turn only in the gaps between columns; a trunk
     branches off beside each column it reaches. */
  function route(wants, rectOf, heads, out) {
    function centre(r) { return out ? r.y + r.h / 2 : r.y + Math.min(r.h, CARD_H) / 2; }
    var spread = out ? 24 : 14;
    var paths = [];
    wants.forEach(function (w) {
      var ra = rectOf[itemKey(w.a)];
      if (!ra) return;
      var to = [];
      w.to.forEach(function (t) { var r = rectOf[itemKey(t[0])]; if (r) to.push([t[0], r, t[1]]); });
      if (!to.length) return;
      paths.push({ a: w.a, ra: ra, to: to, kind: kindOf(ra, to[0][1]), trunk: w.trunk, ya: centre(ra), yb: to.map(function (t) { return centre(t[1]); }), runs: [], lanes: [] });
    });
    var ends = {}, order = [], MAXJ = 1e9;
    function end(key, item, e) {
      if (!ends[key]) { ends[key] = { item: item, list: [] }; order.push(key); }
      ends[key].list.push(e);
    }
    paths.forEach(function (p, k) {
      var mean = p.to.reduce(function (s, t) { return s + centre(t[1]); }, 0) / p.to.length;
      end(itemKey(p.a) + (p.kind !== 'left' ? 'R' : 'L'), p.a, [mean, k, MAXJ]);
      p.to.forEach(function (t, j) { end(itemKey(t[0]) + (p.kind !== 'right' ? 'R' : 'L'), t[0], [centre(p.ra), k, j]); });
    });
    order.forEach(function (key) {
      var en = ends[key], r = rectOf[itemKey(en.item)];
      if (!r) return;
      var list = en.list.sort(function (x, y) { return x[0] - y[0] || x[1] - y[1] || x[2] - y[2]; }), n = list.length;
      var lo = r.y + 10, hi = bottom(r) - 10;
      var step = n <= 1 ? 0 : Math.min(spread, (hi - lo) / (n - 1));
      var half = step * Math.max(0, n - 1) / 2;
      var mid = Math.min(Math.max(centre(r), lo + half), Math.max(hi - half, lo + half));
      list.forEach(function (x, k) { var y = mid - half + k * step; if (x[2] === MAXJ) paths[x[1]].ya = y; else paths[x[1]].yb[x[2]] = y; });
    });
    var blocked = {};
    function block(c) { return blocked[c] || (blocked[c] = []); }
    Object.keys(rectOf).map(function (k) { return rectOf[k]; }).concat(heads).forEach(function (r) {
      for (var c = colOf(r.x); c <= colOf(right(r) - 1); c++) block(c).push([r.y - 4, bottom(r) + 4]);
    });
    function clear(no, y) { return !no.some(function (s) { return s[0] < y && y < s[1]; }); }
    function pick(no, y, want) {
      no.sort(function (a, b) { return a[0] - b[0]; });
      var free = [], from = 0;
      no.forEach(function (s) { if (s[0] > from) free.push([from, s[0]]); from = Math.max(from, s[1]); });
      free.push([from, Infinity]);
      var best = null;
      free.forEach(function (s) {
        var pad = Math.min(14, (s[1] - s[0]) / 2);
        var p = Math.min(Math.max(want, s[0] + pad), Math.max(s[1] - pad, s[0] + pad));
        var cost = Math.abs(p - y) + Math.abs(p - want), near = Math.abs(p - want);
        if (!best || cost < best[0] - 1e-3 || (cost < best[0] + 1e-3 && near < best[1] - 1e-3)) best = [cost, near, p];
      });
      return best ? best[2] : want;
    }
    paths.forEach(function (p) {
      var ca = colOf(p.ra.x), tcol = p.to.map(function (t) { return colOf(t[1].x); });
      var far = p.kind === 'bracket' ? ca : p.kind === 'right' ? Math.max.apply(null, tcol) : Math.min.apply(null, tcol);
      var steps = [], last, c;
      if (p.kind === 'bracket') last = ca + 1;
      else if (p.kind === 'right') { for (c = ca + 1; c < far; c++) steps.push([c, c]); last = far; }
      else { for (c = ca - 1; c > far; c--) steps.push([c, c + 1]); last = far + 1; }
      function here(cc) { var js = []; tcol.forEach(function (t, j) { if (t === cc) js.push(j); }); return js; }
      function beyond(cc) { var js = []; tcol.forEach(function (t, j) { if (p.kind === 'right' ? t > cc : p.kind === 'left' ? t < cc : false) js.push(j); }); return js; }
      function branch(g, y, js) {
        var lo = y, hi = y;
        js.forEach(function (j) { lo = Math.min(lo, p.yb[j]); hi = Math.max(hi, p.yb[j]); });
        return { gap: g, y: y, to: y, span: hi - lo, targets: js };
      }
      var y = p.ya;
      steps.forEach(function (st) {
        var cc = st[0], g = st[1], js = here(cc);
        if (js.length) p.runs.push(branch(g, y, js));
        var past = beyond(cc);
        var want = past.reduce(function (s, j) { return s + p.yb[j]; }, 0) / Math.max(1, past.length);
        if (!clear(block(cc), y)) {
          var y2 = pick(block(cc).slice(), y, want);
          p.runs.push({ gap: g, y: y, to: y2, span: Math.abs(y2 - y), targets: null });
          y = y2;
        }
        block(cc).push([y - 4, y + 4]);
      });
      p.runs.push(branch(last, y, here(far)));
      p.lanes = p.runs.map(function () { return 0; });
    });
    var byGap = {};
    paths.forEach(function (p, k) { p.runs.forEach(function (r, j) { (byGap[r.gap] = byGap[r.gap] || []).push([k, j, r.span]); }); });
    Object.keys(byGap).forEach(function (g) {
      var members = byGap[g].sort(function (x, y) {
        var bx = paths[x[0]].kind === 'bracket', by = paths[y[0]].kind === 'bracket';
        if (bx !== by) return bx ? -1 : 1;
        var d = bx ? x[2] - y[2] : y[2] - x[2];
        return d || x[0] - y[0] || x[1] - y[1];
      });
      var s = gapSpan(+g), n = members.length;
      var step = n <= 1 ? 0 : Math.min(8, (s[1] - s[0]) / (n - 1));
      var start = (s[0] + s[1]) / 2 - step * Math.max(0, n - 1) / 2;
      members.forEach(function (m, k) { paths[m[0]].lanes[m[1]] = start + k * step; });
    });
    var lines = [];
    paths.forEach(function (p) {
      var trunk = [[p.kind === 'left' ? p.ra.x : right(p.ra), p.ya]];
      p.runs.forEach(function (r, k) {
        var x = p.lanes[k];
        if (!r.targets) { trunk.push([x, r.y]); trunk.push([x, r.to]); return; }
        r.targets.forEach(function (j) {
          var t = p.to[j], b0 = p.kind === 'right' ? t[1].x : right(t[1]);
          var pts = trunk.concat([[x, r.y], [x, p.yb[j]], [b0, p.yb[j]]]);
          var x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
          pts.forEach(function (q) { x0 = Math.min(x0, q[0]); y0 = Math.min(y0, q[1]); x1 = Math.max(x1, q[0]); y1 = Math.max(y1, q[1]); });
          lines.push({ from: p.a, to: t[0], points: pts, bounds: { x: x0 - 6, y: y0 - 6, w: x1 - x0 + 12, h: y1 - y0 + 12 }, links: t[2], trunk: p.trunk });
        });
      });
    });
    return lines;
  }

  /* The map's three layouts from `arr`, settled first. */
  function layout(M, arr) {
    var a = settle(M, arr), res = { arrangement: a };
    ['out', 'mid', 'in'].forEach(function (L) { res[L] = pack(M, L, a, blocksOf(M, L)); });
    return res;
  }

  /* `level` laid out from `arr` without its lines (Rust: preview). */
  function preview(M, arr, L) {
    var p = place(M, L, settle(M, arr), blocksOf(M, L)), wh = size(p, []);
    return { width: wh[0], height: wh[1], frames: p.frames, cards: p.cards, lines: [], focus: [], labels: p.labels, marks: p.marks };
  }

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
    M.folders.forEach(function (f) {
      var key = 'folder:' + f.name, span = mid[key].span, per = Math.ceil(f.notes.length / span);
      span = Math.min(span, cols);
      var best = [0, Infinity];
      for (var c2 = 0; c2 <= cols - span; c2++) {
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
    preview: preview, viewArrangement: viewArrangement, fit: fit, LINE_BUDGET: LINE_BUDGET
  };
})(typeof window !== 'undefined' ? window : globalThis);
