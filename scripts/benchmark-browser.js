async page => page.evaluate(async () => {
  const first = 412001;
  const last = 412064;
  const rounds = 10;
  const ticksPerRound = 24;
  const mode = 'direct';
  const view = { scent: false, room: false };
  const engines = ['mlp', 'table'];

  function connect() {
    const worker = new Worker(new URL('./worker.js', location.href), { type: 'module' });
    let id = 0;
    return {
      close: () => worker.terminate(),
      send(type, fields = {}) {
        return new Promise((resolve, reject) => {
          worker.onerror = event => reject(new Error(event.message));
          worker.onmessage = ({ data }) => data.error ? reject(new Error(data.error)) : resolve(data);
          worker.postMessage({ id: ++id, type, ...fields });
        });
      },
    };
  }

  function median(values) {
    const sorted = values.toSorted((a, b) => a - b);
    const center = Math.floor(sorted.length / 2);
    return sorted.length % 2 ? sorted[center] : (sorted[center - 1] + sorted[center]) / 2;
  }

  function compare(left, right, expectedTick, seed) {
    if (left.tick !== expectedTick || right.tick !== expectedTick
      || JSON.stringify(left.metrics) !== JSON.stringify(right.metrics)) {
      throw new Error('Metrics or tick differ for seed ' + seed + ' at tick ' + expectedTick);
    }
    for (const key of ['bodies', 'headings', 'pellets']) {
      const a = left.frame[key];
      const b = right.frame[key];
      if (a.length !== b.length || a.some((word, index) => word !== b[index])) {
        throw new Error(key + ' differ for seed ' + seed + ' at tick ' + expectedTick);
      }
    }
  }

  const workers = Object.fromEntries(engines.map(engine => [engine, connect()]));
  const samples = [];
  try {
    for (let seed = first - 1; seed <= last; seed++) {
      const config = { size: 100, wormCount: 1000, pelletCount: 32, seed, mode, engine: 'table' };
      const warmup = seed % 2 === 0 ? 288 : 0;
      await Promise.all(engines.map(async engine => {
        const client = workers[engine];
        await client.send('reset', { config, view });
        if (warmup) await client.send('advance', { count: warmup, view });
        await client.send('engine', { engine, view });
      }));

      const frames = {};
      const elapsed = {};
      const order = seed % 2 === 1 ? engines : engines.toReversed();
      for (const engine of order) {
        const replies = [];
        const start = performance.now();
        for (let round = 0; round < rounds; round++) {
          replies.push(await workers[engine].send('advance', { count: ticksPerRound, view }));
        }
        elapsed[engine] = performance.now() - start;
        frames[engine] = replies;
      }
      for (let round = 0; round < rounds; round++) {
        compare(frames.mlp[round], frames.table[round], warmup + (round + 1) * ticksPerRound, seed);
      }
      if (seed >= first) samples.push(elapsed);
    }
  } finally {
    for (const worker of Object.values(workers)) worker.close();
  }

  return {
    browser: navigator.userAgent,
    seeds: samples.length,
    ticksPerSeed: rounds * ticksPerRound,
    mlpMedianMs: median(samples.map(sample => sample.mlp)),
    tableMedianMs: median(samples.map(sample => sample.table)),
    pairedSpeedup: median(samples.map(sample => sample.table / sample.mlp)),
    mlpFasterSeeds: samples.filter(sample => sample.mlp < sample.table).length,
  };
})
