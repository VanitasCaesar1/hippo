// The night map's colours are brand/tokens.json. Fails when Hippo.app's
// theme (app/hippo-app/src/ui/night/kit.rs, Nt) or the live demo's CSS
// (docs/site/demo/demo.css) differs from it, so the app and the web draw
// the same map.
//
//   node docs/site/check-tokens.mjs <repository root>

import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const root = process.argv[2] || '.';
const read = (p) => readFileSync(join(root, p), 'utf8');
const tokens = JSON.parse(read('brand/tokens.json'));

// Any colour as [r, g, b, a] with a in hundredths.
function rgba(v) {
  v = v.trim().replace(/\s+/g, '');
  let m = /^#([0-9a-f]{6})$/i.exec(v);
  if (m) return [...[0, 2, 4].map((i) => parseInt(m[1].slice(i, i + 2), 16)), 100];
  m = /^rgba\((\d+),(\d+),(\d+),([0-9.]+)\)$/i.exec(v);
  if (m) return [+m[1], +m[2], +m[3], Math.round(+m[4] * 100)];
  m = /^c\(0x([0-9a-f]{6})\)$/i.exec(v);
  if (m) return rgba('#' + m[1]);
  m = /^ca\(0x([0-9a-f]{6}),([0-9.]+)\)$/i.exec(v);
  if (m) { const c = rgba('#' + m[1]); c[3] = Math.round(+m[2] * 100); return c; }
  return null;
}
const same = (a, b) => a && b && a.every((x, i) => x === b[i]);
const errors = [];

// Hippo.app: the dark block, then the light block of Nt::of.
const kit = read('app/hippo-app/src/ui/night/kit.rs');
const start = kit.indexOf('pub fn of(look: &Look) -> Nt');
const blocks = kit.slice(start).split('} else {')[0] + '\n@@\n' + kit.slice(start).split('} else {')[1];
const [darkSrc, lightSrc] = blocks.split('@@');
const app = { dark: {}, light: {} };
for (const [theme, src] of [['dark', darkSrc], ['light', lightSrc]]) {
  for (const m of src.matchAll(/^\s*([a-z_0-9]+): (ca?\(0x[0-9A-Fa-f]{6}(?:, [0-9.]+)?\)),/gm)) {
    app[theme][m[1].replace(/_/g, '-').replace(/^line-2$/, 'line2')] = m[2];
  }
}
// The stroke lime is a method: the fill in dark, a darker lime in light.
const stroke = /fn you_stroke[\s\S]*?rgba\(0x([0-9A-Fa-f]{6})FF\)/.exec(kit);
app.dark['you-stroke'] = 'c(0xC8F031)';
if (stroke) app.light['you-stroke'] = `c(0x${stroke[1]})`;

// The demo: :root (dark) and :root[data-theme="light"].
const css = read('docs/site/demo/demo.css');
const vars = (block) => Object.fromEntries([...block.matchAll(/--([a-z0-9-]+):\s*([^;]+);/g)].map((m) => [m[1], m[2]]));
const darkCss = vars(/:root \{([\s\S]*?)\n\}/.exec(css)[1]);
const lightCss = vars(/:root\[data-theme="light"\] \{([\s\S]*?)\n\}/.exec(css)[1]);
const lightMedia = vars(/@media \(prefers-color-scheme: light\) \{\s*:root:not\(\[data-theme="dark"\]\) \{([\s\S]*?)\n  \}/.exec(css)[1]);
const web = { dark: darkCss, light: lightCss };

let checked = 0;
for (const theme of ['dark', 'light']) {
  for (const [name, want] of Object.entries(tokens[theme])) {
    const w = rgba(want);
    const a = app[theme][name], c = web[theme][name];
    if (!same(rgba(a || ''), w)) errors.push(`${theme} ${name}: the app has ${a}, tokens.json ${want}`);
    if (!same(rgba(c || ''), w)) errors.push(`${theme} ${name}: demo.css has ${c}, tokens.json ${want}`);
    if (theme === 'light' && !same(rgba(lightMedia[name] || ''), w)) errors.push(`light ${name}: demo.css's prefers-color-scheme block has ${lightMedia[name]}, tokens.json ${want}`);
    checked += 1;
  }
}
if (errors.length) {
  for (const e of errors) console.error(`error: ${e}`);
  process.exit(1);
}
console.log(`tokens: ${checked} colours the same in Hippo.app, the demo and brand/tokens.json`);
