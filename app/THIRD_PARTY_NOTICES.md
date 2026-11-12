# Third-party notices for Hippocamp.app

## Lucide (ISC)

The icons in `hippocamp-app/icons/*.svg` are from Lucide (`lucide-static` 0.460.0; `circle-alert`, `circle-dot-dashed`, `git-pull-request-arrow` and `shield-question` from 1.54.0), stroke width set to 1.75. Licence: `hippocamp-app/icons/LICENSE-lucide.txt` (ISC; portions MIT from Feather).

## T3 Code (MIT)

Some design token values in the built-in themes (`crates/hippocamp-core/themes/hippocamp-*.json`) and the app's kit follow T3 Code's: the foreground `#f1f3f7`, the 8 % and 15 % white border alphas, the radius scale 6 / 8 / 12 / 16, the dialog shadow, and the colour family of each status. The sidebar's behaviour follows T3 Code's too (step 9b): it collapses off-canvas rather than to a rail and opens as an overlay sheet on narrow windows; the traffic lights at x 16 centred in the top bar with a 90-point controls inset, a 28-point toggle and a 12-point gap before the title; a default width of 256 points, at least 208, at most the window less 640, kept between launches. The sidebar's session rows follow its thread row and status indicators (step 9c): the row's geometry (78 points, 10 and 8 insets, radius 6, 4 apart), its hover and open fills (the foreground at 8 % and 11 %), the section header, the status glyph for each state and the pull-request badge's glyphs and tones (emerald open, violet merged). No code or brand asset is copied.

T3 Code is MIT-licensed (https://github.com/pingdotgg/t3code):

> MIT License
>
> Copyright (c) 2026 T3 Tools Inc.
>
> Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

No code or asset is taken from Zed's `ui`, `theme` or `icons` crates (GPL-3.0); `gpui` is Apache-2.0. Claude Code desktop is proprietary: only layout patterns are taken, drawn with Hippocamp's own tokens.
