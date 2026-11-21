//! The arena seeds a world once. All later state changes come from its rule.

use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use crate::{
    CellState, CpuSwarm, PORTS, RuleTable, World,
    rule_mlp::{self, LANES},
    world::{cardinal_neighbors, random_index},
};

use crate::autonomous_protocol::*;
use crate::spawn::{
    Body, MAX_GRID_SIZE, MAX_PELLETS, WORM_LENGTH, initial_bodies, initial_pellets, worm_capacity,
};

pub const ROUND_TICKS: u32 = TICKS_PER_MOVE;
pub const RULE_JSON: &str = include_str!(concat!(env!("OUT_DIR"), "/worm_autonomous_rule.json"));

#[allow(clippy::all)]
mod compiled {
    use super::{CellState, PORTS};

    include!(concat!(env!("OUT_DIR"), "/worm_autonomous_rule.rs"));
}

#[allow(clippy::all)]
mod mlp_kernels {
    use crate::rule_mlp::{InputPlanes, OutputPlanes, Program};

    include!(concat!(env!("OUT_DIR"), "/worm_autonomous_mlp.rs"));
}

pub fn rule_table() -> RuleTable {
    RuleTable::from_json(RULE_JSON).expect("the build validated the autonomous rule")
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryStats {
    pub recovered: u64,
    pub average_rounds: Option<f64>,
    pub p90_rounds: Option<u32>,
    pub longest_completed_rounds: Option<u32>,
    pub still_stuck: usize,
    pub longest_stuck_rounds: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomMode {
    Off,
    Direct,
    Wave,
}

impl RoomMode {
    fn seed(self, world: &mut World) {
        let flag = match self {
            Self::Off => ROOM_DISABLED,
            Self::Direct => return,
            Self::Wave => ROOM_WAVE,
        };
        for cell in &mut world.initial {
            cell.answer |= flag;
        }
    }
}

#[derive(Clone)]
pub struct AutonomousArena {
    engine: CpuSwarm,
    size: usize,
    bodies: Vec<Body>,
    moves: Vec<u32>,
    scores: Vec<u32>,
    stuck_rounds: Vec<u32>,
    recoveries: BTreeMap<u32, u64>,
    room_signals: Vec<[u32; 2]>,
    forwarded_room_signals: Vec<[u32; 2]>,
    yielded_worms: Vec<u32>,
    pub total_food: u64,
    pub blocked_rounds: u64,
    pub reversals: u64,
    pub contested: u64,
    pub cells_freed: u64,
}

impl AutonomousArena {
    pub fn generate(size: usize, seed: u32, count: usize, pellet_count: usize) -> Result<Self> {
        Self::generate_with_make_room(size, seed, count, pellet_count, true)
    }

    pub fn generate_with_make_room(
        size: usize,
        seed: u32,
        count: usize,
        pellet_count: usize,
        make_room: bool,
    ) -> Result<Self> {
        let mode = if make_room {
            RoomMode::Direct
        } else {
            RoomMode::Off
        };
        Self::generate_with_room_mode(size, seed, count, pellet_count, mode)
    }

    pub fn generate_with_room_mode(
        size: usize,
        mut seed: u32,
        count: usize,
        pellet_count: usize,
        room_mode: RoomMode,
    ) -> Result<Self> {
        ensure!(
            (3..=MAX_GRID_SIZE).contains(&size),
            "choose a grid from 3 to {MAX_GRID_SIZE} cells per side"
        );
        let capacity = worm_capacity(size);
        ensure!(
            (1..=capacity).contains(&count),
            "choose from one to {capacity} worms for this grid"
        );
        let cell_count = size * size;
        let capacity = MAX_PELLETS.min(cell_count - WORM_LENGTH * count);
        ensure!(
            (1..=capacity).contains(&pellet_count),
            "choose from one to {capacity} pellets"
        );
        let bodies = initial_bodies(size, count);
        let pellets = initial_pellets(cell_count, &bodies, pellet_count, &mut seed);
        let mut world = seeded_world(size, seed, &bodies, &pellets)?;
        room_mode.seed(&mut world);
        Ok(Self::new(world, size, bodies))
    }

    fn new(world: World, size: usize, bodies: Vec<Body>) -> Self {
        let count = bodies.len();
        Self {
            engine: CpuSwarm::new(world, rule_table()),
            size,
            bodies,
            moves: vec![0; count],
            scores: vec![0; count],
            stuck_rounds: vec![0; count],
            recoveries: BTreeMap::new(),
            room_signals: Vec::new(),
            forwarded_room_signals: Vec::new(),
            yielded_worms: Vec::new(),
            total_food: 0,
            blocked_rounds: 0,
            reversals: 0,
            contested: 0,
            cells_freed: 0,
        }
    }

    pub fn advance(&mut self, ticks: u32) -> Result<()> {
        self.advance_with(ticks, compiled::update)
    }

    /// Advance the arena with the integer-lowered MLP for the whole cell rule.
    pub fn advance_mlp(&mut self, ticks: u32) -> Result<()> {
        self.tick().checked_add(ticks).context("tick overflow")?;
        for _ in 0..ticks {
            let phase = self.tick() % ROUND_TICKS;
            let program = mlp_kernels::PROGRAMS[phase as usize];
            self.engine.advance_lanes::<LANES>(|inputs, cells| {
                debug_assert!(
                    inputs
                        .iter()
                        .all(|input| input.2.memory[3] & PHASE_MASK == phase)
                );
                rule_mlp::run(program, inputs, cells);
            })?;
            if self.tick().is_multiple_of(ROUND_TICKS) {
                self.observe_commit();
            }
        }
        Ok(())
    }

    fn advance_with(
        &mut self,
        mut ticks: u32,
        mut update: impl FnMut(u32, [u32; PORTS], CellState) -> CellState,
    ) -> Result<()> {
        self.tick().checked_add(ticks).context("tick overflow")?;
        while ticks > 0 {
            let until_commit = ROUND_TICKS - self.tick() % ROUND_TICKS;
            let batch = ticks.min(until_commit);
            self.engine
                .advance_with(batch, |_, input, inbox, cell| update(input, inbox, cell))?;
            if batch == until_commit {
                self.observe_commit();
            }
            ticks -= batch;
        }
        Ok(())
    }

    // Rendering and counters read the committed state. The observer never
    // changes a cell, an input, or a link.
    fn observe_commit(&mut self) {
        self.room_signals.clear();
        self.forwarded_room_signals.clear();
        self.yielded_worms.clear();
        for (index, cell) in self.engine.cells().iter().enumerate() {
            let port_mask = (1 << PORTS) - 1;
            let direct = (cell.answer >> ROOM_PORT_SHIFT) & port_mask;
            let forwarded = (cell.answer >> ROOM_FORWARD_PORT_SHIFT) & port_mask;
            if direct | forwarded != 0 {
                for (port, link) in self.engine.sites()[index].links.iter().enumerate() {
                    if !link.is_open() {
                        continue;
                    }
                    let edge = [index as u32, link.site];
                    if direct & (1 << port) != 0 {
                        self.room_signals.push(edge);
                    }
                    if forwarded & (1 << port) != 0 {
                        self.forwarded_room_signals.push(edge);
                    }
                }
            }
            let yielded = (cell.answer >> YIELDED_ID_SHIFT) & (ID_MASK >> 3);
            if yielded != 0 {
                self.yielded_worms.push(yielded - 1);
                self.cells_freed += 1;
            }
            let blocked = cell.answer & BLOCKED != 0;
            self.blocked_rounds += u64::from(blocked);
            self.contested += u64::from(cell.answer & CONTESTED != 0);
            self.reversals += u64::from(cell.answer & REVERSED != 0);
            let segment = cell.memory[0];
            if segment != 0 {
                let worm = ((segment & ID_MASK) >> 3) as usize - 1;
                let position = (segment & ROLE_MASK) as usize - 1;
                self.bodies[worm][position] = index;
                if blocked {
                    self.stuck_rounds[worm] += 1;
                } else if cell.answer & MOVED != 0 {
                    self.moves[worm] += 1;
                    let waited = std::mem::take(&mut self.stuck_rounds[worm]);
                    if waited > 0 {
                        *self.recoveries.entry(waited).or_default() += 1;
                    }
                }
                let pickup = u32::from(cell.answer & PICKED_UP != 0);
                self.scores[worm] += pickup;
                self.total_food += u64::from(pickup);
            }
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn tick(&self) -> u32 {
        self.engine.tick()
    }

    pub fn cells(&self) -> &[CellState] {
        self.engine.cells()
    }

    pub fn bodies(&self) -> &[Body] {
        &self.bodies
    }

    pub fn moves(&self) -> &[u32] {
        &self.moves
    }

    pub fn scores(&self) -> &[u32] {
        &self.scores
    }

    pub fn recovery_stats(&self) -> RecoveryStats {
        let recovered: u64 = self.recoveries.values().sum();
        let total_wait: u64 = self
            .recoveries
            .iter()
            .map(|(&rounds, &count)| u64::from(rounds) * count)
            .sum();
        let rank = (recovered * 9).div_ceil(10);
        let mut seen = 0;
        let p90_rounds = self.recoveries.iter().find_map(|(&rounds, &count)| {
            seen += count;
            (seen >= rank).then_some(rounds)
        });
        RecoveryStats {
            recovered,
            average_rounds: (recovered > 0).then(|| total_wait as f64 / recovered as f64),
            p90_rounds,
            longest_completed_rounds: self.recoveries.last_key_value().map(|(&rounds, _)| rounds),
            still_stuck: self
                .stuck_rounds
                .iter()
                .filter(|&&rounds| rounds > 0)
                .count(),
            longest_stuck_rounds: self.stuck_rounds.iter().copied().max().unwrap_or(0),
        }
    }

    pub fn room_signals(&self) -> &[[u32; 2]] {
        &self.room_signals
    }

    pub fn forwarded_room_signals(&self) -> &[[u32; 2]] {
        &self.forwarded_room_signals
    }

    pub fn yielded_worms(&self) -> &[u32] {
        &self.yielded_worms
    }

    pub fn headings(&self) -> impl Iterator<Item = u8> + '_ {
        self.bodies
            .iter()
            .map(|body| (self.cells()[body[0]].memory[0] >> HEADING_SHIFT) as u8)
    }

    pub fn pellets(&self) -> impl Iterator<Item = usize> + '_ {
        self.cells()
            .iter()
            .enumerate()
            .filter_map(|(index, cell)| (food(cell) == RIPE).then_some(index))
    }

    pub fn scent(&self) -> impl Iterator<Item = u8> + '_ {
        self.cells()
            .iter()
            .map(|cell| (cell.memory[1] & SCENT_MASK) as u8)
    }

    pub fn links(&self) -> impl Iterator<Item = u8> + '_ {
        self.engine.sites().iter().map(|site| {
            site.links.iter().enumerate().fold(0, |mask, (port, link)| {
                mask | (u8::from(link.is_open()) << port)
            })
        })
    }
}

