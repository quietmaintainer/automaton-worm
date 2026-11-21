import init, { AutonomousWorm, FormationWorm } from './pkg/worm.js';
import { FORMATION_MODE } from './shared.js';

let arena;
let formation;

function frame(view) {
  const result = {
    bodies: arena.bodies(),
    headings: arena.headings(),
    pellets: arena.pellets(),
  };
  if (view.scent) result.scent = arena.scentField();
  if (formation) {
    result.formation = true;
    if (view.signals) {
      result.vacancy = arena.vacancySignals();
      result.reservations = arena.reservationPaths();
    }
  } else if (view.room) {
    result.roomSignals = arena.roomSignals();
    result.forwardedRoomSignals = arena.forwardedRoomSignals();
    result.yieldedWorms = arena.yieldedWorms();
  }
  if (Number.isInteger(view.cell)) {
    result.cell = view.cell;
    result.words = arena.cellDetails(view.cell);
  }
  return result;
}

self.onmessage = async ({ data: { id, type, config, mode, module, engine, count, view, cue } }) => {
  try {
    if (type === 'init') {
      await init({ module_or_path: module });
      formation = config.kind === FORMATION_MODE;
      if (formation) arena = new FormationWorm(config.seed);
      else {
        arena = new AutonomousWorm(config.size, config.seed, config.wormCount, config.pelletCount, mode);
        arena.setEngine(config.engine);
      }
    } else if (type === 'advance') {
      arena.advance(count);
    } else if (type === 'engine') {
      arena.setEngine(engine);
    } else if (type === 'formation') {
      arena.cue(cue.command, cue.rule, cue.behavior);
    }

    const response = { id, tick: arena.tick() };
    if (formation) response.pattern = JSON.parse(arena.patternJson());
    else {
      response.metrics = {
        food: arena.totalFood(),
        blocked: arena.blockedRounds(),
        contested: arena.contested(),
        reversals: arena.reversals(),
        cellsFreed: arena.cellsFreed(),
        ...JSON.parse(arena.recoveryJson()),
      };
    }
    const transfers = [];
    if (view) {
      response.frame = frame(view);
      transfers.push(...Object.values(response.frame).filter(ArrayBuffer.isView).map(words => words.buffer));
      if (type === 'init') {
        response.links = arena.passages();
        transfers.push(response.links.buffer);
      }
    }
    self.postMessage(response, transfers);
  } catch (error) {
    self.postMessage({ id, error: String(error) });
  }
};
