use std::collections::VecDeque;

use super::{FormationCell, FormationTiles, Gate, PORTS, ROUND_TICKS, SIDE, WormBoxAxis, role};
use crate::world::cardinal_neighbors;

const ADMISSION_INTERVAL: u32 = 4 * ROUND_TICKS;
const ADMISSIONS_PER_CHECK: usize = 24;
const STAGING_RESERVE: usize = 100;

#[derive(Clone)]
pub(super) struct BranchPlan {
    parent: Vec<Option<usize>>,
    order: Vec<usize>,
    target_nodes: Vec<usize>,
    pub(super) approaches: Vec<[bool; PORTS]>,
    pub(super) staging_cells: Vec<usize>,
    pub(super) supply_waiting: bool,
    admitted: Vec<bool>,
    passages: Vec<Option<WormBoxAxis>>,
    threatened: Vec<bool>,
    root: usize,
    columns: usize,
    rows: usize,
    left: usize,
    top: usize,
    population: usize,
    cursor: usize,
}

impl BranchPlan {
    pub(super) fn new(field: &FormationTiles, population: usize) -> Option<Self> {
        let pattern = field.pattern.as_ref()?;
        let columns = pattern.columns;
        let rows = pattern.rows.len();
        let required: Vec<_> = pattern.rows.iter().flatten().copied().collect();

        // Start at the largest connected area that stays open after assembly.
        // A finite arena can have several disconnected stretches of boundary.
        let mut seen = vec![false; required.len()];
        let mut staging = Vec::new();
        for start in 0..required.len() {
            let x = start % columns;
            let y = start / columns;
            if seen[start]
                || required[start]
                || (x != 0 && x + 1 != columns && y != 0 && y + 1 != rows)
            {
                continue;
            }
            let (_, component) = Self::spread(columns, rows, &[start], |tile| !required[tile]);
            for &tile in &component {
                seen[tile] = true;
            }
            if component.len() > staging.len() {
                staging = component;
            }
        }
        // The smallest arena fits one box and has no other whole tile.
        let root = *staging.first()?;
        let (permanent, _) = Self::spread(columns, rows, &staging, |tile| !required[tile]);
        let (parent, order) = Self::spread(columns, rows, &staging, |_| true);
        let target_nodes: Vec<_> = field
            .boxes
            .iter()
            .map(|target| {
                (target.top - pattern.top) / SIDE * columns + (target.left - pattern.left) / SIDE
            })
            .collect();
        let approaches = target_nodes
            .iter()
            .map(|&node| {
                cardinal_neighbors(node, columns, rows).map(|neighbor| {
                    neighbor
                        .is_some_and(|side| Some(side) == parent[node] || permanent[side].is_some())
                })
            })
            .collect();
        let mut staging_cells = Vec::new();
        for node in staging {
            let left = pattern.left + node % columns * SIDE;
            let top = pattern.top + node / columns * SIDE;
            for y in top..top + SIDE {
                for x in left..left + SIDE {
                    staging_cells.push(field.index(y * field.width + x).unwrap());
                }
            }
        }
        Some(Self {
            parent,
            order,
            target_nodes,
            approaches,
            staging_cells,
            supply_waiting: false,
            admitted: field
                .boxes
                .iter()
                .map(|target| target.anchor.complete())
                .collect(),
            passages: vec![None; required.len()],
            threatened: Vec::new(),
            root,
            columns,
            rows,
            left: pattern.left,
            top: pattern.top,
            population,
            cursor: 0,
        })
    }

    fn spread(
        columns: usize,
        rows: usize,
        roots: &[usize],
        allow: impl Fn(usize) -> bool,
    ) -> (Vec<Option<usize>>, Vec<usize>) {
        Self::spread_edges(columns, rows, roots, |_, _, next| allow(next))
    }

    fn spread_edges(
        columns: usize,
        rows: usize,
        roots: &[usize],
        allow: impl Fn(usize, usize, usize) -> bool,
    ) -> (Vec<Option<usize>>, Vec<usize>) {
        let mut parent = vec![None; columns * rows];
        let mut queue: VecDeque<_> = roots.iter().copied().collect();
        let mut order = Vec::new();
        for &root in roots {
            parent[root] = Some(root);
        }
        while let Some(tile) = queue.pop_front() {
            order.push(tile);
            for (port, neighbor) in cardinal_neighbors(tile, columns, rows)
                .into_iter()
                .enumerate()
            {
                let Some(neighbor) = neighbor else { continue };
                if parent[neighbor].is_none() && allow(tile, port, neighbor) {
                    parent[neighbor] = Some(tile);
                    queue.push_back(neighbor);
                }
            }
        }
        (parent, order)
    }

    fn demand(&self, field: &FormationTiles) -> Vec<usize> {
        let mut demand = vec![0; self.parent.len()];
        for (&node, target) in self.target_nodes.iter().zip(&field.boxes) {
            demand[node] = usize::from(!target.anchor.complete());
        }
        for &tile in self.order.iter().rev() {
            let parent = self.parent[tile].unwrap();
            if parent != tile {
                demand[parent] += demand[tile];
            }
        }
        demand
    }

    pub(super) fn ready(&self, box_index: usize) -> bool {
        self.admitted[box_index]
    }

    pub(super) fn passage(&self, box_index: usize) -> Option<WormBoxAxis> {
        self.passages[self.target_nodes[box_index]]
    }

