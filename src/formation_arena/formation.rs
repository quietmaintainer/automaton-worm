//! Clocked background tiles guide fixed-length worms. The stock rule still
//! grants and commits every move.

use std::collections::BTreeMap;

use anyhow::{Result, ensure};
use serde::Serialize;

use super::CellState;
use super::{
    FormationArena, HEADING_SHIFT, OCCUPIED, OPEN, PORTS, ROLE_MASK, ROUND_TICKS, WORM_LENGTH,
};
use crate::Site;

mod branch_plan;
use branch_plan::BranchPlan;
mod handoff;
use handoff::BoxTraffic;

const SIDE: usize = WORM_LENGTH;
const SQUARE: usize = SIDE * SIDE;
const ROUTE_HOPS: u16 = 16;
const MAX_PATTERN_HOPS: u16 = 128;
const WIDEN_AFTER: u32 = 8 * ROUND_TICKS;
const ROW_INTERVAL: u32 = 4 * ROUND_TICKS;
const RECRUIT_AGAIN_AFTER: u32 = 8 * ROUND_TICKS;
const EMPTY_BOXES_PER_ROW: usize = 2;
const RULE_90: u8 = 90;
const GUIDE_AFTER_HOPS: u16 = 40;
const CLEAR_LANE_AFTER_ROUNDS: u16 = 16;
const RELEASE_PARTIAL_BOX_AFTER_ROUNDS: u16 = 96;

#[derive(Clone, Copy)]
pub enum WormFormationCue {
    Rule(u8),
    RestrictAccess(bool),
    Behavior(WormFormationBehavior),
    Start { rule: Option<u8> },
    Retarget { rule: u8 },
    Seed { rule: Option<u8> },
    Step { rule: Option<u8> },
    Box,
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WormFormationBehavior {
    Fill,
    Restrict,
    FillEmpty,
}

impl WormFormationBehavior {
    fn targets(self, bit: bool) -> bool {
        bit != (self == Self::FillEmpty)
    }
}

// The planner uses role, identity and heading. Keep its tile words compact
// even when the native rule uses 32-bit IDs on the wire.
const FORMATION_HEADING_SHIFT: u64 = 23;
const FORMATION_ID_MASK: u32 = ((1 << FORMATION_HEADING_SHIFT) - 1) & !(ROLE_MASK as u32);

#[derive(Clone, Copy)]
pub(super) struct FormationCell {
    segment: u32,
    blocked: bool,
}

impl FormationCell {
    #[inline]
    pub(super) fn segment(body: u64, heading_shift: u64) -> u32 {
        let heading = ((body >> heading_shift) & 3) as u32;
        let id_and_role = (body & ((1 << heading_shift) - 1)) as u32;
        id_and_role | (heading << FORMATION_HEADING_SHIFT)
    }

