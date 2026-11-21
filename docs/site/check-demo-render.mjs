// Runs the live demo (docs/site/demo/demo.js on docs/site/demo/data.js) in
// Node with a stub page, for Morning and Brain in dark and light, and fails
// when an agent hippo knows is drawn without its own mark from
// brand/agents/, or when anything is drawn as an agent's letters.
//
//   node docs/site/check-demo-render.mjs <repository root>
//
// check-agent-marks.py checks the files; this checks what the page draws.
// In 0.7.1 the files were right and the page still drew a terminal for
// Claude Code: a later `var M = D.map` replaced the mark table.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import vm from 'node:vm';

const root = process.argv[2] || '.';
const read = (p) => readFileSync(join(root, p), 'utf8');

const src = read('crates/hippo-view/src/night/mod.rs');
const block = /pub const KNOWN[^=]*=\s*\[([\s\S]*?)\];/.exec(src);
const known = block
  ? [...block[1].matchAll(/\("([^"]+)",\s*"([^"]+)",\s*"([^"]+)",\s*"([^"]+)"\)/g)].map((m) => ({
      code: m[1], name: m[3], stem: m[4],
    }))
  : [];
if (known.length < 6) {
  console.error(`error: could not read KNOWN from hippo-view (found ${known.length})`);
  process.exit(1);
}
const squeeze = (s) => s.replace(/\s+/g, ' ').trim();
const inner = (svg) => {
  const m = /<svg[^>]*>([\s\S]*)<\/svg>/.exec(svg);
  return m ? squeeze(m[1]) : null;
};
const marks = Object.fromEntries(known.map((a) => [a.name, inner(read(`brand/agents/${a.stem}.svg`))]));

// A page that accepts whatever the demo asks of it.
function stubElement() {
  const el = {
    innerHTML: '',
    style: {},
    classList: { add() {}, remove() {}, toggle() {}, contains: () => false },
    clientWidth: 1280,
    clientHeight: 800,
    scrollWidth: 1280,
    scrollHeight: 800,
    children: [],
    setAttribute() {},
    getAttribute: () => null,
    hasAttribute: () => false,
    removeAttribute() {},
    addEventListener() {},
    removeEventListener() {},
    appendChild: (c) => c,
    removeChild: (c) => c,
    querySelector: () => stubElement(),
    querySelectorAll: () => [],
    getBoundingClientRect: () => ({ x: 0, y: 0, left: 0, top: 0, right: 100, bottom: 40, width: 100, height: 40 }),
    closest: () => null,
    focus() {},
    scrollTo() {},
    scrollIntoView() {},
    getContext: () => null,
  };
  return el;
}

const dataJs = read('docs/site/demo/data.js');
const demoJs = read('docs/site/demo/demo.js');
const arrangeJs = read('docs/site/demo/arrange.js');
const errors = [];
let drawn = 0;

