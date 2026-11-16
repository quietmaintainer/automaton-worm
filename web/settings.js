import { ENGINE_LABELS, PLAYBACK_SPEEDS, ROOM_MODES } from './shared.js';

const KEY = 'automaton-worm:settings:v1';
const DEFAULTS = {
  size: 100, seed: 17001, wormCount: 3, pelletCount: 12,
  mode: 'direct', engine: 'mlp', speed: 1, zoom: 100, scent: false, room: false,
};

function integer(value, fallback, min, max) {
  return Number.isFinite(value) ? Math.max(min, Math.min(max, Math.trunc(value))) : fallback;
}

export function normalize(values, limits) {
  const wormCount = integer(values.wormCount, DEFAULTS.wormCount, 1, limits.maxWorms);
  const size = integer(values.size, DEFAULTS.size, limits.minimumGridSize[wormCount], limits.maxGridSize);
  const maxPellets = pelletCapacity(size, wormCount, limits);
  return {
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

export function loadSettings(limits) {
  try {
    return normalize(JSON.parse(localStorage.getItem(KEY)) ?? {}, limits);
  } catch {
    return normalize({}, limits);
  }
}

export function saveSettings(values) {
  try {
    localStorage.setItem(KEY, JSON.stringify(values));
  } catch {
    // Storage can be disabled by the browser. The session still works.
  }
}
