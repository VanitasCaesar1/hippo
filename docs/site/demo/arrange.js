/* Arranging Brain in the live demo: a port of hippo_view::night::map's
   arrangement, packer and link router (docs/designs/brain-arrange.md).
   Each block (a folder, or a note read in every session) is moved with its
   own cards, words and marks from the default layout that hippo wrote into
   data.js; the links are routed again. docs/site/check-demo-render.mjs
   checks this port against a map Rust arranged. Nothing is saved. */
(function (root) {
  'use strict';
  var CARD_H = 112, GAP = 12, FRAME_PAD = 14, FRAME_HEAD = 40, FRAME_GAP = 24,
    COL_GAP = 72, CARD_W = 340, COL_W = CARD_W + 2 * FRAME_PAD, OUT_GAP = 40;

  function colX(c) { return c * (COL_W + COL_GAP); }
  function right(r) { return r.x + r.w; }
  function bottom(r) { return r.y + r.h; }
  function gapSpan(g) { var r = colX(g) - 6; return [r - (COL_GAP - 12), r]; }
  function gapAfter(x) { return Math.max(0, Math.floor(x / (COL_W + COL_GAP))) + 1; }
  // Rust's f32::round rounds half away from zero; x is never negative here.
  function gapBefore(x) { return Math.max(0, Math.floor(x / (COL_W + COL_GAP) + 0.5)); }
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

  function pack(M, L, arr, blocks) {
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
    var rectOf = {}, itemOf = {};
    cards.forEach(function (x) { rectOf[itemKey(x.item)] = x.rect; itemOf[itemKey(x.item)] = x.item; });
    function lift(i) {
      if (L !== 'out') return { note: i };
      var f = folderOf(M, i);
      return f >= 0 ? { folder: f } : { note: i };
    }
    var pairs = [], index = {};
    M.edges.forEach(function (e) {
      var a = lift(e[0]), b = lift(e[1]), ka = itemKey(a), kb = itemKey(b);
      if (ka === kb) return;
      var key = index[kb + '|' + ka] != null ? kb + '|' + ka : ka + '|' + kb;
      if (index[key] == null) { index[key] = pairs.length; pairs.push({ a: key === ka + '|' + kb ? a : b, b: key === ka + '|' + kb ? b : a, n: 0 }); }
      pairs[index[key]].n++;
    });
    pairs.sort(function (p, q) { return itemCmp(p.a, q.a) || itemCmp(p.b, q.b); });
    var lines = route(pairs, rectOf);
    var w = 0, h = 0;
    cards.forEach(function (x) { w = Math.max(w, right(x.rect)); h = Math.max(h, bottom(x.rect)); });
    frames.forEach(function (f) { w = Math.max(w, right(f[1])); h = Math.max(h, bottom(f[1])); });
    lines.forEach(function (l) { w = Math.max(w, right(l.bounds)); h = Math.max(h, bottom(l.bounds)); });
    return { width: w + FRAME_PAD, height: h, frames: frames, cards: cards, lines: lines, labels: labels, marks: marks };
  }

  function route(pairs, rectOf) {
    function centre(r) { return r.y + Math.min(r.h, CARD_H) / 2; }
    var plans = [];
    pairs.forEach(function (p) {
      var ra = rectOf[itemKey(p.a)], rb = rectOf[itemKey(p.b)];
      if (!ra || !rb) return;
      var overlap = ra.x < right(rb) && rb.x < right(ra), kind, gap;
      if (overlap) { kind = 'bracket'; gap = gapAfter(Math.max(right(ra), right(rb))); }
      else if (ra.x < rb.x) { kind = 'right'; gap = gapBefore(rb.x); }
      else { kind = 'left'; gap = gapBefore(ra.x); }
      plans.push({ a: p.a, b: p.b, n: p.n, ra: ra, rb: rb, kind: kind, ya: centre(ra), yb: centre(rb), gap: gap, lane: 0, via: null });
    });
    var ends = {}, order = [];
    plans.forEach(function (p, k) {
      var aR = p.kind !== 'left', bR = p.kind !== 'right';
      [[itemKey(p.a) + (aR ? 'R' : 'L'), p.a, centre(p.rb), true], [itemKey(p.b) + (bR ? 'R' : 'L'), p.b, centre(p.ra), false]].forEach(function (e) {
        if (!ends[e[0]]) { ends[e[0]] = { item: e[1], list: [] }; order.push(e[0]); }
        ends[e[0]].list.push([e[2], k, e[3]]);
      });
    });
    order.forEach(function (key) {
      var en = ends[key], r = rectOf[itemKey(en.item)];
      if (!r) return;
      var list = en.list.sort(function (x, y) { return x[0] - y[0] || x[1] - y[1]; }), n = list.length;
      var lo = r.y + 10, hi = bottom(r) - 10;
      var step = n <= 1 ? 0 : Math.min(7, (hi - lo) / (n - 1));
      var half = step * Math.max(0, n - 1) / 2;
      var mid = Math.min(Math.max(centre(r), lo + half), Math.max(hi - half, lo + half));
      list.forEach(function (x, k) { var y = mid - half + k * step; if (x[2]) plans[x[1]].ya = y; else plans[x[1]].yb = y; });
    });
    var rects = Object.keys(rectOf).map(function (k) { return rectOf[k]; });
    function crosses(x0, x1, y) {
      return rects.some(function (r) { return r.x < x1 - 0.5 && x0 + 0.5 < right(r) && r.y - 4 < y && y < bottom(r) + 4; });
    }
    var taken = [];
    plans.forEach(function (p) {
      if (p.kind === 'bracket') return;
      var nearB, x0, x1, y;
      if (p.kind === 'right') { nearB = gapAfter(right(p.ra)); x0 = right(p.ra); x1 = p.rb.x; y = p.ya; }
      else { nearB = gapAfter(right(p.rb)); x0 = right(p.rb); x1 = p.ra.x; y = p.yb; }
      if (nearB === p.gap || !crosses(x0, x1, y)) return;
      var ga = p.kind === 'right' ? nearB : p.gap, gb = p.kind === 'right' ? p.gap : nearB;
      var cx0 = gapSpan(Math.min(ga, gb))[0], cx1 = gapSpan(Math.max(ga, gb))[1];
      var no = rects.filter(function (r) { return r.x < cx1 && cx0 < right(r); }).map(function (r) { return [r.y - 4, bottom(r) + 4]; });
      taken.forEach(function (t) { if (t[0] < cx1 && cx0 < t[1]) no.push([t[2] - 4, t[2] + 4]); });
      var want = (p.ya + p.yb) / 2;
      no.sort(function (a, b) { return a[0] - b[0]; });
      var free = [], from = 0;
      no.forEach(function (s) { if (s[0] > from) free.push([from, s[0]]); from = Math.max(from, s[1]); });
      free.push([from, Infinity]);
      var yc = want, bestD = Infinity;
      free.forEach(function (s) {
        var pad = Math.min(14, (s[1] - s[0]) / 2), c = Math.min(Math.max(want, s[0] + pad), Math.max(s[1] - pad, s[0] + pad));
        if (Math.abs(c - want) < bestD) { bestD = Math.abs(c - want); yc = c; }
      });
      taken.push([cx0, cx1, yc]);
      p.gap = ga; p.via = { gb: gb, yc: yc, x2: 0 };
    });
    var byGap = {};
    plans.forEach(function (p, k) {
      function add(g, second, span) { (byGap[g] = byGap[g] || []).push([k, second, span]); }
      if (!p.via) add(p.gap, false, Math.abs(p.ya - p.yb));
      else { add(p.gap, false, Math.abs(p.ya - p.via.yc)); add(p.via.gb, true, Math.abs(p.via.yc - p.yb)); }
    });
    Object.keys(byGap).forEach(function (g) {
      var members = byGap[g].sort(function (x, y) {
        var bx = plans[x[0]].kind === 'bracket', by = plans[y[0]].kind === 'bracket';
        if (bx !== by) return bx ? -1 : 1;
        var d = bx ? x[2] - y[2] : y[2] - x[2];
        return d || x[0] - y[0] || (x[1] ? 1 : 0) - (y[1] ? 1 : 0);
      });
      var s = gapSpan(+g), n = members.length;
      var step = n <= 1 ? 0 : Math.min(8, (s[1] - s[0]) / (n - 1));
      var start = (s[0] + s[1]) / 2 - step * Math.max(0, n - 1) / 2;
      members.forEach(function (m, k) { var x = start + k * step; if (m[1]) plans[m[0]].via.x2 = x; else plans[m[0]].lane = x; });
    });
    return plans.map(function (p) {
      var x = p.lane, pts;
      if (p.kind === 'bracket') pts = [[right(p.ra), p.ya], [x, p.ya], [x, p.yb], [right(p.rb), p.yb]];
      else if (!p.via) pts = p.kind === 'right' ? [[right(p.ra), p.ya], [x, p.ya], [x, p.yb], [p.rb.x, p.yb]] : [[p.ra.x, p.ya], [x, p.ya], [x, p.yb], [right(p.rb), p.yb]];
      else {
        var v = p.via, a0 = p.kind === 'right' ? right(p.ra) : p.ra.x, b0 = p.kind === 'right' ? p.rb.x : right(p.rb);
        pts = [[a0, p.ya], [x, p.ya], [x, v.yc], [v.x2, v.yc], [v.x2, p.yb], [b0, p.yb]];
      }
      var x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
      pts.forEach(function (q) { x0 = Math.min(x0, q[0]); y0 = Math.min(y0, q[1]); x1 = Math.max(x1, q[0]); y1 = Math.max(y1, q[1]); });
      return { from: p.a, to: p.b, points: pts, bounds: { x: x0 - 6, y: y0 - 6, w: x1 - x0 + 12, h: y1 - y0 + 12 }, links: p.n };
    });
  }

  /* The map's three layouts from `arr`, settled first. */
  function layout(M, arr) {
    var a = settle(M, arr), res = { arrangement: a };
    ['out', 'mid', 'in'].forEach(function (L) { res[L] = pack(M, L, a, blocksOf(M, L)); });
    return res;
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
    blockKey: blockKey, handleAt: handleAt, blockRect: blockRect, dropAt: dropAt
  };
})(typeof window !== 'undefined' ? window : globalThis);
