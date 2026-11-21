import { FORMATION_MODE, PLAYBACK_SPEEDS, ROOM_MODES, SEQUENCE_RESET_MS, SEQUENCE_RULE_MS, SEQUENCE_RULES } from './shared.js';
import { loadSettings, normalize, pelletCapacity, saveSettings } from './settings.js';
import { connectView } from './view.js';

const $ = id => document.getElementById(id);
const play = $('play');
const step = $('step');
const reset = $('reset');
const sequenceButton = $('sequence');
const formationStatus = $('formation-status');
const clearFormation = $('clear-formation');
const settings = $('settings');
const panel = $('settings-panel');
const fields = $('settings-fields');
const error = $('error');
const progress = $('progress');
const canvas = $('arena');
const inputs = {
  kind: $('arena-kind'),
  engine: $('engine'), mode: $('mode'), size: $('size'), seed: $('seed'),
  wormCount: $('worm-count'), pelletCount: $('pellet-count'), speed: $('speed'),
  zoom: $('zoom'), scent: $('scent'), room: $('room'), signals: $('signals'), behavior: $('behavior'),
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
let rules;
let config;
let pattern;
let sequence;
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
  return { cell, scent: settings.scent, room: settings.room, signals: settings.signals };
}

function controls() {
  const title = running ? 'Pause' : 'Play';
  play.title = title;
  play.setAttribute('aria-label', title);
  play.dataset.playing = String(running);
  play.disabled = step.disabled = busy || !links || Boolean(workerError);
  if (running) step.disabled = true;
  reset.disabled = settings.disabled = fields.disabled = busy || !config || Boolean(workerError);
  sequenceButton.disabled = reset.disabled;
  sequenceButton.setAttribute('aria-pressed', String(Boolean(sequence)));
  clearFormation.disabled = !pattern;
}

function pause() {
  if (sequence?.deadline != null) {
    sequence.remaining = Math.max(0, sequence.deadline - performance.now());
    sequence.deadline = null;
  }
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
  if (busy) return false;
  pause();
  busy = true;
  controls();
  error.hidden = true;
  try {
    await playbackTask;
    await action();
    return true;
  } catch (failure) {
    reportError(failure);
    if (panel.matches(':popover-open')) panel.hidePopover();
    return false;
  } finally {
    busy = false;
    if (config) syncSettings();
    controls();
  }
}

