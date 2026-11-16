import { PLAYBACK_SPEEDS, ROOM_MODES } from './shared.js';
import { loadSettings, normalize, pelletCapacity, saveSettings } from './settings.js';
import { connectView } from './view.js';

const $ = id => document.getElementById(id);
const play = $('play');
const step = $('step');
const reset = $('reset');
const settings = $('settings');
const panel = $('settings-panel');
const fields = $('settings-fields');
const error = $('error');
const progress = $('progress');
const canvas = $('arena');
const inputs = {
  engine: $('engine'), mode: $('mode'), size: $('size'), seed: $('seed'),
  wormCount: $('worm-count'), pelletCount: $('pellet-count'), speed: $('speed'),
  zoom: $('zoom'), scent: $('scent'), room: $('room'),
};
const directions = ['West', 'East', 'North', 'South'];
const messages = ['—', 'Open', 'Request', 'Space ticket', 'Occupied', 'Head leads', 'Tail leads', 'Try other end'];
const comparison = $('comparison');
const modeHeaders = comparison.querySelectorAll('[data-mode]');
const metricRows = comparison.querySelectorAll('[data-metric]');
const portRows = $('cell-ports').tBodies[0].rows;
const ruleTable = $('rule-table');
const roomKeys = document.querySelectorAll('.room-key');
const recoveryFormat = new Intl.NumberFormat(undefined, { maximumFractionDigits: 2 });
const ROUND_MS = 40;

const worker = new Worker(new URL('./worker.js', import.meta.url), { type: 'module' });
const pending = new Map();
let requestId = 0;
let workerError;
let limits;
let rule;
let config;
let links;
let tick = 0;
let ticksPerMove;
let selected = null;
let running = false;
let busy = true;
let playbackTask;
let timer;
const view = connectView(canvas, { onSelect: selectCell, onZoom: updateZoom });

function workerFailed(failure) {
  workerError = failure;
  for (const { reject } of pending.values()) reject(failure);
  pending.clear();
  reportError(failure);
}

worker.onmessage = ({ data }) => {
  const request = pending.get(data.id);
  if (!request) return;
  pending.delete(data.id);
  if (data.error) request.reject(new Error(data.error));
  else request.resolve(data);
};
worker.onerror = event => workerFailed(new Error(event.message || 'Simulation worker stopped'));
worker.onmessageerror = () => workerFailed(new Error('Simulation worker sent an unreadable frame'));

function send(type, fields = {}) {
  if (workerError) return Promise.reject(workerError);
  const id = ++requestId;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    try {
      worker.postMessage({ ...fields, type, id });
    } catch (failure) {
      pending.delete(id);
      reject(failure);
    }
  });
}

function viewOptions(settings = config, cell = selected) {
  return { cell, scent: settings.scent, room: settings.room };
}

function controls() {
  const title = running ? 'Pause' : 'Play';
  play.title = title;
  play.setAttribute('aria-label', title);
  play.dataset.playing = String(running);
  play.disabled = step.disabled = busy || !links || Boolean(workerError);
  if (running) step.disabled = true;
  reset.disabled = settings.disabled = fields.disabled = busy || !config || Boolean(workerError);
}

function pause() {
  running = false;
  clearTimeout(timer);
  controls();
}

function reportError(failure) {
  pause();
  error.textContent = failure.message || String(failure);
  error.hidden = false;
}

async function operate(action) {
  if (busy) return;
  pause();
  busy = true;
  controls();
  error.hidden = true;
  try {
    await playbackTask;
    await action();
  } catch (failure) {
    reportError(failure);
    if (panel.matches(':popover-open')) panel.hidePopover();
  } finally {
    busy = false;
    if (config) syncSettings();
    controls();
  }
}

function show({ tick: nextTick, mode, metrics, frame }, duration = 0) {
  tick = nextTick;
  progress.textContent = 'Tick ' + tick.toLocaleString();
  progress.dataset.tick = String(tick);
  for (const [index, name] of ROOM_MODES.entries()) {
    const active = name === mode;
    if (active) modeHeaders[index].setAttribute('aria-current', 'true');
    else modeHeaders[index].removeAttribute('aria-current');

    for (const row of metricRows) {
      const value = metrics[name][row.dataset.metric];
      const cell = row.cells[index + 1];
      cell.textContent = value == null ? '—' : recoveryFormat.format(value);
      cell.dataset.active = String(active);
    }
  }
  view.show(frame, duration);
  showCell(frame);
}

