//! Browser arena for the fixed-length formation rules from Automaton Swarm.

use anyhow::{Context, Result, ensure};

use crate::formation_protocol::*;
use crate::spawn::{Body, WORM_LENGTH, initial_bodies, initial_pellets};
use crate::world::{cardinal_neighbors, random_index};
use crate::{MEMORY_WORDS, PORTS, Site, World};

mod formation;
use formation::{FormationCell, WormBox};
pub use formation::{
    WormBoxStatus, WormFormationBehavior, WormFormationCue, WormPatternOverview, WormPatternStatus,
};

mod compiled {
    #![allow(clippy::all)]
    use super::{CellState, PORTS};
    include!(concat!(env!("OUT_DIR"), "/worm_formation_rule.rs"));
}

pub const BROWSER_GRID_SIZE: usize = 350;
pub const BROWSER_WORMS: usize = 10_000;
pub const BROWSER_PELLETS: usize = 300;
pub const ROUND_TICKS: u32 = TICKS_PER_MOVE;

/// Body memory stays compact while packets carry the wider worm identities.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellState {
    pub memory: [u32; MEMORY_WORDS],
    pub outbox: [u64; PORTS],
    pub answer: u64,
}

#[derive(Clone)]
struct FormationGrid {
    sites: Vec<Site>,
    current: Vec<CellState>,
    next: Vec<CellState>,
    tick: u32,
}

impl FormationGrid {
    fn new(world: World<CellState>) -> Self {
        Self {
            sites: world.sites,
            next: vec![CellState::default(); world.initial.len()],
            current: world.initial,
            tick: 0,
        }
    }

    fn sites(&self) -> &[Site] {
        &self.sites
    }

    fn inbox(&self, site: &Site) -> [u64; PORTS] {
        site.links.map(|link| {
            if link.is_open() {
                self.current[link.site as usize].outbox[link.port as usize]
            } else {
                0
            }
        })
    }

    fn advance<const PHASE: u32>(&mut self, guide: Option<&WormBox>) {
        for (index, site) in self.sites.iter().enumerate() {
            let previous = self.current[index];
            let mut inbox = self.inbox(site);
            if let Some(guide) =
                guide.filter(|_| matches!(u64::from(PHASE), ASK_FIRST | ASK_SECOND))
            {
                inbox = guide.movement_inbox(
                    index,
                    FormationCell::segment(u64::from(previous.memory[0]), HEADING_SHIFT),
                    inbox,
                );
            }
            self.next[index] = compiled::update_at_phase::<PHASE>(site.input, inbox, previous);
        }
        std::mem::swap(&mut self.current, &mut self.next);
        self.tick += 1;
    }
}

#[derive(Clone)]
pub struct FormationArena {
    engine: FormationGrid,
    size: usize,
    bodies: Vec<Body>,
    blocked_rounds: u64,
    worm_box: Option<WormBox>,
}

impl FormationArena {
    pub fn browser(seed: u32) -> Result<Self> {
        Self::generate(
            BROWSER_GRID_SIZE,
            seed,
            BROWSER_WORMS,
            BROWSER_PELLETS,
            ROOM_WAVE,
        )
    }

    fn generate(size: usize, seed: u32, count: usize, pellets: usize, room: u64) -> Result<Self> {
        let bodies = initial_bodies(size, count);
        let mut seed = seed;
        let pellets = initial_pellets(size * size, &bodies, pellets, &mut seed);
        let mut world = seeded_world(size, seed, &bodies, &pellets)?;
        seed_room(&mut world, room);
        Ok(Self::new(world, size, bodies))
    }

    fn new(world: World<CellState>, size: usize, bodies: Vec<Body>) -> Self {
        Self {
            engine: FormationGrid::new(world),
            size,
            bodies,
            blocked_rounds: 0,
            worm_box: None,
        }
    }

    pub fn advance(&mut self, ticks: u32) -> Result<()> {
        let end = self.tick().checked_add(ticks).context("tick overflow")?;
        while self.tick() < end {
            let phase = self.tick() % ROUND_TICKS;
            let guide = self.worm_box.as_ref().filter(|guide| guide.active());
            // Specialize the compiled table to the shared phase, outside the cell loop.
            macro_rules! advance_at_phase {
                ($($value:literal),+) => {
                    match phase {
                        $($value => self.engine.advance::<$value>(guide),)+
                        _ => unreachable!("unsupported formation phase"),
                    }
                };
            }
            const { assert!(ROUND_TICKS == 24) };
            advance_at_phase!(
                0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22,
                23
            );
            if self.tick().is_multiple_of(ROUND_TICKS) {
                self.observe_commit();
                if let Some(mut guide) = self.worm_box.take() {
                    guide.refresh_browser(self);
                    self.worm_box = Some(guide);
                }
            }
        }
        Ok(())
    }

