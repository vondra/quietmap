// The friendly 404 page for browser visitors (API clients keep JSON — web.ts).
// Self-contained by design: one HTML string, inline CSS, zero requests except
// /favicon.svg — referenced, not embedded, so a logo redesign shows up here
// automatically. The noise-ramp stops mirror the map's heatmap palette
// (frontend/src/lib/heatmap-palette.ts, same list as favicon.svg).

export const NOT_FOUND_PAGE_HTML = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<meta name="robots" content="noindex" />
<title>404 — It's quiet here · quietmap.org</title>
<style>
  :root { color-scheme: light; }
  * { margin: 0; box-sizing: border-box; }
  body {
    min-height: 100vh; display: grid; place-items: center; padding: 24px;
    background: #f7f8f7; color: #171717;
    font-family: ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif;
  }
  main { max-width: 460px; text-align: center; }
  img { width: 88px; height: 88px; }
  h1 { margin: 20px 0 10px; font-size: 28px; font-weight: 650; letter-spacing: -0.02em; }
  h1 em { font-style: normal; color: #75085C; }
  .sub { color: #555; font-size: 16px; line-height: 1.5; }
  .meter { margin: 34px 0 6px; position: relative; }
  .ramp {
    height: 12px; border-radius: 6px; border: 1px solid rgba(0,0,0,.10);
    /* same stops as the map heatmap ramp (v5.b, 30–80 dB) — quiet at the left, loud at the right */
    background: linear-gradient(90deg,
      #FFFFFF 0%, #82A6AD 8%, #A0BABF 18%, #B8D6D1 28%, #CEE4CC 38%, #E2F2BF 48%,
      #F3C683 58%, #E87E4D 67%, #CD463E 76%, #A11A4D 85%, #75085C 93%, #430A4A 100%);
  }
  .pin {
    position: absolute; left: 0; top: -19px; font-size: 13px; line-height: 1;
    transform: translateX(-1px);
  }
  .scale { display: flex; justify-content: space-between; margin-top: 7px; font-size: 12.5px; color: #777; }
  .scale .you { color: #171717; font-weight: 600; }
  a.home {
    display: inline-block; margin-top: 30px; padding: 11px 22px; border-radius: 999px;
    background: #171717; color: #fff; text-decoration: none; font-size: 15px; font-weight: 600;
  }
  a.home:hover { background: #430A4A; }
  .foot { margin-top: 26px; font-size: 12.5px; color: #999; }
</style>
</head>
<body>
<main>
  <img src="/favicon.svg" alt="quietmap.org logo" width="88" height="88" />
  <h1>It's quiet here. <em>Too quiet.</em></h1>
  <p class="sub">This address doesn't exist — which makes it the quietest
  spot on the whole map.</p>
  <div class="meter" role="img" aria-label="Noise scale: you are below the quiet end">
    <span class="pin" aria-hidden="true">▼</span>
    <div class="ramp"></div>
    <div class="scale"><span class="you">0 dB · you are here</span><span>80+ dB</span></div>
  </div>
  <a class="home" href="/">← Back to the map</a>
  <p class="foot">404 · quietmap.org — Find your quiet place</p>
</main>
</body>
</html>
`