    pub(super) fn new(body: u64, blocked: bool, heading_shift: u64) -> Self {
        Self {
            segment: Self::segment(body, heading_shift),
            blocked,
        }
    }
}

impl From<&CellState> for FormationCell {
    fn from(cell: &CellState) -> Self {
        Self::new(
            u64::from(cell.memory[0]),
            cell.answer & super::BLOCKED != 0,
            HEADING_SHIFT,
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    Box,
    Pattern(usize),
}

#[derive(Clone)]
struct Pattern {
    left: usize,
    top: usize,
    columns: usize,
    max_rows: usize,
    rows: Vec<Vec<bool>>,
    next_row: Option<u32>,
}

impl Pattern {
    fn max_rows(width: usize) -> usize {
        width / SIDE
    }

    fn new(width: usize) -> Self {
        let max_rows = Self::max_rows(width);
        let columns = width / SIDE;
        let mut seed = vec![false; columns];
        seed[columns / 2] = true;
        Self {
            left: (width - columns * SIDE) / 2,
            top: 0,
            columns,
            max_rows,
            rows: vec![seed],
            next_row: None,
        }
    }

    fn step(&mut self, rule: u8) {
        // One background bit per box position. The three neighbors in the
        // committed row supply the next tile, with zero at either boundary.
        let above = self.rows.last().expect("the pattern has a seed");
        let next = (0..self.columns)
            .map(|column| {
                let left = column > 0 && above[column - 1];
                let center = above[column];
                let right = above.get(column + 1).copied().unwrap_or(false);
                let index = 4 * u8::from(left) + 2 * u8::from(center) + u8::from(right);
                rule & (1 << index) != 0
            })
            .collect();
        self.rows.push(next);
    }

    fn schedule_next(&mut self, tick: u32) {
        self.next_row =
            (self.rows.len() < self.max_rows).then(|| tick.saturating_add(ROW_INTERVAL));
    }

    fn boxes_in_row(
        &self,
        row: usize,
        behavior: WormFormationBehavior,
    ) -> impl Iterator<Item = [usize; 2]> + '_ {
        let bits = &self.rows[row];
        let mut columns: Vec<_> = bits
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(column, bit)| behavior.targets(bit).then_some(column))
            .collect();
        if behavior == WormFormationBehavior::FillEmpty {
            // Fill the background nearest the pattern first so its shape is visible
            // while there are still more empty boxes than worms can occupy.
            columns.sort_by_cached_key(|&column| {
                let center = column.abs_diff(self.columns / 2);
                let distance = bits
                    .iter()
                    .enumerate()
                    .filter_map(|(other, &bit)| bit.then_some(column.abs_diff(other)))
                    .min()
                    .unwrap_or(center);
                (distance, center)
            });
        }
        columns
            .into_iter()
            .map(move |column| [self.left + column * SIDE, self.top + row * SIDE])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WormBoxAxis {
    Horizontal,
    Vertical,
}

impl WormBoxAxis {
    const ALL: [Self; 2] = [Self::Horizontal, Self::Vertical];

    fn forward(self) -> usize {
        1 + 2 * self as usize
    }

    fn lane(self, tile: usize) -> usize {
        match self {
            Self::Horizontal => tile / SIDE,
            Self::Vertical => tile % SIDE,
        }
    }

    fn offset(self, tile: usize) -> usize {
        match self {
            Self::Horizontal => tile % SIDE,
            Self::Vertical => tile / SIDE,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Dock {
    box_index: usize,
    axis: WormBoxAxis,
    lane: usize,
    increasing: bool,
}

impl Dock {
    fn port(self) -> usize {
        self.axis.forward() ^ usize::from(!self.increasing)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Grant {
    dock: Dock,
    worm: u32,
}

#[derive(Clone, Copy)]
struct Route {
    dock: Dock,
    distance: u16,
    bends: u16,
    port: usize,
}

impl Route {
    fn key(self) -> (u16, u16, Dock, usize) {
        (self.distance, self.bends, self.dock, self.port)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Offer {
    distance: u16,
    bends: u16,
    grant: Grant,
    head: usize,
}

#[derive(Clone)]
struct Assignment {
    grant: Grant,
    path: Vec<usize>,
    step: usize,
    issued_at: u32,
}

impl Assignment {
    fn head_and_next(&self) -> Option<(usize, usize)> {
        Some((*self.path.get(self.step)?, *self.path.get(self.step + 1)?))
    }

    fn advance(&mut self, tiles: &[Tile], tick: u32) -> bool {
        let Some((head, next)) = self.head_and_next() else {
            return false;
        };
        let owns_head = |index: usize| {
            let segment = tiles[index].segment;
            role(segment) == 1 && worm_id(segment) == self.grant.worm
        };
        if tick == self.issued_at {
            return owns_head(head);
        }
        if !owns_head(next) {
            return false;
        }
        self.step += 1;
        true
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lane {
    Clear,
    Partial(u32),
    Filled(u32),
    Blocked,
}

/// A proof extends one tile at a time along consecutive roles of one worm.
#[derive(Clone, Copy)]
struct Proof {
    occupants: Lane,
    ascending: bool,
    descending: bool,
}

impl Proof {
    const BLOCKED: Self = Self {
        occupants: Lane::Blocked,
        ascending: false,
        descending: false,
    };

    fn start(segment: u32) -> Self {
        let id = worm_id(segment);
        Self {
            occupants: if id == 0 {
                Lane::Clear
            } else {
                Lane::Partial(id)
            },
            ascending: id != 0,
            descending: id != 0,
        }
    }

    fn prepend(self, segment: u32, neighbor: u32) -> Self {
        let id = worm_id(segment);
        let occupants = match self.occupants {
            lane if id == 0 => lane,
            Lane::Clear => Lane::Partial(id),
            Lane::Partial(other) if other == id => self.occupants,
            _ => Lane::Blocked,
        };
        let same = id != 0 && id == worm_id(neighbor);
        Self {
            occupants,
            ascending: same && self.ascending && role(segment) + 1 == role(neighbor),
            descending: same && self.descending && role(segment) == role(neighbor) + 1,
        }
    }

    fn report(self, segment: u32) -> Lane {
        if (role(segment) == 1 && self.ascending) || (role(segment) == SIDE && self.descending) {
            Lane::Filled(worm_id(segment))
        } else {
            self.occupants
        }
    }
}

#[derive(Clone, Copy)]
struct Anchor {
    axis: Option<WormBoxAxis>,
    lanes: [[Lane; SIDE]; 2],
    grant: Option<Grant>,
}

impl Anchor {
    const EMPTY: Self = Self {
        axis: None,
        lanes: [[Lane::Blocked; SIDE]; 2],
        grant: None,
    };

    fn lane(self, dock: Dock) -> Lane {
        self.lanes[dock.axis as usize][dock.lane]
    }

    fn observe(&mut self, lanes: [[Lane; SIDE]; 2]) {
        self.lanes = lanes;
        // Perpendicular full lanes share a tile, so only one axis can win.
        if self.axis.is_none() {
            self.axis = WormBoxAxis::ALL.into_iter().find(|&axis| {
                lanes[axis as usize]
                    .iter()
                    .any(|lane| matches!(lane, Lane::Filled(_)))
            });
        }
        self.grant = self.grant.filter(|grant| {
            self.axis.is_none_or(|axis| axis == grant.dock.axis)
                && match self.lane(grant.dock) {
                    Lane::Clear => true,
                    Lane::Partial(id) => id == grant.worm,
                    _ => false,
                }
        });
    }

    fn filled(self) -> [bool; SIDE] {
        self.axis.map_or([false; SIDE], |axis| {
            self.lanes[axis as usize].map(|lane| matches!(lane, Lane::Filled(_)))
        })
    }

    fn complete(self) -> bool {
        self.filled() == [true; SIDE]
    }
}

#[derive(Clone)]
struct WormBoxTiles {
    left: usize,
    top: usize,
    tiles: [usize; SQUARE],
    // Extended state of the northwest tile.
    anchor: Anchor,
    assignment: Option<Assignment>,
    second_assignment: Option<Assignment>,
    corridor: Option<WormBoxAxis>,
    traffic: BoxTraffic,
    guarded: bool,
    blocked_rounds: u16,
    flushing: bool,
}

impl WormBoxTiles {
    fn has_assignment(&self) -> bool {
        self.assignment.is_some() || self.second_assignment.is_some()
    }

    fn status(&self) -> WormBoxStatus {
        WormBoxStatus {
            active: true,
            left: self.left,
            top: self.top,
            axis: self.anchor.axis,
            lanes: self.anchor.filled(),
        }
    }
}

#[derive(Clone, Copy)]
struct Footprint {
    box_index: usize,
    local: usize,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Gate {
    #[default]
    Stock,
    Parked,
    Wait,
    Move(usize),
}

#[derive(Clone, Default)]
struct Tile {
    footprint: Option<Footprint>,
    links: [Option<usize>; PORTS],
    segment: u32,
    route: Option<Route>,
    reply: Option<Offer>,
    gate: Gate,
    reserved: bool,
    lane_owner: Option<u32>,
    seek: Option<u16>,
    egress: Option<u16>,
    through: bool,
}

impl Tile {
    fn offer(&mut self, reply: Offer) {
        if self.reply.is_none_or(|old| reply < old) {
            self.reply = Some(reply);
        }
    }

    fn is_reserved(&self) -> bool {
        self.reserved || self.lane_owner.is_some()
    }
}

#[derive(Clone)]
struct FormationTiles {
    width: usize,
    left: usize,
    top: usize,
    columns: usize,
    rows: usize,
    tiles: Vec<Tile>,
    boxes: Vec<WormBoxTiles>,
    pattern: Option<Pattern>,
    behavior: WormFormationBehavior,
    cells_inside: usize,
    reached: Vec<usize>,
    route_hops: u16,
    widen_at: Option<u32>,
    released_until: BTreeMap<u32, u32>,
    branch_population: Option<usize>,
    branches: Option<BranchPlan>,
    retargeted: bool,
    evacuating: bool,
}

impl FormationTiles {
    fn new(
        width: usize,
        pattern: Option<Pattern>,
        behavior: WormFormationBehavior,
        neighbors: impl Fn(usize) -> [Option<usize>; PORTS],
    ) -> Self {
        let offset = (width - SIDE) / 2;
        let (x, y, large_width, large_height) =
            pattern
                .as_ref()
                .map_or((offset, offset, SIDE, SIDE), |pattern| {
                    (
                        pattern.left,
                        pattern.top,
                        pattern.columns * SIDE,
                        pattern.max_rows * SIDE,
                    )
                });
        // Signals and full bodies near their edge stay inside the work area.
        let max_hops = if pattern.is_some() {
            MAX_PATTERN_HOPS
        } else {
            ROUTE_HOPS
        };
        let margin = usize::from(max_hops) + SIDE + 1;
        let left = x.saturating_sub(margin);
        let top = y.saturating_sub(margin);
        let columns = width.min(x + large_width + margin) - left;
        let rows = width.min(y + large_height + margin) - top;
        let targets = pattern.as_ref().map_or_else(
            || vec![[offset, offset]],
            |pattern| {
                (0..pattern.rows.len())
                    .flat_map(|row| pattern.boxes_in_row(row, behavior))
                    .collect()
            },
        );
        let mut field = Self {
            width,
            left,
            top,
            columns,
            rows,
            tiles: vec![Tile::default(); columns * rows],
            boxes: Vec::new(),
            pattern,
            behavior,
            cells_inside: 0,
            reached: Vec::new(),
            route_hops: ROUTE_HOPS,
            widen_at: None,
            released_until: BTreeMap::new(),
            branch_population: None,
            branches: None,
            retargeted: false,
            evacuating: false,
        };
        for i in 0..field.tiles.len() {
            let site = field.site(i);
            field.tiles[i].links =
                neighbors(site).map(|link| link.and_then(|site| field.index(site)));
        }
        for [left, top] in targets {
            field.add_box(left, top);
        }
        field
    }

    fn site(&self, index: usize) -> usize {
        (self.top + index / self.columns) * self.width + self.left + index % self.columns
    }

    fn index(&self, site: usize) -> Option<usize> {
        let x = (site % self.width).checked_sub(self.left)?;
        let y = (site / self.width).checked_sub(self.top)?;
        (x < self.columns && y < self.rows).then_some(y * self.columns + x)
    }

    fn add_box(&mut self, left: usize, top: usize) {
        let first = self
            .index(top * self.width + left)
            .expect("the box fits the work area");
        let box_index = self.boxes.len();
        let tiles = std::array::from_fn(|local| {
            let index = first + (local / SIDE) * self.columns + local % SIDE;
            debug_assert!(self.tiles[index].footprint.is_none());
            self.tiles[index].footprint = Some(Footprint { box_index, local });
            index
        });
        self.boxes.push(WormBoxTiles {
            left,
            top,
            tiles,
            anchor: Anchor::EMPTY,
            assignment: None,
            second_assignment: None,
            corridor: None,
            traffic: BoxTraffic::default(),
            guarded: false,
            blocked_rounds: 0,
            flushing: false,
        });
    }

    fn step_pattern(&mut self, rule: u8) {
        let pattern = self
            .pattern
            .as_mut()
            .expect("the pattern was seeded before stepping");
        pattern.step(rule);
        let targets: Vec<_> = pattern
            .boxes_in_row(pattern.rows.len() - 1, self.behavior)
            .collect();
        for [left, top] in targets {
            self.add_box(left, top);
        }
    }

    fn plan_rule_90(&mut self, population: usize) {
        let max_rows = self.pattern.as_ref().unwrap().max_rows;
        // Closing a box can cut off future rows, so plan the full pattern first.
        for _ in 1..max_rows {
            self.step_pattern(RULE_90);
        }
        self.branch_population = Some(population);
        self.branches = BranchPlan::new(self, population);
    }

    fn set_behavior(&mut self, behavior: WormFormationBehavior) {
        let Some(pattern) = &self.pattern else {
            return;
        };
        if self.behavior == behavior {
            return;
        }
        let targets_changed = self.behavior.targets(false) != behavior.targets(false);
        self.behavior = behavior;
        self.cells_inside = 0;
        self.reached.clear();
        self.released_until.clear();
        self.route_hops = ROUTE_HOPS;
        self.widen_at = None;
        self.evacuating = false;
        for tile in &mut self.tiles {
            tile.gate = Gate::Stock;
            tile.reserved = false;
            tile.lane_owner = None;
            tile.seek = None;
            tile.egress = None;
            tile.through = false;
            if targets_changed {
                tile.footprint = None;
            }
        }
        if targets_changed {
            let targets: Vec<_> = (0..pattern.rows.len())
                .flat_map(|row| pattern.boxes_in_row(row, behavior))
                .collect();
            self.boxes.clear();
            for [left, top] in targets {
                self.add_box(left, top);
            }
        } else {
            for box_tiles in &mut self.boxes {
                box_tiles.anchor = Anchor::EMPTY;
                box_tiles.assignment = None;
                box_tiles.second_assignment = None;
                box_tiles.corridor = None;
                box_tiles.traffic = BoxTraffic::default();
                box_tiles.guarded = false;
                box_tiles.blocked_rounds = 0;
                box_tiles.flushing = false;
            }
        }
        self.branches = self
            .branch_population
            .filter(|_| behavior == WormFormationBehavior::Fill)
            .and_then(|population| BranchPlan::new(self, population));
    }

    fn refresh(&mut self, cell: impl Fn(usize) -> FormationCell + Copy + Sync, tick: u32) {
        if self.behavior == WormFormationBehavior::Restrict {
            self.cells_inside = self
                .boxes
                .iter()
                .flat_map(|box_tiles| box_tiles.tiles)
                .filter(|&index| cell(self.site(index)).segment != 0)
                .count();
            return;
        }
        let branch_fill = self.branches.is_some();
        let parallel_arrivals = branch_fill || self.retargeted;
        let traffic_enabled = (self.branch_population.is_some() || self.retargeted)
            && self.behavior == WormFormationBehavior::Fill;
        let requests = if traffic_enabled {
            self.passage_requests(cell, tick)
        } else {
            Vec::new()
        };
        // A finished pattern only needs a new movement plan when traffic asks for room.
        if requests.is_empty()
            && self
                .boxes
                .iter()
                .all(|target| target.anchor.complete() && !target.traffic.in_progress())
        {
            return;
        }
        let prior_docked = self.pattern.is_some().then(|| self.docked_worms());
        if let Some(mut plan) = self.branches.take() {
            plan.admit(self, cell, tick);
            self.branches = Some(plan);
        }
        self.released_until.retain(|_, until| tick < *until);
        self.reached.clear();
        let (left, top, width, columns) = (self.left, self.top, self.width, self.columns);
        let clear = |(i, tile): (usize, &mut Tile)| {
            let site = (top + i / columns) * width + left + i % columns;
            tile.segment = cell(site).segment;
            tile.route = None;
            tile.reply = None;
            tile.seek = None;
            tile.egress = None;
            tile.through = false;
            tile.gate = Gate::Stock;
            tile.reserved = false;
            tile.lane_owner = None;
        };
        self.tiles.iter_mut().enumerate().for_each(clear);
        let mut recruiting = Vec::new();
        let mut row = None;
        let mut open_in_row = 0;
        for i in 0..self.boxes.len() {
            self.boxes[i].guarded = false;
            self.boxes[i].corridor = self.branches.as_ref().and_then(|plan| plan.passage(i));
            let box_tiles = &self.boxes[i];
            if !box_tiles.anchor.complete() || box_tiles.traffic.in_progress() {
                let corridor = box_tiles.corridor;
                if corridor.is_none() && self.branches.as_ref().is_some_and(|plan| !plan.ready(i)) {
                    continue;
                }
                if let Some(axis) = corridor {
                    for (local, site) in box_tiles.tiles.into_iter().enumerate() {
                        self.tiles[site].through = axis.lane(local) == SIDE / 2;
                    }
                }
                if self.behavior == WormFormationBehavior::FillEmpty {
                    // Recruit to a few boxes in each row at once. Holding every
                    // nearby worm in a different half-empty box can exhaust the arena.
                    if row != Some(box_tiles.top) {
                        row = Some(box_tiles.top);
                        open_in_row = 0;
                    }
                    if open_in_row == EMPTY_BOXES_PER_ROW {
                        continue;
                    }
                    open_in_row += 1;
                }
                self.boxes[i].guarded = branch_fill || self.boxes[i].traffic.in_progress();
                let lanes = WormBoxAxis::ALL.map(|axis| {
                    if corridor.is_some_and(|passage| passage != axis)
                        || self.branches.as_ref().is_some_and(|plan| {
                            let ports = plan.approaches[i];
                            !ports[axis.forward()] && !ports[axis.forward() ^ 1]
                        })
                    {
                        [Lane::Blocked; SIDE]
                    } else {
                        let mut lanes = self.read_lanes(i, axis);
                        if corridor.is_some() {
                            lanes[SIDE / 2] = Lane::Blocked;
                        }
                        self.boxes[i].traffic.reserve_lane(axis, &mut lanes);
                        lanes
                    }
                });
                self.boxes[i].anchor.observe(lanes);
                recruiting.push(i);
            }
            self.park(i);
        }
        if let Some(prior) = prior_docked {
            self.update_search_range(prior, tick);
        }
        if parallel_arrivals {
            recruiting.retain(|&index| self.clear_blocked_bay(index));
        }
        if traffic_enabled {
            self.advance_handoffs(tick);
        }
        for i in 0..self.boxes.len() {
            self.continue_assignment(i, tick);
            if parallel_arrivals {
                self.continue_second_assignment(i, tick);
            }
        }
        for &i in &recruiting {
            self.broadcast(i);
        }
        let endangered = self
            .branches
            .as_ref()
            .map_or_else(Vec::new, |plan| plan.endangered_sites(self));
        if !endangered.is_empty() {
            self.relay_egress(&endangered);
        }
        self.evacuating = endangered.is_empty()
            && self
                .branches
                .as_ref()
                .is_some_and(|plan| plan.supply_waiting)
            && self.boxes.iter().all(|target| !target.has_assignment());
        if self.evacuating || (parallel_arrivals && self.route_hops >= GUIDE_AFTER_HOPS) {
            let origins = if self.evacuating {
                self.branches.as_ref().unwrap().staging_cells.clone()
            } else {
                self.reached.clone()
            };
            self.relay_seek(origins);
        }
        self.relay_routes();
        self.relay_offers();
        for i in recruiting {
            let anchor = self.boxes[i].anchor;
            if !anchor.complete() {
                if parallel_arrivals && anchor.axis.is_some() {
                    self.recruit_two(i, tick);
                } else if anchor.grant.is_none() {
                    self.recruit(i, tick);
                }
            }
        }
        self.begin_handoffs(requests, tick);
    }

    fn docked_worms(&self) -> usize {
        self.boxes
            .iter()
            .flat_map(|box_tiles| box_tiles.anchor.filled())
            .filter(|&lane| lane)
            .count()
    }

    fn update_search_range(&mut self, prior_docked: usize, tick: u32) {
        let docked = self.docked_worms();
        if docked == self.boxes.len() * SIDE {
            self.route_hops = ROUTE_HOPS;
            self.widen_at = None;
        } else if docked > prior_docked || self.widen_at.is_none() {
            self.widen_at = Some(tick.saturating_add(WIDEN_AFTER));
        } else if self.widen_at.is_some_and(|deadline| tick >= deadline) {
            let limit = self.branches.as_ref().map_or(MAX_PATTERN_HOPS, |_| {
                (2 * self.width).min(usize::from(u16::MAX - 16)) as u16
            });
            self.route_hops = (self.route_hops + 8).min(limit);
            self.widen_at = Some(tick.saturating_add(WIDEN_AFTER));
        }
    }

    fn read_lanes(&self, box_index: usize, axis: WormBoxAxis) -> [Lane; SIDE] {
        let tiles = &self.boxes[box_index].tiles;
        let mut proofs = [Proof::BLOCKED; SQUARE];
        for (i, &index) in tiles.iter().enumerate().rev() {
            let tile = &self.tiles[index];
            if axis.offset(i) == SIDE - 1 {
                proofs[i] = Proof::start(tile.segment);
            } else if let Some(next) = tile.links[axis.forward()]
                && let Some(peer) = self.tiles[next].footprint
                && peer.box_index == box_index
            {
                proofs[i] = proofs[peer.local].prepend(tile.segment, self.tiles[next].segment);
            }
        }
        // The local proofs relay along the west or north edge to the corner.
        let cross = axis.forward() ^ 2;
        let mut packets = [[Lane::Blocked; SIDE]; SIDE];
        for lane in (0..SIDE).rev() {
            let i = if axis == WormBoxAxis::Horizontal {
                lane * SIDE
            } else {
                lane
            };
            let tile = &self.tiles[tiles[i]];
            if lane + 1 < SIDE && tile.links[cross].is_some() {
                packets[lane] = packets[lane + 1];
            }
            packets[lane][lane] = proofs[i].report(tile.segment);
        }
        packets[0]
    }

    fn park(&mut self, box_index: usize) {
        let box_tiles = &self.boxes[box_index];
        let Some(axis) = box_tiles.anchor.axis else {
            return;
        };
        for (local, index) in box_tiles.tiles.into_iter().enumerate() {
            let tile = &mut self.tiles[index];
            if matches!(box_tiles.anchor.lanes[axis as usize][axis.lane(local)], Lane::Filled(id) if id == worm_id(tile.segment))
            {
                tile.gate = Gate::Parked;
            }
        }
    }

    fn continue_assignment(&mut self, box_index: usize, tick: u32) {
        let Some(mut assignment) = self.boxes[box_index].assignment.take() else {
            return;
        };
        let grant = assignment.grant;
        let anchor = self.boxes[box_index].anchor;
        if anchor.grant == Some(grant)
            && assignment.advance(&self.tiles, tick)
            && self.issue(&assignment)
        {
            assignment.issued_at = tick;
            self.boxes[box_index].assignment = Some(assignment);
            return;
        }
        self.boxes[box_index].anchor.grant = None;
        if anchor.lane(grant.dock) != Lane::Filled(grant.worm) {
            self.released_until
                .insert(grant.worm, tick.saturating_add(RECRUIT_AGAIN_AFTER));
        }
    }

    fn continue_second_assignment(&mut self, box_index: usize, tick: u32) {
        let Some(mut assignment) = self.boxes[box_index].second_assignment.take() else {
            return;
        };
        let grant = assignment.grant;
        let anchor = self.boxes[box_index].anchor;
        let lane = anchor.lane(grant.dock);
        if anchor.axis == Some(grant.dock.axis)
            && (lane == Lane::Clear || lane == Lane::Partial(grant.worm))
            && assignment.advance(&self.tiles, tick)
            && self.issue(&assignment)
        {
            assignment.issued_at = tick;
            self.boxes[box_index].second_assignment = Some(assignment);
        } else if lane != Lane::Filled(grant.worm) {
            self.released_until
                .insert(grant.worm, tick.saturating_add(RECRUIT_AGAIN_AFTER));
        }
    }

    fn issue(&mut self, assignment: &Assignment) -> bool {
        let Some((head, next)) = assignment.head_and_next() else {
            return false;
        };
        let tile = &self.tiles[head];
        let Some(port) = tile.links.iter().position(|&link| link == Some(next)) else {
            return false;
        };
        let grant = assignment.grant;
        if tile.gate != Gate::Stock
            || role(tile.segment) != 1
            || worm_id(tile.segment) != grant.worm
            || self.tiles[next].segment != 0
            || self.tiles[next].is_reserved()
        {
            return false;
        }

        let dock = grant.dock;
        if self.tiles[next]
            .footprint
            .is_some_and(|tile| tile.box_index == dock.box_index)
        {
            // Reserve the whole straight lane only as the head crosses into it.
            // Empty boxes remain available for ordinary traffic.
            let lane = self.boxes[dock.box_index]
                .tiles
                .into_iter()
                .enumerate()
                .filter(|&(local, _)| dock.axis.lane(local) == dock.lane)
                .map(|(_, index)| index);
            if lane.clone().any(|index| {
                let tile = &self.tiles[index];
                tile.is_reserved() || (tile.segment != 0 && worm_id(tile.segment) != grant.worm)
            }) {
                return false;
            }
            for index in lane {
                self.tiles[index].lane_owner = Some(grant.worm);
            }
        }

        self.tiles[next].reserved = true;
        self.steer_body(head, grant.worm, Gate::Move(port));
        true
    }

    fn steer_body(&mut self, head: usize, worm: u32, gate: Gate) {
        self.tiles[head].gate = gate;
        let mut index = head;
        for next_role in 2..=SIDE {
            let Some(next) = self.tiles[index].links.into_iter().flatten().find(|&next| {
                let segment = self.tiles[next].segment;
                worm_id(segment) == worm && role(segment) == next_role
            }) else {
                break;
            };
            self.tiles[next].gate = Gate::Wait;
            index = next;
        }
    }

    fn broadcast(&mut self, box_index: usize) {
        let box_tiles = &self.boxes[box_index];
        let tiles = box_tiles.tiles;
        let anchor = box_tiles.anchor;
        let second_lane = box_tiles
            .second_assignment
            .as_ref()
            .map(|assignment| assignment.grant.dock.lane);
        let two_lanes = (self.branches.is_some() || self.retargeted) && anchor.axis.is_some();
        if anchor.complete()
            || (anchor.grant.is_some() && (!two_lanes || box_tiles.second_assignment.is_some()))
        {
            return;
        }
        let mut packets = [false; SQUARE];
        for (i, index) in tiles.into_iter().enumerate() {
            let links = self.tiles[index].links;
            let received = parent_port(i).is_none_or(|port| {
                links[port]
                    .and_then(|parent| self.tiles[parent].footprint)
                    .is_some_and(|parent| parent.box_index == box_index && packets[parent.local])
            });
            packets[i] = received;
            if !received {
                continue;
            }
            for axis in WormBoxAxis::ALL {
                if anchor.axis.is_some_and(|locked| locked != axis) {
                    continue;
                }
                let lane = axis.lane(i);
                if anchor.lanes[axis as usize][lane] != Lane::Clear {
                    continue;
                }
                let offset = axis.offset(i);
                if offset != 0 && offset != SIDE - 1 {
                    continue;
                }
                let dock = Dock {
                    box_index,
                    axis,
                    lane,
                    increasing: offset == 0,
                };
                if anchor.grant.is_some_and(|grant| grant.dock.lane == lane)
                    || second_lane == Some(lane)
                {
                    continue;
                }
                let port = dock.port();
                if self
                    .branches
                    .as_ref()
                    .is_some_and(|plan| !plan.approaches[box_index][port ^ 1])
                {
                    continue;
                }
                if let Some(entry) = links[port ^ 1]
                    && !self.tiles[entry].is_reserved()
                    && role(self.tiles[entry].segment) <= 1
                {
                    self.publish(
                        entry,
                        Route {
                            dock,
                            port,
                            distance: 0,
                            bends: 0,
                        },
                    );
                }
            }
        }
    }

    fn publish(&mut self, index: usize, route: Route) {
        let tile = &mut self.tiles[index];
        if tile.route.is_none() {
            self.reached.push(index);
        }
        if tile.route.is_none_or(|current| route.key() < current.key()) {
            tile.route = Some(route);
        }
    }

    fn relay_routes(&mut self) {
        // All competing packets for a distance arrive before the next hop.
        let mut start = 0;
        for distance in 1..=self.route_hops {
            let end = self.reached.len();
            if start == end {
                break;
            }
            for i in start..end {
                let index = self.reached[i];
                let tile = &self.tiles[index];
                if tile.segment != 0 {
                    continue;
                }
                let route = tile.route.expect("a reached tile has a route");
                for (outgoing, next) in tile.links.into_iter().enumerate() {
                    let Some(next) = next else { continue };
                    let tile = &self.tiles[next];
                    if tile.is_reserved()
                        || role(tile.segment) > 1
                        || tile.footprint.is_some_and(|cell| {
                            cell.box_index == route.dock.box_index
                                || (self.boxes[cell.box_index].guarded && !tile.through)
                        })
                    {
                        continue;
                    }
                    let port = outgoing ^ 1;
                    let bends = route.bends + u16::from(port != route.port);
                    self.publish(
                        next,
                        Route {
                            distance,
                            port,
                            bends,
                            ..route
                        },
                    );
                }
            }
            start = end;
        }
    }

    fn relay_seek(&mut self, mut reached: Vec<usize>) {
        // Navigation cues can cross a mobile body. Move grants still check
        // the actual next site and can only advance through a free opening.
        for &entrance in &reached {
            self.tiles[entrance].seek = Some(0);
        }
        let mut next = 0;
        while next < reached.len() {
            let index = reached[next];
            next += 1;
            let distance = self.tiles[index].seek.unwrap().saturating_add(1);
            for target in self.tiles[index].links.into_iter().flatten() {
                let tile = &mut self.tiles[target];
                if tile.seek.is_none()
                    && tile.gate != Gate::Parked
                    && !tile
                        .footprint
                        .is_some_and(|cell| self.boxes[cell.box_index].guarded && !tile.through)
                {
                    tile.seek = Some(distance);
                    reached.push(target);
                }
            }
        }
    }

    fn relay_egress(&mut self, endangered: &[bool]) {
        let passable = |tile: &Tile| {
            tile.gate != Gate::Parked
                && !tile
                    .footprint
                    .is_some_and(|cell| self.boxes[cell.box_index].guarded && !tile.through)
        };
        let mut reached = Vec::new();
        for (index, tile) in self.tiles.iter_mut().enumerate() {
            if !endangered[index]
                && passable(tile)
                && tile
                    .links
                    .into_iter()
                    .flatten()
                    .any(|next| endangered[next])
            {
                tile.egress = Some(0);
                reached.push(index);
            }
        }
        let mut next = 0;
        while next < reached.len() {
            let index = reached[next];
            next += 1;
            let distance = self.tiles[index].egress.unwrap().saturating_add(1);
            for target in self.tiles[index].links.into_iter().flatten() {
                let tile = &mut self.tiles[target];
                if endangered[target] && tile.egress.is_none() && passable(tile) {
                    tile.egress = Some(distance);
                    reached.push(target);
                }
            }
        }
    }

    fn clear_blocked_bay(&mut self, index: usize) -> bool {
        let target = &mut self.boxes[index];
        if target.anchor.complete()
            || target.has_assignment()
            || target.corridor.is_some()
            || target.traffic.in_progress()
        {
            return true;
        }

        let has_lane = WormBoxAxis::ALL.into_iter().any(|axis| {
            target.anchor.axis.is_none_or(|active| active == axis)
                && target.anchor.lanes[axis as usize].contains(&Lane::Clear)
        });
        let occupied = target
            .tiles
            .iter()
            .any(|&tile| self.tiles[tile].segment != 0);
        if !occupied || (has_lane && !target.flushing) {
            target.blocked_rounds = 0;
            target.flushing = false;
            return true;
        }

        target.blocked_rounds = target.blocked_rounds.saturating_add(1);
        if target.blocked_rounds < CLEAR_LANE_AFTER_ROUNDS {
            return true;
        }

        // Give loose worms a chance to leave before releasing any docked lanes.
        // Keep the axis from the first worm while the incomplete box clears.
        if target.blocked_rounds >= RELEASE_PARTIAL_BOX_AFTER_ROUNDS {
            target.flushing = true;
            target.anchor.lanes = [[Lane::Blocked; SIDE]; 2];
            target.anchor.grant = None;
            for tile in target.tiles {
                self.tiles[tile].gate = Gate::Stock;
            }
        }
        let sites = target.tiles;
        let mut queue = Vec::new();
        let mut depths = [u16::MAX; SQUARE];
        for (local, site) in sites.into_iter().enumerate() {
            if self.tiles[site].gate == Gate::Parked {
                continue;
            }
            for next in self.tiles[site].links.into_iter().flatten() {
                let outside = &self.tiles[next];
                if outside.gate != Gate::Parked
                    && outside.footprint.is_none_or(|peer| {
                        peer.box_index != index
                            && (!self.boxes[peer.box_index].guarded || outside.through)
                    })
                {
                    depths[local] = 1;
                    self.tiles[next].egress = Some(0);
                }
            }
            if depths[local] == 1 {
                queue.push(local);
            }
        }
        let mut head = 0;
        while head < queue.len() {
            let local = queue[head];
            head += 1;
            let site = sites[local];
            self.tiles[site].egress = Some(depths[local]);
            for next in self.tiles[site].links.into_iter().flatten() {
                let tile = &self.tiles[next];
                if let Some(peer) = tile.footprint
                    && peer.box_index == index
                    && tile.gate != Gate::Parked
                    && depths[peer.local] == u16::MAX
                {
                    depths[peer.local] = depths[local] + 1;
                    queue.push(peer.local);
                }
            }
        }
        false
    }

    fn relay_offers(&mut self) {
        for &index in self.reached.iter().rev() {
            let tile = &mut self.tiles[index];
            let route = tile.route.expect("a reached tile has a route");
            let worm = worm_id(tile.segment);
            if role(tile.segment) == 1
                && tile.gate == Gate::Stock
                && tile.egress.unwrap_or(0) == 0
                && !self.released_until.contains_key(&worm)
            {
                let heading = (tile.segment >> FORMATION_HEADING_SHIFT) as usize & 3;
                tile.offer(Offer {
                    distance: route.distance,
                    bends: route.bends + u16::from(heading != route.port),
                    grant: Grant {
                        dock: route.dock,
                        worm,
                    },
                    head: index,
                });
            }
            if route.distance != 0
                && let Some(reply) = tile.reply
                && let Some(parent) = tile.links[route.port]
            {
                self.tiles[parent].offer(reply);
            }
        }
    }

    fn best_offer(&self, box_index: usize) -> Option<Offer> {
        // Inside the box, bids follow its own contact tree to the corner.
        // A tile can also relay an unrelated public call through an idle lane.
        let mut receipts = [None; SQUARE];
        for (local, index) in self.boxes[box_index].tiles.into_iter().enumerate().rev() {
            receipts[local] = self.tiles[index]
                .links
                .iter()
                .enumerate()
                .filter_map(|(port, peer)| {
                    let tile = &self.tiles[(*peer)?];
                    if let Some(peer) = tile.footprint
                        && peer.box_index == box_index
                        && parent_port(peer.local) == Some(port ^ 1)
                    {
                        return receipts[peer.local];
                    }
                    let route = tile.route?;
                    (route.dock.box_index == box_index
                        && route.distance == 0
                        && route.port == port ^ 1)
                        .then_some(tile.reply)
                        .flatten()
                })
                .min();
        }
        receipts[0]
    }

    fn recruit(&mut self, box_index: usize, tick: u32) {
        let Some(offer) = self.best_offer(box_index) else {
            return;
        };
        let Some(assignment) = self.assignment_for(offer, tick) else {
            return;
        };
        if self.issue(&assignment) {
            self.boxes[box_index].anchor.grant = Some(assignment.grant);
            self.boxes[box_index].assignment = Some(assignment);
        }
    }

    fn recruit_two(&mut self, box_index: usize, tick: u32) {
        let target = &self.boxes[box_index];
        if target.assignment.is_some() && target.second_assignment.is_some() {
            return;
        }
        let mut offers: [Option<Offer>; SIDE] = [None; SIDE];
        for index in target.tiles {
            for (port, peer) in self.tiles[index].links.into_iter().enumerate() {
                let Some(peer) = peer else {
                    continue;
                };
                let tile = &self.tiles[peer];
                let (Some(route), Some(offer)) = (tile.route, tile.reply) else {
                    continue;
                };
                if route.dock.box_index == box_index
                    && route.distance == 0
                    && route.port == port ^ 1
                {
                    let lane = route.dock.lane;
                    if offers[lane].is_none_or(|current| offer < current) {
                        offers[lane] = Some(offer);
                    }
                }
            }
        }
        let mut offers: Vec<_> = offers.into_iter().flatten().collect();
        offers.sort_unstable();
        for offer in offers {
            let target = &self.boxes[box_index];
            let first_free = target.assignment.is_none();
            if !first_free && target.second_assignment.is_some() {
                break;
            }
            let Some(assignment) = self.assignment_for(offer, tick) else {
                continue;
            };
            let neighborhood = &assignment.path[..assignment.path.len().min(SIDE)];
            let interferes = [&target.assignment, &target.second_assignment]
                .into_iter()
                .flatten()
                .any(|current| {
                    current.path[current.step..]
                        .iter()
                        .take(SIDE)
                        .any(|site| neighborhood.contains(site))
                });
            if interferes || !self.issue(&assignment) {
                continue;
            }
            if first_free {
                self.boxes[box_index].anchor.grant = Some(assignment.grant);
                self.boxes[box_index].assignment = Some(assignment);
            } else {
                self.boxes[box_index].second_assignment = Some(assignment);
            }
        }
    }

    fn assignment_for(&self, offer: Offer, tick: u32) -> Option<Assignment> {
        let mut index = offer.head;
        let mut path = vec![index];
        loop {
            let route = self.tiles[index]
                .route
                .expect("a recruited head follows its returned call");
            let next = self.tiles[index].links[route.port]?;
            if route.distance == 0 {
                // Append the five cells of the lane in arrival order.
                index = next;
                path.push(index);
                for _ in 1..SIDE {
                    let next = self.tiles[index].links[route.port]?;
                    index = next;
                    path.push(index);
                }
                break;
            }
            index = next;
            path.push(index);
        }

        Some(Assignment {
            grant: offer.grant,
            path,
            step: 0,
            issued_at: tick,
        })
    }

    fn reservation_path(&self, box_index: usize) -> Vec<u32> {
        self.boxes[box_index]
            .assignment
            .as_ref()
            .map_or_else(Vec::new, |assignment| {
                assignment.path[assignment.step..]
                    .iter()
                    .map(|&index| self.site(index) as u32)
                    .collect()
            })
    }
}

fn parent_port(tile: usize) -> Option<usize> {
    if !tile.is_multiple_of(SIDE) {
        Some(0)
    } else if tile >= SIDE {
        Some(2)
    } else {
        None
    }
}

fn worm_id(segment: u32) -> u32 {
    segment & FORMATION_ID_MASK
}

fn role(segment: u32) -> usize {
    (segment & ROLE_MASK as u32) as usize
}

#[derive(Clone)]
pub(super) struct WormBox {
    width: usize,
    offset: usize,
    rule: u8,
    behavior: WormFormationBehavior,
    field: Option<FormationTiles>,
    // Cues received during a round are applied in order at the next commit.
    cues: Vec<WormFormationCue>,
}

#[derive(Debug, Serialize)]
pub struct WormBoxStatus {
    pub active: bool,
    pub left: usize,
    pub top: usize,
    pub axis: Option<WormBoxAxis>,
    pub lanes: [bool; SIDE],
}

#[derive(Debug, Serialize)]
pub struct WormPatternStatus<'a> {
    pub left: usize,
    pub top: usize,
    pub columns: usize,
    pub max_rows: usize,
    pub rule: u8,
    pub behavior: WormFormationBehavior,
    pub restricted: bool,
    pub cells_inside: usize,
    pub rows: &'a [Vec<bool>],
    pub boxes: Vec<WormBoxStatus>,
}

#[derive(Debug, Serialize)]
pub struct WormPatternOverview {
    pub left: usize,
    pub top: usize,
    pub columns: usize,
    pub max_rows: usize,
    pub row_count: usize,
    pub rule: u8,
    pub behavior: WormFormationBehavior,
    pub restricted: bool,
    pub cells_inside: usize,
    pub target_count: usize,
    pub filled_count: usize,
}

impl WormBox {
    pub(super) fn new(width: usize) -> Self {
        Self {
            width,
            offset: (width - SIDE) / 2,
            rule: 90,
            behavior: WormFormationBehavior::Fill,
            field: None,
            cues: Vec::new(),
        }
    }

    fn requested_layout(&self) -> Option<Layout> {
        let mut layout = self.field.as_ref().map(|field| {
            field
                .pattern
                .as_ref()
                .map_or(Layout::Box, |pattern| Layout::Pattern(pattern.rows.len()))
        });
        let mut rule = self.rule;
        let mut behavior = self.behavior;
        for &cue in &self.cues {
            match cue {
                WormFormationCue::Box => layout = Some(Layout::Box),
                WormFormationCue::Start { rule: next } => {
                    rule = next.unwrap_or(rule);
                    let rows = if rule == RULE_90 && behavior == WormFormationBehavior::Fill {
                        Pattern::max_rows(self.width)
                    } else {
                        1
                    };
                    layout = Some(Layout::Pattern(rows));
                }
                WormFormationCue::Retarget { rule: next } => {
                    rule = next;
                    layout = Some(Layout::Pattern(Pattern::max_rows(self.width)));
                }
                WormFormationCue::Seed { rule: next } => {
                    rule = next.unwrap_or(rule);
                    layout = Some(Layout::Pattern(1));
                }
                WormFormationCue::Step { rule: next } => {
                    rule = next.unwrap_or(rule);
                    if let Some(Layout::Pattern(rows)) = layout {
                        layout = Some(Layout::Pattern(rows + 1));
                    }
                }
                WormFormationCue::Clear => layout = None,
                WormFormationCue::Rule(next) => rule = next,
                WormFormationCue::RestrictAccess(true) => {
                    behavior = WormFormationBehavior::Restrict
                }
                WormFormationCue::RestrictAccess(false) => behavior = WormFormationBehavior::Fill,
                WormFormationCue::Behavior(next) => behavior = next,
            }
        }
        layout
    }

    pub(super) fn active(&self) -> bool {
        self.field.is_some()
    }

    pub(super) fn held_at(&self, site: usize) -> bool {
        self.field.as_ref().is_some_and(|field| {
            field
                .index(site)
                .is_some_and(|index| field.tiles[index].gate == Gate::Parked)
        })
    }

    pub(super) fn movement_inbox(
        &self,
        site: usize,
        segment: u32,
        mut inbox: [u64; PORTS],
    ) -> [u64; PORTS] {
        if !matches!(role(segment), 1 | SIDE) {
            return inbox;
        }
        let Some(field) = &self.field else {
            return inbox;
        };
        let Some(index) = field.index(site) else {
            return inbox;
        };
        let tile = &field.tiles[index];
        let guarded_dock = |tile: &Tile| {
            tile.footprint
                .filter(|cell| field.boxes[cell.box_index].guarded && !tile.through)
                .map(|cell| cell.box_index)
        };
        let dock = guarded_dock(tile);
        let unreserved =
            |tile: &Tile| !tile.reserved && tile.lane_owner.is_none_or(|id| id == worm_id(segment));
        let dock_exit = dock.is_some()
            && tile.links.iter().zip(inbox).any(|(&next, message)| {
                message & ROLE_MASK == OPEN
                    && next.is_some_and(|next| {
                        unreserved(&field.tiles[next]) && guarded_dock(&field.tiles[next]).is_none()
                    })
            });
        let stock_open = |target: &Tile| {
            let other_dock = guarded_dock(target);
            let enters_closing_region =
                tile.egress == Some(0) && target.egress.is_some_and(|distance| distance > 0);
            unreserved(target)
                && !enters_closing_region
                && (other_dock.is_none() || (other_dock == dock && !dock_exit))
        };
        let needs_exit = tile.egress.is_some_and(|distance| distance > 0);
        let downhill = |target: &Tile| {
            if needs_exit {
                return tile
                    .egress
                    .zip(target.egress)
                    .is_some_and(|(here, there)| there < here);
            }
            tile.seek.zip(target.seek).is_some_and(|(here, there)| {
                there < here || (field.evacuating && here == 0 && there == 0)
            })
        };
        let can_follow_signal = (needs_exit || tile.seek.is_some())
            && tile.gate == Gate::Stock
            && tile.links.iter().zip(inbox).any(|(&next, message)| {
                message & ROLE_MASK == OPEN
                    && next.is_some_and(|next| {
                        let target = &field.tiles[next];
                        stock_open(target) && downhill(target)
                    })
            });
        let inside = tile.footprint.is_some();
        let restricted = field.behavior == WormFormationBehavior::Restrict;
        // Residents take a free exit when one is adjacent. Interior room remains
        // available while they maneuver, and worms outside cannot move back in.
        let has_exit = restricted
            && inside
            && tile.links.iter().zip(inbox).any(|(&next, message)| {
                message & ROLE_MASK == OPEN
                    && next.is_some_and(|next| field.tiles[next].footprint.is_none())
            });
        for (port, message) in inbox.iter_mut().enumerate() {
            if *message & ROLE_MASK != OPEN {
                continue;
            }
            let Some(next) = tile.links[port] else {
                continue;
            };
            let target = &field.tiles[next];
            let allowed = if restricted {
                target.footprint.is_none() || (inside && !has_exit)
            } else {
                match tile.gate {
                    Gate::Parked | Gate::Wait => false,
                    Gate::Move(direction) => port == direction,
                    Gate::Stock => stock_open(target) && (!can_follow_signal || downhill(target)),
                }
            };
            if !allowed {
                // Keep the edge, food, and scent word intact apart from the move gate.
                *message = (*message & !ROLE_MASK) | OCCUPIED;
            }
        }
        inbox
    }

    pub(super) fn refresh(
        &mut self,
        tick: u32,
        population: usize,
        cell: impl Fn(usize) -> FormationCell + Copy + Sync,
        neighbors: impl Fn(usize) -> [Option<usize>; PORTS] + Copy,
    ) {
        for cue in std::mem::take(&mut self.cues) {
            match cue {
                WormFormationCue::Rule(rule) => self.rule = rule,
                WormFormationCue::RestrictAccess(true) => {
                    self.behavior = WormFormationBehavior::Restrict;
                }
                WormFormationCue::RestrictAccess(false) => {
                    self.behavior = WormFormationBehavior::Fill;
                }
                WormFormationCue::Behavior(behavior) => self.behavior = behavior,
                WormFormationCue::Retarget { rule } => {
                    self.rule = rule;
                    self.field = None;
                    let mut pattern = Pattern::new(self.width);
                    for _ in 1..pattern.max_rows {
                        pattern.step(rule);
                    }
                    // Replace the targets at this commit. The next refresh proves
                    // which existing bodies fit the new pattern from live cells.
                    let mut field =
                        FormationTiles::new(self.width, Some(pattern), self.behavior, neighbors);
                    field.retargeted = true;
                    self.field = Some(field);
                }
                WormFormationCue::Seed { rule } | WormFormationCue::Start { rule } => {
                    self.rule = rule.unwrap_or(self.rule);
                    self.field = None;
                    let mut pattern = Pattern::new(self.width);
                    let start = matches!(cue, WormFormationCue::Start { .. });
                    let branch_fill = start
                        && self.rule == RULE_90
                        && self.behavior == WormFormationBehavior::Fill;
                    if start && !branch_fill {
                        pattern.schedule_next(tick);
                    }
                    let mut field =
                        FormationTiles::new(self.width, Some(pattern), self.behavior, neighbors);
                    if branch_fill {
                        field.plan_rule_90(population);
                    }
                    self.field = Some(field);
                }
                WormFormationCue::Step { rule } => {
                    self.rule = rule.unwrap_or(self.rule);
                    let field = self.field.as_mut().expect("the pattern was seeded");
                    field.step_pattern(self.rule);
                    if let Some(pattern) = &mut field.pattern
                        && pattern.next_row.is_some()
                    {
                        pattern.schedule_next(tick);
                    }
                }
                WormFormationCue::Box => {
                    if self
                        .field
                        .as_ref()
                        .is_none_or(|field| field.pattern.is_some())
                    {
                        self.field = Some(FormationTiles::new(
                            self.width,
                            None,
                            WormFormationBehavior::Fill,
                            neighbors,
                        ));
                    }
                }
                WormFormationCue::Clear => self.field = None,
            }
        }
        if let Some(field) = &mut self.field {
            field.set_behavior(self.behavior);
            if field
                .pattern
                .as_ref()
                .and_then(|pattern| pattern.next_row)
                .is_some_and(|due| tick >= due)
            {
                field.step_pattern(self.rule);
                field.pattern.as_mut().unwrap().schedule_next(tick);
            }
            field.refresh(cell, tick);
        }
    }

    pub(super) fn cue(
        &mut self,
        cue: WormFormationCue,
        behavior: Option<WormFormationBehavior>,
    ) -> Result<()> {
        if let WormFormationCue::Step { .. } = cue {
            let Some(Layout::Pattern(rows)) = self.requested_layout() else {
                anyhow::bail!("Seed the pattern before stepping a generation");
            };
            ensure!(
                rows < Pattern::max_rows(self.width),
                "The pattern has reached its last row"
            );
        }
        self.cues.extend(behavior.map(WormFormationCue::Behavior));
        self.cues.push(cue);
        Ok(())
    }

    pub(super) fn box_status(&self) -> WormBoxStatus {
        let mut status = self
            .field
            .as_ref()
            .filter(|field| field.pattern.is_none())
            .map_or(
                WormBoxStatus {
                    active: false,
                    left: self.offset,
                    top: self.offset,
                    axis: None,
                    lanes: [false; SIDE],
                },
                |field| field.boxes[0].status(),
            );
        status.active = self.requested_layout() == Some(Layout::Box);
        status
    }

    pub(super) fn overview(&self) -> Option<WormPatternOverview> {
        let field = self.field.as_ref()?;
        let pattern = field.pattern.as_ref()?;
        Some(WormPatternOverview {
            left: pattern.left,
            top: pattern.top,
            columns: pattern.columns,
            max_rows: pattern.max_rows,
            row_count: pattern.rows.len(),
            rule: field.branch_population.map_or(self.rule, |_| RULE_90),
            behavior: field.behavior,
            restricted: field.behavior == WormFormationBehavior::Restrict,
            cells_inside: field.cells_inside,
            target_count: field.boxes.len(),
            filled_count: field
                .boxes
                .iter()
                .filter(|target| target.anchor.complete())
                .count(),
        })
    }

    pub(super) fn vacancy_signals(&self, max_edges: usize) -> impl Iterator<Item = [u32; 2]> + '_ {
        self.field.iter().flat_map(move |field| {
            let stride = field.reached.len().div_ceil(max_edges).max(1);
            field.reached.iter().step_by(stride).filter_map(|&index| {
                let tile = &field.tiles[index];
                let from = tile.links[tile.route?.port]?;
                Some([field.site(from) as u32, field.site(index) as u32])
            })
        })
    }

    pub(super) fn reservation_paths(&self, max_words: usize) -> Vec<u32> {
        let mut paths = Vec::new();
        if let Some(field) = &self.field {
            for target in &field.boxes {
                let arrivals = [&target.assignment, &target.second_assignment]
                    .into_iter()
                    .flatten()
                    .map(|assignment| &assignment.path[assignment.step..]);
                for path in arrivals.chain(target.traffic.route()) {
                    if path.len() >= max_words - paths.len() {
                        return paths;
                    }
                    paths.push(path.len() as u32);
                    paths.extend(path.iter().map(|&index| field.site(index) as u32));
                }
            }
        }
        paths
    }

    pub(super) fn refresh_browser(&mut self, arena: &FormationArena) {
        self.refresh(
            arena.tick(),
            arena.bodies.len(),
            |site| (&arena.cells()[site]).into(),
            |site| site_neighbors(arena.engine.sites(), site),
        );
    }
}

fn site_neighbors(sites: &[Site], site: usize) -> [Option<usize>; PORTS] {
    sites[site]
        .links
        .map(|link| link.is_open().then_some(link.site as usize))
}

impl FormationArena {
    fn check_formation(&self) -> Result<()> {
        ensure!(
            self.size >= SIDE + 2,
            "Worm formations need a grid at least 7 × 7"
        );
        Ok(())
    }

    pub fn set_worm_box(&mut self, active: bool) -> Result<()> {
        if active {
            return self.cue_worm_formation(WormFormationCue::Box);
        }
        self.check_formation()?;
        let guide = self.worm_box.get_or_insert_with(|| WormBox::new(self.size));
        if guide.requested_layout() == Some(Layout::Box) {
            guide.cues.clear();
        }
        if guide
            .field
            .as_ref()
            .is_some_and(|field| field.pattern.is_none())
        {
            guide.field = None;
        }
        Ok(())
    }

    pub fn cue_worm_formation(&mut self, cue: WormFormationCue) -> Result<()> {
        self.cue_with_behavior(cue, None)
    }

    pub fn cue_with_behavior(
        &mut self,
        cue: WormFormationCue,
        behavior: Option<WormFormationBehavior>,
    ) -> Result<()> {
        self.check_formation()?;
        let mut guide = self
            .worm_box
            .take()
            .unwrap_or_else(|| WormBox::new(self.size));
        let result = guide.cue(cue, behavior);
        if result.is_ok() && self.tick().is_multiple_of(ROUND_TICKS) {
            guide.refresh_browser(self);
        }
        self.worm_box = Some(guide);
        result
    }

    pub fn worm_box_status(&self) -> Option<WormBoxStatus> {
        self.worm_box.as_ref().map(WormBox::box_status)
    }

    pub fn worm_pattern_status(&self) -> Option<WormPatternStatus<'_>> {
        let guide = self.worm_box.as_ref()?;
        let field = guide.field.as_ref()?;
        let pattern = field.pattern.as_ref()?;
        Some(WormPatternStatus {
            left: pattern.left,
            top: pattern.top,
            columns: pattern.columns,
            max_rows: pattern.max_rows,
            rule: field.branch_population.map_or(guide.rule, |_| RULE_90),
            behavior: field.behavior,
            restricted: field.behavior == WormFormationBehavior::Restrict,
            cells_inside: field.cells_inside,
            rows: &pattern.rows,
            boxes: field.boxes.iter().map(WormBoxTiles::status).collect(),
        })
    }

    pub fn worm_pattern_overview(&self) -> Option<WormPatternOverview> {
        self.worm_box.as_ref()?.overview()
    }

    /// The retained vacancy relays, from sender to receiver.
    pub fn worm_box_vacancy_signals(&self) -> impl Iterator<Item = [u32; 2]> + '_ {
        self.vacancy_signals(usize::MAX)
    }

    pub fn vacancy_signals(&self, max_edges: usize) -> impl Iterator<Item = [u32; 2]> + '_ {
        self.worm_box
            .iter()
            .flat_map(move |guide| guide.vacancy_signals(max_edges.max(1)))
    }

    /// First active path for callers that display a single box.
    pub fn worm_box_reservation_path(&self) -> Vec<u32> {
        self.worm_box
            .as_ref()
            .and_then(|guide| guide.field.as_ref())
            .map_or_else(Vec::new, |field| {
                (0..field.boxes.len())
                    .map(|index| field.reservation_path(index))
                    .find(|path| !path.is_empty())
                    .unwrap_or_default()
            })
    }

    /// Length-prefixed routes for approaching, passing, and replacing worms.
    pub fn worm_formation_reservation_paths(&self) -> Vec<u32> {
        self.reservation_paths(usize::MAX)
    }

    pub fn reservation_paths(&self, max_words: usize) -> Vec<u32> {
        self.worm_box
            .as_ref()
            .map_or_else(Vec::new, |guide| guide.reservation_paths(max_words))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formation_arena::{Body, ROOM_DISABLED, ROOM_WAVE, food, seed_room, seeded_world};
    use crate::world::cardinal_neighbors;

    fn lane_sites(guide: &WormBox, axis: WormBoxAxis, lane: usize) -> Body {
        let start = guide.offset * (guide.width + 1);
        match axis {
            WormBoxAxis::Horizontal => std::array::from_fn(|i| start + lane * guide.width + i),
            WormBoxAxis::Vertical => std::array::from_fn(|i| start + lane + i * guide.width),
        }
    }

    fn arriving_body(guide: &WormBox, dock: Dock) -> Body {
        let lane = lane_sites(guide, dock.axis, dock.lane);
        let mut site = lane[if dock.increasing { 0 } else { SIDE - 1 }];
        std::array::from_fn(|_| {
            site = cardinal_neighbors(site, guide.width, guide.width)[dock.port() ^ 1].unwrap();
            site
        })
    }

    fn fixture(guide: &WormBox, bodies: Vec<Body>) -> Result<FormationArena> {
        let mut world = seeded_world(guide.width, 29, &bodies, &[0])?;
        seed_room(&mut world, ROOM_DISABLED);
        Ok(FormationArena::new(world, guide.width, bodies))
    }

    #[test]
    fn the_first_completed_lane_sets_the_axis_from_any_side() -> Result<()> {
        let guide = WormBox::new(19);
        for axis in WormBoxAxis::ALL {
            for increasing in [false, true] {
                let dock = Dock {
                    box_index: 0,
                    axis,
                    lane: 2,
                    increasing,
                };
                let mut arena = fixture(&guide, vec![arriving_body(&guide, dock)])?;
                arena.set_worm_box(true)?;
                arena.advance(4 * ROUND_TICKS)?;
                assert_eq!(arena.worm_box_status().unwrap().axis, None);
                arena.advance(ROUND_TICKS)?;
                let status = arena.worm_box_status().unwrap();
                assert_eq!(status.axis, Some(axis));
                assert_eq!(status.lanes, [false, false, true, false, false]);
                let mut expected = lane_sites(&guide, axis, 2);
                if increasing {
                    expected.reverse();
                }
                assert_eq!(arena.bodies()[0], expected);
                arena.set_worm_box(false)?;
                assert_eq!(arena.worm_box_status().unwrap().axis, None);
            }
        }
        Ok(())
    }

    #[test]
    fn a_docked_worm_accepts_an_opposite_facing_neighbor_on_its_axis() -> Result<()> {
        let guide = WormBox::new(19);
        for axis in WormBoxAxis::ALL {
            let mut first = lane_sites(&guide, axis, 0);
            first.reverse();
            let dock = Dock {
                box_index: 0,
                axis,
                lane: 1,
                increasing: false,
            };
            let mut arena = fixture(&guide, vec![first, arriving_body(&guide, dock)])?;
            arena.set_worm_box(true)?;
            assert_eq!(arena.worm_box_status().unwrap().axis, Some(axis));
            arena.advance(8 * ROUND_TICKS)?;
            let status = arena.worm_box_status().unwrap();
            assert_eq!(status.axis, Some(axis));
            assert_eq!(status.lanes, [true, true, false, false, false]);
            assert_eq!(arena.bodies(), &[first, lane_sites(&guide, axis, 1)]);
        }
        Ok(())
    }

    #[test]
    fn a_lane_proof_needs_links_and_one_worm_id() -> Result<()> {
        let guide = WormBox::new(19);
        let axis = WormBoxAxis::Horizontal;
        let lane = lane_sites(&guide, axis, 2);
        let mut arena = fixture(&guide, vec![lane])?;
        arena.set_worm_box(true)?;
        let field = arena.worm_box.as_mut().unwrap().field.as_mut().unwrap();
        assert!(matches!(field.read_lanes(0, axis)[2], Lane::Filled(_)));

        let index = field.index(lane[2]).unwrap();
        let original = field.tiles[index].segment;
        field.tiles[index].segment ^= 2 << 3;
        assert!(matches!(field.read_lanes(0, axis)[2], Lane::Blocked));
        field.tiles[index].segment = original;

        field.tiles[index].links[axis.forward()] = None;
        assert!(matches!(field.read_lanes(0, axis)[2], Lane::Blocked));
        Ok(())
    }

    #[test]
    fn simultaneous_offers_get_one_reservation_and_clear_between_commits() -> Result<()> {
        let guide = WormBox::new(19);
        let axis = WormBoxAxis::Horizontal;
        let west = arriving_body(
            &guide,
            Dock {
                box_index: 0,
                axis,
                lane: 2,
                increasing: true,
            },
        );
        let east = arriving_body(
            &guide,
            Dock {
                box_index: 0,
                axis,
                lane: 2,
                increasing: false,
            },
        );
        let mut arena = fixture(&guide, vec![west, east])?;
        arena.set_worm_box(true)?;
        let field = arena.worm_box.as_ref().unwrap().field.as_ref().unwrap();
        let moves = [west, east].map(|body| {
            matches!(
                field.tiles[field.index(body[0]).unwrap()].gate,
                Gate::Move(_)
            )
        });
        assert_ne!(moves[0], moves[1]);
        assert_eq!(field.tiles.iter().filter(|tile| tile.reserved).count(), 1);
        assert_eq!(arena.worm_box_status().unwrap().axis, None);

        let lane = lane_sites(&guide, axis, 2).map(|site| site as u32);
        let vacancy: Vec<_> = arena.worm_box_vacancy_signals().collect();
        assert!(vacancy.contains(&[lane[0], west[0] as u32]));
        assert!(vacancy.contains(&[lane[SIDE - 1], east[0] as u32]));

        let path = arena.worm_box_reservation_path();
        let (head, target) = if moves[0] {
            (west[0], lane[0])
        } else {
            (east[0], lane[SIDE - 1])
        };
        assert_eq!(path[..2], [head as u32, target]);
        assert_eq!(path.len(), 1 + SIDE);
        assert_eq!(
            path.iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            path.len()
        );
        assert!(path.windows(2).all(|hop| {
            cardinal_neighbors(hop[0] as usize, guide.width, guide.width)
                .contains(&Some(hop[1] as usize))
        }));

        arena.advance(3)?;
        arena.set_worm_box(false)?;
        assert_eq!(arena.tick(), 3);
        assert!(arena.worm_box.as_ref().unwrap().field.is_none());
        assert_eq!(arena.worm_box_status().unwrap().lanes, [false; SIDE]);
        assert_eq!(arena.worm_box_vacancy_signals().count(), 0);
        assert!(arena.worm_box_reservation_path().is_empty());
        Ok(())
    }

    #[test]
    fn inactive_guide_matches_stock_from_the_same_seed() -> Result<()> {
        let mut stock = FormationArena::generate(17, 17001, 12, 6, ROOM_WAVE)?;
        let mut box_arena = stock.clone();
        box_arena.set_worm_box(false)?;
        stock.advance(16 * ROUND_TICKS + 7)?;
        box_arena.advance(16 * ROUND_TICKS + 7)?;
        assert_eq!(box_arena.cells(), stock.cells());
        assert_eq!(box_arena.bodies(), stock.bodies());
        assert_eq!(box_arena.blocked_rounds, stock.blocked_rounds);
        Ok(())
    }

    #[test]
    fn rule_90_keeps_the_full_space_time_pattern_as_box_targets() -> Result<()> {
        let guide = WormBox::new(157);
        let mut arena = fixture(&guide, Vec::new())?;
        use WormFormationCue::{Rule, Seed, Step};

        assert!(arena.cue_worm_formation(Step { rule: None }).is_err());
        arena.advance(3)?;
        arena.cue_worm_formation(Seed { rule: None })?;
        arena.cue_worm_formation(Step { rule: None })?;
        assert!(arena.worm_pattern_status().is_none());
        arena.advance(ROUND_TICKS - 3)?;

        let state = arena.worm_pattern_status().unwrap();
        assert_eq!(
            (state.columns, state.max_rows, state.rows.len()),
            (31, 31, 2)
        );
        assert_eq!(state.boxes.len(), 3);
        for _ in 2..16 {
            arena.cue_worm_formation(Step { rule: None })?;
        }
        let state = arena.worm_pattern_status().unwrap();
        assert_eq!(state.rows.len(), 16);
        for (generation, row) in state.rows.iter().enumerate() {
            let expected: Vec<_> = (0..31)
                .map(|column| {
                    // Iterate Pascal's coefficients modulo 2 in the cone of the seed.
                    let index = column + generation;
                    if !(15..=15 + 2 * generation).contains(&index)
                        || !(index - 15).is_multiple_of(2)
                    {
                        return false;
                    }
                    let k = (index - 15) / 2;
                    k & generation == k
                })
                .collect();
            assert_eq!(*row, expected);
        }
        let targets: Vec<_> = state
            .rows
            .iter()
            .enumerate()
            .flat_map(|(row, bits)| {
                bits.iter()
                    .enumerate()
                    .filter(|(_, bit)| **bit)
                    .map(move |(column, _)| [state.left + column * SIDE, state.top + row * SIDE])
            })
            .collect();
        assert_eq!(state.boxes.len(), 81);
        assert_eq!(
            state
                .boxes
                .iter()
                .map(|b| [b.left, b.top])
                .collect::<Vec<_>>(),
            targets,
        );
        assert!(state.boxes.iter().all(|b| b.lanes == [false; SIDE]));
        assert!(arena.worm_box_reservation_path().is_empty());
        for _ in 16..31 {
            arena.cue_worm_formation(Step { rule: None })?;
        }
        assert!(arena.cue_worm_formation(Step { rule: None }).is_err());
        assert_eq!(arena.tick(), ROUND_TICKS);
        assert_eq!(
            arena
                .worm_box
                .as_ref()
                .unwrap()
                .field
                .as_ref()
                .unwrap()
                .route_hops,
            ROUTE_HOPS
        );
        arena.advance(WIDEN_AFTER)?;
        assert_eq!(
            arena
                .worm_box
                .as_ref()
                .unwrap()
                .field
                .as_ref()
                .unwrap()
                .route_hops,
            ROUTE_HOPS + 8
        );

        arena.cue_worm_formation(Rule(110))?;
        arena.cue_worm_formation(Seed { rule: None })?;
        arena.cue_worm_formation(Step { rule: None })?;
        let state = arena.worm_pattern_status().unwrap();
        assert_eq!(state.rule, 110);
        assert_eq!(state.rows[1][13..18], [false, true, true, false, false]);
        Ok(())
    }

    #[test]
    fn two_boxes_recruit_independently_while_the_seed_box_holds() -> Result<()> {
        let guide = WormBox::new(37);
        let size = guide.width;
        let pattern = Pattern::new(size);
        let [left, top] = pattern
            .boxes_in_row(0, WormFormationBehavior::Fill)
            .next()
            .unwrap();
        let parked: [Body; SIDE] = std::array::from_fn(|row| {
            std::array::from_fn(|segment| {
                let x = if row.is_multiple_of(2) {
                    segment
                } else {
                    SIDE - 1 - segment
                };
                (top + row) * size + left + x
            })
        });
        let west_y = top + SIDE + 2;
        let west: Body = std::array::from_fn(|i| west_y * size + left - SIDE - 1 - i);
        let south_x = left + SIDE + 2;
        let south: Body = std::array::from_fn(|i| (top + 2 * SIDE + i) * size + south_x);
        let mut arena = fixture(&guide, parked.into_iter().chain([west, south]).collect())?;
        use WormFormationCue::{Clear, Seed, Step};

        arena.cue_worm_formation(Seed { rule: Some(90) })?;
        assert!(
            arena.worm_pattern_status().unwrap().boxes[0]
                .lanes
                .into_iter()
                .all(|lane| lane)
        );
        arena.cue_worm_formation(Step { rule: None })?;
        let state = arena.worm_pattern_status().unwrap();
        assert_eq!(state.boxes.len(), 3);
        assert_eq!(state.boxes[0].lanes, [true; SIDE]);
        let field = arena.worm_box.as_ref().unwrap().field.as_ref().unwrap();
        let expected = [west[0] as u32 + 1, (south[0] - size) as u32];
        for (index, target) in (1..=2).zip(expected) {
            assert_eq!(field.reservation_path(index).get(1), Some(&target));
        }
        let paths = arena.worm_formation_reservation_paths();
        let boundary = 1 + paths[0] as usize;
        assert_eq!(paths[2], expected[0]);
        assert_eq!(paths[boundary + 2], expected[1]);

        arena.advance(5 * ROUND_TICKS)?;
        let state = arena.worm_pattern_status().unwrap();
        assert_eq!(state.boxes[1].axis, Some(WormBoxAxis::Horizontal));
        assert_eq!(state.boxes[2].axis, Some(WormBoxAxis::Vertical));
        assert!(state.boxes[1].lanes[2] && state.boxes[2].lanes[2]);
        assert_eq!(arena.bodies()[..SIDE], parked);
        assert_eq!(arena.bodies().len(), SIDE + 2);

        let tick = arena.tick();
        arena.cue_worm_formation(Clear)?;
        assert!(arena.worm_pattern_status().is_none());
        assert!(arena.worm_formation_reservation_paths().is_empty());
        assert_eq!(arena.tick(), tick);
        Ok(())
    }

    #[test]
    fn one_cue_grows_from_the_top_to_the_last_canvas_row() -> Result<()> {
        let guide = WormBox::new(47);
        let mut arena = fixture(&guide, Vec::new())?;
        arena.cue_worm_formation(WormFormationCue::Start { rule: Some(30) })?;
        let seeded = arena.worm_pattern_status().unwrap();
        assert_eq!((seeded.top, seeded.columns, seeded.max_rows), (0, 9, 9));
        assert_eq!(seeded.boxes[0].left, 21);
        arena.advance(ROW_INTERVAL - 1)?;
        assert_eq!(arena.worm_pattern_status().unwrap().rows.len(), 1);
        arena.advance(7 * ROW_INTERVAL + 1)?;
        let complete = arena.worm_pattern_status().unwrap();
        assert_eq!(complete.rows.len(), 9);
        assert!(complete.boxes.iter().any(|b| b.top == 8 * SIDE));
        arena.advance(ROW_INTERVAL)?;
        assert_eq!(arena.worm_pattern_status().unwrap().rows.len(), 9);
        Ok(())
    }

    #[test]
    fn retargeting_updates_all_rows_and_keeps_aligned_worms() -> Result<()> {
        let guide = WormBox::new(25);
        let first: Body = std::array::from_fn(|segment| 2 * 25 + 14 - segment);
        let mut arena = fixture(&guide, vec![first])?;
        let pellets: Vec<_> = arena.pellets().collect();
        for rule in [30, 54, 90, 222, 250] {
            arena.cue_worm_formation(WormFormationCue::Retarget { rule })?;
            let status = arena.worm_pattern_status().unwrap();
            assert_eq!(status.rule, rule);
            assert_eq!(status.rows.len(), status.max_rows);
            assert_eq!(status.rows.len(), 5);
            assert_eq!((status.boxes[0].left, status.boxes[0].top), (10, 0));
            assert!(status.boxes[0].lanes[2]);
            assert_eq!(arena.tick(), 0);
            assert_eq!(arena.bodies(), &[first]);
            assert_eq!(arena.pellets().collect::<Vec<_>>(), pellets);
        }
        Ok(())
    }

    #[test]
    fn rule_90_fill_plans_the_last_row_before_movement() -> Result<()> {
        use WormFormationBehavior::{Fill, FillEmpty, Restrict};
        use WormFormationCue::{Behavior, Start, Step};

        let guide = WormBox::new(47);
        let mut arena = fixture(&guide, Vec::new())?;
        arena.advance(1)?;
        arena.cue_worm_formation(Start { rule: Some(90) })?;
        assert!(arena.cue_worm_formation(Step { rule: None }).is_err());
        arena.advance(ROUND_TICKS - 1)?;

        let status = arena.worm_pattern_status().unwrap();
        assert_eq!(status.rows.len(), status.max_rows);
        assert!(status.boxes.iter().any(|b| b.top == 8 * SIDE));
        let target_count = status.boxes.len();
        for (behavior, branch_fill) in [(Restrict, false), (FillEmpty, false), (Fill, true)] {
            arena.cue_worm_formation(Behavior(behavior))?;
            let field = arena.worm_box.as_ref().unwrap().field.as_ref().unwrap();
            assert_eq!(field.branches.is_some(), branch_fill);
            assert_eq!(arena.worm_pattern_status().unwrap().behavior, behavior);
            assert_eq!(arena.tick(), ROUND_TICKS);
        }
        assert_eq!(
            arena.worm_pattern_status().unwrap().boxes.len(),
            target_count
        );
        Ok(())
    }

    #[test]
    fn restriction_releases_parked_worms_and_blocks_new_entries() -> Result<()> {
        let guide = WormBox::new(27);
        let [left, top] = Pattern::new(guide.width)
            .boxes_in_row(0, WormFormationBehavior::Fill)
            .next()
            .unwrap();
        let occupants: [Body; SIDE] = std::array::from_fn(|row| {
            std::array::from_fn(|segment| (top + row) * guide.width + left + SIDE - 1 - segment)
        });
        let outside =
            std::array::from_fn(|segment| (top + SIDE + segment) * guide.width + left + 2);
        let mut arena = fixture(&guide, occupants.into_iter().chain([outside]).collect())?;
        use WormFormationCue::{RestrictAccess, Seed};

        arena.cue_worm_formation(Seed { rule: Some(90) })?;
        assert_eq!(
            arena.worm_pattern_status().unwrap().boxes[0].lanes,
            [true; SIDE]
        );
        let rows = arena.worm_pattern_status().unwrap().rows.to_vec();
        arena.advance(3)?;
        arena.cue_worm_formation(RestrictAccess(true))?;
        assert_eq!(arena.tick(), 3);
        assert!(!arena.worm_pattern_status().unwrap().restricted);
        arena.advance(ROUND_TICKS - 3)?;

        let status = arena.worm_pattern_status().unwrap();
        assert!(status.restricted);
        assert_eq!(status.rows, rows);
        assert_eq!(status.cells_inside, SQUARE);
        assert!(arena.worm_formation_reservation_paths().is_empty());
        assert_eq!(arena.worm_box_vacancy_signals().count(), 0);

        let active = arena.worm_box.as_ref().unwrap();
        assert!(
            occupants
                .into_iter()
                .flatten()
                .all(|site| !active.held_at(site))
        );
        let outside_entry = (top + SIDE) * guide.width + left + 2;
        let entry = outside_entry - guide.width;
        let with_scent = OPEN | (3 << HEADING_SHIFT);
        let head = (1 << 3) | 1;
        assert_eq!(
            active.movement_inbox(outside_entry, head, [with_scent; PORTS]),
            [
                with_scent,
                with_scent,
                (with_scent & !ROLE_MASK) | OCCUPIED,
                with_scent
            ],
        );
        assert_eq!(
            active.movement_inbox(entry, head, [OPEN; PORTS]),
            [OCCUPIED, OCCUPIED, OCCUPIED, OPEN],
        );
        assert_eq!(
            active.movement_inbox(entry - guide.width, head, [OPEN; PORTS]),
            [OPEN; PORTS],
        );

        for _ in 0..32 {
            arena.advance(ROUND_TICKS)?;
            if arena.worm_pattern_status().unwrap().cells_inside == 0 {
                break;
            }
        }
        assert_eq!(arena.worm_pattern_status().unwrap().cells_inside, 0);
        for _ in 0..8 {
            arena.advance(ROUND_TICKS)?;
            assert_eq!(arena.worm_pattern_status().unwrap().cells_inside, 0);
        }
        assert_eq!(arena.bodies().len(), SIDE + 1);
        let tick = arena.tick();
        arena.cue_worm_formation(RestrictAccess(false))?;
        assert_eq!(arena.tick(), tick);
        assert_eq!(arena.worm_pattern_status().unwrap().rows, rows);
        assert!(!arena.worm_pattern_status().unwrap().restricted);
        assert!(arena.worm_box_vacancy_signals().count() > 0);
        Ok(())
    }

    #[test]
    fn filling_empty_space_switches_docks_without_changing_the_pattern_or_tick() -> Result<()> {
        use WormFormationBehavior::{Fill, FillEmpty};
        use WormFormationCue::{Behavior, Seed, Step};

        let guide = WormBox::new(27);
        let dark: Body = std::array::from_fn(|i| 6 + i);
        let lit: Body = std::array::from_fn(|i| 11 + i);
        let waiting: Body = std::array::from_fn(|i| 1 + i);
        let mut arena = fixture(&guide, vec![dark, lit, waiting])?;
        arena.cue_worm_formation(Seed { rule: Some(90) })?;
        arena.cue_worm_formation(Step { rule: None })?;
        let rows = arena.worm_pattern_status().unwrap().rows.to_vec();
        let tick = arena.tick();
        assert!(arena.worm_box.as_ref().unwrap().held_at(lit[0]));

        arena.cue_worm_formation(Behavior(FillEmpty))?;
        let pattern = arena.worm_pattern_status().unwrap();
        assert_eq!(pattern.behavior, FillEmpty);
        assert_eq!(pattern.rows, rows);
        assert_eq!(arena.tick(), tick);
        assert_eq!(arena.bodies(), &[dark, lit, waiting]);
        let mut positions: Vec<_> = pattern.boxes.iter().map(|b| [b.top, b.left]).collect();
        positions.sort_unstable();
        assert_eq!(
            positions,
            [[0, 1], [0, 6], [0, 16], [0, 21], [5, 1], [5, 11], [5, 21]]
        );
        let active = arena.worm_box.as_ref().unwrap();
        assert!(dark.into_iter().all(|site| active.held_at(site)));
        assert!(lit.into_iter().all(|site| !active.held_at(site)));
        assert!(waiting.into_iter().all(|site| !active.held_at(site)));

        arena.cue_worm_formation(Behavior(Fill))?;
        let pattern = arena.worm_pattern_status().unwrap();
        assert_eq!(pattern.rows, rows);
        assert_eq!(pattern.boxes.len(), 3);
        let active = arena.worm_box.as_ref().unwrap();
        assert!(lit.into_iter().all(|site| active.held_at(site)));
        assert!(dark.into_iter().all(|site| !active.held_at(site)));
        Ok(())
    }

    #[test]
    fn filling_an_empty_complement_releases_the_previous_box() -> Result<()> {
        let guide = WormBox::new(7);
        let body: Body = std::array::from_fn(|i| 1 + i);
        let mut arena = fixture(&guide, vec![body])?;
        arena.cue_worm_formation(WormFormationCue::Seed { rule: None })?;
        assert!(arena.worm_box.as_ref().unwrap().held_at(body[0]));

        arena.cue_worm_formation(WormFormationCue::Behavior(WormFormationBehavior::FillEmpty))?;
        assert!(arena.worm_pattern_status().unwrap().boxes.is_empty());
        assert!(!arena.worm_box.as_ref().unwrap().held_at(body[0]));
        assert!(arena.worm_formation_reservation_paths().is_empty());
        assert_eq!(arena.worm_box_vacancy_signals().count(), 0);
        Ok(())
    }

    #[test]
    fn assignment_releases_at_a_blocker_and_leaves_idle_lanes_open() -> Result<()> {
        let guide = WormBox::new(19);
        let dock = Dock {
            box_index: 0,
            axis: WormBoxAxis::Horizontal,
            lane: 2,
            increasing: true,
        };
        let body = arriving_body(&guide, dock);
        let mut arena = fixture(&guide, vec![body])?;
        arena.set_worm_box(true)?;
        let idle = lane_sites(&guide, dock.axis, 1)[0] - 1;
        let active = arena.worm_box.as_ref().unwrap();
        assert_eq!(
            active.movement_inbox(idle, (2 << 3) | 1, [OPEN; PORTS])[1],
            OPEN
        );

        let field = arena.worm_box.as_mut().unwrap().field.as_mut().unwrap();
        let original = field.boxes[0].assignment.as_ref().unwrap().clone();
        let (_, next) = original.head_and_next().unwrap();
        for tile in &mut field.tiles {
            tile.gate = Gate::Stock;
            tile.reserved = false;
            tile.lane_owner = None;
        }
        field.tiles[next].segment = (2 << 3) | 1;
        field.continue_assignment(0, original.issued_at);
        assert!(field.boxes[0].assignment.is_none());
        assert_eq!(
            field.released_until.get(&original.grant.worm),
            Some(&RECRUIT_AGAIN_AFTER)
        );
        assert!(field.tiles[original.path[0]].gate == Gate::Stock);
        assert!(field.tiles.iter().all(|tile| !tile.is_reserved()));
        Ok(())
    }

    #[test]
    fn box_fills_with_five_existing_worms_and_releases_on_clear() -> Result<()> {
        let size = 33;
        let mut arena = FormationArena::generate(size, 17001, 24, 24, ROOM_WAVE)?;
        arena.advance(11 * ROUND_TICKS)?;
        let tick = arena.tick();
        arena.set_worm_box(true)?;
        assert_eq!(arena.tick(), tick);
        for _ in 0..180 {
            let before = arena.bodies().to_vec();
            arena.advance(ROUND_TICKS)?;
            for (old, next) in before.iter().zip(arena.bodies()) {
                assert!(
                    old == next
                        || next[1..] == old[..SIDE - 1]
                        || next[1..].iter().copied().eq(old[1..].iter().rev().copied())
                );
            }
            if arena.worm_box_status().unwrap().lanes == [true; SIDE] {
                break;
            }
        }
        let status = arena.worm_box_status().unwrap();
        assert_eq!(status.lanes, [true; SIDE]);
        assert_eq!(arena.worm_box_vacancy_signals().count(), 0);
        assert!(arena.worm_box_reservation_path().is_empty());
        assert_eq!(arena.bodies().len(), 24);
        assert_eq!(
            arena.cells().iter().filter(|cell| food(cell) != 0).count(),
            24
        );
        let guide = arena.worm_box.as_ref().unwrap();
        let axis = status.axis.unwrap();
        let lanes = guide.field.as_ref().unwrap().boxes[0].anchor.lanes[axis as usize];
        let parked: [usize; SIDE] = std::array::from_fn(|i| {
            let Lane::Filled(id) = lanes[i] else {
                panic!("expected a filled lane")
            };
            (id >> 3) as usize - 1
        });
        for (lane, worm) in parked.into_iter().enumerate() {
            let sites = lane_sites(guide, axis, lane);
            let body = &arena.bodies()[worm];
            assert!(*body == sites || body.iter().rev().eq(sites.iter()));
        }
        arena.advance(8 * ROUND_TICKS)?;
        assert_eq!(arena.worm_box_status().unwrap().lanes, [true; SIDE]);
        let tick = arena.tick();
        let before = arena.bodies().to_vec();
        arena.set_worm_box(false)?;
        assert_eq!(arena.tick(), tick);
        arena.advance(16 * ROUND_TICKS)?;
        assert!(
            parked
                .into_iter()
                .any(|worm| before[worm] != arena.bodies()[worm])
        );
        Ok(())
    }
}
