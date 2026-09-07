// Live hover regression: Electra One example on 8745, shared daemon and attached Mini.
// No parameter edits. Delay only incoming screen reports to prove local input stays responsive.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { chromium } from '../third_party/av-frameworks/web/node_modules/playwright/index.mjs';
const output = process.argv[2] ?? '/tmp/surface-hover';
fs.mkdirSync(output, { recursive: true });
const browser = await chromium.launch();
try {
  const page = await browser.newPage({ viewport: { width: 1500, height: 1000 } });
  await page.addInitScript(() => {
    window.holdSurfaceReplies = false;
    window.surfaceReplyQueue = [];
    window.surfaceHoverTimes = [];
    const handler = Object.getOwnPropertyDescriptor(WebSocket.prototype, 'onmessage');
    Object.defineProperty(WebSocket.prototype, 'onmessage', {
      configurable: true,
      get: handler.get,
      set(callback) {
        handler.set.call(this, event => {
          const bytes = event.data instanceof ArrayBuffer ? new Uint8Array(event.data) : null;
          if (window.holdSurfaceReplies && bytes?.[0] === 6 && bytes[1] === 5)
            window.surfaceReplyQueue.push(() => callback.call(this, event));
          else callback.call(this, event);
        });
      },
    });
    document.addEventListener('pointerover', event => {
      const target = event.target.closest?.('[data-av-param-path]')
        ?? event.target.closest?.('.row')?.querySelector('[data-av-param-path]');
      if (!target) return;
      const path = target.getAttribute('data-av-param-path'), start = performance.now();
      requestAnimationFrame(() => window.surfaceHoverTimes.push({
        path, ms: performance.now() - start,
        state: (target.closest('.row') ?? target).getAttribute('data-av-surface-requested'),
      }));
    }, true);
  });
  await page.goto('http://127.0.0.1:8745');
  const control = path => page.locator(`[data-av-param-path="${path}"]`).first();
  const state = path => control(path).evaluate(el => (el.closest('.row') ?? el).getAttribute('data-av-surface-requested'));
  const waitReady = async path => {
    await page.waitForFunction(path => {
      const el = document.querySelector(`[data-av-param-path="${path}"]`);
      return (el?.closest('.row') ?? el)?.getAttribute('data-av-surface-requested') === 'ready';
    }, path, { timeout: 5000 });
  };
  await control('/synth/waveform').hover();
  await waitReady('/synth/waveform');
  await page.evaluate(() => { window.holdSurfaceReplies = true; });
  for (const path of ['/lighting/rgba', '/synth/position', '/lighting/rgba']) {
    await control(path).hover();
    assert.equal(await state(path), 'pending', `${path} must follow the mouse before any reply`);
    assert.equal(await page.locator('[data-av-surface-mapped]').count(), 0, 'Old screen cannot masquerade as the requested one');
  }
  await page.waitForTimeout(500); // More than the old throttle, still no invented confirmation.
  assert.equal(await state('/lighting/rgba'), 'pending');
  await page.screenshot({ path: `${output}/pending.png` });
  await page.evaluate(() => {
    window.holdSurfaceReplies = false;
    window.surfaceReplyQueue.splice(0).forEach(deliver => deliver());
  });
  await waitReady('/lighting/rgba');
  await page.screenshot({ path: `${output}/confirmed.png` });
  // The row label and padding share its control's hit area.
  const rowBox = await control('/synth/waveform').evaluate(el => {
    const r = el.closest('.row').getBoundingClientRect();
    return { x: r.x + 3, y: r.y + r.height / 2 };
  });
  await page.mouse.move(rowBox.x, rowBox.y);
  await waitReady('/synth/waveform');
  await page.waitForTimeout(40); // Let the final animation-frame timing sample complete.
  const trace = await page.evaluate(() => window.surfaceHoverTimes);
  fs.writeFileSync(`${output}/trace.json`, JSON.stringify(trace, null, 2));
  const feedback = trace.filter(event => event.state);
  assert(feedback.length >= 5);
  assert(feedback.every(event => event.ms < 50), 'Input feedback exceeded 50 ms');
  const style = await control('/synth/waveform').evaluate(el => {
    const row = el.closest('.row'); const s = getComputedStyle(row);
    return { background: s.backgroundColor, outline: s.outlineStyle, labelColor: s.color };
  });
  fs.writeFileSync(`${output}/result.json`, JSON.stringify({ trace, style }, null, 2));
  console.log('PASS: immediate hover, row padding, delayed/stale screen reports, pending-to-confirmed transition');
  console.log(JSON.stringify({ feedbackMs: feedback.map(e => e.ms), style }));
} finally { await browser.close(); }
