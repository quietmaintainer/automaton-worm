import init, { AutonomousWorm, arenaLimits } from './pkg/worm.js';
import { ENGINE_LABELS, ROOM_MODES } from './shared.js';

let wasmModule;
let limits;
let active;
let queue = Promise.resolve();

function openArena() {
  const worker = new Worker(new URL('./autonomous-worker.js', import.meta.url), { type: 'module' });
  let request;
  let failure;
  let nextId = 0;
  worker.onmessage = ({ data }) => {
    if (request?.id !== data.id) return;
    const { resolve, reject } = request;
    request = null;
    if (data.error) reject(new Error(data.error));
    else resolve(data);
  };
  worker.onerror = event => {
    failure = new Error(event.message || 'Arena worker stopped');
    request?.reject(failure);
    request = null;
  };
  return {
    send(message) {
      if (failure) return Promise.reject(failure);
      const id = ++nextId;
      return new Promise((resolve, reject) => {
        request = { id, resolve, reject };
        worker.postMessage({ ...message, id });
      });
    },
    close: () => worker.terminate(),
  };
}

async function load() {
  if (wasmModule) return;
  const response = await fetch(new URL('./pkg/worm_bg.wasm', import.meta.url));
  if (!response.ok) throw new Error('Could not load the WebAssembly simulation');
  const module = await WebAssembly.compile(await response.arrayBuffer());
  await init({ module_or_path: module });
  limits = JSON.parse(arenaLimits());
  wasmModule = module;
}

function close(arena) {
  for (const worker of Object.values(arena?.workers ?? {})) worker.close();
}

async function gather(arena, message, expectedTick, view) {
  const replies = await Promise.all(ROOM_MODES.map(async mode => {
    const response = await arena.workers[mode].send({
      ...message, mode, view: mode === arena.mode ? view ?? {} : null,
    });
    if (response.tick !== expectedTick) throw new Error('Arena workers lost synchronization');
    return [mode, response];
  }));
  const byMode = Object.fromEntries(replies);
  return {
    tick: expectedTick,
    mode: arena.mode,
    engine: arena.engine,
    frame: byMode[arena.mode].frame,
    links: byMode[arena.mode].links,
    metrics: Object.fromEntries(replies.map(([mode, result]) => [mode, result.metrics])),
  };
}

async function handle({ type, config, engine, mode, count, view }) {
  if (type === 'init') {
    await load();
    return { limits, rule: JSON.parse(AutonomousWorm.ruleJson()), ticksPerMove: AutonomousWorm.ticksPerMove() };
  }
  if (type === 'reset') {
    await load();
    if (!ROOM_MODES.includes(config.mode) || !Object.hasOwn(ENGINE_LABELS, config.engine)) {
      throw new Error('Unknown arena or engine');
    }
    const next = { mode: config.mode, engine: config.engine, workers: {}, tick: 0 };
    try {
      for (const room of ROOM_MODES) next.workers[room] = openArena();
      const reply = await gather(next, { type: 'init', config, module: wasmModule }, 0, view);
      close(active);
      active = next;
      return reply;
    } catch (error) {
      close(next);
      throw error;
    }
  }
  if (!active) throw new Error('Reset to start an arena');
  if (type === 'advance' && (!Number.isInteger(count) || count < 1 || count > 0xffffffff - active.tick)) {
    throw new Error('Invalid tick count');
  }
  if (type === 'view' && !ROOM_MODES.includes(mode)) throw new Error('Unknown arena');
  if (type === 'engine' && !Object.hasOwn(ENGINE_LABELS, engine)) throw new Error('Unknown engine');
  if (!['advance', 'engine', 'view', 'snapshot'].includes(type)) throw new Error('Unknown request');
  const tick = active.tick + (type === 'advance' ? count : 0);

  // Publish a frame only after all three workers report the same expected tick.
  if (type === 'view') active.mode = mode;
  if (type === 'engine') active.engine = engine;
  try {
    const reply = await gather(active, { type: type === 'view' ? 'snapshot' : type, count, engine }, tick, view);
    active.tick = tick;
    return reply;
  } catch (error) {
    close(active);
    active = null;
    throw new Error('Arena interrupted. Reset to continue. ' + error.message);
  }
}

self.onmessage = ({ data }) => {
  queue = queue.then(async () => {
    try {
      const result = await handle(data);
      const transfers = Object.values(result.frame ?? {}).filter(ArrayBuffer.isView).map(words => words.buffer);
      if (result.links) transfers.push(result.links.buffer);
      self.postMessage({ id: data.id, ...result }, transfers);
    } catch (error) {
      self.postMessage({ id: data.id, error: error.message || String(error) });
    }
  });
};
