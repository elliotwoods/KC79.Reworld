// Live acceptance: start Console, Scope, Charts and Electra One from av-gallery first.
// Requires the framework web dependencies and an attached, provisioned Mini.
// Uses example parameters only; never sends Portal Test Bench motion commands.
import assert from 'node:assert/strict';
import { chromium } from '../third_party/av-frameworks/web/node_modules/playwright/index.mjs';

const scopeDiagnostics = await (await fetch('http://127.0.0.1:8731/api/control-surfaces')).json();
const admin = process.argv[2] ?? scopeDiagnostics.providers.flatMap(p => p.device?.details ?? [])
  .find(([key]) => key === 'router admin')?.[1];
assert(admin, 'Scope must report the discovered daemon admin URL');
const browser = await chromium.launch({ headless: true });
const failures = [];
const pages = new Map();
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function state() {
  const response = await fetch(`${admin}/api/status`);
  assert(response.ok);
  return (await response.json()).providers.find(p => p.provider_id.endsWith('electra-mini-fw'));
}
async function until(check, description) {
  for (let i = 0; i < 40; i++) {
    const current = await state();
    if (check(current)) return current;
    await delay(100);
  }
  assert.fail(description);
}
async function focus(name, port, path) {
  let page = pages.get(name);
  if (!page) {
    page = await browser.newPage({ viewport: { width: 1500, height: 1000 } });
    pages.set(name, page);
    page.on('pageerror', error => failures.push(`${name}: ${error.message}`));
    await page.goto(`http://127.0.0.1:${port}`);
    await page.waitForTimeout(1500); // Let Scope reach sustained telemetry before the first hover.
  }
  await page.bringToFront();
  await page.mouse.move(4, 4);
  await page.locator(`[data-av-param-path="${path}"]`).first().hover();
  if (name === 'scope' || name === 'electra-one') {
    let selected = false;
    for (let i = 0; i < 40; i++) {
      const hub = await (await fetch(`http://127.0.0.1:${port}/api/control-surfaces`)).json();
      if (hub.trigger_path === path) { selected = true; break; }
      await delay(100);
    }
    assert(selected, `${name} did not process hover for ${path}`);
  }
  const current = await until(s => s.focused_app_id === `example-${name}`
    && s.screen.confirmed && s.focused_fields.some(f => f.path === path), `${name} did not claim the Mini`);
  assert.equal(current.phase, 'ready');
  console.log(`${name}: confirmed ${current.focused_fields.length} fields`);
  return [page, current];
}
try {
  let [scope] = await focus('scope', 8731, '/gen/amplitude');
  const amplitude = scope.getByRole('slider', { name: 'Amplitude', exact: true });
  const original = Number(await amplitude.getAttribute('aria-valuenow'));
  const up = original < 2;
  try {
    await amplitude.press(up ? 'ArrowUp' : 'ArrowDown');
    await until(s => s.focused_app_id === 'example-scope'
      && Math.abs(s.focused_fields.find(f => f.path === '/gen/amplitude')?.value.value - original) > .005,
    'Scope edit was starved by telemetry');
  } finally {
    await amplitude.press(up ? 'ArrowDown' : 'ArrowUp');
  }
  await until(s => Math.abs(s.focused_fields.find(f => f.path === '/gen/amplitude')?.value.value - original) < .0001,
    'Scope value was not restored');
  await focus('scope', 8731, '/gen/frequency');
  assert.equal((await (await fetch('http://127.0.0.1:8731/api/control-surfaces')).json()).trigger_path, '/gen/frequency');

  const [, consoleState] = await focus('console', 8730, '/projector/0/galvo/gain');
  assert(consoleState.focused_fields.some(f => f.label === 'Galvo offset X'));
  assert(consoleState.focused_fields.some(f => f.label === 'Galvo offset Y'));
  await focus('charts', 8736, '/shape/freq-x');
  let [electra] = await focus('electra-one', 8745, '/synth/waveform');
  const waveform = electra.getByRole('combobox', { name: 'Waveform', exact: true });
  const oldChoice = (await waveform.innerText()).replace('▾', '').trim();
  try {
    await waveform.click();
    await electra.getByRole('option', { name: 'Noise', exact: true }).click();
    await until(s => s.focused_fields.find(f => f.path === '/synth/waveform')?.value.value === 2,
      'Enum value 90 must map to Mini choice index 2');
  } finally {
    await waveform.click();
    await electra.getByRole('option', { name: oldChoice, exact: true }).click();
  }
  await focus('electra-one', 8745, '/synth/position');
  await until(s => s.screen.visible_ids.includes(8) && s.screen.visible_ids.includes(9), 'Vector lanes did not reveal their page');
  const [, color] = await focus('electra-one', 8745, '/lighting/rgba');
  assert.equal(color.focused_fields.find(f => f.path === '/lighting/rgba').value.value.length, 4);
  const [, overflow] = await focus('electra-one', 8745, '/overflow/field-63');
  assert.equal(overflow.focused_fields.length, 64);
  await until(s => s.screen.visible_ids.includes(63), 'The final supported field is not reachable');

  await focus('console', 8730, '/projector/0/galvo/gain');
  for (let i = 0; i < 20; i++) {
    await delay(500);
    assert.equal((await state()).focused_app_id, 'example-console', 'Background telemetry stole the device');
  }
  await focus('scope', 8731, '/gen/amplitude');
  await focus('charts', 8736, '/shape/freq-x');
  assert.deepEqual(failures, []);
  console.log('PASS: four apps, sustained telemetry, value round trip, enums, vectors, RGBA, 64-field paging and handoff');
} finally {
  await browser.close();
}
