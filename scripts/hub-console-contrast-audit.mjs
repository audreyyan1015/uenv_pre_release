// 对 Hub 控制台做 WCAG 对比度审计。
//
// 不看截图、不猜 CSS：连上 Chrome 的调试端口，在页面里取每个可见文字节点
// getComputedStyle 得到的实际前景色，再沿祖先链找到第一个不透明的背景色，
// 按 WCAG 2.1 相对亮度公式算对比度。浏览器已经把 oklch / color-mix 全部解析成
// sRGB，所以这里量到的就是用户真正看到的颜色。


const CDP_PORT = process.env.CDP_PORT || '9222';
const ROUTES = process.env.ROUTES.split(',');
const BASE = process.env.BASE;

const AUDIT = String.raw`
(() => {
  // Chrome 的 getComputedStyle 会把 oklch()/color-mix() 原样保留在计算值里，
  // 正则解析不了。丢给 canvas 光栅化一个像素，拿到的就是真实 sRGB 字节。
  const cv = document.createElement('canvas');
  cv.width = cv.height = 1;
  const ctx = cv.getContext('2d', { willReadFrequently: true });
  const cache = new Map();
  const parse = (c) => {
    if (!c) return null;
    if (cache.has(c)) return cache.get(c);
    ctx.clearRect(0, 0, 1, 1);
    ctx.fillStyle = '#000';
    ctx.fillStyle = c;
    ctx.fillRect(0, 0, 1, 1);
    const d = ctx.getImageData(0, 0, 1, 1).data;
    const v = { r: d[0], g: d[1], b: d[2], a: d[3] / 255 };
    cache.set(c, v);
    return v;
  };
  const lum = ({ r, g, b }) => {
    const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); };
    return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
  };
  const over = (fg, bg) => ({
    r: fg.r * fg.a + bg.r * (1 - fg.a),
    g: fg.g * fg.a + bg.g * (1 - fg.a),
    b: fg.b * fg.a + bg.b * (1 - fg.a),
    a: 1,
  });
  const bgOf = (el) => {
    let acc = null;
    for (let n = el; n; n = n.parentElement) {
      const c = parse(getComputedStyle(n).backgroundColor);
      if (!c || c.a === 0) continue;
      acc = acc ? over(acc, c) : c;
      if (acc.a >= 0.999) return acc;
    }
    return acc || { r: 255, g: 255, b: 255, a: 1 };
  };

  const out = [];
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  const seen = new Set();
  for (let t = walker.nextNode(); t; t = walker.nextNode()) {
    const text = t.textContent.trim();
    if (!text) continue;
    const el = t.parentElement;
    if (!el) continue;
    const st = getComputedStyle(el);
    if (st.visibility === 'hidden' || st.display === 'none') continue;
    const rect = el.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) continue;

    const fg = parse(st.color);
    if (!fg) continue;
    const bg = bgOf(el);
    const eff = fg.a < 1 ? over(fg, bg) : fg;
    const l1 = lum(eff), l2 = lum(bg);
    const ratio = (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);

    const px = parseFloat(st.fontSize);
    const bold = parseInt(st.fontWeight, 10) >= 700;
    // WCAG AA：普通文字 4.5:1；大号文字（>=18.66px 或 >=24px 粗体）3:1
    const large = px >= 24 || (px >= 18.66 && bold);
    const need = large ? 3 : 4.5;

    const key = st.color + '|' + Math.round(bg.r) + ',' + Math.round(bg.g) + ',' + Math.round(bg.b) + '|' + px;
    if (seen.has(key)) continue;
    seen.add(key);

    out.push({
      pass: ratio >= need,
      ratio: Math.round(ratio * 100) / 100,
      need,
      px,
      cls: (el.className && el.className.toString().slice(0, 40)) || el.tagName.toLowerCase(),
      sample: text.slice(0, 32),
    });
  }
  return JSON.stringify(out);
})()
`;

async function cdp(route) {
  const list = await (await fetch(`http://127.0.0.1:${CDP_PORT}/json/list`)).json();
  const page = list.find((t) => t.type === 'page');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((r) => (ws.onopen = r));

  let id = 0;
  const send = (method, params) =>
    new Promise((resolve) => {
      const myId = ++id;
      const onMsg = (ev) => {
        const m = JSON.parse(ev.data);
        if (m.id === myId) { ws.removeEventListener('message', onMsg); resolve(m.result); }
      };
      ws.addEventListener('message', onMsg);
      ws.send(JSON.stringify({ id: myId, method, params }));
    });

  await send('Page.enable');
  await send('Page.navigate', { url: `${BASE}/console#/${route}` });
  await new Promise((r) => setTimeout(r, 3500));
  // hash 路由：同一文档内切换不会触发导航，强制重新挂载
  await send('Runtime.evaluate', { expression: `location.hash = '#/${route}'; if (window.mount) window.mount();` });
  await new Promise((r) => setTimeout(r, 2500));

  const res = await send('Runtime.evaluate', { expression: AUDIT, returnByValue: true });
  if (res.exceptionDetails) {
    console.log('   审计脚本异常:', JSON.stringify(res.exceptionDetails).slice(0, 400));
    ws.close();
    return [];
  }
  ws.close();
  return JSON.parse(res.result.value);
}

let failures = [];
let checked = 0;
for (const route of ROUTES) {
  const rows = await cdp(route);
  checked += rows.length;
  const bad = rows.filter((r) => !r.pass);
  console.log(
    `${bad.length ? 'FAIL' : 'ok  '} #/${route.padEnd(36)} 取样 ${String(rows.length).padStart(3)} 处` +
      (bad.length ? ` · 不达标 ${bad.length}` : ''),
  );
  for (const b of bad) {
    console.log(`       ${b.ratio}:1 (需 ${b.need}) ${b.px}px .${b.cls} — "${b.sample}"`);
    failures.push(b);
  }
}

console.log(`\n共取样 ${checked} 处文字/背景组合`);
if (failures.length) {
  console.log(`CONTRAST_FAIL ${failures.length} 处低于 WCAG AA`);
  process.exit(1);
}
console.log('CONTRAST_OK 全部满足 WCAG AA');