function describePacket(word, phase) {
  const code = word & 7;
  const worm = (word >>> 3) & 1023;
  const hop = word >>> 28;
  if (!code && worm) return hop ? 'Forwarded · ' + worm + ' · hop ' + hop : 'Make room · ' + worm;
  if (phase >= 2 && phase <= 6 && code > 4 && worm) return 'Room relay · ' + worm + ' · hop ' + (code & 3);
  return messages[code] + (worm ? ' · ' + worm : '');
}

function showCell(frame) {
  const details = $('cell-details');
  const words = frame.cell === selected ? frame.words : null;
  details.hidden = !words;
  if (selected == null) {
    $('cell-title').textContent = 'Cell inspector';
    $('cell-phase').textContent = '';
    $('cell-help').hidden = false;
    return;
  }
  const x = selected % config.size;
  const y = Math.floor(selected / config.size);
  $('cell-title').textContent = 'Cell (' + x + ', ' + y + ')';
  $('cell-help').hidden = true;
  if (!words) {
    $('cell-phase').textContent = '';
    return;
  }

  const [segment, field, movement, clock] = words;
  const role = segment & 7;
  const worm = (segment >>> 3) & 1023;
  const phase = clock & 31;
  const food = (field >>> 6) & 63;
  const waiting = (field >>> 12) & 15;
  const request = words.subarray(4, 8).findIndex(word => (word & 7) === 2);
  const position = role === 1 ? 'Head · ' + directions[(segment >>> 13) & 3] : role === 5 ? 'Tail' : 'Body ' + role;
  $('cell-phase').textContent = 'Phase ' + phase;
  $('cell-role').textContent = worm ? 'Worm ' + worm + ' · ' + position : 'Background';
  $('cell-scent').textContent = field & 63;
  $('cell-food').textContent = food === 1 ? 'Pellet' : food > 1 ? 'Cooling seed' : '—';
  $('cell-move').textContent = request >= 0 ? 'Requests ' + directions[request].toLowerCase()
    : movement >= 5 && movement <= 7 && worm ? messages[movement]
    : waiting && worm ? 'Waiting · ' + waiting : '—';
  $('cell-memory').textContent = '[' + [...words.subarray(0, 4)].join(', ') + ']';
  $('cell-report').textContent = words[12];

  for (const [index, row] of [...portRows].entries()) {
    const open = links[selected] & (1 << index);
    row.cells[1].textContent = open ? describePacket(words[8 + index], phase) : 'Edge';
    row.cells[2].textContent = open ? describePacket(words[4 + index], phase) : 'Edge';
  }
}

function selectCell(index) {
  if (!links || busy || workerError) return;
  selected = index;
  view.select(selected);
  if (!running) void operate(async () => show(await send('snapshot', { view: viewOptions() })));
}

function updateZoom(zoom) {
  if (!config) return;
  config.zoom = zoom;
  inputs.zoom.value = zoom;
  updateSliders();
  saveSettings(config);
}

function updateSliders() {
  const speed = PLAYBACK_SPEEDS[inputs.speed.valueAsNumber];
  for (const [name, label] of [
    ['wormCount', inputs.wormCount.value], ['pelletCount', inputs.pelletCount.value],
    ['speed', speed === 'max' ? 'Max' : speed + '×'], ['zoom', inputs.zoom.value + '%'],
  ]) {
    inputs[name].setAttribute('aria-valuetext', label);
    $(inputs[name].id + '-value').value = label;
  }
}

function syncSettings() {
  inputs.size.min = limits.minimumGridSize[config.wormCount];
  inputs.size.max = limits.maxGridSize;
  inputs.wormCount.max = limits.maxWorms;
  inputs.pelletCount.max = pelletCapacity(config.size, config.wormCount, limits);
  for (const name of ['size', 'seed', 'wormCount', 'pelletCount', 'mode', 'engine', 'zoom']) {
    inputs[name].value = config[name];
  }
  inputs.speed.value = PLAYBACK_SPEEDS.indexOf(config.speed);
  inputs.scent.checked = config.scent;
  inputs.room.checked = config.room;
  for (const key of roomKeys) key.hidden = !config.room;
  updateSliders();
}

