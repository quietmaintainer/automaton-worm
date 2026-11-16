async page => {
  const origin = new URL(page.url()).origin;
  const docker = new URL(page.url()).searchParams.has('docker');
  const paths = docker ? ['/'] : ['/', '/automaton-worm/'];
  const storageKey = 'automaton-worm:settings:v1';
  const failures = [];
  page.on('pageerror', error => failures.push(error.message));
  page.on('response', response => {
    if (response.status() >= 400) failures.push(response.status() + ' ' + response.url());
  });

  function check(condition, message) {
    if (!condition) throw new Error(message);
  }

  async function ready(tick) {
    await page.waitForFunction(expected => document.querySelector('#progress').dataset.tick === String(expected)
      && !document.querySelector('#settings').disabled, tick);
  }

  async function canvasImage() {
    return page.locator('#arena').evaluate(canvas => new Promise(resolve => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve(canvas.toDataURL())));
    }));
  }

  const settings = page.getByRole('button', { name: 'Settings', exact: true });
  const step = page.getByRole('button', { name: 'Step', exact: true });
  const reset = page.getByRole('button', { name: 'Reset', exact: true });
  const engine = page.getByRole('combobox', { name: 'Simulation engine', exact: true });
  const mode = page.getByRole('combobox', { name: 'Display arena', exact: true });
  const seed = page.getByLabel('Seed', { exact: true });
  const displayedMode = () => page.locator('#comparison th[aria-current="true"]').textContent();

  for (const path of paths) {
    await page.goto(origin + path);
    await page.evaluate(key => localStorage.removeItem(key), storageKey);
    await page.reload();
    await ready(0);
    check(await page.locator('.toolbar button').count() === 4, 'Expected exactly four toolbar buttons');
    check(await page.locator('#dimensions').textContent() === '100 × 100', 'Wrong default grid');
    check(await displayedMode() === 'Direct', 'Wrong default arena');
    check(await page.getByRole('button', { name: 'Play', exact: true }).isEnabled(), 'Expected initial pause');
    const initial = await canvasImage();
    await step.click();
    await ready(1);
    const before = await canvasImage();
    const metrics = await page.locator('#comparison').innerText();

    await settings.click();
    check(await engine.inputValue() === 'mlp', 'Wrong default engine');
    check(await seed.inputValue() === '17001', 'Wrong default seed');
    check(await page.locator('#worm-count').inputValue() === '3', 'Wrong default worm count');
    check(await page.locator('#pellet-count').inputValue() === '12', 'Wrong default pellet count');
    await engine.selectOption('table');
    await ready(1);
    check(await canvasImage() === before, 'Engine switch changed the displayed cells');
    check(await seed.inputValue() === '17001', 'Engine switch changed the seed');
    check(await page.locator('#comparison').innerText() === metrics, 'Engine switch changed the scores');
    await mode.selectOption('wave');
    await ready(1);
    check(await displayedMode() === 'Wave', 'Wave view did not catch up to the common tick');
    await mode.selectOption('direct');
    await ready(1);
    await page.keyboard.press('Escape');

    await step.click();
    await ready(2);
    await reset.click();
    await ready(0);
    check(await canvasImage() === initial, 'Reset did not reproduce the seed');
    await page.reload();
    await ready(0);
    await settings.click();
    check(await engine.inputValue() === 'table', 'Engine selection did not persist');
    await page.getByLabel('Show food scent', { exact: true }).check();
    await ready(0);
    check(await canvasImage() !== initial, 'Food scent overlay had no effect');
    await page.getByLabel('Show make-room signals', { exact: true }).check();
    await ready(0);
    check(await page.locator('#legend .forwarded').isVisible(), 'Room signal overlay did not enable');
    await page.locator('#speed').fill('7');
    await ready(0);
    check(await page.locator('#speed-value').textContent() === 'Max', 'Max playback could not be selected');

    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'Play', exact: true }).click();
    await page.waitForFunction(() => Number(document.querySelector('#progress').dataset.tick) >= 48);
    await page.getByRole('button', { name: 'Pause', exact: true }).click();
    await page.locator('#arena').press('ArrowRight');
    await page.locator('#cell-details').waitFor({ state: 'visible' });
    check((await page.locator('#cell-title').textContent()).startsWith('Cell ('), 'Inspector did not select a cell');
    await page.locator('#rule-table summary').click();
    await page.locator('#rule-rows tr').first().waitFor({ state: 'visible' });
    check(await page.locator('#rule-rows tr').count() > 100, 'Compiled rule rows are missing');

    await settings.click();
    const firstSeed = await seed.inputValue();
    await page.getByRole('button', { name: 'New seed', exact: true }).click();
    await ready(0);
    check(await seed.inputValue() !== firstSeed, 'New seed did not update the seed');
    await page.locator('#worm-count').fill('1000');
    await ready(0);
    await page.locator('#pellet-count').fill('32');
    await ready(0);
    check(await page.locator('#worm-count').inputValue() === '1000', 'Could not spawn 1,000 worms');
    check(await page.locator('#pellet-count').inputValue() === '32', 'Could not spawn 32 pellets');

    const served = await page.request.get(origin + path + 'pkg/worm_bg.wasm');
    check(served.ok(), 'Wasm failed to load under ' + path);
    check(served.headers()['content-type'].includes('application/wasm'), 'Wrong Wasm MIME type');
  }

  await page.evaluate(key => localStorage.setItem(key, '{invalid'), storageKey);
  await page.reload();
  await ready(0);
  check(await displayedMode() === 'Direct', 'Malformed settings did not fall back to defaults');
  await settings.click();
  check(await engine.inputValue() === 'mlp', 'Malformed settings did not restore default engine');

  const limited = await page.context().newPage();
  try {
    await limited.addInitScript(() => {
      Object.defineProperty(window, 'localStorage', {
        get() { throw new DOMException('Storage is disabled', 'SecurityError'); },
      });
    });
    await limited.goto(origin + paths.at(-1));
    await limited.waitForFunction(() => !document.querySelector('#step').disabled);
    await limited.getByRole('button', { name: 'Step', exact: true }).click();
    await limited.locator('#progress[data-tick="1"]').waitFor();
  } finally {
    await limited.close();
  }

  check(failures.length === 0, failures.join('\n'));
  return { paths, controls: 'passed', storage: 'passed', synchronizedPlayback: 'passed', wasmMime: 'passed' };
}