for (const view of ['morning', 'brain']) {
  for (const theme of ['dark', 'light']) {
    const app = stubElement();
    const documentElement = stubElement();
    documentElement.getAttribute = (k) => (k === 'data-theme' ? theme : null);
    const window = {
      innerWidth: 1280,
      innerHeight: 800,
      devicePixelRatio: 2,
      addEventListener() {},
      removeEventListener() {},
      scrollTo() {},
      matchMedia: () => ({ matches: theme === 'dark', addEventListener() {}, addListener() {} }),
      requestAnimationFrame: (f) => { f(0); return 1; },
      cancelAnimationFrame() {},
      setTimeout: () => 0,
      clearTimeout() {},
      getComputedStyle: () => ({ getPropertyValue: () => '' }),
    };
    const context = {
      window,
      document: {
        getElementById: (id) => (id === 'app' ? app : stubElement()),
        documentElement,
        body: stubElement(),
        createElement: () => stubElement(),
        querySelector: () => stubElement(),
        querySelectorAll: () => [],
        addEventListener() {},
      },
      location: { hash: `#${view}` },
      history: { replaceState() {} },
      localStorage: { getItem: () => theme, setItem() {} },
      navigator: { userAgent: 'node' },
      console,
    };
    Object.assign(context, window);
    context.window = context;
    vm.createContext(context);
    try {
      vm.runInContext(dataJs, context, { filename: 'data.js' });
      vm.runInContext(arrangeJs, context, { filename: 'arrange.js' });
      vm.runInContext(demoJs, context, { filename: 'demo.js' });
    } catch (e) {
      errors.push(`${view}/${theme}: the demo threw: ${e.message}`);
      continue;
    }
    const html = app.innerHTML;
    if (!html) {
      errors.push(`${view}/${theme}: the demo drew nothing`);
      continue;
    }
    for (const m of html.matchAll(/<span class="mark" title="([^"]*)">(<svg[\s\S]*?<\/svg>)<\/span>/g)) {
      const name = m[1].replace(/&amp;/g, '&');
      if (!(name in marks)) continue;
      drawn += 1;
      if (inner(m[2]) !== marks[name]) {
        errors.push(`${view}/${theme}: ${name} is drawn without its mark (brand/agents/)`);
      }
    }
    for (const a of known) {
      const letters = new RegExp(`>\\s*${a.code.toUpperCase()}\\s*<|>\\s*${a.code[0].toUpperCase()}${a.code[1]}\\s*<`);
      if (letters.test(html)) errors.push(`${view}/${theme}: ${a.name} is drawn as letters`);
    }
    if (view === 'morning' && !/class="ask"[\s\S]*?class="who"><span class="mark" title="Claude Code">/.test(html)) {
      errors.push(`${view}/${theme}: the Ask bar's "Claude Code answers" chip has no Claude Code mark`);
    }
  }
}

// The demo's port of the arranger lays out the map Rust arranged
// (data.js's `arranged`, from hippo view map --arrangement) the same.
{
  const ctx = { globalThis: {}, window: {} };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(dataJs, ctx, { filename: 'data.js' });
  vm.runInContext(arrangeJs, ctx, { filename: 'arrange.js' });
  const D = ctx.window.HIPPO_DEMO, A = ctx.window.HippoArrange;
  if (!D || !D.arranged || !A) {
    errors.push('arrange: data.js has no arranged map or arrange.js did not load');
  } else {
    const got = A.layout(D.map, D.arranged.arrangement);
    const diff = (a, b, at) => {
      if (typeof a === 'number' && typeof b === 'number') return Math.abs(a - b) > 0.01 ? `${at}: ${a} != ${b}` : null;
      if (Array.isArray(a) && Array.isArray(b)) {
        if (a.length !== b.length) return `${at}: ${a.length} items != ${b.length}`;
        for (let i = 0; i < a.length; i++) { const d = diff(a[i], b[i], `${at}[${i}]`); if (d) return d; }
        return null;
      }
      if (a && b && typeof a === 'object' && typeof b === 'object') {
        for (const k of Object.keys(b)) { const d = diff(a[k], b[k], `${at}.${k}`); if (d) return d; }
        return null;
      }
      return a === b ? null : `${at}: ${JSON.stringify(a)} != ${JSON.stringify(b)}`;
    };
    for (const L of ['out', 'mid', 'in']) {
      const d = diff(got[L], D.arranged[L], `arranged.${L}`);
      if (d) errors.push(`arrange: the demo lays the arranged map out differently from hippo: ${d}`);
    }
    const da = diff(got.arrangement, D.arranged.arrangement, 'arrangement');
    if (da) errors.push(`arrange: ${da}`);
    // Hippo's layout for one view: the same columns, laid out the same.
    if (!D.fitted) {
      errors.push('fit: data.js has no fitted map');
    } else {
      const [w, h] = D.fitted.view;
      const arr = A.viewArrangement(D.map, w, h);
      const df = diff(arr, D.fitted.arrangement, 'fitted.arrangement');
      if (df) errors.push(`fit: the demo chooses other columns than hippo: ${df}`);
      const lay = A.layout(D.map, arr);
      for (const L of ['out', 'mid', 'in']) {
        const d = diff(lay[L], D.fitted[L], `fitted.${L}`);
        if (d) errors.push(`fit: the demo lays the fitted map out differently from hippo: ${d}`);
      }
    }
  }
}

