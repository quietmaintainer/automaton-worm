import { AUTONOMOUS_MODE, ENGINE_LABELS, FORMATION_BEHAVIORS, FORMATION_MODE, PLAYBACK_SPEEDS, ROOM_MODES } from './shared.js';

const KEY = 'automaton-worm:settings:v1';
const MODE_KEY = 'automaton-worm:mode';
const FORMATION_KEY = 'automaton-worm:formation-settings:v1';
const DEFAULTS = {
  size: 100, seed: 17001, wormCount: 100, pelletCount: 32,
  mode: 'direct', engine: 'mlp', speed: 1, zoom: 100, scent: false, room: false,
};

function integer(value, fallback, min, max) {
  return Number.isFinite(value) ? Math.max(min, Math.min(max, Math.trunc(value))) : fallback;
}

export function normalize(values, limits) {
  if (values.kind === FORMATION_MODE) {
    const { size, wormCount, pelletCount } = limits.formation;
    return {
      kind: FORMATION_MODE, size, wormCount, pelletCount, engine: 'table', mode: 'wave',
      seed: integer(values.seed, DEFAULTS.seed, 0, 0xffffffff),
      speed: PLAYBACK_SPEEDS.includes(values.speed) ? values.speed : 'max',
      zoom: integer(values.zoom, DEFAULTS.zoom, 100, 800),
      scent: values.scent === true,
      signals: values.signals === true,
      behavior: FORMATION_BEHAVIORS.includes(values.behavior) ? values.behavior : 'fill',
    };
  }
  const wormCount = integer(values.wormCount, DEFAULTS.wormCount, 1, limits.maxWorms);
  const size = integer(values.size, DEFAULTS.size, limits.minimumGridSize[wormCount], limits.maxGridSize);
  const maxPellets = pelletCapacity(size, wormCount, limits);
  return {
    kind: AUTONOMOUS_MODE,
    size,
    wormCount,
    seed: integer(values.seed, DEFAULTS.seed, 0, 0xffffffff),
    pelletCount: integer(values.pelletCount, Math.min(DEFAULTS.pelletCount, maxPellets), 1, maxPellets),
    mode: ROOM_MODES.includes(values.mode) ? values.mode : DEFAULTS.mode,
    engine: typeof values.engine === 'string' && Object.hasOwn(ENGINE_LABELS, values.engine) ? values.engine : DEFAULTS.engine,
    speed: PLAYBACK_SPEEDS.includes(values.speed) ? values.speed : DEFAULTS.speed,
    zoom: integer(values.zoom, DEFAULTS.zoom, 100, 800),
    scent: values.scent === true,
    room: values.room === true,
  };
}

export function pelletCapacity(size, count, limits) {
  return Math.min(limits.maxPellets, size * size - count * limits.wormLength);
}

export function loadSettings(limits, kind) {
  let values;
  try {
    const requested = new URLSearchParams(location.search).get('mode');
    kind ??= [FORMATION_MODE, AUTONOMOUS_MODE].includes(requested) ? requested : localStorage.getItem(MODE_KEY);
    values = JSON.parse(localStorage.getItem(kind === FORMATION_MODE ? FORMATION_KEY : KEY));
  } catch {
    // Browser storage is optional.
  }
  return normalize({ ...values, kind }, limits);
}

export function saveSettings(values) {
  try {
    localStorage.setItem(values.kind === FORMATION_MODE ? FORMATION_KEY : KEY, JSON.stringify(values));
    localStorage.setItem(MODE_KEY, values.kind);
  } catch {
    // Storage can be disabled by the browser. The session still works.
  }
}
