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

if (errors.length) {
  for (const e of [...new Set(errors)]) console.error(`error: ${e}`);
  process.exit(1);
}
console.log(`demo render: Morning and Brain, dark and light, ${drawn} agent marks drawn, each its own (brand/agents/), no letters`);