// The demo steps through the zoom table hippo's style holds, as Hippo.app
// does (hippo_view::night::map::Style::step), and its badges count what its
// status says.
{
  const ctx = { globalThis: {}, window: {} };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(dataJs, ctx, { filename: 'data.js' });
  vm.runInContext(arrangeJs, ctx, { filename: 'arrange.js' });
  const D = ctx.window.HIPPO_DEMO, A = ctx.window.HippoArrange, st = D && D.map && D.map.style;
  if (!st || !Array.isArray(st.steps)) {
    errors.push('zoom: the map has no zoom table (style.steps)');
  } else {
    const fit = 0.44;
    const want = [[fit, 1, 0.5], [fit, -1, fit], [1, 1, 1.25], [1, -1, 0.85], [3, 1, 3]];
    for (const [z, dir, to] of want) {
      const got = A.step(st, z, dir, fit);
      if (Math.abs(got - to) > 1e-6) errors.push(`zoom: one step ${dir > 0 ? 'in' : 'out'} from ${z} goes to ${got}, Hippo.app to ${to}`);
    }
    if (!/HippoArrange\.step\(/.test(demoJs) || /\* \(act === 'zin' \? 1\.25/.test(demoJs)) {
      errors.push('zoom: demo.js does not step with HippoArrange.step');
    }
    for (const L of ['out', 'mid', 'in']) {
      const lay = D.map[L];
      // One badge per trunk, on its root; the badge and the run under it
      // give one number, at every zoom of the level.
      const zs = st.steps.filter((z) => (L === 'out' ? z < st.out_below : L === 'in' ? z > st.in_above : z >= st.out_below && z <= st.in_above));
      for (const hovered of [false, true]) {
        for (const [f, bs] of (hovered ? lay.hovered_badges : lay.badges) || []) {
          const [lines] = A.set(lay, f, hovered);
          for (const b of bs) {
            const mine = lines.filter((l) => l.trunk === b.trunk);
            if (A.linksOf(mine) !== b.links) errors.push(`badge: ${L} badge says ${b.links}, its trunk ${A.linksOf(mine)}`);
            for (const z of zs) {
              const c = A.badgeAt(st, b, z);
              if (!c) continue;
              const n = A.linksOf(A.linesAt(lay, f, hovered, c[0], c[1], 6 / z, z, st));
              if (n !== b.links) errors.push(`badge: ${L} badge says ${b.links}, hovering it ${n}`);
              const need = (A.badgeHalf(st, b.links) + st.badge_room) / z;
              for (const card of lay.cards) {
                const r = card.rect;
                const dx = Math.max(r.x - c[0], 0, c[0] - r.x - r.w), dy = Math.max(r.y - c[1], 0, c[1] - r.y - r.h);
                if (Math.hypot(dx, dy) < need - 0.5) errors.push(`badge: ${L} badge at ${c} is in a card at ${z}`);
              }
            }
          }
        }
      }
      for (const [, ls] of lay.focus) {
        for (const l of ls) if (l.out === l.back) errors.push(`line: ${L} line with no one way (out or in)`);
      }
      // Every line that leaves a view ends in an edge chip, at every zoom of
      // the level, in views centred on a few cards.
      for (const z of zs) {
        const [w, h] = [860 / z, 600 / z];
        for (const card of lay.cards.filter((_, i) => i % 4 === 0)) {
          const v = { x: card.rect.x + card.rect.w / 2 - w / 2, y: card.rect.y + Math.min(card.rect.h, 112) / 2 - h / 2, w, h };
          for (const [f, ls] of lay.focus) {
            const shown = ls.filter((l) => A.shows(l, v));
            const chips = A.exits(shown, v, 8 / z, 28 / z);
            for (const l of shown) {
              const e = l.points[l.points.length - 1];
              const out = e[0] < v.x || e[0] > v.x + v.w || e[1] < v.y || e[1] > v.y + v.h;
              if (out && !chips.some((c) => c.to.some((t) => A.itemKey(t) === A.itemKey(l.to)))) {
                errors.push(`chip: ${L} at ${z}, a line from ${A.itemKey(f)} leaves the view with no chip`);
              }
            }
          }
        }
      }
    }
  }
}

if (errors.length) {
  for (const e of [...new Set(errors)]) console.error(`error: ${e}`);
  process.exit(1);
}
console.log(`demo render: Morning and Brain, dark and light, ${drawn} agent marks drawn, each its own (brand/agents/), no letters; the arranged and fitted maps laid out as hippo does`);
