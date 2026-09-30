// 設定ページの画面写真 (0.5.0〜、docs/settings-server.md「プレビュー」): mock_server.py を動かした状態で
//
//   NODE_PATH=$(npm root -g) node tools/settings-mock/screenshots.mjs <出力ディレクトリ> [切り抜きに使う写真]
//
// デスクトップ (1280 幅) / スマートフォン (390 幅) の全体と、写真の追加 (切り抜き) の画面を PNG にする。
// 都市の検索は Open-Meteo の代わりに決まった結果を返す (オフラインでも同じ絵になるように)。
import { chromium } from 'playwright';
import path from 'node:path';

const out = process.argv[2] || 'out';
const photo = process.argv[3];
const base = process.env.MOCK_URL || 'http://127.0.0.1:8080/';

const geocode = {
  results: [
    { name: '札幌市', admin1: '北海道', country: '日本', latitude: 43.0642, longitude: 141.3469, timezone: 'Asia/Tokyo' },
    { name: '札幌村', admin1: '北海道', country: '日本', latitude: 43.0833, longitude: 141.3667, timezone: 'Asia/Tokyo' },
  ],
};

async function page(browser, opts) {
  const ctx = await browser.newContext(opts);
  const p = await ctx.newPage();
  await p.route('https://geocoding-api.open-meteo.com/**', (r) => r.fulfill({ json: geocode, headers: { 'Access-Control-Allow-Origin': '*' } }));
  // 時計を止めて毎回同じ絵にする (2026-09-30 21:53:44 JST)
  await p.clock.setFixedTime(new Date('2026-09-30T12:53:44Z'));
  await p.goto(base);
  await p.waitForFunction(() => document.querySelectorAll('.photo canvas').length > 0);
  await p.waitForTimeout(1500); // サムネイル
  return { ctx, p };
}

/** 画面下の保存の帯 (position: fixed) がページの途中に写らないよう、窓の高さをページ全体に合わせて撮る */
async function full(p, file) {
  const vp = p.viewportSize();
  const h = await p.evaluate(() => document.documentElement.scrollHeight);
  await p.setViewportSize({ width: vp.width, height: h });
  await p.screenshot({ path: path.join(out, file) });
  await p.setViewportSize(vp);
}

const browser = await chromium.launch();

// --- デスクトップ: 未保存の変更がある状態 (都市を検索して札幌を選んだところ) ---
{
  const { ctx, p } = await page(browser, { viewport: { width: 1280, height: 900 }, deviceScaleFactor: 1 });
  await p.fill('#city-q', '札幌');
  await p.click('#city-go');
  await p.waitForSelector('#city-results button');
  await full(p, 'settings-desktop-search.png');
  await p.click('#city-results button');
  await p.click('label:has(#l-dock)');
  await full(p, 'settings-desktop.png');
  await ctx.close();
}

// --- 流れる文字の欄 (0.5.1〜: 設定 URL とコードを流れる文字に入れるかの切り替えと、その注意) ---
{
  const { ctx, p } = await page(browser, { viewport: { width: 1280, height: 900 }, deviceScaleFactor: 2 });
  // 保存の帯 (position: fixed) が欄に重ならないよう、窓をページ全体の高さにする
  await p.setViewportSize({ width: 1280, height: await p.evaluate(() => document.documentElement.scrollHeight + 120) });
  await p.locator('#panel-message').screenshot({ path: path.join(out, 'settings-message.png') });
  await p.uncheck('#f-show_settings');
  await p.locator('#panel-message').screenshot({ path: path.join(out, 'settings-message-off.png') });
  await ctx.close();
}

// --- スマートフォン ---
{
  const { ctx, p } = await page(browser, { viewport: { width: 390, height: 844 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true });
  await full(p, 'settings-mobile.png');
  await ctx.close();
}

// --- 写真の追加 (切り抜き) ---
if (photo) {
  const { ctx, p } = await page(browser, { viewport: { width: 1100, height: 820 }, deviceScaleFactor: 1 });
  await p.setInputFiles('#file', photo);
  await p.waitForSelector('#dlg-crop:not([hidden])');
  // 少し拡大して右へずらす (ドラッグの操作と同じ値を入れる)
  await p.fill('#crop-zoom', '1.35');
  await p.dispatchEvent('#crop-zoom', 'input');
  const box = await p.locator('#crop-view').boundingBox();
  await p.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await p.mouse.down();
  await p.mouse.move(box.x + box.width / 2 - 120, box.y + box.height / 2 + 10, { steps: 6 });
  await p.mouse.up();
  await p.check('#crop-guide-on');
  await p.screenshot({ path: path.join(out, 'settings-crop.png') });
  // 送って一覧に入るところまで (コードの入力も)
  await p.click('#crop-ok');
  await p.waitForSelector('#dlg-code:not([hidden])');
  await p.fill('#code-input', '123456');
  await p.screenshot({ path: path.join(out, 'settings-code.png') });
  await p.click('#code-form button[type=submit]');
  await p.waitForSelector('#dlg-crop', { state: 'hidden', timeout: 20000 });
  await p.waitForTimeout(800);
  await p.locator('#panel-photos').screenshot({ path: path.join(out, 'settings-photos-after-upload.png') });
  await ctx.close();
}

await browser.close();
console.log('screenshots in', out);
