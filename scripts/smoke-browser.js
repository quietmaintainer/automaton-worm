async page => {
  const url = new URL(page.url());
  const { origin, pathname } = url;
  const paths = url.searchParams.has('docker') || pathname !== '/' ? [pathname] : ['/', '/automaton-worm/'];
  await page.setViewportSize({ width: 1440, height: 1050 });
  // Read the coordinator's real frames without relying on canvas rasterization.
  await page.addInitScript(() => {
    window.Worker = class extends window.Worker {
      constructor(url, options) {
        super(url, options);
        if (new URL(url, location.href).pathname.endsWith('/worker.js')) {
          this.addEventListener('message', ({ data }) => {
            if (data.frame) window.wormTestFrame = data.frame;
          });
        }
      }
    };
  });
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

  async function arenaState() {
    return page.evaluate(() => {
      const { bodies, headings, pellets } = window.wormTestFrame;
      return { bodies: [...bodies], headings: [...headings], pellets: [...pellets] };
    });
  }

  async function canvasImage() {
    return page.locator('#arena').evaluate(canvas => new Promise(resolve => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve(canvas.toDataURL())));
    }));
  }

  async function checkZoomedPointer() {
    const canvas = page.locator('#arena');
    await canvas.scrollIntoViewIfNeeded();
    const { x, y, width, height } = await canvas.boundingBox();
    const startX = x + width / 2;
    const startY = y + height / 2;
    const cellTitle = page.locator('#cell-title');

    await page.mouse.click(startX, startY);
    await page.locator('#cell-details').waitFor({ state: 'visible' });
    const selected = await cellTitle.textContent();
    const afterClick = await canvasImage();
    await page.mouse.move(startX + 45, startY + 15, { steps: 3 });
    check(await canvasImage() === afterClick, 'Mouse movement panned the zoomed grid after a cell click');

    await page.mouse.move(startX, startY);
    await page.mouse.down();
    await page.mouse.move(startX + 45, startY + 15, { steps: 3 });
    await page.mouse.up();
    const afterDrag = await canvasImage();
    check(afterDrag !== afterClick, 'Dragging did not pan the zoomed grid');
    check(await cellTitle.textContent() === selected, 'Dragging changed the selected cell');
    await page.mouse.move(startX + 75, startY + 30, { steps: 3 });
    check(await canvasImage() === afterDrag, 'Mouse movement continued panning after a drag ended');

    await page.mouse.click(startX - 65, startY);
    await page.waitForFunction(title => document.querySelector('#cell-title').textContent !== title, selected);
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
    await page.evaluate(key => {
      localStorage.removeItem(key);
      localStorage.removeItem('automaton-worm:mode');
    }, storageKey);
    await page.reload();
    await ready(0);
    check(await page.locator('.toolbar button:visible').count() === 4, 'Expected exactly four visible toolbar buttons');
    check(await page.locator('#dimensions').textContent() === '100 × 100', 'Wrong default grid');
    check(await displayedMode() === 'Direct', 'Wrong default arena');
    check(await page.getByRole('button', { name: 'Play', exact: true }).isEnabled(), 'Expected initial pause');
    const initial = JSON.stringify(await arenaState());
    await step.click();
    await ready(1);
    const before = JSON.stringify(await arenaState());
    const metrics = await page.locator('#comparison').innerText();

    await settings.click();
    check(await engine.inputValue() === 'mlp', 'Wrong default engine');
    check(await seed.inputValue() === '17001', 'Wrong default seed');
    check(await page.locator('#worm-count').inputValue() === '100', 'Wrong default worm count');
    check(await page.locator('#pellet-count').inputValue() === '32', 'Wrong default pellet count');
    await engine.selectOption('table');
    await ready(1);
    check(JSON.stringify(await arenaState()) === before, 'Engine switch changed the displayed cells');
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
    check(JSON.stringify(await arenaState()) === initial, 'Reset did not reproduce the seed');
    await page.reload();
    await ready(0);
    await settings.click();
    check(await engine.inputValue() === 'table', 'Engine selection did not persist');
    const beforeScent = await canvasImage();
    await page.getByLabel('Show food scent', { exact: true }).check();
    await ready(0);
    check(await canvasImage() !== beforeScent, 'Food scent overlay had no effect');
    await page.getByLabel('Show make-room signals', { exact: true }).check();
    await ready(0);
    check(await page.locator('#legend .forwarded').isVisible(), 'Room signal overlay did not enable');
    await page.locator('#speed').fill('7');
    await ready(0);
    check(await page.locator('#speed-value').textContent() === 'Max', 'Max playback could not be selected');

    await page.locator('#zoom').fill('200');
    await page.keyboard.press('Escape');
    await checkZoomedPointer();
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
    check(await page.locator('#worm-count').inputValue() === '1000', 'Could not spawn 1,000 worms');
    await page.locator('#pellet-count').fill('12');
    await ready(0);
    await page.reload();
    await ready(0);
    await settings.click();
    check(await page.locator('#worm-count').inputValue() === '1000', 'Saved worm count did not persist');
    check(await page.locator('#pellet-count').inputValue() === '12', 'Saved pellet count did not persist');

    const served = await page.evaluate(async url => {
      const response = await fetch(url, { method: 'HEAD' });
      return { ok: response.ok, contentType: response.headers.get('content-type') };
    }, origin + path + 'pkg/worm_bg.wasm');
    check(served.ok, 'Wasm failed to load under ' + path);
    check(served.contentType?.includes('application/wasm'), 'Wrong Wasm MIME type');
  }

  await page.evaluate(key => localStorage.setItem(key, '{invalid'), storageKey);
  await page.reload();
  await ready(0);
  check(await displayedMode() === 'Direct', 'Malformed settings did not fall back to defaults');
  await settings.click();
  check(await engine.inputValue() === 'mlp', 'Malformed settings did not restore default engine');

  const arenaMode = page.getByRole('combobox', { name: 'Mode', exact: true });
  await arenaMode.selectOption('worm-automata');
  await ready(0);
  check(await page.locator('#dimensions').textContent() === '350 × 350', 'Wrong formation grid');
  const formation = await arenaState();
  check(formation.bodies.length === 50_000 && new Set(formation.bodies).size === 50_000,
    'Expected 10,000 non-overlapping five-cell worms');
  check(formation.pellets.length === 300, 'Wrong formation food preset');
  const sequence = page.getByRole('button', { name: '16-rule sequence', exact: true });
  check(await sequence.isVisible(), 'Sequence button is missing');
  await page.getByRole('button', { name: 'Start Rule 90', exact: true }).click();
  await page.locator('#formation-status[data-rule="90"]').waitFor({ state: 'visible' });
  await page.waitForFunction(() => Number(document.querySelector('#progress').dataset.tick) >= 24);
  await page.getByRole('button', { name: 'Pause', exact: true }).click();
  await sequence.click();
  await page.locator('#formation-status[data-rule="30"]').waitFor({ state: 'visible' });
  await page.locator('#formation-status[data-phase="reset"]').waitFor({ state: 'visible' });
  const resetTick = Number(await page.locator('#progress').getAttribute('data-tick'));
  await page.locator('#formation-status[data-rule="54"]').waitFor({ state: 'visible' });
  check(Number(await page.locator('#progress').getAttribute('data-tick')) > resetTick,
    'Worms stopped moving between rules');
  await sequence.click();
  await page.locator('#formation-status[data-phase="idle"]').waitFor({ state: 'visible' });
  check(await sequence.getAttribute('aria-pressed') === 'false', 'The sequence did not stop');
  await settings.click();
  await arenaMode.selectOption('autonomous');
  await ready(0);
  check(await page.locator('#dimensions').textContent() === '100 × 100', 'Could not return to Autonomous CA');
  check(await engine.inputValue() === 'mlp', 'Mode switch replaced the original engine setting');

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
  return { paths, controls: 'passed', storage: 'passed', synchronizedPlayback: 'passed', wasmMime: 'passed', zoomedPointer: 'passed', formationMode: 'passed', ruleSequence: 'passed' };
}