    fn observe_commit(&mut self) {
        for (index, cell) in self.engine.current.iter().enumerate() {
            let segment = u64::from(cell.memory[0]);
            if segment != 0 {
                let worm = ((segment & ID_MASK) >> 3) as usize - 1;
                let part = (segment & ROLE_MASK) as usize - 1;
                self.bodies[worm][part] = index;
                self.blocked_rounds += u64::from(cell.answer & BLOCKED != 0);
            }
        }
    }

    pub fn tick(&self) -> u32 {
        self.engine.tick
    }

    pub fn bodies(&self) -> &[Body] {
        &self.bodies
    }

    pub fn cells(&self) -> &[CellState] {
        &self.engine.current
    }

    pub fn headings(&self) -> impl Iterator<Item = u8> + '_ {
        self.bodies
            .iter()
            .map(|body| ((u64::from(self.cells()[body[0]].memory[0]) >> HEADING_SHIFT) & 3) as u8)
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
            .map(|cell| (cell.memory[1] & SCENT_MASK as u32) as u8)
    }

    pub fn links(&self) -> impl Iterator<Item = u8> + '_ {
        self.engine.sites().iter().map(|site| {
            site.links.iter().enumerate().fold(0, |mask, (port, link)| {
                mask | (u8::from(link.is_open()) << port)
            })
        })
    }

    pub fn cell_words(&self, index: usize) -> Option<Vec<u64>> {
        let cell = self.cells().get(index)?;
        let mut words = Vec::with_capacity(MEMORY_WORDS + PORTS * 2 + 1);
        words.extend(cell.memory.into_iter().map(u64::from));
        words.extend(cell.outbox);
        words.extend(self.engine.inbox(&self.engine.sites()[index]));
        words.push(cell.answer);
        Some(words)
    }

    pub fn held_worms(&self) -> impl Iterator<Item = u32> + '_ {
        self.worm_box
            .iter()
            .filter(|guide| guide.active())
            .flat_map(|guide| {
                self.bodies
                    .iter()
                    .enumerate()
                    .filter_map(|(index, body)| guide.held_at(body[0]).then_some(index as u32))
            })
    }
}

fn food(cell: &CellState) -> u64 {
    (u64::from(cell.memory[1]) >> FOOD_SHIFT) & FOOD_MASK
}

fn seed_room(world: &mut World<CellState>, room: u64) {
    for cell in &mut world.initial {
        cell.answer |= room;
    }
}

fn seeded_world(
    size: usize,
    mut seed: u32,
    bodies: &[Body],
    pellets: &[usize],
) -> Result<World<CellState>> {
    ensure!(size >= 7, "Worm formations need a grid at least 7 × 7");
    let open = CellState {
        outbox: [OPEN; PORTS],
        ..CellState::default()
    };
    let mut cells = vec![open; size * size];
    for (worm, body) in bodies.iter().enumerate() {
        let id = (worm as u64 + 1) << 3;
        let heading = cardinal_neighbors(body[1], size, size)
            .into_iter()
            .position(|neighbor| neighbor == Some(body[0]))
            .expect("the initial head has a neck") as u64;
        for (index, &site) in body.iter().enumerate() {
            let role = index as u64 + 1;
            let direction = if index == 0 {
                heading << HEADING_SHIFT
            } else {
                0
            };
            cells[site].memory[0] = (id | role | direction) as u32;
            cells[site].outbox = [id | OCCUPIED | (role << PORT_ROLE_SHIFT); PORTS];
        }
    }
    for &site in pellets {
        let field = FOOD_SCENT | (RIPE << FOOD_SHIFT);
        cells[site].memory[1] = field as u32;
        cells[site].outbox = [OPEN | (field << HEADING_SHIFT); PORTS];
    }
    let inputs: Vec<_> = (0..cells.len())
        .map(|index| {
            let parity = ((index % size) & 1) | (((index / size) & 1) << 1);
            ((random_index(&mut seed, 1 << 30) as u32) << 2) | parity as u32
        })
        .collect();
    World::grid(size, size, &inputs, cells)
}