fn food(cell: &CellState) -> u32 {
    (cell.memory[1] >> FOOD_SHIFT) & FOOD_MASK
}

fn seeded_world(size: usize, mut seed: u32, bodies: &[Body], pellets: &[usize]) -> Result<World> {
    let open = CellState {
        outbox: [OPEN; PORTS],
        ..CellState::default()
    };
    let mut cells = vec![open; size * size];
    for (worm, body) in bodies.iter().enumerate() {
        let id = (worm as u32 + 1) << 3;
        let heading = cardinal_neighbors(body[1], size, size)
            .into_iter()
            .position(|neighbor| neighbor == Some(body[0]))
            .expect("the initial head has a neck") as u32;
        for (index, &cell) in body.iter().enumerate() {
            let role = index as u32 + 1;
            cells[cell].memory[0] = id | role;
            cells[cell].outbox = [id | OCCUPIED | (role << PORT_ROLE_SHIFT); PORTS];
        }
        cells[body[0]].memory[0] |= heading << HEADING_SHIFT;
    }
    for &cell in pellets {
        let field = FOOD_SCENT | (RIPE << FOOD_SHIFT);
        cells[cell].memory[1] = field;
        cells[cell].outbox = [OPEN | (field << HEADING_SHIFT); PORTS];
    }
    // Bits 0–1 are the coordinate parities used for disjoint neighbor pairs.
    // All other bits provide fixed per-site entropy for tie breaks and food.
    let inputs: Vec<_> = (0..cells.len())
        .map(|index| {
            let odd_column = (index % size) & 1;
            let odd_row = (index / size) & 1;
            let parity = odd_column | (odd_row << 1);
            ((random_index(&mut seed, 1 << 30) as u32) << 2) | parity as u32
        })
        .collect();
    World::grid(size, size, &inputs, cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_mlp_matches_table_on_full_words() {
        let mut seed = 412_001u32;
        let mut word = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let table = rule_table();
        for (phase, program) in mlp_kernels::PROGRAMS.into_iter().enumerate() {
            let inputs = std::array::from_fn::<_, LANES, _>(|lane| {
                let mut next = || match lane {
                    0 => 0,
                    1 => u32::MAX,
                    2 => 1 << 31,
                    3 => 1,
                    _ => word(),
                };
                let input = next();
                let inbox = std::array::from_fn(|_| next());
                let mut cell = CellState {
                    memory: std::array::from_fn(|_| next()),
                    answer: next(),
                    ..CellState::default()
                };
                cell.memory[3] = (cell.memory[3] & !PHASE_MASK) | phase as u32;
                (input, inbox, cell)
            });
            let mut cells = [CellState::default(); LANES];
            rule_mlp::run(program, &inputs, &mut cells);
            for (lane, ((input, inbox, cell), actual)) in inputs.into_iter().zip(cells).enumerate()
            {
                assert_eq!(
                    actual,
                    table.update(input, inbox, cell),
                    "phase {phase}, lane {lane}"
                );
            }
        }
    }

    #[test]
    fn integer_mlp_preserves_cells_and_observers_in_every_room_mode() -> Result<()> {
        for mode in [RoomMode::Off, RoomMode::Direct, RoomMode::Wave] {
            let mut reference = AutonomousArena::generate_with_room_mode(16, 412_002, 18, 4, mode)?;
            reference.advance(7)?;
            let mut model = reference.clone();
            for ticks in [1, 1, 13, 24, 23, 38] {
                reference.advance(ticks)?;
                model.advance_mlp(ticks)?;
                assert_eq!(
                    model.cells(),
                    reference.cells(),
                    "{mode:?} at tick {}",
                    model.tick()
                );
                assert_eq!(model.bodies(), reference.bodies());
                assert_eq!(model.room_signals(), reference.room_signals());
                assert_eq!(
                    model.forwarded_room_signals(),
                    reference.forwarded_room_signals()
                );
                assert_eq!(model.recovery_stats(), reference.recovery_stats());
            }
        }
        Ok(())
    }

    fn check_grid(arena: &AutonomousArena, count: usize, pellets: usize) {
        let mut segments = vec![[None; WORM_LENGTH]; count];
        let mut tokens = 0;
        for (index, cell) in arena.cells().iter().enumerate() {
            let segment = cell.memory[0];
            let food = food(cell);
            tokens += usize::from(food != 0);
            if segment == 0 {
                continue;
            }
            assert_ne!(food, RIPE, "a pellet grew on a worm");
            let worm = ((segment & ID_MASK) >> 3) as usize - 1;
            let role = (segment & ROLE_MASK) as usize - 1;
            assert!(segments[worm][role].replace(index).is_none());
        }
        assert_eq!(tokens, pellets);
        for (worm, segments) in segments.into_iter().enumerate() {
            let body = segments.map(|cell| cell.expect("lost a body segment"));
            assert_eq!(body, arena.bodies()[worm]);
            for pair in body.windows(2) {
                assert!(
                    cardinal_neighbors(pair[0], arena.size(), arena.size())
                        .contains(&Some(pair[1]))
                );
            }
        }
    }

    #[test]
    fn trapped_head_passes_leadership_to_its_tail() -> Result<()> {
        let bodies = vec![[0, 1, 7, 6, 12]];
        let world = seeded_world(6, 11, &bodies, &[18])?;
        let mut arena = AutonomousArena::new(world, 6, bodies);
        arena.advance(ROUND_TICKS - 1)?;
        assert_eq!(arena.bodies()[0], [0, 1, 7, 6, 12]);
        check_grid(&arena, 1, 1);
        arena.advance(1)?;
        assert_eq!(arena.bodies()[0], [18, 12, 6, 7, 1]);
        assert_eq!(arena.reversals, 1);
        assert_eq!(arena.total_food, 1);
        check_grid(&arena, 1, 1);
        for _ in 0..128 {
            arena.advance(1)?;
            check_grid(&arena, 1, 1);
            if let Some(pellet) = arena.pellets().next() {
                assert_ne!(pellet, 18);
                return Ok(());
            }
        }
        panic!("the seed did not regrow");
    }

    #[test]
    fn one_worm_wins_a_contested_food_cell() -> Result<()> {
        let bodies = vec![[59, 58, 57, 56, 55], [61, 62, 63, 64, 65]];
        let world = seeded_world(11, 19, &bodies, &[60])?;
        let mut arena = AutonomousArena::new(world, 11, bodies);
        arena.advance(ROUND_TICKS)?;
        assert_eq!(
            arena.bodies().iter().filter(|body| body[0] == 60).count(),
            1
        );
        assert_eq!(arena.total_food, 1);
        assert_eq!(arena.contested, 1);
        check_grid(&arena, 2, 1);
        Ok(())
    }

    #[test]
    fn both_blocked_ends_latch_a_request_for_the_next_round() -> Result<()> {
        let bodies = vec![[0, 1, 8, 7, 14], [15, 16, 17, 18, 19], [37, 36, 35, 28, 21]];
        let world = seeded_world(7, 19, &bodies, &[22])?;
        let mut arena = AutonomousArena::new(world, 7, bodies);
        arena.advance(ROUND_TICKS)?;
        assert_eq!(arena.bodies()[0], [0, 1, 8, 7, 14]);
        assert_eq!(arena.blocked_rounds, 1);
        for end in [0, 14] {
            assert_ne!(arena.cells()[end].answer & ROOM_READY, 0);
        }
        assert_eq!(arena.cells()[7].answer & ROOM_READY, 0);
        assert!(arena.room_signals().is_empty());

        arena.advance(1)?;
        assert_eq!(arena.cells()[14].outbox[1] & (ID_MASK | ROLE_MASK), 1 << 3);
        assert_ne!(arena.cells()[14].outbox[3] & ROLE_MASK, 0);
        arena.advance(ROUND_TICKS - 1)?;
        assert_eq!(arena.room_signals(), &[[14, 15]]);
        check_grid(&arena, 3, 1);
        Ok(())
    }

    #[test]
    fn make_room_control_keeps_the_seed_and_suppresses_only_requests() -> Result<()> {
        let mut enabled = AutonomousArena::generate_with_make_room(11, 17_001, 12, 12, true)?;
        let mut disabled = AutonomousArena::generate_with_make_room(11, 17_001, 12, 12, false)?;
        for _ in 0..=ROUND_TICKS {
            for (a, b) in enabled.cells().iter().zip(disabled.cells()) {
                assert_eq!(a.memory, b.memory);
                assert_eq!(a.outbox, b.outbox);
                assert_eq!(a.answer | ROOM_DISABLED, b.answer);
            }
            if enabled.tick() == ROUND_TICKS {
                break;
            }
            enabled.advance(1)?;
            disabled.advance(1)?;
        }
        let mut requests = 0;
        for _ in 0..80 {
            enabled.advance(ROUND_TICKS)?;
            disabled.advance(ROUND_TICKS)?;
            requests += enabled.room_signals().len();
            assert!(disabled.room_signals().is_empty());
            assert!(disabled.forwarded_room_signals().is_empty());
            assert!(enabled.forwarded_room_signals().is_empty());
            assert!(disabled.yielded_worms().is_empty());
        }
        assert!(requests > 0);
        assert_eq!(enabled.tick(), disabled.tick());
        Ok(())
    }

    #[test]
    fn a_blocked_recipient_forwards_to_its_blocker_through_local_cells() -> Result<()> {
        let bodies = vec![
            [30, 23, 22, 21, 14],
            [15, 16, 17, 18, 19],
            [12, 13, 20, 27, 26],
            [8, 7, 0, 1, 2],
        ];
        let mut world = seeded_world(7, 19, &bodies, &[5])?;
        world.initial[14].answer = ROOM_READY;
        let mut direct = AutonomousArena::new(world.clone(), 7, bodies.clone());
        RoomMode::Wave.seed(&mut world);
        let mut reference = CpuSwarm::new(world.clone(), rule_table());
        let mut wave = AutonomousArena::new(world, 7, bodies);
        reference.advance(2 * ROUND_TICKS)?;
        direct.advance(2 * ROUND_TICKS)?;
        wave.advance(ROUND_TICKS)?;
        assert!(wave.room_signals().contains(&[14, 15]));
        assert_eq!(wave.bodies()[1], [15, 16, 17, 18, 19]);
        assert_eq!(
            wave.cells()[19].memory[1] & ROOM_NEXT_HOP_MASK,
            1 << ROOM_NEXT_HOP_SHIFT
        );
        wave.advance(1)?;
        let message = wave.cells()[19].outbox[2];
        assert_eq!(message & (ID_MASK | ROLE_MASK), 2 << 3);
        assert_eq!(message >> PORT_WAIT_SHIFT, 1);
        wave.advance(ROUND_TICKS - 1)?;
        assert!(wave.forwarded_room_signals().contains(&[19, 12]));
        assert!(!wave.room_signals().contains(&[19, 12]));
        assert!(direct.forwarded_room_signals().is_empty());
        assert!(direct.room_signals().contains(&[19, 12]));
        assert_eq!(wave.cells(), reference.cells());
        check_grid(&wave, 4, 1);
        Ok(())
    }

    #[test]
    fn a_request_at_the_hop_limit_can_be_heard_and_then_expires() -> Result<()> {
        let bodies = vec![
            [30, 23, 22, 21, 14],
            [15, 16, 17, 18, 19],
            [12, 13, 20, 27, 26],
            [8, 7, 0, 1, 2],
        ];
        let mut world = seeded_world(7, 19, &bodies, &[5])?;
        RoomMode::Wave.seed(&mut world);
        world.initial[14].answer |= ROOM_READY;
        world.initial[14].memory[1] |= MAX_ROOM_FORWARDS << ROOM_NEXT_HOP_SHIFT;
        let mut wave = AutonomousArena::new(world, 7, bodies);
        wave.advance(ROUND_TICKS)?;
        assert!(wave.forwarded_room_signals().contains(&[14, 15]));
        assert_eq!(
            wave.cells()[19].answer & ROOM_HOP_MASK,
            MAX_ROOM_FORWARDS << ROOM_HOP_SHIFT
        );
        assert_eq!(wave.cells()[19].memory[1] & ROOM_NEXT_HOP_MASK, 0);
        wave.advance(1)?;
        assert_eq!(wave.cells()[19].outbox[2] >> PORT_WAIT_SHIFT, 0);
        Ok(())
    }

    #[test]
    fn request_can_change_which_end_leads_and_free_the_contacted_tile() -> Result<()> {
        let bodies = vec![[0, 1, 8, 7, 14], [15, 16, 17, 18, 19], [37, 36, 35, 28, 21]];
        let mut world = seeded_world(7, 19, &bodies, &[22])?;
        let mut control = AutonomousArena::new(world.clone(), 7, bodies.clone());
        // Start from a sender already waiting to ask in this round.
        world.initial[14].answer = ROOM_READY;
        let mut signaled = AutonomousArena::new(world, 7, bodies);
        control.advance(ROUND_TICKS)?;
        signaled.advance(ROUND_TICKS)?;

        assert_eq!(control.bodies()[1], [22, 15, 16, 17, 18]);
        assert_eq!(signaled.bodies()[1][1..], [19, 18, 17, 16]);
        assert_eq!(signaled.cells()[15].memory[0], 0);
        assert_eq!(signaled.room_signals(), &[[14, 15], [14, 21]]);
        assert!(signaled.yielded_worms().contains(&1));
        assert_eq!(
            signaled.cells_freed as usize,
            signaled.yielded_worms().len()
        );
        check_grid(&signaled, 3, 1);
        Ok(())
    }

    #[test]
    fn a_blocked_preferred_end_falls_back_without_reporting_a_false_yield() -> Result<()> {
        let bodies = vec![[23, 22, 21, 20, 19], [17, 11, 10, 4, 5]];
        let mut world = seeded_world(6, 19, &bodies, &[16])?;
        world.initial[23].answer = ROOM_READY;
        let mut arena = AutonomousArena::new(world, 6, bodies);
        arena.advance(ROUND_TICKS)?;
        assert_eq!(arena.room_signals(), &[[23, 17]]);
        assert_eq!(arena.bodies()[1], [16, 17, 11, 10, 4]);
        assert!(arena.yielded_worms().is_empty());
        assert_eq!(arena.cells_freed, 0);
        check_grid(&arena, 2, 1);
        Ok(())
    }

    #[test]
    fn a_make_room_preference_expires_at_the_start_of_the_next_round() -> Result<()> {
        let bodies = vec![[59, 58, 57, 56, 55]];
        let mut world = seeded_world(11, 19, &bodies, &[60])?;
        // The previous round just ended with a preference to move tail-first.
        for &index in &bodies[0] {
            let cell = &mut world.initial[index];
            cell.answer = ROOM_PRESSURE_MASK;
            for message in &mut cell.outbox {
                *message |= ROLE_MASK << PORT_WAIT_SHIFT;
            }
        }
        let mut arena = AutonomousArena::new(world, 11, bodies);
        arena.advance(1)?;
        for &index in &arena.bodies()[0] {
            assert!(
                arena.cells()[index]
                    .outbox
                    .iter()
                    .all(|message| message >> PORT_WAIT_SHIFT == 0)
            );
        }
        arena.advance(ROUND_TICKS - 1)?;
        assert_eq!(arena.bodies()[0], [60, 59, 58, 57, 56]);
        assert_eq!(arena.reversals, 0);
        assert!(arena.room_signals().is_empty());
        Ok(())
    }

    #[test]
    fn the_tail_waits_before_undoing_a_move_and_can_still_back_out() -> Result<()> {
        let bodies = vec![[1, 4, 3, 6, 7]];
        let world = seeded_world(3, 19, &bodies, &[0])?;
        let mut arena = AutonomousArena::new(world, 3, bodies);
        arena.advance(ROUND_TICKS)?;
        assert_eq!(arena.bodies()[0], [0, 1, 4, 3, 6]);
        assert_ne!(arena.cells()[6].memory[0] & RETURN_FRESH, 0);
        assert_eq!((arena.cells()[6].memory[0] >> RETURN_PORT_SHIFT) & 15, 2);

        arena.advance(ROUND_TICKS)?;
        assert_eq!(arena.bodies()[0], [0, 1, 4, 3, 6]);
        assert_eq!(arena.cells()[6].memory[0] & RETURN_FRESH, 0);
        assert_eq!(arena.blocked_rounds, 1);
        let waiting = arena.recovery_stats();
        assert_eq!(waiting.average_rounds, None);
        assert_eq!(waiting.still_stuck, 1);
        assert_eq!(waiting.longest_stuck_rounds, 1);
        arena.advance(ROUND_TICKS)?;
        assert_eq!(arena.bodies()[0], [7, 6, 3, 4, 1]);
        assert_eq!(arena.reversals, 1);
        let recovered = arena.recovery_stats();
        assert_eq!(recovered.recovered, 1);
        assert_eq!(recovered.average_rounds, Some(1.0));
        assert_eq!(recovered.p90_rounds, Some(1));
        assert_eq!(recovered.longest_completed_rounds, Some(1));
        assert_eq!(recovered.still_stuck, 0);
        check_grid(&arena, 1, 1);
        Ok(())
    }

    #[test]
    fn direct_table_ticks_match_the_observed_arena() -> Result<()> {
        let bodies = initial_bodies(11, 12);
        let world = seeded_world(11, 17_001, &bodies, &[14, 16, 36, 58])?;
        let mut reference = CpuSwarm::new(world.clone(), rule_table());
        let mut arena = AutonomousArena::new(world, 11, bodies);
        // The reference has no arena, RNG calls, or input writes between ticks.
        for _ in 0..16 * ROUND_TICKS {
            reference.advance(1)?;
            arena.advance(1)?;
            assert_eq!(reference.cells(), arena.cells());
            check_grid(&arena, 12, 4);
        }
        assert!(arena.total_food > 0);
        assert!(arena.blocked_rounds > 0);
        Ok(())
    }
}