async function startArena(next) {
  const result = await send('reset', { config: next, view: viewOptions(next, null) });
  config = next;
  links = result.links;
  selected = null;
  view.reset(config.size, config.zoom);
  $('dimensions').textContent = config.size + ' × ' + config.size;
  show(result);
  saveSettings(config);
}

async function changeSetting(name, value) {
  const next = normalize({ ...config, [name]: value }, limits);
  if (['size', 'seed', 'wormCount', 'pelletCount'].some(key => next[key] !== config[key])) {
    await startArena(next);
    return;
  }
  const request = name === 'engine' || name === 'mode' || name === 'scent' || name === 'room';
  if (request) {
    const type = name === 'engine' ? 'engine' : name === 'mode' ? 'view' : 'snapshot';
    const result = await send(type, { engine: next.engine, mode: next.mode, view: viewOptions(next) });
    config = next;
    show(result);
  } else {
    config = next;
  }
  saveSettings(config);
}

function playRound() {
  if (!running || busy || playbackTask) return;
  const started = performance.now();
  const count = ticksPerMove - tick % ticksPerMove;
  const duration = config.speed === 'max' ? 0 : ROUND_MS / config.speed;
  playbackTask = send('advance', { count, view: viewOptions() })
    .then(result => show(result, duration))
    .catch(reportError)
    .finally(() => {
      playbackTask = null;
      if (!running) return;
      if (duration === 0) playRound();
      else timer = setTimeout(playRound, Math.max(0, duration - (performance.now() - started)));
    });
}

play.addEventListener('click', () => {
  if (running) pause();
  else {
    running = true;
    controls();
    playRound();
  }
});
step.addEventListener('click', () => operate(async () => show(await send('advance', { count: 1, view: viewOptions() }))));
reset.addEventListener('click', () => operate(() => startArena(config)));
panel.addEventListener('beforetoggle', event => {
  const open = event.newState === 'open';
  settings.setAttribute('aria-expanded', String(open));
  if (open) pause();
});
$('settings-form').addEventListener('submit', event => event.preventDefault());
$('new-seed').addEventListener('click', () => operate(() => startArena({
  ...config, seed: crypto.getRandomValues(new Uint32Array(1))[0],
})));

for (const [name, input] of Object.entries(inputs)) {
  if (input.type === 'range') input.addEventListener('input', updateSliders);
  input.addEventListener('change', () => {
    if (name === 'zoom') {
      view.setZoom(input.valueAsNumber);
      updateZoom(input.valueAsNumber);
      return;
    }
    const value = input.type === 'checkbox' ? input.checked : name === 'speed' ? PLAYBACK_SPEEDS[input.valueAsNumber]
      : input.type === 'number' || input.type === 'range' ? input.valueAsNumber : input.value;
    void operate(() => changeSetting(name, value));
  });
}
inputs.zoom.addEventListener('input', () => view.setZoom(inputs.zoom.valueAsNumber));

canvas.addEventListener('keydown', event => {
  const offsets = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
  if (!links || !Object.hasOwn(offsets, event.key)) return;
  event.preventDefault();
  const size = config.size;
  const center = Math.floor(size / 2);
  const index = selected ?? center * size + center;
  const [dx, dy] = offsets[event.key];
  const x = Math.max(0, Math.min(size - 1, index % size + dx));
  const y = Math.max(0, Math.min(size - 1, Math.floor(index / size) + dy));
  selectCell(y * size + x);
});

function fillRuleRows() {
  const body = $('rule-rows');
  if (!ruleTable.open || !rule || body.rows.length) return;
  const rows = document.createDocumentFragment();
  for (const [index, { op, dst, a, b, c }] of rule.rows.entries()) {
    const row = document.createElement('tr');
    for (const value of [index + 1, op, dst, a, b, c]) row.insertCell().textContent = value;
    rows.append(row);
  }
  body.append(rows);
}
ruleTable.addEventListener('toggle', fillRuleRows);

try {
  ({ limits, rule, ticksPerMove } = await send('init'));
  $('rule-version').textContent = 'v' + rule.version + ' · ' + rule.rows.length + ' rows';
  $('rule-lookup').textContent = '[' + rule.lookup.join(', ') + ']';
  fillRuleRows();
  config = loadSettings(limits);
  await startArena(config);
} catch (failure) {
  reportError(failure);
} finally {
  busy = false;
  if (config) syncSettings();
  controls();
}
