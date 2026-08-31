// The /a/stats browser view — one self-contained dark HTML document (no build
// step, no framework, no external assets) that polls /a/api/stats/summary
// (60 s) and /a/api/stats/live (10 s) and renders the anonymous web analytics
// for quietmap.org. The world map is a
// zero-dependency canvas scatter over the embedded 0.5° land mask
// (web-stats-land-grid.ts) — deliberately NOT MapLibre for an admin page.
// Clicking a country row filters the page to that country's slice (live
// log-window data; the URL param ?country=XX makes the filter shareable).
import { COUNTRY_BBOX, COUNTRY_NAME, LAND_GRID_BASE64, LAND_GRID_HEIGHT, LAND_GRID_WIDTH } from './web-stats-land-grid.js'

export function statsPage(): string {
  const staticData = JSON.stringify({
    land: LAND_GRID_BASE64,
    gridW: LAND_GRID_WIDTH,
    gridH: LAND_GRID_HEIGHT,
    bbox: COUNTRY_BBOX,
    names: COUNTRY_NAME,
  }).replace(/</g, '\\u003c')
  return `<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Quiet Map — web stats</title>
<style>
:root{--bg:#0b101c;--pan:#121a2c;--ln:#1d2740;--fg:#e7ecf6;--dim:#7e8aa6;--grn:#36d07a;--red:#ff5d5d;--cy:#37c7c7;--yel:#ffd11a}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:14px/1.45 ui-monospace,Menlo,Consolas,monospace}
header{display:flex;align-items:center;gap:14px;padding:11px 18px;background:#080c16;border-bottom:1px solid var(--ln)}
header h1{font-size:17px;margin:0;font-weight:700}
header .right{margin-left:auto;font-size:11px;color:var(--dim);white-space:nowrap}
.dot{width:10px;height:10px;border-radius:50%;display:inline-block;background:var(--dim)}
.dot.live{background:var(--grn);box-shadow:0 0 8px var(--grn)}
main{padding:16px;max-width:1400px;margin:0 auto}
.notice{background:#2a1a1a;border:1px solid var(--red);border-radius:6px;padding:8px 12px;margin-bottom:12px;font-size:12px}
.filterchip{display:none;align-items:center;gap:8px;background:#0e1f2a;border:1px solid #18313e;border-radius:6px;padding:5px 10px;margin-bottom:12px;font-size:12px;color:var(--cy);cursor:pointer}
.filterchip:hover{border-color:var(--cy)}
.cards{display:grid;grid-template-columns:repeat(auto-fill,minmax(150px,1fr));gap:10px;margin-bottom:14px}
.card{background:var(--pan);border:1px solid var(--ln);border-radius:7px;padding:10px 12px}
.card .lbl{font-size:11px;color:var(--dim);white-space:nowrap}
.card .val{font-size:24px;font-weight:700;font-variant-numeric:tabular-nums;margin-top:2px}
.card .delta{font-size:11px;margin-top:1px;font-variant-numeric:tabular-nums}
.up{color:var(--grn)}.down{color:var(--red)}.flat{color:var(--dim)}
section{margin-bottom:16px}
h3.sec{margin:0 0 8px;font-size:13px;color:var(--dim);font-weight:600;display:flex;align-items:center;gap:10px;flex-wrap:wrap}
h3.sec .note{font-weight:400;font-size:11px}
.switch{margin-left:auto;display:inline-flex;gap:2px}
.switch button{background:#0e1626;border:1px solid var(--ln);color:var(--dim);font:inherit;font-size:11px;padding:2px 9px;cursor:pointer;border-radius:4px}
.switch button.on{background:#0e1f2a;color:var(--cy);border-color:#18313e}
canvas.chart,canvas.map{width:100%;display:block;background:var(--pan);border:1px solid var(--ln);border-radius:7px}
.tgrid{display:grid;grid-template-columns:repeat(auto-fill,minmax(430px,1fr));gap:12px}
.tbox{background:var(--pan);border:1px solid var(--ln);border-radius:7px;padding:10px 12px}
.tbox h4{margin:0 0 6px;font-size:12px;color:var(--dim);font-weight:600;display:flex;gap:8px;align-items:center}
.tbox h4 .switch button{padding:1px 7px}
.note{color:var(--dim);font-weight:400;font-size:11px}
table.tt{width:100%;border-collapse:collapse;font-size:12px;font-variant-numeric:tabular-nums}
.tt th{text-align:right;color:var(--dim);font-weight:600;padding:2px 8px 4px;border-bottom:1px solid var(--ln);font-size:10px}
.tt th.l{text-align:left}
.tt td{padding:3px 8px;border-bottom:1px solid #141d30;text-align:right}
.tt td.l{text-align:left}
.tt tr.pick{cursor:pointer}.tt tr.pick:hover td{background:#0e1830}
.tt tr.sel td{background:#0e1f2a}
.badge{color:#080c16;background:var(--grn);border-radius:3px;font-size:9px;font-weight:700;padding:0 4px;margin-left:5px}
.more{display:inline-block;margin-top:5px;font-size:11px;color:var(--cy);cursor:pointer}
.empty{color:var(--dim);font-size:12px;padding:6px 2px}
.ins{list-style:none;margin:0;padding:0}
.ins li{padding:3px 0 3px 16px;position:relative;font-size:13px}
.ins li::before{content:'•';color:var(--cy);position:absolute;left:2px}
.feed{list-style:none;margin:0;padding:0;font-size:12px}
.feed li{padding:2px 0;border-bottom:1px solid #141d30;display:flex;gap:8px;white-space:nowrap;overflow:hidden}
.feed .t{color:var(--dim);flex:none;font-variant-numeric:tabular-nums}
.feed .a{color:var(--fg);overflow:hidden;text-overflow:ellipsis}
@media(max-width:680px){main{padding:8px}.cards{grid-template-columns:repeat(2,1fr)}.tgrid{grid-template-columns:1fr}}
</style></head><body>
<header><span id="dot" class="dot"></span><h1>Quiet Map<span> — web stats</span></h1><span class="right" id="gen"></span></header>
<main>
<div id="notice" class="notice" style="display:none"></div>
<div id="filterchip" class="filterchip"></div>
<div class="cards" id="cards"></div>
<section><h3 class="sec">Traffic <span class="note" id="trafficnote"></span><span class="switch" id="trafswitch">
<button data-m="today" class="on">today</button><button data-m="7d">7 days</button><button data-m="30d">30 days</button></span></h3>
<canvas class="chart" id="chart" height="170"></canvas></section>
<section><h3 class="sec">Popup clicks on the map <span class="note">~1 km cells, weighted by opens</span></h3>
<canvas class="map" id="map"></canvas></section>
<section><h3 class="sec">Insights <span class="note">auto-generated from the aggregates, each with its number</span></h3>
<ul class="ins" id="insights"></ul></section>
<section><h3 class="sec">Breakdown <span class="note">click a country to filter the whole page</span></h3>
<div class="tgrid">
<div class="tbox"><h4>Countries</h4><div id="tcountries"></div></div>
<div class="tbox"><h4>Referers <span class="note">page loads by source domain</span></h4><div id="treferers"></div></div>
<div class="tbox"><h4>Devices</h4><div id="tdevices"></div></div>
<div class="tbox"><h4>Search terms <span class="note">k≥3 anonymous only</span><span class="switch" id="termswitch">
<button data-m="today" class="on">today</button><button data-m="week">week</button></span></h4><div id="tterms"></div></div>
</div></section>
<section><h3 class="sec">Live <span class="note">last 20 human events · refreshes every 10 s · anonymous: country + browser family only, never an IP, live search terms never shown</span></h3>
<div class="tbox"><ul class="feed" id="feed"></ul></div></section>
</main>
<script>window.STATS_STATIC=${staticData};</script>
<script>
'use strict';
const S = window.STATS_STATIC;
const $ = (id) => document.getElementById(id);
const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const fmt = (n) => n == null ? '—' : Number(n).toLocaleString('en-US');
const flag = (code) => /^[A-Z]{2}$/.test(code) ? String.fromCodePoint(...[...code].map((c) => 0x1F1E6 + c.charCodeAt(0) - 65)) : '🏳';
const countryLabel = (code) => flag(code) + ' ' + (S.names[code] ? esc(S.names[code]) : code);

let summary = null;
let live = null;
let chartMode = 'today';
let termMode = 'today';
const expanded = new Set();
let country = new URLSearchParams(location.search).get('country');
if (country && !/^[A-Z]{2}$/.test(country)) country = null;

function deltaHtml(now, prev, invert) {
  if (now == null || prev == null || prev === 0) return '<span class="flat">— vs prev day</span>';
  const pct = Math.round(((now - prev) / prev) * 100);
  if (pct === 0) return '<span class="flat">= prev day</span>';
  const up = pct > 0;
  const good = invert ? !up : up;
  return '<span class="' + (good ? 'up' : 'down') + '">' + (up ? '▲ +' : '▼ ') + pct + '%</span><span class="flat"> vs prev day</span>';
}

function renderCards() {
  const t = summary && summary.today, p = summary && summary.previous;
  const slice = summary && summary.slice;
  const online = live && live.ok ? live.onlineNow : null;
  const botShare = t && (t.requests + t.botRequests) > 0 ? Math.round((t.botRequests / (t.requests + t.botRequests)) * 1000) / 10 : null;
  const cards = [
    { lbl: 'Online now (5 min)', val: fmt(online), delta: '<span class="flat">live</span>' },
    slice
      ? { lbl: 'Visitors today · ' + esc(summary.filter.country), val: fmt(slice.visitors), delta: '<span class="flat">live window</span>' }
      : { lbl: 'Visitors today', val: t ? fmt(t.visitors) : '—', delta: deltaHtml(t && t.visitors, p && p.visitors) },
    slice
      ? { lbl: 'Popup opens · ' + esc(summary.filter.country), val: fmt(slice.popupOpens), delta: '<span class="flat">live window</span>' }
      : { lbl: 'Popup opens today', val: t ? fmt(t.popupOpens) : '—', delta: deltaHtml(t && t.popupOpens, p && p.popupOpens) },
    slice
      ? { lbl: 'Searches · ' + esc(summary.filter.country), val: fmt(slice.searches), delta: '<span class="flat">live window</span>' }
      : { lbl: 'Searches today', val: t ? fmt(t.searches) : '—', delta: deltaHtml(t && t.searches, p && p.searches) },
    { lbl: 'Countries today', val: summary && summary.dbAvailable ? fmt(summary.countriesToday) : '—',
      delta: deltaHtml(summary && summary.countriesToday, p && p.countries) },
    { lbl: 'Bot share of requests', val: botShare == null ? '—' : botShare + '%', delta: '<span class="flat">transparency, not alarm</span>' },
  ];
  $('cards').innerHTML = cards.map((c) =>
    '<div class="card"><div class="lbl">' + c.lbl + '</div><div class="val">' + c.val + '</div><div class="delta">' + c.delta + '</div></div>').join('');
}

function drawChart() {
  const cv = $('chart');
  const dpr = window.devicePixelRatio || 1;
  const w = cv.clientWidth, h = 170;
  cv.width = w * dpr; cv.height = h * dpr;
  const ctx = cv.getContext('2d');
  ctx.scale(dpr, dpr);
  ctx.clearRect(0, 0, w, h);
  if (!summary || !summary.dbAvailable) return;
  let values, avgLine = null, labels = null;
  if (chartMode === 'today') {
    values = summary.slice ? summary.slice.hours : summary.hoursToday;
    avgLine = summary.slice ? null : summary.hoursAvg7;
    labels = (i) => i % 6 === 0 ? String(i) : '';
  } else {
    const days = chartMode === '7d' ? summary.days.slice(-7) : summary.days;
    values = days.map((d) => d.visitors);
    labels = (i) => days[i].day.slice(8);
  }
  const max = Math.max(1, ...values, ...(avgLine || [0]));
  const n = values.length;
  const bw = w / n;
  ctx.font = '9px ui-monospace,monospace';
  for (let i = 0; i < n; i++) {
    const bh = Math.max(1, (values[i] / max) * (h - 18));
    ctx.fillStyle = '#228451';
    ctx.fillRect(i * bw + 1, h - bh, Math.max(1, bw - 2), bh);
    const lab = labels(i);
    if (lab && (chartMode !== '30d' || i % 5 === 0)) {
      ctx.fillStyle = '#7e8aa6';
      ctx.fillText(lab, i * bw + 2, h - 3);
    }
  }
  if (avgLine) {
    ctx.strokeStyle = '#37c7c7';
    ctx.beginPath();
    for (let i = 0; i < n; i++) {
      const y = h - Math.max(1, (avgLine[i] / max) * (h - 18));
      if (i === 0) ctx.moveTo(bw / 2, y); else ctx.lineTo(i * bw + bw / 2, y);
    }
    ctx.stroke();
  }
  $('trafficnote').textContent = chartMode === 'today'
    ? (summary.slice ? 'requests by UTC hour today · live window' : 'human requests by UTC hour · cyan = 7-day hourly average')
    : 'visitors per day';
}

let landBits = null;
function landGrid() {
  if (!landBits) {
    const raw = atob(S.land);
    landBits = new Uint8Array(raw.length);
    for (let i = 0; i < raw.length; i++) landBits[i] = raw.charCodeAt(i);
  }
  return landBits;
}

function drawMap() {
  const cv = $('map');
  const dpr = window.devicePixelRatio || 1;
  const w = cv.clientWidth, h = Math.round(w / 2);
  cv.width = w * dpr; cv.height = h * dpr;
  cv.style.height = h + 'px';
  const ctx = cv.getContext('2d');
  ctx.scale(dpr, dpr);
  ctx.clearRect(0, 0, w, h);
  const bits = landGrid(), gw = S.gridW, gh = S.gridH;
  const cw = w / gw, ch = h / gh;
  const bb = summary && summary.filter.country ? S.bbox[summary.filter.country] : null;
  ctx.fillStyle = bb ? '#161f33' : '#232f4d';
  for (let r = 0; r < gh; r++) {
    for (let c = 0; c < gw; c++) {
      const bit = r * gw + c;
      if (bits[bit >> 3] >> (bit & 7) & 1) ctx.fillRect(c * cw, r * ch, cw + 0.4, ch + 0.4);
    }
  }
  if (bb) { // highlight the filtered country's bbox (best effort: antimeridian boxes span full width)
    const c0 = Math.floor(((bb[0] + 180) / 360) * gw), c1 = Math.ceil(((bb[2] + 180) / 360) * gw);
    const r0 = Math.floor(((90 - bb[3]) / 180) * gh), r1 = Math.ceil(((90 - bb[1]) / 180) * gh);
    ctx.fillStyle = '#2e3d63';
    for (let r = Math.max(0, r0); r <= Math.min(gh - 1, r1); r++) {
      for (let c = Math.max(0, c0); c <= Math.min(gw - 1, c1); c++) {
        const bit = r * gw + c;
        if (bits[bit >> 3] >> (bit & 7) & 1) ctx.fillRect(c * cw, r * ch, cw + 0.4, ch + 0.4);
      }
    }
  }
  const cells = summary ? (summary.slice ? summary.slice.cells : summary.popupCells) : [];
  if (cells && cells.length > 0) {
    const maxOpens = Math.max(...cells.map((c) => c.opens));
    for (const cell of cells) {
      const x = ((cell.lng + 180) / 360) * w, y = ((90 - cell.lat) / 180) * h;
      const rad = 2.5 + 5 * Math.sqrt(cell.opens / maxOpens);
      ctx.beginPath(); ctx.arc(x, y, rad * 2.2, 0, 7); ctx.fillStyle = 'rgba(55,199,199,0.13)'; ctx.fill();
      ctx.beginPath(); ctx.arc(x, y, rad, 0, 7); ctx.fillStyle = 'rgba(55,199,199,0.9)'; ctx.fill();
    }
  } else {
    ctx.fillStyle = '#7e8aa6'; ctx.font = '12px ui-monospace,monospace'; ctx.textAlign = 'center';
    ctx.fillText(summary && !summary.dbAvailable ? 'aggregate DB unavailable' : 'No popup clicks logged yet — cell logging started 2026-07-19', w / 2, 24);
    ctx.textAlign = 'left';
  }
}

function tableHtml(rows, cols, idPrefix, key) {
  if (rows.length === 0) return '<div class="empty">(no data yet)</div>';
  const limit = expanded.has(key) ? rows.length : Math.min(8, rows.length);
  let html = '<table class="tt"><tr>' + cols.map((c) => '<th class="' + (c.l ? 'l' : '') + '">' + c.h + '</th>').join('') + '</tr>';
  for (let i = 0; i < limit; i++) html += rows[i];
  html += '</table>';
  if (rows.length > 8) html += '<span class="more" data-k="' + key + '">' + (expanded.has(key) ? 'show less' : 'show all ' + rows.length + ' …') + '</span>';
  return html;
}

function renderTables() {
  if (!summary || !summary.dbAvailable) {
    for (const id of ['tcountries', 'treferers', 'tdevices', 'tterms']) $(id).innerHTML = '<div class="empty">aggregate DB unavailable</div>';
    return;
  }
  const total = Math.max(1, summary.today.requests);
  const rows = summary.countries.map((c) => {
    const sel = summary.filter.country === c.code;
    return '<tr class="pick' + (sel ? ' sel' : '') + '" data-cc="' + c.code + '"><td class="l">' + countryLabel(c.code) +
      '</td><td>' + fmt(c.requests) + '</td><td>' + c.sharePct + '%</td></tr>';
  });
  $('tcountries').innerHTML = tableHtml(rows, [{ h: 'country', l: 1 }, { h: 'requests' }, { h: 'share' }], '', 'countries');

  let refRows, refCols;
  if (summary.slice) {
    const entries = Object.entries(summary.slice.referers);
    refCols = [{ h: 'domain', l: 1 }, { h: 'visits' }];
    refRows = entries.map(([d, v]) => '<tr><td class="l">' + esc(d) + '</td><td>' + fmt(v) + '</td></tr>');
  } else {
    refCols = [{ h: 'domain', l: 1 }, { h: 'visits' }, { h: 'first seen' }];
    refRows = summary.referers.map((r) => '<tr><td class="l">' + esc(r.domain) + (r.isNew ? '<span class="badge">NEW</span>' : '') +
      '</td><td>' + fmt(r.visits) + '</td><td class="flat">' + r.firstSeen.slice(5) + '</td></tr>');
  }
  $('treferers').innerHTML = tableHtml(refRows, refCols, '', 'referers');

  let devRows, devCols;
  if (summary.slice) {
    devCols = [{ h: 'device', l: 1 }, { h: 'requests' }, { h: 'popups' }, { h: 'searches' }];
    devRows = Object.entries(summary.slice.devices).map(([d, v]) =>
      '<tr><td class="l">' + esc(d) + '</td><td>' + fmt(v) + '</td><td>' + fmt(summary.slice.devicePopups[d] || 0) + '</td><td>' + fmt(summary.slice.deviceSearches[d] || 0) + '</td></tr>');
  } else {
    devCols = [{ h: 'device', l: 1 }, { h: 'requests' }, { h: 'share' }];
    devRows = summary.devices.map((d) => '<tr><td class="l">' + esc(d.device) + '</td><td>' + fmt(d.requests) + '</td><td>' + d.sharePct + '%</td></tr>');
  }
  $('tdevices').innerHTML = tableHtml(devRows, devCols, '', 'devices');

  const terms = termMode === 'today' ? summary.searchTermsToday : summary.searchTermsWeek;
  $('tterms').innerHTML = tableHtml(
    terms.map((t) => '<tr><td class="l">' + esc(t.term) + '</td><td>' + fmt(t.searches) + '</td></tr>'),
    [{ h: 'term', l: 1 }, { h: 'searches' }], '', 'terms');
}

function renderInsights() {
  const list = summary && summary.insights ? summary.insights : [];
  $('insights').innerHTML = list.length === 0
    ? '<li class="flat">Nothing noteworthy yet — insights appear as the aggregates grow.</li>'
    : list.map((i) => '<li>' + esc(i.text) + '</li>').join('');
}

function renderLive() {
  const dot = $('dot');
  if (live && live.ok) {
    dot.className = 'dot live';
    const events = live.events.slice().reverse();
    $('feed').innerHTML = events.length === 0
      ? '<li><span class="a flat">no human events in the recent window</span></li>'
      : events.map((e) => {
        const d = new Date(e.ts);
        const hh = String(d.getHours()).padStart(2, '0'), mm = String(d.getMinutes()).padStart(2, '0'), ss = String(d.getSeconds()).padStart(2, '0');
        return '<li><span class="t">' + hh + ':' + mm + ':' + ss + '</span><span class="a">' +
          flag(e.country) + ' · ' + esc(e.agent) + ' · ' + esc(e.action) + '</span></li>';
      }).join('');
  } else {
    dot.className = 'dot';
    $('feed').innerHTML = '<li><span class="a flat">live feed unavailable — ' + esc(live && live.error ? live.error : '…') + '</span></li>';
  }
  renderCards();
}

function renderFilterChip() {
  const chip = $('filterchip');
  if (summary && summary.filter.country) {
    chip.style.display = 'inline-flex';
    chip.innerHTML = '<span>Filtered to ' + countryLabel(summary.filter.country) +
      ' — live log window (today so far, approximate; counts from distinct IPs held in memory only)</span><b>× clear</b>';
  } else {
    chip.style.display = 'none';
  }
}

async function loadSummary() {
  try {
    const url = '/a/api/stats/summary' + (country ? '?country=' + country : '');
    summary = await (await fetch(url, { cache: 'no-store' })).json();
  } catch (e) {
    summary = null;
  }
  $('gen').textContent = summary && summary.generatedAt ? 'day ' + (summary.day || '—') + ' · generated ' + summary.generatedAt.slice(11, 19) + 'Z' : '';
  $('notice').style.display = summary && !summary.dbAvailable ? 'block' : 'none';
  if (summary && !summary.dbAvailable) $('notice').textContent = 'Aggregate database missing or empty — showing live data only. Run pipeline/web-stats.ts to build it.';
  renderFilterChip(); renderCards(); drawChart(); drawMap(); renderTables(); renderInsights();
}

async function loadLive() {
  try {
    live = await (await fetch('/a/api/stats/live', { cache: 'no-store' })).json();
  } catch (e) {
    live = { ok: false, error: 'request failed' };
  }
  renderLive();
}

function setCountry(code) {
  country = country === code ? null : code;
  const params = new URLSearchParams(location.search);
  if (country) params.set('country', country); else params.delete('country');
  history.replaceState(null, '', location.pathname + (params.toString() ? '?' + params : ''));
  loadSummary();
}

document.addEventListener('click', (ev) => {
  const row = ev.target.closest('tr.pick');
  if (row && row.dataset.cc) { setCountry(row.dataset.cc); return; }
  const more = ev.target.closest('.more');
  if (more) { expanded.has(more.dataset.k) ? expanded.delete(more.dataset.k) : expanded.add(more.dataset.k); renderTables(); return; }
  if (ev.target.closest('#filterchip')) { setCountry(country); return; }
  const sw = ev.target.closest('#trafswitch button');
  if (sw) { chartMode = sw.dataset.m; [...document.querySelectorAll('#trafswitch button')].forEach((b) => b.className = b === sw ? 'on' : ''); drawChart(); return; }
  const ts = ev.target.closest('#termswitch button');
  if (ts) { termMode = ts.dataset.m; [...document.querySelectorAll('#termswitch button')].forEach((b) => b.className = b === ts ? 'on' : ''); renderTables(); }
});
window.addEventListener('resize', () => { drawChart(); drawMap(); });

loadLive();
loadSummary();
setInterval(loadLive, 10000);
setInterval(loadSummary, 60000);
</script></body></html>`
}