    fn reachable(&self, closed: &[bool]) -> (Vec<Option<usize>>, Vec<usize>) {
        Self::spread_edges(self.columns, self.rows, &[self.root], |tile, port, next| {
            let axis = if port < 2 {
                WormBoxAxis::Horizontal
            } else {
                WormBoxAxis::Vertical
            };
            !closed[next]
                && self.passages[tile].is_none_or(|passage| passage == axis)
                && self.passages[next].is_none_or(|passage| passage == axis)
        })
    }

    pub(super) fn admit(
        &mut self,
        field: &FormationTiles,
        cell: impl Fn(usize) -> FormationCell,
        tick: u32,
    ) {
        if !tick.is_multiple_of(ADMISSION_INTERVAL) {
            return;
        }
        let boxes = &field.boxes;
        let demand = self.demand(field);
        let capacity = boxes.len() * SIDE;
        let needed = capacity - field.docked_worms();
        let reserve = self
            .population
            .saturating_sub(capacity)
            .min(STAGING_RESERVE);
        let mut mobile = vec![0usize; self.parent.len()];
        for (index, tile) in field.tiles.iter().enumerate() {
            let site = field.site(index);
            if tile.gate == Gate::Parked || role(cell(site).segment) != 1 {
                continue;
            }
            let x = site % field.width;
            let y = site / field.width;
            if x < self.left || y < self.top {
                continue;
            }
            let column = (x - self.left) / SIDE;
            let row = (y - self.top) / SIDE;
            if column < self.columns && row < self.rows {
                mobile[row * self.columns + column] += 1;
            }
        }
        let mut closed = vec![false; self.parent.len()];
        for (i, &node) in self.target_nodes.iter().enumerate() {
            closed[node] = self.admitted[i] || boxes[i].anchor.complete();
        }
        let required = if self.population < capacity {
            // In undersupplied arenas, keep the worms that can still reach the formation.
            let (_, reachable) = self.reachable(&closed);
            reachable.into_iter().map(|tile| mobile[tile]).sum()
        } else {
            needed + reserve
        };

        let mut scan = 0;
        let mut considered = 0;
        self.supply_waiting = false;
        self.threatened.clear();
        while scan < boxes.len() && considered < ADMISSIONS_PER_CHECK {
            let index = (self.cursor + scan) % boxes.len();
            scan += 1;
            let node = self.target_nodes[index];
            if closed[node] || demand[node] != 1 {
                continue;
            }
            // A ready box has no unfinished descendants. Test the cut before
            // reserving it so closing one branch cannot starve another of worms.
            considered += 1;
            closed[node] = true;
            let (after, accessible) = self.reachable(&closed);
            let supply: usize = accessible.into_iter().map(|tile| mobile[tile]).sum();
            if supply >= required {
                self.admitted[index] = true;
                self.passages[node] = None;
                continue;
            }
            closed[node] = false;
            self.supply_waiting = true;
            if self.threatened.is_empty() {
                let (before, _) = self.reachable(&closed);
                let cut: Vec<_> = before
                    .iter()
                    .zip(&after)
                    .map(|(before, after)| before.is_some() && after.is_none())
                    .collect();
                if cut
                    .iter()
                    .zip(&mobile)
                    .any(|(&inside, &heads)| inside && heads != 0)
                {
                    self.threatened = cut;
                }
            }
        }
        self.keep_straight_passages(&closed, &demand, &mobile, required);
        self.cursor = (self.cursor + scan) % boxes.len();
    }

    fn keep_straight_passages(
        &mut self,
        closed: &[bool],
        demand: &[usize],
        mobile: &[usize],
        needed: usize,
    ) {
        let mut considered = 0;
        for offset in 0..self.target_nodes.len() {
            let node = self.target_nodes[(self.cursor + offset) % self.target_nodes.len()];
            if closed[node] || demand[node] <= 1 || self.passages[node].is_some() {
                continue;
            }
            let edges = cardinal_neighbors(node, self.columns, self.rows);
            let port = edges
                .iter()
                .position(|neighbor| *neighbor == self.parent[node])
                .unwrap();
            let through = edges[port ^ 1].is_some_and(|neighbor| {
                self.parent[neighbor] == Some(node) && demand[neighbor] > 0
            });
            let turns = edges.iter().enumerate().any(|(edge, &neighbor)| {
                edge != (port ^ 1)
                    && neighbor
                        .is_some_and(|child| self.parent[child] == Some(node) && demand[child] > 0)
            });
            if !through || turns {
                continue;
            }
            self.passages[node] = Some(if port < 2 {
                WormBoxAxis::Horizontal
            } else {
                WormBoxAxis::Vertical
            });
            let (parents, reachable) = self.reachable(closed);
            let available: usize = reachable.into_iter().map(|tile| mobile[tile]).sum();
            let keeps_targets = self
                .target_nodes
                .iter()
                .all(|&target| closed[target] || parents[target].is_some());
            if available < needed || !keeps_targets {
                self.passages[node] = None;
            }
            considered += 1;
            if considered == ADMISSIONS_PER_CHECK {
                break;
            }
        }
    }

    pub(super) fn endangered_sites(&self, field: &FormationTiles) -> Vec<bool> {
        if self.threatened.is_empty() {
            return Vec::new();
        }
        let mut sites = vec![false; field.tiles.len()];
        for (node, &inside) in self.threatened.iter().enumerate() {
            if !inside {
                continue;
            }
            let left = self.left + node % self.columns * SIDE;
            let top = self.top + node / self.columns * SIDE;
            for y in top..top + SIDE {
                for x in left..left + SIDE {
                    sites[field.index(y * field.width + x).unwrap()] = true;
                }
            }
        }
        sites
    }
}
