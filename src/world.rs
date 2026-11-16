use anyhow::{Context, Result, ensure};
use bytemuck::{Pod, Zeroable};

use crate::RuleTable;

pub const PORTS: usize = 4;
pub const MEMORY_WORDS: usize = 4;

// Tommy Ettinger's CC0 Mulberry32 with Automata Core's bounded-index selection.
pub(crate) fn random_index(state: &mut u32, limit: usize) -> usize {
    *state = state.wrapping_add(0x6d2b_79f5);
    let mut value = (*state ^ (*state >> 15)).wrapping_mul(*state | 1);
    value ^= value.wrapping_add((value ^ (value >> 7)).wrapping_mul(value | 61));
    ((value ^ (value >> 14)) as usize) % limit
}

pub(crate) fn cardinal_neighbors(
    index: usize,
    width: usize,
    height: usize,
) -> [Option<usize>; PORTS] {
    let x = index % width;
    let y = index / width;
    [
        (x > 0).then(|| index - 1),
        (x + 1 < width).then(|| index + 1),
        (y > 0).then(|| index - width),
        (y + 1 < height).then(|| index + width),
    ]
}

/// A local input port can read one previous-tick outbox on another site.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct Link {
    pub site: u32,
    pub port: u32,
}

impl Link {
    pub const CLOSED: Self = Self {
        site: u32::MAX,
        port: 0,
    };

    pub fn is_open(self) -> bool {
        self.site != Self::CLOSED.site
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct Site {
    pub input: u32,
    pub links: [Link; PORTS],
}

impl Site {
    fn inbox(self, cells: &[CellState]) -> [u32; PORTS] {
        self.links.map(|link| {
            if link.is_open() {
                cells[link.site as usize].outbox[link.port as usize]
            } else {
                0
            }
        })
    }
}

/// State committed at the end of a tick, including the messages produced in that tick.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct CellState {
    pub memory: [u32; MEMORY_WORDS],
    pub outbox: [u32; PORTS],
    pub answer: u32,
}

pub(crate) type CellInput = (u32, [u32; PORTS], CellState);

#[derive(Clone, Debug)]
pub struct World {
    pub(crate) sites: Vec<Site>,
    pub(crate) initial: Vec<CellState>,
}

impl World {
    pub fn new(sites: Vec<Site>, initial: Vec<CellState>) -> Result<Self> {
        ensure!(!sites.is_empty(), "a world needs at least one site");
        ensure!(sites.len() <= u32::MAX as usize, "too many sites");
        ensure!(
            sites.len() == initial.len(),
            "state count must match site count"
        );
        for (index, site) in sites.iter().enumerate() {
            for link in site.links {
                ensure!(
                    link == Link::CLOSED
                        || (link.site as usize) < sites.len() && (link.port as usize) < PORTS,
                    "invalid link at site {index}"
                );
            }
        }
        Ok(Self { sites, initial })
    }

    /// Ports are west, east, north, south. Grid boundaries supply zero.
    pub fn grid(
        width: usize,
        height: usize,
        inputs: &[u32],
        initial: Vec<CellState>,
    ) -> Result<Self> {
        ensure!(width > 0 && height > 0, "grid dimensions must be positive");
        ensure!(
            width.checked_mul(height) == Some(inputs.len()),
            "grid size must match inputs"
        );
        ensure!(inputs.len() <= u32::MAX as usize, "too many grid sites");
        let sites = inputs
            .iter()
            .enumerate()
            .map(|(index, &input)| {
                let neighbors = cardinal_neighbors(index, width, height);
                Site {
                    input,
                    links: std::array::from_fn(|port| match neighbors[port] {
                        Some(site) => Link {
                            site: site as u32,
                            port: (port ^ 1) as u32,
                        },
                        _ => Link::CLOSED,
                    }),
                }
            })
            .collect();
        Self::new(sites, initial)
    }

    pub fn sites(&self) -> &[Site] {
        &self.sites
    }

    pub fn initial(&self) -> &[CellState] {
        &self.initial
    }
}

#[derive(Clone)]
pub struct CpuSwarm {
    sites: Vec<Site>,
    rules: RuleTable,
    current: Vec<CellState>,
    next: Vec<CellState>,
    tick: u32,
}

impl CpuSwarm {
    pub fn new(world: World, rules: RuleTable) -> Self {
        let next = vec![CellState::default(); world.initial.len()];
        Self {
            sites: world.sites,
            rules,
            current: world.initial,
            next,
            tick: 0,
        }
    }

    pub fn tick(&self) -> u32 {
        self.tick
    }

    pub fn cells(&self) -> &[CellState] {
        &self.current
    }

    pub(crate) fn sites(&self) -> &[Site] {
        &self.sites
    }

    pub fn advance(&mut self, ticks: u32) -> Result<()> {
        self.advance_with(ticks, |rules, input, inbox, cell| {
            rules.update(input, inbox, cell)
        })
    }

    pub(crate) fn advance_with(
        &mut self,
        ticks: u32,
        mut update: impl FnMut(&RuleTable, u32, [u32; PORTS], CellState) -> CellState,
    ) -> Result<()> {
        let end = self.tick.checked_add(ticks).context("tick overflow")?;
        for _ in 0..ticks {
            for (index, site) in self.sites.iter().enumerate() {
                self.next[index] = update(
                    &self.rules,
                    site.input,
                    site.inbox(&self.current),
                    self.current[index],
                );
            }
            std::mem::swap(&mut self.current, &mut self.next);
        }
        self.tick = end;
        Ok(())
    }

    pub(crate) fn advance_lanes<const LANES: usize>(
        &mut self,
        mut update: impl FnMut(&[CellInput], &mut [CellState]),
    ) -> Result<()> {
        let end = self.tick.checked_add(1).context("tick overflow")?;
        let mut inputs = [(0, [0; PORTS], CellState::default()); LANES];
        for (batch, next) in self.next.chunks_mut(LANES).enumerate() {
            let start = batch * LANES;
            let end = start + next.len();
            for ((input, &site), &state) in inputs
                .iter_mut()
                .zip(&self.sites[start..end])
                .zip(&self.current[start..end])
            {
                *input = (site.input, site.inbox(&self.current), state);
            }
            update(&inputs[..next.len()], next);
        }
        std::mem::swap(&mut self.current, &mut self.next);
        self.tick = end;
        Ok(())
    }
}
