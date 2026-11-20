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
    morning: '<path d="M12 2v8"/><path d="m4.93 10.93 1.41 1.41"/><path d="M2 18h2"/><path d="M20 18h2"/><path d="m19.07 10.93-1.41 1.41"/><path d="M22 22H2"/><path d="m8 6 4-4 4 4"/><path d="M16 18a4 4 0 0 0-8 0"/>',
    brain: '<path d="M12 18V5"/><path d="M15 13a4.17 4.17 0 01-3-4 4.17 4.17 0 01-3 4"/><path d="M17.707 6.294A3 3 0 1012 5a3 3 0 10-5.707 1.294"/><path d="M17.996 5.125a4 4 0 012.526 5.77"/><path d="M18 18a4 4 0 002-7.465"/><path d="M19.967 17.482a4 4 0 11-7.967.517 4 4 0 11-7.967-.517"/><path d="M6 18a4 4 0 01-2-7.465"/><path d="M6.004 5.125a4 4 0 00-2.526 5.77"/>',
    agents: '<path d="M12 8V4H8"/><rect width="16" height="12" x="4" y="8" rx="2"/><path d="M2 14h2"/><path d="M20 14h2"/><path d="M15 13v2"/><path d="M9 13v2"/>',
    guard: '<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/>',
    note: '<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M10 9H8"/><path d="M16 13H8"/><path d="M16 17H8"/>',
    folder: '<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>',
    check: '<path d="M20 6 9 17l-5-5"/>',
    x: '<path d="M18 6 6 18"/><path d="m6 6 12 12"/>',
    undo: '<path d="M9 14 4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5a5.5 5.5 0 0 1-5.5 5.5H11"/>',
    said: '<path d="M14 14a2 2 0 0 0 2-2V8h-2"/><path d="M22 17a2 2 0 0 1-2 2H6.828a2 2 0 0 0-1.414.586l-2.202 2.202A.71.71 0 0 1 2 21.286V5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2z"/><path d="M8 14a2 2 0 0 0 2-2V8H8"/>',
    receipt: '<path d="M12 17V7"/><path d="M16 8h-6a2 2 0 0 0 0 4h4a2 2 0 0 1 0 4H8"/><path d="M4 3a1 1 0 0 1 1-1 1.3 1.3 0 0 1 .7.2l.933.6a1.3 1.3 0 0 0 1.4 0l.934-.6a1.3 1.3 0 0 1 1.4 0l.933.6a1.3 1.3 0 0 0 1.4 0l.933-.6a1.3 1.3 0 0 1 1.4 0l.934.6a1.3 1.3 0 0 0 1.4 0l.933-.6A1.3 1.3 0 0 1 19 2a1 1 0 0 1 1 1v18a1 1 0 0 1-1 1 1.3 1.3 0 0 1-.7-.2l-.933-.6a1.3 1.3 0 0 0-1.4 0l-.934.6a1.3 1.3 0 0 1-1.4 0l-.933-.6a1.3 1.3 0 0 0-1.4 0l-.933.6a1.3 1.3 0 0 1-1.4 0l-.934-.6a1.3 1.3 0 0 0-1.4 0l-.933.6a1.3 1.3 0 0 1-.7.2 1 1 0 0 1-1-1z"/>',
    up: '<path d="m5 12 7-7 7 7"/><path d="M12 19V5"/>',
    sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/>',
    moon: '<path d="M20.985 12.486a9 9 0 1 1-9.473-9.472c.405-.022.617.46.402.803a6 6 0 0 0 8.268 8.268c.344-.215.825-.004.803.401"/>',
    start: '<path d="M4 22V4a1 1 0 0 1 .4-.8A6 6 0 0 1 8 2c3 0 5 2 7.333 2q2 0 3.067-.8A1 1 0 0 1 20 4v10a1 1 0 0 1-.4.8A6 6 0 0 1 16 16c-3 0-5-2-8-2a6 6 0 0 0-4 1.528"/>',
    right: '<path d="M5 12h14"/><path d="m12 5 7 7-7 7"/>',
    minus: '<path d="M5 12h14"/>',
    plus: '<path d="M5 12h14"/><path d="M12 5v14"/>',
    grid: '<rect width="7" height="7" x="3" y="3" rx="1"/><rect width="7" height="7" x="14" y="3" rx="1"/><rect width="7" height="7" x="14" y="14" rx="1"/><rect width="7" height="7" x="3" y="14" rx="1"/>'
  };
  function ic(n, s) {
    s = s || 16;
    return '<svg width="' + s + '" height="' + s + '" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' + (P[n] || '') + '</svg>';
  }
  // Hippo's mark (brand/hippo-mark.svg), as the app draws it.
  var MARK = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 473 387" fill="currentColor" aria-hidden="true"><g transform="translate(0,387) scale(0.1,-0.1)"><path d="M2167 3229 c-294 -28 -582 -125 -766 -257 -41 -30 -81 -56 -87 -59 -19 -7 -144 45 -248 103 -176 98 -282 107 -364 31 -59 -55 -90 -196 -63 -280 29 -85 144 -147 304 -162 42 -4 77 -9 77 -11 0 -2 -24 -41 -54 -87 -270 -414 -391 -1001 -241 -1174 52 -60 62 -32 68 202 13 470 192 876 528 1200 202 195 395 288 719 347 138 25 453 31 560 10 269 -53 448 -177 528 -369 31 -73 31 -73 -43 -73 -205 0 -478 -48 -637 -112 -116 -47 -298 -146 -298 -163 0 -35 32 -31 261 38 390 118 871 141 1289 62 124 -24 350 -84 390 -105 122 -63 155 -177 148 -512 -5 -198 -7 -218 -31 -285 -105 -293 -387 -427 -892 -426 -566 1 -953 119 -1134 345 -100 124 -141 229 -189 478 -4 21 -14 36 -24 38 -26 5 -42 -66 -41 -183 1 -207 104 -400 288 -542 76 -58 76 -58 68 -98 -33 -166 5 -213 212 -266 220 -55 287 -39 320 75 19 65 6 62 180 41 66 -8 201 -15 301 -15 181 0 181 0 197 -60 30 -106 80 -130 237 -112 118 14 177 34 227 76 38 34 38 34 41 129 4 96 4 96 68 138 131 85 236 233 280 394 25 91 31 469 10 593 -39 226 -100 286 -363 358 -220 61 -362 86 -573 105 -97 9 -140 17 -140 25 0 7 -2 19 -5 27 -5 11 9 14 69 14 258 2 372 356 150 464 -72 34 -113 24 -184 -48 -32 -32 -89 -75 -128 -96 -70 -37 -70 -37 -139 14 -206 157 -514 223 -876 188z m1287 -185 c58 -57 6 -194 -80 -210 -46 -9 -162 14 -173 34 -7 13 6 26 67 63 43 26 95 65 117 87 47 46 49 47 69 26z m-2463 -134 c47 -27 113 -62 147 -77 61 -28 61 -28 31 -62 -29 -32 -35 -34 -113 -38 -101 -7 -215 11 -258 41 -32 21 -33 24 -33 90 0 124 67 138 226 46z m1490 -1774 c35 -13 99 -32 142 -42 77 -18 77 -18 77 -51 0 -38 5 -37 -96 -18 -180 34 -221 59 -198 119 8 20 0 20 75 -8z m1395 -61 c21 -54 -15 -77 -151 -99 -97 -15 -100 -15 -110 26 -6 23 -3 28 17 32 12 3 66 16 118 29 119 30 119 30 126 12z M2736 2889 c-50 -59 -24 -172 45 -194 30 -10 78 14 94 46 58 112 -62 240 -139 148z M2193 2865 c-52 -36 -54 -141 -4 -194 68 -73 159 -11 149 101 -9 88 -84 136 -145 93z M3895 2333 c-87 -17 -125 -56 -125 -128 0 -150 273 -228 351 -101 65 107 -82 256 -226 229z M2522 2308 c-142 -25 -228 -180 -145 -263 82 -81 251 -33 312 88 54 109 -31 199 -167 175z"/></g></svg>';
  // Agent marks (brand/agents/README.md: LobeHub icons, MIT; the marks belong to their owners).
  var MARKS = {
    cc: '<path d="M4.709 15.955l4.72-2.647.08-.23-.08-.128H9.2l-.79-.048-2.698-.073-2.339-.097-2.266-.122-.571-.121L0 11.784l.055-.352.48-.321.686.06 1.52.103 2.278.158 1.652.097 2.449.255h.389l.055-.157-.134-.098-.103-.097-2.358-1.596-2.552-1.688-1.336-.972-.724-.491-.364-.462-.158-1.008.656-.722.881.06.225.061.893.686 1.908 1.476 2.491 1.833.365.304.145-.103.019-.073-.164-.274-1.355-2.446-1.446-2.49-.644-1.032-.17-.619a2.97 2.97 0 01-.104-.729L6.283.134 6.696 0l.996.134.42.364.62 1.414 1.002 2.229 1.555 3.03.456.898.243.832.091.255h.158V9.01l.128-1.706.237-2.095.23-2.695.08-.76.376-.91.747-.492.584.28.48.685-.067.444-.286 1.851-.559 2.903-.364 1.942h.212l.243-.242.985-1.306 1.652-2.064.73-.82.85-.904.547-.431h1.033l.76 1.129-.34 1.166-1.064 1.347-.881 1.142-1.264 1.7-.79 1.36.073.11.188-.02 2.856-.606 1.543-.28 1.841-.315.833.388.091.395-.328.807-1.969.486-2.309.462-3.439.813-.042.03.049.061 1.549.146.662.036h1.622l3.02.225.79.522.474.638-.079.485-1.215.62-1.64-.389-3.829-.91-1.312-.329h-.182v.11l1.093 1.068 2.006 1.81 2.509 2.33.127.578-.322.455-.34-.049-2.205-1.657-.851-.747-1.926-1.62h-.128v.17l.444.649 2.345 3.521.122 1.08-.17.353-.608.213-.668-.122-1.374-1.925-1.415-2.167-1.143-1.943-.14.08-.674 7.254-.316.37-.729.28-.607-.461-.322-.747.322-1.476.389-1.924.315-1.53.286-1.9.17-.632-.012-.042-.14.018-1.434 1.967-2.18 2.945-1.726 1.845-.414.164-.717-.37.067-.662.401-.589 2.388-3.036 1.44-1.882.93-1.086-.006-.158h-.055L4.132 18.56l-1.13.146-.487-.456.061-.746.231-.243 1.908-1.312-.006.006z"></path>',
    cx: '<path clip-rule="evenodd" d="M8.086.457a6.105 6.105 0 013.046-.415c1.333.153 2.521.72 3.564 1.7a.117.117 0 00.107.029c1.408-.346 2.762-.224 4.061.366l.063.03.154.076c1.357.703 2.33 1.77 2.918 3.198.278.679.418 1.388.421 2.126a5.655 5.655 0 01-.18 1.631.167.167 0 00.04.155 5.982 5.982 0 011.578 2.891c.385 1.901-.01 3.615-1.183 5.14l-.182.22a6.063 6.063 0 01-2.934 1.851.162.162 0 00-.108.102c-.255.736-.511 1.364-.987 1.992-1.199 1.582-2.962 2.462-4.948 2.451-1.583-.008-2.986-.587-4.21-1.736a.145.145 0 00-.14-.032c-.518.167-1.04.191-1.604.185a5.924 5.924 0 01-2.595-.622 6.058 6.058 0 01-2.146-1.781c-.203-.269-.404-.522-.551-.821a7.74 7.74 0 01-.495-1.283 6.11 6.11 0 01-.017-3.064.166.166 0 00.008-.074.115.115 0 00-.037-.064 5.958 5.958 0 01-1.38-2.202 5.196 5.196 0 01-.333-1.589 6.915 6.915 0 01.188-2.132c.45-1.484 1.309-2.648 2.577-3.493.282-.188.55-.334.802-.438.286-.12.573-.22.861-.304a.129.129 0 00.087-.087A6.016 6.016 0 015.635 2.31C6.315 1.464 7.132.846 8.086.457zm-.804 7.85a.848.848 0 00-1.473.842l1.694 2.965-1.688 2.848a.849.849 0 001.46.864l1.94-3.272a.849.849 0 00.007-.854l-1.94-3.393zm5.446 6.24a.849.849 0 000 1.695h4.848a.849.849 0 000-1.696h-4.848z"></path>',
    cu: '<path d="M22.106 5.68L12.5.135a.998.998 0 00-.998 0L1.893 5.68a.84.84 0 00-.419.726v11.186c0 .3.16.577.42.727l9.607 5.547a.999.999 0 00.998 0l9.608-5.547a.84.84 0 00.42-.727V6.407a.84.84 0 00-.42-.726zm-.603 1.176L12.228 22.92c-.063.108-.228.064-.228-.061V12.34a.59.59 0 00-.295-.51l-9.11-5.26c-.107-.062-.063-.228.062-.228h18.55c.264 0 .428.286.296.514z"></path>',
    gm: '<path d="M16.793 10.358v3.867L7.236 18.82v-2.8l7.751-3.728-7.75-3.728V5.763l9.556 4.595z"></path><path clip-rule="evenodd" d="M19.608 0A4.392 4.392 0 0124 4.392v15.216A4.392 4.392 0 0119.608 24H4.392A4.392 4.392 0 010 19.608V4.392A4.392 4.392 0 014.392 0h15.216zM4.26 1.444A2.816 2.816 0 001.444 4.26v15.48a2.816 2.816 0 002.816 2.816h15.48a2.816 2.816 0 002.816-2.816V4.26a2.816 2.816 0 00-2.816-2.816H4.26z"></path>',
    oc: '<path d="M16 6H8v12h8V6zm4 16H4V2h16v20z"></path>',
    ds: '<path d="M23.748 4.482c-.254-.124-.364.113-.512.234-.051.039-.094.09-.137.136-.372.397-.806.657-1.373.626-.829-.046-1.537.214-2.163.848-.133-.782-.575-1.248-1.247-1.548-.352-.156-.708-.311-.955-.65-.172-.241-.219-.51-.305-.774-.055-.16-.11-.323-.293-.35-.2-.031-.278.136-.356.276-.313.572-.434 1.202-.422 1.84.027 1.436.633 2.58 1.838 3.393.137.093.172.187.129.323-.082.28-.18.552-.266.833-.055.179-.137.217-.329.14a5.526 5.526 0 01-1.736-1.18c-.857-.828-1.631-1.742-2.597-2.458a11.365 11.365 0 00-.689-.471c-.985-.957.13-1.743.388-1.836.27-.098.093-.432-.779-.428-.872.004-1.67.295-2.687.684a3.055 3.055 0 01-.465.137 9.597 9.597 0 00-2.883-.102c-1.885.21-3.39 1.102-4.497 2.623C.082 8.606-.231 10.684.152 12.85c.403 2.284 1.569 4.175 3.36 5.653 1.858 1.533 3.997 2.284 6.438 2.14 1.482-.085 3.133-.284 4.994-1.86.47.234.962.327 1.78.397.63.059 1.236-.03 1.705-.128.735-.156.684-.837.419-.961-2.155-1.004-1.682-.595-2.113-.926 1.096-1.296 2.746-2.642 3.392-7.003.05-.347.007-.565 0-.845-.004-.17.035-.237.23-.256a4.173 4.173 0 001.545-.475c1.396-.763 1.96-2.015 2.093-3.517.02-.23-.004-.467-.247-.588zM11.581 18c-2.089-1.642-3.102-2.183-3.52-2.16-.392.024-.321.471-.235.763.09.288.207.486.371.739.114.167.192.416-.113.603-.673.416-1.842-.14-1.897-.167-1.361-.802-2.5-1.86-3.301-3.307-.774-1.393-1.224-2.887-1.298-4.482-.02-.386.093-.522.477-.592a4.696 4.696 0 011.529-.039c2.132.312 3.946 1.265 5.468 2.774.868.86 1.525 1.887 2.202 2.891.72 1.066 1.494 2.082 2.48 2.914.348.292.625.514.891.677-.802.09-2.14.11-3.054-.614zm1-6.44a.306.306 0 01.415-.287.302.302 0 01.2.288.306.306 0 01-.31.307.303.303 0 01-.304-.308zm3.11 1.596c-.2.081-.399.151-.59.16a1.245 1.245 0 01-.798-.254c-.274-.23-.47-.358-.552-.758a1.73 1.73 0 01.016-.588c.07-.327-.008-.537-.239-.727-.187-.156-.426-.199-.688-.199a.559.559 0 01-.254-.078c-.11-.054-.2-.19-.114-.358.028-.054.16-.186.192-.21.356-.202.767-.136 1.146.016.352.144.618.408 1.001.782.391.451.462.576.685.914.176.265.336.537.445.848.067.195-.019.354-.25.452z"></path>',
    mu: '<path d="M5.91 3.869C6.777 3.869 7.396 4.546 7.396 5.607 7.396 6.669 7.225 7.626 6.941 9.659 6.738 11.084 6.389 13.258 6.243 14.162 6.523 13.628 7.178 12.378 7.294 12.144 8.666 9.375 10.096 6.901 10.508 6.246 11.054 5.372 11.535 5.021 12.278 5.021 12.664 5.022 13.124 5.217 13.352 5.477 13.66 5.829 13.725 6.398 13.714 7.019 13.698 7.97 13.574 9.486 13.405 11.177 13.33 11.929 13.185 13.466 13.087 14.496 13.965 12.925 15.591 10.035 16.246 9.025 16.817 8.146 17.392 7.579 18.164 7.579 18.83 7.579 19.651 8.05 19.603 9.302 19.531 11.149 19.264 15.174 19.143 16.979 19.96 15.79 21.033 14.029 21.436 13.367 21.842 12.698 22.26 12.157 22.894 12.139 23.544 12.12 23.979 12.562 23.999 13.244 24.019 13.927 23.612 14.694 23.072 15.565 22.486 16.511 21.831 17.523 21.033 18.468 20.236 19.413 19.502 20.131 18.384 20.131 17.382 20.131 16.677 19.465 16.628 18.246 16.602 17.607 16.723 16.234 16.814 15.16 16.92 13.917 17.048 12.717 17.137 11.923 16.256 13.535 14.402 16.912 13.568 18.23 12.752 19.52 12.355 19.974 11.493 19.975 10.631 19.975 10.253 19.354 10.241 18.42 10.227 17.407 10.988 11.324 11.246 9.297 10.828 10.191 9.801 12.385 9.157 13.69 8.534 14.953 7.489 17.135 6.73 18.393 6.184 19.298 5.57 19.975 4.66 19.975 3.75 19.975 3.236 19.363 3.236 18.379 3.236 17.395 3.521 15.756 3.827 13.723 3.98 12.739 4.155 11.667 4.352 10.53 4.496 9.708 4.746 8.26 4.9 7.371 4.2 8.461 2.941 10.423 2.558 11.014 2.131 11.671 1.749 12.15 1.115 12.15 0.416 12.15 0 11.605 0 10.971 0 10.337 0.322 9.848 0.846 8.988 1.518 7.883 2.726 6.377 3.388 5.586 4.168 4.654 4.947 3.869 5.91 3.869Z"></path>',
    co: '<path clip-rule="evenodd" d="M19.334 0a4.67 4.67 0 0 0-4.666 4.668v2H9.334v-2A4.67 4.67 0 0 0 4.668 0 4.67 4.67 0 0 0 0 4.668a4.67 4.67 0 0 0 4.668 4.665h2v5.334h-2A4.67 4.67 0 0 0 0 19.334 4.67 4.67 0 0 0 4.668 24a4.67 4.67 0 0 0 4.666-4.666v-2h5.334v2A4.67 4.67 0 0 0 19.334 24 4.67 4.67 0 0 0 24 19.334a4.67 4.67 0 0 0-4.666-4.667h-2V9.333h2A4.67 4.67 0 0 0 24 4.668 4.673 4.673 0 0 0 19.334 0m-2 6.666v-2c0-1.107.893-2 2-2s2 .893 2 2-.893 2-2 2zm-12.668 0c-1.107 0-2-.893-2-2s.893-2 2-2 2 .893 2 2v2zm4.666 8V9.334h5.334v5.334zm10.002 6.667c-1.107 0-2-.893-2-2v-2h2c1.107 0 2 .892 2 2 0 1.107-.893 2-2 2m-14.668 0c-1.107 0-2-.893-2-2 0-1.108.893-2 2-2h2v2c0 1.107-.893 2-2 2"></path>'
  };
  var TERM = '<path d="M12 19h8"/><path d="m4 17 6-6-6-6"/>';
  function badge(a) {
    if (!a) return '';
    var m = Object.prototype.hasOwnProperty.call(MARKS, a.code) ? MARKS[a.code] : null;
    var svg = m
      ? '<svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" fill-rule="evenodd" aria-hidden="true">' + m + '</svg>'
      : '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' + TERM + '</svg>';
    return '<span class="mark" title="' + esc(a.name) + '">' + svg + '</span>';
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
      '<div class="figures">' + fig(f.rules_waiting, 'rules waiting for you', 'var(--you-text)') + fig(f.notes_fixed, 'notes fixed and confirmed', 'var(--ok)') +
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
  var M = D.map, A = window.HippoArrange;
  // Motion: blocks slide aside, a dropped block settles, a block put back
  // flies home; none under Reduce Motion.
  var still = !!(window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches);
  var SLIDE = still ? 0 : 150, SETTLE = still ? 0 : 180, BACK = still ? 0 : 220;
  // The app's frame: its top bar, Brain's header, the Ask bar's clearance,
  // the panel and the gaps (app/hippo-app/src/ui/night). The map view is
  // the app's at the same window size, so it gets the same columns.
  var TOP_H = 56, HEAD = 118, ASK_CLEAR = 102, PANEL_W = 360, EDGE = 18;
  function appView() {
    return [window.innerWidth - PANEL_W - 2 * EDGE + EDGE, Math.max(200, window.innerHeight - TOP_H - HEAD - ASK_CLEAR)];
  }
  // The layouts drawn: hippo's layout for the view (S.def, from
  // demo/arrange.js), or the person's arrangement (kept in this page only).
  function LY() {
    var arr = S.arr || S.def;
    if (!arr || !A) return M;
    if (!S.lay) S.lay = A.layout(M, arr);
    return S.lay;
  }
  function setArr(a) {
    var def = S.def || M.default_arrangement;
    a = a && A ? A.settle(M, a) : null;
    S.arr = a && !A.same(a, def) ? a : null;
    S.lay = null;
  }
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
    var a = cardAt(LY()[from], x, y);
    if (!a) return [x, y];
    var b = cardOf(LY()[to], sameItem(a.item, to));
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
  // The card whose lines show: the hovered one, else the selected note's.
  function focusOf(L) {
    if (S.hover) return S.hover;
    var sel = selIndex();
    return sel >= 0 ? sameItem({ note: sel }, L) : null;
  }
  // Its lines (Rust: Layout::shown), bundled; none without a focus.
  function shown(lay, f) {
    var fk = f ? key(f) : null, own = [];
    (lay.focus || []).forEach(function (x) { if (key(x[0]) === fk) own = x[1]; });
    return own;
  }
  // As the app draws them (hippo_view::night::map::STYLE, from the data).
  var ST = M.style || { line_out: 1, line_out_step: 0.35, line_out_max: 2, line_out_thin: 0.55, line_in: 1.5, corner: 7, arrow: 7, port_from: 0.6, rows_below: 0.39, meta_below: 0.39 };
  function lineWidth(L, l) {
    if (L !== 'out') return ST.line_in;
    var w = Math.min(ST.line_out_max, ST.line_out + ST.line_out_step * Math.log2(Math.max(1, l.links)));
    return w * Math.min(1, Math.max(ST.line_out_thin, S.zoom / (S.fitZoom || 1)));
  }
  // A polyline with its corners rounded on screen.
  function roundedPath(pts) {
    var r0 = ST.corner / S.zoom, v = [];
    pts.forEach(function (q) { var l = v[v.length - 1]; if (!l || Math.abs(l[0] - q[0]) + Math.abs(l[1] - q[1]) > 0.05) v.push(q); });
    var d = 'M' + v[0][0] + ' ' + v[0][1];
    for (var i = 1; i < v.length - 1; i++) {
      var a = v[i - 1], c = v[i], n = v[i + 1];
      var di = Math.hypot(c[0] - a[0], c[1] - a[1]), dn = Math.hypot(n[0] - c[0], n[1] - c[1]);
      var r = Math.min(r0, di / 2, dn / 2);
      if (r < 0.5 / S.zoom) { d += 'L' + c[0] + ' ' + c[1]; continue; }
      d += 'L' + (c[0] - (c[0] - a[0]) / di * r) + ' ' + (c[1] - (c[1] - a[1]) / di * r) +
        'Q' + c[0] + ' ' + c[1] + ' ' + (c[0] + (n[0] - c[0]) / dn * r) + ' ' + (c[1] + (n[1] - c[1]) / dn * r);
    }
    var z = v[v.length - 1];
    return d + 'L' + z[0] + ' ' + z[1];
  }
  // The selected note's lines in the text colour; a hovered card's in the
  // accent over them, the selection's faint. An arrowhead where a line
  // points; a count where a branch of more than one link leaves its trunk.
  function linesSvg(L) {
    var lay = LY()[L], f = focusOf(L), lifting = !!S.lifting;
    var sel = selIndex(), selF = sel >= 0 ? sameItem({ note: sel }, L) : null;
    var sets = [];
    if (S.hover && f && key(S.hover) === key(f)) {
      if (selF && key(selF) !== key(f)) sets.push([selF, 'sel', 0.3]);
      sets.push([f, 'acc', 1]);
    } else sets.push([f, 'sel', 1]);
    var hot = S.hoverLine ? S.hoverLine.ends : [];
    var h = '', z = S.zoom;
    sets.forEach(function (set) {
      var list = shown(lay, set[0]), alpha = lifting ? Math.min(set[2], 0.35) : set[2];
      var heads = [], badges = {}, order = [];
      list.forEach(function (l) {
        var isHot = hot.some(function (e) { return e[0] === key(l.from) && e[1] === key(l.to); });
        var cls = (isHot && alpha > 0.5 ? 'acc' : set[1]);
        var w = lineWidth(L, l) + (isHot ? 1 : 0);
        h += '<path class="' + cls + '" vector-effect="non-scaling-stroke" style="stroke-width:' + w + 'px;opacity:' + alpha + '" d="' + roundedPath(l.points) + '"/>';
        var n = l.points.length;
        if (l.out) heads.push([l.points[n - 1], l.points[n - 2]]);
        if (l.back && !heads.some(function (q) { return q[0][0] === l.points[0][0] && q[0][1] === l.points[0][1]; })) heads.push([l.points[0], l.points[1]]);
        if (n >= 4) {
          var foot = l.points[n - 4], k = l.trunk + '|' + Math.round(foot[0]) + '|' + Math.round(foot[1]);
          if (!badges[k]) { badges[k] = [foot, 0]; order.push(k); }
          badges[k][1] += l.links;
        }
      });
      if (alpha < 0.5) return;
      var a = ST.arrow * Math.min(1, Math.max(0.7, z / (S.fitZoom || 1))) / z;
      heads.forEach(function (hd) {
        var tip = hd[0], from = hd[1], dx = tip[0] - from[0], dy = tip[1] - from[1], len = Math.max(0.01, Math.hypot(dx, dy));
        var ux = dx / len, uy = dy / len, bx = tip[0] - ux * a, by = tip[1] - uy * a;
        h += '<path class="head ' + set[1] + '" d="M' + tip[0] + ' ' + tip[1] + 'L' + (bx - uy * a / 2) + ' ' + (by + ux * a / 2) + 'L' + (bx + uy * a / 2) + ' ' + (by - ux * a / 2) + 'Z"/>';
      });
      order.forEach(function (k) {
        var b = badges[k];
        if (b[1] < 2) return;
        var txt = String(b[1]), bw = Math.max(18, txt.length * 7 + 10) / z, bh = 18 / z;
        h += '<g class="badge ' + set[1] + '"><rect x="' + (b[0][0] - bw / 2) + '" y="' + (b[0][1] - bh / 2) + '" width="' + bw + '" height="' + bh + '" rx="' + bh / 2 + '" vector-effect="non-scaling-stroke"/>' +
          '<text x="' + b[0][0] + '" y="' + b[0][1] + '" font-size="' + 11 / z + '" text-anchor="middle" dominant-baseline="central">' + txt + '</text></g>';
      });
    });
    return h;
  }
  function worldHtml(L) {
    var lay = LY()[L], sel = selIndex(), selItem = sel >= 0 ? key(sameItem({ note: sel }, L)) : null;
    var blk = {}, order = [];
    function into(item, html) {
      var k = A ? A.blockKey(M, item) : 'all';
      if (!blk[k]) { blk[k] = []; order.push(k); }
      blk[k].push(html);
    }
    // Every word on the map is one of the layout's labels, fitted to its
    // box by hippo; a note card's title is its card's own label.
    var titles = {};
    (lay.labels || []).forEach(function (l) { if (L !== 'out' && l.kind === 'title' && l.item.note != null) titles[l.item.note] = l; });
    lay.frames.forEach(function (fr) {
      var r = fr[1];
      into({ folder: fr[0] }, '<div class="frame" style="left:' + r.x + 'px;top:' + r.y + 'px;width:' + r.w + 'px;height:' + r.h + 'px"></div>');
    });
    lay.cards.forEach(function (c) {
      var r = c.rect, st = 'left:' + r.x + 'px;top:' + r.y + 'px;width:' + r.w + 'px;height:' + r.h + 'px';
      var isSel = key(c.item) === selItem ? ' sel' : '';
      if (c.item.folder != null) {
        into(c.item, '<div class="folder' + isSel + '" data-folder="' + c.item.folder + '" style="' + st + '"></div>');
        return;
      }
      if (L === 'out') {
        into(c.item, '<div class="ocard' + isSel + '" data-note="' + c.item.note + '" style="' + st + '"></div>');
        return;
      }
      var i = c.item.note, n = M.notes[i], fl = n.flags, tl = titles[i];
      var ch = fl.rejected ? chip('fix rejected', 'bad') : fl.waiting ? chip(fl.waiting === 1 ? 'rule waiting' : fl.waiting + ' rules waiting', 'you') : fl.fixed ? chip('fixed last night', 'ok') : fl.stale_days != null ? chip('stale, ' + fl.stale_days + ' days') : (fl.required || fl.entry) ? chip('every session', 'night') : '';
      var inner = '';
      if (L === 'in') {
        inner = '<div class="in">' + n.pending.map(function (p) { return '<p class="wait">' + esc(p) + '</p>'; }).join('') +
          n.evidence.map(function (e) { return '<p class="said"><span>' + esc(e) + '</span></p>'; }).join('') +
          n.rules.map(function (p) { return '<p class="kept">' + esc(p) + '</p>'; }).join('') + '</div>';
      }
      into(c.item, '<div class="note' + isSel + (L === 'in' ? ' inner' : '') + '" data-note="' + i + '" style="' + st + '"><div class="row">' + ic(fl.entry ? 'start' : 'note') + '<span class="t"' + (tl ? ' style="font-size:' + tl.size + 'px"' : '') + '>' + esc(tl ? tl.lines[0] : n.title) + '</span></div>' +
        '<p class="s">' + esc(n.summary) + '</p><div class="row chips">' + ch + chip(n.reads_week ? 'read ' + n.reads_week + '× this week' : 'not read this week') + '</div>' + inner + '</div>');
    });
    (lay.labels || []).forEach(function (l) {
      if (L !== 'out' && l.kind === 'title') return;
      var r = l.rect;
      into(l.item, '<div class="lab k-' + l.kind + '" style="left:' + r.x + 'px;top:' + r.y + 'px;width:' + r.w + 'px;font-size:' + l.size + 'px">' + l.lines.map(esc).join('<br>') + '</div>');
    });
    (lay.marks || []).forEach(function (m) {
      var st = 'left:' + (m.x - m.d / 2) + 'px;top:' + (m.y - m.d / 2) + 'px;width:' + m.d + 'px;height:' + m.d + 'px';
      if (m.tone.indexOf('icon-') === 0) {
        var name = m.tone.slice(5);
        into(m.item, '<span class="mk icon i-' + name + '" style="' + st + '">' + ic(name === 'flag' ? 'start' : name === 'folder' ? 'folder' : 'note', m.d) + '</span>');
        return;
      }
      into(m.item, '<span class="mk t-' + m.tone + (m.row ? ' row' : '') + '" style="' + st + '"></span>');
    });
    var h = '<svg class="links" width="' + lay.width + '" height="' + lay.height + '">' + linesSvg(L) + '</svg>';
    // Each block in a box of its own, so it can slide and be lifted whole.
    order.forEach(function (k) { h += '<div class="blk" data-key="' + esc(k) + '">' + blk[k].join('') + '</div>'; });
    return h;
  }
  function redrawLines() {
    var wrap = app.querySelector('.mapwrap'), svg = wrap && wrap.querySelector('svg.links');
    if (svg) svg.innerHTML = linesSvg(level(S.zoom));
    if (!wrap) return;
    var hov = S.hover ? key(S.hover) : null;
    wrap.querySelectorAll('.hover').forEach(function (x) { x.classList.remove('hover'); });
    if (hov) {
      var el = wrap.querySelector(S.hover.note != null ? '[data-note="' + S.hover.note + '"]' : '[data-folder="' + S.hover.folder + '"]');
      if (el) el.classList.add('hover');
    }
    var slot = app.querySelector('.mslot');
    if (slot) slot.innerHTML = slotHtml();
  }
  // Folders whose name band has scrolled above the view keep their names
  // in an opaque strip at its top; the cards pass under it (Mid and In).
  function stickies() {
    var wrap = app.querySelector('.mapwrap');
    if (!wrap) return;
    var strip = wrap.querySelector('.strip');
    var L = level(S.zoom), names = [];
    if (L !== 'out') {
      var lay = LY()[L], vy = S.pan[1], vx0 = S.pan[0], vx1 = S.pan[0] + appView()[0] / S.zoom;
      lay.frames.forEach(function (fr) {
        var r = fr[1];
        if (r.y >= vy || r.y + r.h < vy + 60 / S.zoom || r.x + r.w < vx0 || r.x > vx1) return;
        var words = function (k) { var l = (lay.labels || []).filter(function (x) { return x.kind === k && x.item.folder === fr[0]; })[0]; return l ? l.lines[0] : ''; };
        names.push('<span style="left:' + Math.max(12, (r.x + 14 - S.pan[0]) * S.zoom) + 'px"><b>' + esc(words('frame')) + '</b>' + esc(words('count')) + '</span>');
      });
    }
    if (!names.length) { if (strip && strip.parentNode) strip.parentNode.removeChild(strip); return; }
    if (!strip) { strip = document.createElement('div'); strip.className = 'strip'; wrap.appendChild(strip); }
    strip.innerHTML = names.join('');
  }
  var drawnLevel = null;
  function placeWorld(force) {
    var wrap = app.querySelector('.mapwrap');
    if (!wrap) return;
    var world = wrap.querySelector('.world'), L = level(S.zoom);
    if (force || drawnLevel !== L) {
      world.innerHTML = worldHtml(L);
      world.style.width = LY()[L].width + 'px';
      world.style.height = LY()[L].height + 'px';
      drawnLevel = L;
    } else {
      // Line widths and port rings keep their size on screen.
      redrawLines();
    }
    world.style.transform = 'translate(' + (-S.pan[0] * S.zoom) + 'px,' + (-S.pan[1] * S.zoom) + 'px) scale(' + S.zoom + ')';
    // Far out a folder card keeps its name and counts; further out, its name.
    wrap.classList.toggle('far', L === 'out' && S.zoom < 0.33);
    wrap.classList.toggle('farther', L === 'out' && S.zoom < 0.27);
    stickies();
    var pct = app.querySelector('[data-act="fit"]');
    if (pct) pct.textContent = Math.round(S.zoom * 100) + '%';
  }
  function fit() {
    var wrap = app.querySelector('.mapwrap');
    if (!wrap) return;
    // The app's view at this window size: the same columns and the same
    // zoom as Hippo.app.
    var v = appView(), w = v[0], h = v[1];
    if (A) S.def = A.viewArrangement(M, w, h);
    S.lay = null;
    var f = A ? A.fit(LY(), w, h) : [1, [0, 0]];
    S.zoom = f[0];
    S.fitZoom = f[0];
    S.pan = f[1];
    S.fitted = true;
    placeWorld(true);
  }
  // Whether the view shows the whole map.
  function showsAll() {
    var wrap = app.querySelector('.mapwrap'), lay = LY()[level(S.zoom)];
    if (!wrap) return false;
    return S.pan[0] <= 0.5 && S.pan[1] <= 0.5 && S.pan[0] + wrap.clientWidth / S.zoom >= lay.width - 0.5 && S.pan[1] + wrap.clientHeight / S.zoom >= lay.height - 0.5;
  }
  function zoomAt(z, px, py) {
    // Never further out than the whole map.
    z = Math.max(S.fitZoom || MIN_ZOOM, Math.min(MAX_ZOOM, z));
    var from = level(S.zoom), to = level(z);
    var m = [S.pan[0] + px / S.zoom, S.pan[1] + py / S.zoom];
    if (from !== to) m = anchor(from, to, m[0], m[1]);
    S.zoom = z;
    S.pan = [m[0] - px / z, m[1] - py / z];
    placeWorld(false);
  }
  function brain() {
    // The map's controls sit in the header band, never over the map.
    var zoom = '<div class="ctl"><button data-act="zout" aria-label="Zoom out" title="Zoom out">' + ic('minus', 15) + '</button><button data-act="fit" class="pct" title="Fit the map">100%</button><button data-act="zin" aria-label="Zoom in" title="Zoom in">' + ic('plus', 15) + '</button></div>';
    return '<div class="mhead"><div class="krow"><p class="kicker">' + esc(M.kicker) + '</p>' +
      '<button data-act="reset" class="qbtn' + (S.arr ? '' : ' hidden') + '" title="Back to the layout hippo chooses">' + ic('undo', 14) + 'Reset layout</button>' + zoom +
      '<button data-act="treemap" class="qbtn">' + ic('grid', 14) + 'Treemap</button></div>' +
      '<h1>' + esc(M.headline) + '</h1><div class="lrow"><p class="lead">' + esc(M.lead) + '</p><span class="mslot" role="status">' + slotHtml() + '</span></div></div>' +
      '<div class="mapwrap"><div class="world"></div></div>' +
      '<div class="boardwrap"></div>';
  }
  // One slot for what just happened and what the pointer is on: a move
  // with its Undo, or a line's links.
  function slotHtml() {
    if (S.toastUndo) return '<span>' + esc(S.toast) + '</span><button class="qbtn" data-act="undo-layout">' + ic('undo', 14) + 'Undo</button>';
    if (S.hoverLine) return '<span>' + esc(S.hoverLine.from) + '</span>' + ic('right', 13) + '<span>' + esc(S.hoverLine.to) + '</span>';
    return '';
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
    var h = '<div class="row"><span class="tile">' + ic('folder') + '</span><span style="color:var(--muted)">' + esc(p.folder || 'Notes') + '</span></div>' +
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
      (S.toast && !S.toastUndo ? '<div class="toast" role="status">' + esc(S.toast) + '</div>' : '');
    if (S.view === 'brain') {
      var wrap = app.querySelector('.mapwrap'), board = app.querySelector('.boardwrap');
      if (narrow) { wrap.style.display = 'none'; board.innerHTML = boardHtml(); } else {
        // The map ends above the Ask bar: nothing floats over it.
        wrap.style.height = appView()[1] + 'px';
        drawnLevel = null;
        if (!S.fitted) fit(); else placeWorld(true);
        bindMap(wrap);
      }
    } else {
      drawWires();
    }
  }
  function toast(t, undo) {
    S.toast = t;
    S.toastUndo = !!undo;
    render();
    setTimeout(function () { if (S.toast === t) { S.toast = ''; S.toastUndo = false; render(); } }, undo ? 6000 : 2600);
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
    var toMap = function (p) { return [S.pan[0] + p[0] / S.zoom, S.pan[1] + p[1] / S.zoom]; };
    // Escape while a block is lifted: every block flies back.
    wrap.__cancel = function () {
      if (!drag || !drag.key || !drag.moved) return false;
      var d = drag;
      drag = null;
      wrap.classList.remove('drag');
      putBack(wrap, d);
      return true;
    };
    wrap.addEventListener('pointerdown', function (e) {
      if (e.target.closest('.zoombar')) return;
      var p = pt(e), m = toMap(p), L = level(S.zoom);
      // A block's handle lifts the block (any card at Out; a folder's name
      // band, or a note in every session, at Mid and In); the rest pans.
      var key = A ? A.handleAt(M, LY()[L], L, m[0], m[1]) : null;
      drag = { p: p, pan: S.pan.slice(), moved: false, id: e.pointerId, key: key, L: L };
    });
    wrap.addEventListener('pointermove', function (e) {
      if (!drag) { hoverAt(pt(e)); return; }
      var p = pt(e), dx = p[0] - drag.p[0], dy = p[1] - drag.p[1];
      if (!drag.moved && Math.abs(dx) + Math.abs(dy) <= 4) return;
      if (!drag.moved) { wrap.setPointerCapture(drag.id); wrap.classList.add('drag'); }
      drag.moved = true;
      if (!drag.key) {
        S.pan = [drag.pan[0] - dx / S.zoom, drag.pan[1] - dy / S.zoom];
        placeWorld(false);
        return;
      }
      // Near the edge the map scrolls under the lifted block.
      var edge = 40, sx = p[0] < edge ? -8 : p[0] > wrap.clientWidth - edge ? 8 : 0, sy = p[1] < edge ? -8 : p[1] > wrap.clientHeight - edge ? 8 : 0;
      if (sx || sy) { S.pan = [S.pan[0] + sx / S.zoom, S.pan[1] + sy / S.zoom]; placeWorld(false); }
      drawLift(wrap, drag, p);
    });
    wrap.addEventListener('pointerleave', function () {
      if (drag || (!S.hover && !S.hoverLine)) return;
      S.hover = null; S.hoverLine = null; redrawLines();
    });
    wrap.addEventListener('pointerup', function (e) {
      var d = drag;
      drag = null;
      wrap.classList.remove('drag');
      if (d && d.moved && d.key && d.target) { settle(wrap, d); return; }
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
  /* The pointer at view point p, no button down: the card under it, and
     at Out the lines under it with what they stand for. Over a line of the
     hovered card, that card stays hovered. */
  function hoverAt(p) {
    var L = level(S.zoom), lay = LY()[L], m = [S.pan[0] + p[0] / S.zoom, S.pan[1] + p[1] / S.zoom];
    var c = cardAt(lay, m[0], m[1]), hits = [];
    if (!c) {
      var tol = 6 / S.zoom;
      shown(lay, focusOf(L)).forEach(function (l) {
        var d = Infinity;
        for (var i = 1; i < l.points.length; i++) {
          var a = l.points[i - 1], b = l.points[i];
          var qx = Math.min(Math.max(m[0], Math.min(a[0], b[0])), Math.max(a[0], b[0]));
          var qy = Math.min(Math.max(m[1], Math.min(a[1], b[1])), Math.max(a[1], b[1]));
          d = Math.min(d, Math.hypot(qx - m[0], qy - m[1]));
        }
        if (d <= tol) hits.push([d, l]);
      });
      var best = Math.min.apply(null, hits.map(function (h) { return h[0]; }).concat([Infinity]));
      hits = hits.filter(function (h) { return h[0] <= best + 0.5; }).map(function (h) { return h[1]; });
    }
    var hover = c ? c.item : hits.length ? S.hover : null;
    var line = null;
    if (L === 'out' && hits.length) {
      var name = function (it) {
        var k = A.blockKey(M, it);
        return k.indexOf('folder:') === 0 ? (k.slice(7) || 'Notes') : M.notes[it.note].title;
      };
      var n = hits.reduce(function (s2, l) { return s2 + l.links; }, 0), plural = function (k, a, b) { return k + ' ' + (k === 1 ? a : b); };
      var to = hits.map(function (l) { return name(l.to); });
      line = {
        ends: hits.map(function (l) { return [key(l.from), key(l.to)]; }), at: p,
        from: name(hits[0].from),
        to: (to.length <= 3 ? to.join(', ') : plural(to.length, 'other', 'others')) + ' · ' + plural(n, 'link', 'links')
      };
      line.words = line.from + '|' + line.to;
    }
    var same = (hover ? key(hover) : '') === (S.hover ? key(S.hover) : '') && !line === !S.hoverLine && (!line || line.words === S.hoverLine.words);
    S.hover = hover; S.hoverLine = line;
    if (!same || line) redrawLines();
  }
  /* The lifted block: the real card under the pointer, raised; the others
     sliding aside to open where it lands, shown as a sunk slot. */
  function drawLift(wrap, d, p) {
    var world = wrap.querySelector('.world'), lay = LY()[d.L], r = A.blockRect(M, lay, d.key);
    if (!r) return;
    var m = [S.pan[0] + p[0] / S.zoom, S.pan[1] + p[1] / S.zoom];
    if (!d.grab) {
      d.grab = [m[0] - r.x, m[1] - r.y];
      S.hover = null; S.hoverLine = null;
      S.lifting = true;
      wrap.classList.add('lifting');
      redrawLines();
    }
    var arr = A.settle(M, S.arr || S.def || M.default_arrangement);
    var t = A.dropAt(M, lay, arr, d.key, m[0], m[1]);
    var blocks = world.querySelectorAll('.blk');
    if (!d.target || d.target[0] !== t[0] || d.target[1] !== t[1]) {
      d.target = t;
      // Where every block goes with the lifted one dropped there.
      var pv = A.preview(M, A.moveTo(arr, d.key, t[0], t[1]), d.L);
      blocks.forEach(function (b) {
        var k = b.getAttribute('data-key');
        if (k === d.key) return;
        var a = A.blockRect(M, lay, k), z = A.blockRect(M, pv, k);
        if (!a || !z) return;
        b.style.transition = 'transform ' + SLIDE + 'ms cubic-bezier(.2,.8,.2,1)';
        b.style.transform = 'translate(' + (z.x - a.x) + 'px,' + (z.y - a.y) + 'px)';
      });
      var s2 = A.blockRect(M, pv, d.key), slot = world.querySelector('.slot');
      if (!slot) { slot = document.createElement('div'); slot.className = 'slot'; world.insertBefore(slot, world.firstChild.nextSibling); }
      slot.style.transition = slot.style.left ? 'left ' + SLIDE + 'ms cubic-bezier(.2,.8,.2,1), top ' + SLIDE + 'ms cubic-bezier(.2,.8,.2,1)' : 'none';
      slot.style.left = s2.x + 'px'; slot.style.top = s2.y + 'px'; slot.style.width = s2.w + 'px'; slot.style.height = s2.h + 'px';
      slot.classList.toggle('round', d.L === 'out' || d.key.indexOf('folder:') === 0);
      d.slot = s2;
      // Once the blocks stop, where the focus's lines will run, faint.
      var svg = world.querySelector('svg.links');
      if (svg) svg.innerHTML = '';
      var tgt = t;
      setTimeout(function () {
        if (d.target !== tgt || !S.lifting || !svg) return;
        var keep = S.lay;
        S.lay = { out: pv, mid: pv, in: pv };
        svg.innerHTML = linesSvg(d.L);
        S.lay = keep;
      }, SLIDE + 20);
    }
    var me = world.querySelector('.blk[data-key="' + cssq(d.key) + '"]');
    if (me) {
      me.classList.add('lifted');
      me.style.transition = 'none';
      me.style.transformOrigin = (r.x + d.grab[0]) + 'px ' + (r.y + d.grab[1]) + 'px';
      me.style.transform = 'translate(' + (m[0] - r.x - d.grab[0]) + 'px,' + (m[1] - r.y - d.grab[1]) + 'px) scale(1.03)';
      d.lay = lay; d.r = r;
    }
  }
  function cssq(s) { return String(s).replace(/["\\]/g, '\\$&'); }
  /* Dropped: the card settles into its slot, then the map is laid out. */
  function settle(wrap, d) {
    var world = wrap.querySelector('.world'), me = world.querySelector('.blk[data-key="' + cssq(d.key) + '"]');
    var from = S.arr || S.def || M.default_arrangement, next = A.moveTo(A.settle(M, from), d.key, d.target[0], d.target[1]);
    if (me && d.slot && d.r) {
      me.style.transition = 'transform ' + SETTLE + 'ms cubic-bezier(.2,.8,.2,1)';
      me.style.transform = 'translate(' + (d.slot.x - d.r.x) + 'px,' + (d.slot.y - d.r.y) + 'px) scale(1)';
    }
    setTimeout(function () {
      var whole = showsAll();
      S.lifting = false;
      // A move is undone, not confirmed.
      var prev = S.arr, moved = !A.same(A.settle(M, from), next);
      setArr(next);
      if (moved) { S.prevArr = prev; S.toast = (d.key.indexOf('folder:') === 0 ? (d.key.slice(7) || 'Notes') : d.key.slice(5).replace(/^.*\//, '').replace(/\.md$/, '')) + ' moved.'; S.toastUndo = true; var tw = S.toast; setTimeout(function () { if (S.toast === tw) { S.toast = ''; S.toastUndo = false; render(); } }, 6000); }
      wrap.classList.remove('lifting');
      render();
      // The zoom stays: the cards keep their size under the pointer.
      void whole;
    }, SETTLE);
  }
  /* Put back: every block flies home, the lifted one too. */
  function putBack(wrap, d) {
    var world = wrap.querySelector('.world');
    world.querySelectorAll('.blk').forEach(function (b) {
      b.style.transition = 'transform ' + BACK + 'ms cubic-bezier(.2,.8,.2,1)';
      b.style.transform = '';
    });
    var slot = world.querySelector('.slot');
    if (slot) slot.remove();
    setTimeout(function () { clearLift(wrap); redrawLines(); }, BACK);
  }
  function clearLift(wrap) {
    S.lifting = false;
    wrap.classList.remove('lifting');
    wrap.querySelectorAll('.slot').forEach(function (x) { x.remove(); });
    wrap.querySelectorAll('.blk').forEach(function (b) { b.classList.remove('lifted'); b.style.transition = ''; b.style.transform = ''; });
  }
  // Escape puts a lifted block back; ⌥ and an arrow move the selected
  // note's block a column or a place.
  document.addEventListener('keydown', function (e) {
    if (S.view !== 'brain' || !A) return;
    if (e.key === 'Escape') {
      var wrap = app.querySelector('.mapwrap');
      if (wrap && wrap.__cancel) wrap.__cancel();
      return;
    }
    if (!e.altKey || e.metaKey || e.ctrlKey) return;
    var dir = { ArrowLeft: 'left', ArrowRight: 'right', ArrowUp: 'up', ArrowDown: 'down' }[e.key];
    var i = selIndex();
    if (!dir || i < 0) return;
    e.preventDefault();
    var key = A.blockKey(M, { note: i });
    setArr(A.nudge(A.settle(M, S.arr || S.def || M.default_arrangement), key, dir));
    render();
  });
  function zoomInto(item) {
    var wrap = app.querySelector('.mapwrap'), w = wrap.clientWidth, h = wrap.clientHeight;
    var L = item.folder != null ? 'mid' : 'in', target = item.folder != null ? 1 : IN_ABOVE + 0.2;
    var r = null;
    if (item.folder != null) LY().mid.frames.forEach(function (f) { if (f[0] === item.folder) r = f[1]; });
    else { var c = cardOf(LY()[L], item); r = c && c.rect; }
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
    else if (act === 'zin' || act === 'zout') {
      // About the selected card when it is in view; else the view's middle.
      var v = appView(), at = [v[0] / 2, v[1] / 2], si = selIndex();
      var sc = si >= 0 ? cardOf(LY()[level(S.zoom)], sameItem({ note: si }, level(S.zoom))) : null;
      if (sc) { var cx = (sc.rect.x + sc.rect.w / 2 - S.pan[0]) * S.zoom, cy = (sc.rect.y + Math.min(sc.rect.h, 112) / 2 - S.pan[1]) * S.zoom; if (cx > 0 && cy > 0 && cx < v[0] && cy < v[1]) at = [cx, cy]; }
      zoomAt(S.zoom * (act === 'zin' ? 1.25 : 0.8), at[0], at[1]);
    }
    else if (act === 'fit') { fit(); }
    else if (act === 'treemap') { toast('Treemap is in the app.'); }
    else if (act === 'reset' && !S.arr) { /* nothing to reset */ }
    else if (act === 'reset') { S.prevArr = S.arr; setArr(null); toast('Layout reset to the default.', true); }
    else if (act === 'undo-layout') { setArr(S.prevArr); S.toast = ''; S.toastUndo = false; render(); }
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