function show({ tick: nextTick, mode, metrics, frame, pattern: nextPattern }, duration = 0) {
  tick = nextTick;
  pattern = nextPattern;
  progress.textContent = 'Tick ' + tick.toLocaleString();
  progress.dataset.tick = String(tick);
  if (config.kind === FORMATION_MODE) showFormationStatus();
  else for (const [index, name] of ROOM_MODES.entries()) {
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

function showFormationStatus() {
  const count = config.wormCount.toLocaleString() + ' worms';
  const next = sequence ? ` · ${sequence.index + 1}/${SEQUENCE_RULES.length}` : '';
  formationStatus.dataset.rule = pattern?.rule ?? '';
  formationStatus.dataset.phase = sequence?.resting ? 'reset' : pattern ? 'formation' : 'idle';
  if (sequence?.resting) {
    const rule = SEQUENCE_RULES[(sequence.index + 1) % SEQUENCE_RULES.length];
    formationStatus.textContent = `${count} · Resetting · Next Rule ${rule}`;
  } else if (pattern) {
    const boxes = pattern.restricted ? `${pattern.cells_inside.toLocaleString()} cells inside targets`
      : `${pattern.filled_count.toLocaleString()}/${pattern.target_count.toLocaleString()} boxes filled`;
    formationStatus.textContent = `${count} · Rule ${pattern.rule}${next} · ${boxes}`;
  } else {
    formationStatus.textContent = count;
  }
}

function readBits(word, shift, count) {
  return Math.floor(word / 2 ** shift) % count;
}

function describePacket(word, phase) {
  const code = word & 7;
  const { idCount, roomHopShift } = config.kind === FORMATION_MODE ? limits.formation : { idCount: 1024, roomHopShift: 28 };
  const worm = readBits(word, 3, idCount);
  const hop = config.kind === FORMATION_MODE ? readBits(word, roomHopShift, 4) : word >>> roomHopShift;
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
  const { idCount, headingShift } = config.kind === FORMATION_MODE ? limits.formation : { idCount: 1024, headingShift: 13 };
  const worm = readBits(segment, 3, idCount);
  const phase = clock & 31;
  const food = (field >>> 6) & 63;
  const waiting = (field >>> 12) & 15;
  const request = words.subarray(4, 8).findIndex(word => (word & 7) === 2);
  const position = role === 1 ? 'Head · ' + directions[readBits(segment, headingShift, 4)] : role === 5 ? 'Tail' : 'Body ' + role;
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
  const formation = config.kind === FORMATION_MODE;
  if (!formation) {
    inputs.size.min = limits.minimumGridSize[config.wormCount];
    inputs.size.max = limits.maxGridSize;
    inputs.wormCount.max = limits.maxWorms;
    inputs.pelletCount.max = pelletCapacity(config.size, config.wormCount, limits);
  }
  for (const name of ['kind', 'size', 'seed', 'wormCount', 'pelletCount', 'mode', 'engine', 'zoom', 'behavior']) {
    inputs[name].value = config[name];
  }
  inputs.speed.value = PLAYBACK_SPEEDS.indexOf(config.speed);
  inputs.scent.checked = config.scent;
  inputs.room.checked = config.room;
  inputs.signals.checked = config.signals;
  for (const key of roomKeys) key.hidden = !config.room;
  for (const key of [$('vacancy-key'), $('reservation-key')]) key.hidden = !config.signals;
  for (const field of document.querySelectorAll('[data-autonomous]')) field.hidden = formation;
  for (const field of document.querySelectorAll('[data-formation]')) field.hidden = !formation;
  comparison.hidden = formation;
  sequenceButton.hidden = formationStatus.hidden = !formation;
  $('arena-title').textContent = formation ? '1D Worm Automata' : 'Make room';
  updateSliders();
}

async function startArena(next) {
  const result = await send('reset', { config: next, view: viewOptions(next, null) });
  if (config?.kind !== next.kind || !$('rule-version').textContent) {
    const rule = rules[next.kind];
    $('rule-version').textContent = 'v' + rule.version + ' · ' + rule.rows.length + ' rows';
    $('rule-lookup').textContent = '[' + rule.lookup.join(', ') + ']';
    $('rule-rows').replaceChildren();
  }
  config = next;
  sequence = null;
  links = result.links;
  selected = null;
  view.reset(config.size, config.zoom);
  $('dimensions').textContent = config.size + ' × ' + config.size;
  show(result);
  fillRuleRows();
  saveSettings(config);
}

async function changeSetting(name, value) {
  if (name === 'kind') {
    await startArena(loadSettings(limits, value));
    const url = new URL(location.href);
    if (url.searchParams.has('mode')) {
      url.searchParams.set('mode', value);
      history.replaceState(history.state, '', url);
    }
    return;
  }
  const next = normalize({ ...config, [name]: value }, limits);
  if (['size', 'seed', 'wormCount', 'pelletCount'].some(key => next[key] !== config[key])) {
    await startArena(next);
    return;
  }
  const types = { engine: 'engine', mode: 'view', scent: 'snapshot', room: 'snapshot', signals: 'snapshot' };
  if (Object.hasOwn(types, name) || (name === 'behavior' && pattern)) {
    const result = name === 'behavior' ? await formationCue('behavior', pattern.rule, next)
      : await send(types[name], { engine: next.engine, mode: next.mode, view: viewOptions(next) });
    config = next;
    show(result);
  } else {
    config = next;
  }
  saveSettings(config);
}

function formationCue(command, rule = 0, settings = config) {
  return send('formation', { cue: { command, rule, behavior: settings.behavior }, view: viewOptions(settings) });
}

async function alignFormationCue() {
  const phase = tick % ticksPerMove;
  if (phase) show(await send('advance', { count: ticksPerMove - phase, view: viewOptions() }));
}

async function setSequencePhase(index, resting) {
  const result = await formationCue(resting ? 'clear' : 'retarget', SEQUENCE_RULES[index]);
  sequence.index = index;
  sequence.resting = resting;
  sequence.remaining = resting ? SEQUENCE_RESET_MS : SEQUENCE_RULE_MS;
  sequence.deadline = running ? performance.now() + sequence.remaining : null;
  show(result);
  controls();
}

function playRound() {
  if (!running || busy || playbackTask) return;
  const started = performance.now();
  const duration = config.speed === 'max' ? 0 : ROUND_MS / config.speed;
  playbackTask = (async () => {
    if (sequence?.deadline != null && started >= sequence.deadline) {
      const index = sequence.resting ? (sequence.index + 1) % SEQUENCE_RULES.length : sequence.index;
      await setSequencePhase(index, !sequence.resting);
    }
    if (running) {
      const count = ticksPerMove - tick % ticksPerMove;
      show(await send('advance', { count, view: viewOptions() }), duration);
    }
  })()
    .catch(reportError)
    .finally(() => {
      playbackTask = null;
      if (!running) return;
      const now = performance.now();
      const delay = Math.min(started + duration, sequence?.deadline ?? Infinity) - now;
      if (duration === 0) playRound();
      else timer = setTimeout(playRound, Math.max(0, delay));
    });
}

function startPlayback() {
  running = true;
  if (sequence) sequence.deadline = performance.now() + sequence.remaining;
  controls();
  playRound();
}

play.addEventListener('click', () => {
  if (running) pause();
  else startPlayback();
});
step.addEventListener('click', () => operate(async () => show(await send('advance', { count: 1, view: viewOptions() }))));
reset.addEventListener('click', () => operate(() => startArena(config)));
sequenceButton.addEventListener('click', async () => {
  const stopping = Boolean(sequence);
  const resume = !stopping || running;
  const changed = await operate(async () => {
    await alignFormationCue();
    if (stopping) {
      sequence = null;
      show(await formationCue('clear'));
    } else {
      sequence = {};
      await setSequencePhase(0, false);
    }
  });
  if (changed && resume) startPlayback();
  if (!changed) {
    sequence = null;
    controls();
  }
});
for (const rule of SEQUENCE_RULES) {
  const button = document.createElement('button');
  button.type = 'button';
  button.textContent = rule;
  button.setAttribute('aria-label', `Start Rule ${rule}`);
  button.addEventListener('click', async () => {
    if (await operate(async () => {
      panel.hidePopover();
      sequence = null;
      await alignFormationCue();
      show(await formationCue(rule === 90 ? 'start' : 'retarget', rule));
    })) startPlayback();
  });
  $('rule-picker').append(button);
}
clearFormation.addEventListener('click', () => operate(async () => {
  sequence = null;
  await alignFormationCue();
  show(await formationCue('clear'));
}));
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
  const rule = rules?.[config?.kind];
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
  ({ limits, rules, ticksPerMove } = await send('init'));
  config = loadSettings(limits);
  await startArena(config);
} catch (failure) {
  reportError(failure);
} finally {
  busy = false;
  if (config) syncSettings();
  controls();
}
