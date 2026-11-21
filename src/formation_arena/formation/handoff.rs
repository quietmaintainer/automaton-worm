use std::collections::{BTreeMap, VecDeque};

use super::{
    Dock, FORMATION_HEADING_SHIFT, FormationCell, FormationTiles, Gate, Lane, ROUND_TICKS, SIDE,
    WormBoxAxis, role, worm_id,
};

const WAIT_FOR_PASSAGE: u32 = 2 * ROUND_TICKS;
const REPLACEMENT_HOPS: usize = 12;
const HANDOFF_TIMEOUT: u32 = 8 * ROUND_TICKS;
const REQUEST_COOLDOWN: u32 = 12 * ROUND_TICKS;
const ACTIVE_HANDOFF_LIMIT: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct PassageRequest {
    dock: Dock,
    visitor: u32,
    resident: u32,
}

#[derive(Clone, Default)]
pub(super) struct BoxTraffic {
    pressure: Option<(PassageRequest, u32, u32)>,
    retry_at: u32,
    handoff: Option<Box<Handoff>>,
}

impl BoxTraffic {
    pub(super) fn in_progress(&self) -> bool {
        self.handoff.is_some()
    }

    pub(super) fn route(&self) -> Option<&[usize]> {
        let handoff = self.handoff.as_ref()?;
        Some(match handoff.stage {
            Stage::Queue => &handoff.replacement.path[handoff.replacement.step..=handoff.queue_end],
            Stage::Yield => &handoff.resident.path[handoff.resident.step..],
            Stage::Cross => &handoff.visitor.path[handoff.visitor.step..],
            Stage::Refill => &handoff.replacement.path[handoff.replacement.step..],
        })
    }

    pub(super) fn reserve_lane(&self, axis: WormBoxAxis, lanes: &mut [Lane; SIDE]) {
        if let Some(handoff) = &self.handoff
            && handoff.dock.axis == axis
        {
            let lane = &mut lanes[handoff.dock.lane];
            if !matches!(*lane, Lane::Filled(worm) if worm == handoff.resident.worm || worm == handoff.replacement.worm)
            {
                // A passing body can briefly span the lane. It still has to leave it.
                *lane = Lane::Blocked;
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Queue,
    Yield,
    Cross,
    Refill,
}

#[derive(Clone)]
struct Walk {
    worm: u32,
    path: Vec<usize>,
    step: usize,
}

impl Walk {
    fn new(worm: u32, path: Vec<usize>) -> Self {
        Self {
            worm,
            path,
            step: 0,
        }
    }

    fn head(&self) -> usize {
        self.path[self.step]
    }

    fn is_present(&self, field: &FormationTiles) -> bool {
        owns_head(field.tiles[self.head()].segment, self.worm)
    }
}

#[derive(Clone)]
struct Handoff {
    dock: Dock,
    resident: Walk,
    visitor: Walk,
    replacement: Walk,
    queue_end: usize,
    reserved: Vec<usize>,
    stage: Stage,
    last_progress: u32,
}

impl Handoff {
    fn new(field: &FormationTiles, request: PassageRequest, tick: u32) -> Option<Self> {
        let dock = request.dock;
        let forward = dock.port();
        let lane = field.lane_path(dock);
        let resident_head = lane[SIDE - 1];
        let entry = field.tiles[lane[0]].links[forward ^ 1]?;
        let outside = field.tiles[resident_head].links[forward]?;
        if field.tiles[entry].gate != Gate::Stock
            || field.tiles[entry].is_reserved()
            || !owns_head(field.tiles[entry].segment, request.visitor)
            || !owns_head(field.tiles[resident_head].segment, request.resident)
            || !field.open_for_handoff(outside, dock.box_index)
        {
            return None;
        }

        // Check that both bodies have room to clear the far mouth. Their routes
        // may turn outside the box and share only the mouth itself.
        let (exit, resident_exit) =
            [forward, forward ^ 2, forward ^ 3]
                .into_iter()
                .find_map(|port| {
                    let mut exit = vec![outside];
                    if !field.extend_exit(dock.box_index, &mut exit, SIDE - 1, port, &[entry]) {
                        return None;
                    }
                    let mut resident_exit = vec![outside];
                    let avoid = [&exit[1..], &[entry]].concat();
                    field
                        .extend_exit(
                            dock.box_index,
                            &mut resident_exit,
                            SIDE,
                            forward ^ 2,
                            &avoid,
                        )
                        .then_some((exit, resident_exit))
                })?;
        let resident_path = [vec![resident_head], resident_exit].concat();
        let visitor_path = [vec![entry], lane.clone(), exit].concat();
        let mut reserved = [resident_path.as_slice(), visitor_path.as_slice()].concat();

        let (replacement, mut replacement_path) = field.tiles[entry]
            .links
            .into_iter()
            .flatten()
            .filter(|site| !reserved.contains(site))
            .filter_map(|site| field.replacement_call(site, dock.box_index, &reserved))
            .min_by_key(|(id, path)| (path.len(), *id))?;
        let queue_end = replacement_path.len() - 1;
        reserved.extend_from_slice(&replacement_path);
        reserved.sort_unstable();
        reserved.dedup();
        if reserved
            .iter()
            .any(|&index| field.tiles[index].is_reserved())
        {
            return None;
        }
        replacement_path.push(entry);
        replacement_path.extend(lane);

        Some(Self {
            dock,
            resident: Walk::new(request.resident, resident_path),
            visitor: Walk::new(request.visitor, visitor_path),
            replacement: Walk::new(replacement, replacement_path),
            queue_end,
            reserved,
            stage: Stage::Queue,
            last_progress: tick,
        })
    }

    // Each phase is committed by the ordinary cell movement rule. The next
    // phase starts only after every step in the current path has arrived.
    fn advance(&mut self, field: &mut FormationTiles, tick: u32) -> bool {
        let (moving, next) = loop {
            let walk = match self.stage {
                Stage::Queue | Stage::Refill => &mut self.replacement,
                Stage::Yield => &mut self.resident,
                Stage::Cross => &mut self.visitor,
            };
            let end = if self.stage == Stage::Queue {
                self.queue_end
            } else {
                walk.path.len() - 1
            };
            if walk.step < end
                && owns_head(field.tiles[walk.path[walk.step + 1]].segment, walk.worm)
            {
                walk.step += 1;
                self.last_progress = tick;
            }
            if !walk.is_present(field) {
                return false;
            }
            if walk.step < end {
                break (walk.worm, walk.path[walk.step + 1]);
            }
            self.stage = match self.stage {
                Stage::Queue => Stage::Yield,
                Stage::Yield => Stage::Cross,
                Stage::Cross => Stage::Refill,
                Stage::Refill => return false,
            };
        };

        let participants = [&self.resident, &self.visitor, &self.replacement];
        if tick.saturating_sub(self.last_progress) >= HANDOFF_TIMEOUT
            || participants.iter().any(|walk| !walk.is_present(field))
            || self
                .reserved
                .iter()
                .any(|&index| field.tiles[index].is_reserved())
        {
            return false;
        }
        for &index in &self.reserved {
            field.tiles[index].reserved = true;
        }
        for walk in participants {
            if self.stage == Stage::Queue && walk.worm == self.resident.worm {
                continue;
            }
            let head = walk.head();
            let gate = if walk.worm == moving && field.tiles[next].segment == 0 {
                let port = field.tiles[head]
                    .links
                    .iter()
                    .position(|&peer| peer == Some(next))
                    .unwrap();
                Gate::Move(port)
            } else {
                Gate::Wait
            };
            field.steer_body(head, walk.worm, gate);
        }
        true
    }
}

fn owns_head(segment: u32, worm: u32) -> bool {
    role(segment) == 1 && worm_id(segment) == worm
}

impl FormationTiles {
    fn lane_path(&self, dock: Dock) -> Vec<usize> {
        let mut path: Vec<_> = self.boxes[dock.box_index]
            .tiles
            .into_iter()
            .enumerate()
            .filter_map(|(local, site)| (dock.axis.lane(local) == dock.lane).then_some(site))
            .collect();
        if !dock.increasing {
            path.reverse();
        }
        path
    }

    fn open_for_handoff(&self, index: usize, source: usize) -> bool {
        let tile = &self.tiles[index];
        tile.segment == 0
            && !tile.is_reserved()
            && tile.footprint.is_none_or(|cell| {
                cell.box_index != source && (!self.boxes[cell.box_index].guarded || tile.through)
            })
    }

    fn extend_exit(
        &self,
        source: usize,
        path: &mut Vec<usize>,
        steps: usize,
        ahead: usize,
        avoid: &[usize],
    ) -> bool {
        if steps == 0 {
            return true;
        }
        let current = *path.last().unwrap();
        for port in [ahead, ahead ^ 2, ahead ^ 3, ahead ^ 1] {
            if let Some(next) = self.tiles[current].links[port]
                && !avoid.contains(&next)
                && !path.contains(&next)
                && self.open_for_handoff(next, source)
            {
                path.push(next);
                if self.extend_exit(source, path, steps - 1, port, avoid) {
                    return true;
                }
                path.pop();
            }
        }
        false
    }

    // Relay a bounded vacancy call outward from the staging cell. The first
    // free head replies along the decreasing hop count back to the queue.
    fn replacement_call(
        &self,
        queue: usize,
        source: usize,
        avoid: &[usize],
    ) -> Option<(u32, Vec<usize>)> {
        let mut parent = BTreeMap::from([(queue, queue)]);
        let mut reached = VecDeque::from([(queue, 0)]);
        while let Some((index, distance)) = reached.pop_front() {
            let tile = &self.tiles[index];
            if role(tile.segment) == 1
                && tile.gate == Gate::Stock
                && !tile.is_reserved()
                && tile.footprint.is_none_or(|cell| {
                    cell.box_index != source && !self.boxes[cell.box_index].guarded
                })
            {
                let mut current = index;
                let mut path = vec![current];
                while current != queue {
                    current = parent[&current];
                    path.push(current);
                }
                return Some((worm_id(tile.segment), path));
            }
            if distance == REPLACEMENT_HOPS || !self.open_for_handoff(index, source) {
                continue;
            }
            for next in tile.links.into_iter().flatten() {
                if !avoid.contains(&next)
                    && let std::collections::btree_map::Entry::Vacant(slot) = parent.entry(next)
                {
                    slot.insert(index);
                    reached.push_back((next, distance + 1));
                }
            }
        }
        None
    }

    fn blocked_passage(
        &self,
        index: usize,
        cell: impl Fn(usize) -> FormationCell,
    ) -> Option<PassageRequest> {
        let target = &self.boxes[index];
        let axis = target.anchor.axis?;
        for (local, site) in target.tiles.into_iter().enumerate() {
            let offset = axis.offset(local);
            if offset != 0 && offset != SIDE - 1 {
                continue;
            }
            let dock = Dock {
                box_index: index,
                axis,
                lane: axis.lane(local),
                increasing: offset == 0,
            };
            let inward = dock.port();
            let Some(entry) = self.tiles[site].links[inward ^ 1] else {
                continue;
            };
            let visitor = cell(self.site(entry));
            let visitor_head = visitor.segment;
            if !visitor.blocked
                || role(visitor_head) != 1
                || ((visitor_head >> FORMATION_HEADING_SHIFT) as usize & 3) != inward
            {
                continue;
            }
            let Lane::Filled(resident) = target.anchor.lane(dock) else {
                continue;
            };
            let word = cell(self.site(site)).segment;
            if role(word) == SIDE && worm_id(word) == resident {
                return Some(PassageRequest {
                    dock,
                    visitor: worm_id(visitor_head),
                    resident,
                });
            }
        }
        None
    }

    pub(super) fn passage_requests(
        &mut self,
        cell: impl Fn(usize) -> FormationCell + Copy,
        tick: u32,
    ) -> Vec<PassageRequest> {
        let active = self
            .boxes
            .iter()
            .filter(|box_tiles| box_tiles.traffic.in_progress())
            .count();
        let mut requests = Vec::new();
        for index in 0..self.boxes.len() {
            if active + requests.len() >= ACTIVE_HANDOFF_LIMIT {
                break;
            }
            let target = &self.boxes[index];
            if !target.anchor.complete()
                || target.traffic.in_progress()
                || target.traffic.retry_at > tick
            {
                continue;
            }
            let request = self.blocked_passage(index, cell);
            let state = &mut self.boxes[index].traffic;
            state.pressure = request.map(|request| {
                let since = state
                    .pressure
                    .filter(|&(previous, _, seen)| {
                        previous == request && tick.saturating_sub(seen) <= ROUND_TICKS
                    })
                    .map_or(tick, |(_, since, _)| since);
                (request, since, tick)
            });
            if let Some((request, since, _)) = state.pressure
                && tick.saturating_sub(since) >= WAIT_FOR_PASSAGE
            {
                state.retry_at = tick.saturating_add(REQUEST_COOLDOWN);
                requests.push(request);
            }
        }
        requests
    }

    pub(super) fn advance_handoffs(&mut self, tick: u32) {
        for index in 0..self.boxes.len() {
            let Some(mut handoff) = self.boxes[index].traffic.handoff.take() else {
                continue;
            };
            if handoff.advance(self, tick) {
                self.boxes[index].traffic.handoff = Some(handoff);
            } else {
                self.boxes[index].traffic.retry_at = tick.saturating_add(REQUEST_COOLDOWN);
            }
        }
    }

    pub(super) fn begin_handoffs(&mut self, requests: Vec<PassageRequest>, tick: u32) {
        for request in requests {
            if let Some(mut handoff) = Handoff::new(self, request, tick)
                && handoff.advance(self, tick)
            {
                self.boxes[request.dock.box_index].traffic.handoff = Some(Box::new(handoff));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formation_arena::formation::{BranchPlan, Pattern, WormBox, WormFormationBehavior};
    use crate::formation_arena::{
        Body, FormationArena, REVERSALS_DISABLED, ROOM_DISABLED, seed_room, seeded_world,
    };
    use crate::world::Link;

    const WIDTH: usize = 25;
    const LANE: usize = 2;
    const VISITOR: usize = SIDE;
    const REPLACEMENT: usize = SIDE + 1;

    #[derive(Clone, Copy)]
    enum Spare {
        AtMouth,
        Nearby,
        Absent,
    }

    fn cell(x: usize, y: usize) -> usize {
        y * WIDTH + x
    }

    fn arena(spare: Spare) -> anyhow::Result<FormationArena> {
        let mut bodies: Vec<Body> = (0..SIDE)
            .map(|row| std::array::from_fn(|segment| cell(14 - segment, row)))
            .collect();
        bodies.push(std::array::from_fn(|segment| cell(9 - segment, LANE)));
        match spare {
            Spare::AtMouth => bodies.push(std::array::from_fn(|segment| cell(9, 3 + segment))),
            Spare::Nearby => bodies.push(std::array::from_fn(|segment| cell(7 - segment, 3))),
            Spare::Absent => {}
        }
        let mut world = seeded_world(WIDTH, 17001, &bodies, &[cell(24, 24)])?;
        seed_room(&mut world, ROOM_DISABLED);
        for state in &mut world.initial {
            state.memory[1] |= REVERSALS_DISABLED as u32;
        }
        let mut close = |site: usize, port: usize| {
            let peer = world.sites[site].links[port].site as usize;
            world.sites[site].links[port] = Link::CLOSED;
            world.sites[peer].links[port ^ 1] = Link::CLOSED;
        };
        close(cell(9, 2), 2);
        if matches!(spare, Spare::AtMouth) {
            close(cell(9, 3), 0);
        } else if matches!(spare, Spare::Absent) {
            close(cell(9, 2), 3);
        }
        let mut arena = FormationArena::new(world, WIDTH, bodies);
        let mut guide = WormBox::new(WIDTH);
        let mut field = FormationTiles::new(
            WIDTH,
            Some(Pattern::new(WIDTH)),
            WormFormationBehavior::Fill,
            |site| super::super::site_neighbors(arena.engine.sites(), site),
        );
        field.refresh(|site| (&arena.cells()[site]).into(), 0);
        assert!(field.boxes[0].anchor.complete());
        field.branch_population = Some(arena.bodies().len());
        field.branches = BranchPlan::new(&field, arena.bodies().len());
        guide.field = Some(field);
        arena.worm_box = Some(guide);
        Ok(arena)
    }

    fn field(arena: &FormationArena) -> &FormationTiles {
        arena.worm_box.as_ref().unwrap().field.as_ref().unwrap()
    }

    fn check_crossing_and_refill(arena: &mut FormationArena) -> anyhow::Result<()> {
        let lane: Body = std::array::from_fn(|segment| cell(14 - segment, LANE));
        let mut entered = false;
        for _ in 0..40 {
            arena.advance(ROUND_TICKS)?;
            let visitor_in_lane = arena.bodies()[VISITOR]
                .iter()
                .any(|site| lane.contains(site));
            entered |= visitor_in_lane;
            if arena.bodies()[REPLACEMENT]
                .iter()
                .any(|site| lane.contains(site))
            {
                assert!(
                    entered && !visitor_in_lane,
                    "the queue must wait for the passing tail"
                );
            }
            if arena.bodies()[REPLACEMENT] == lane {
                assert!(field(arena).boxes[0].anchor.complete());
                assert!(!field(arena).boxes[0].traffic.in_progress());
                return Ok(());
            }
        }
        anyhow::bail!("the reserved replacement never finished docking")
    }

    #[test]
    fn waits_to_yield_until_the_replacement_reaches_the_queue() -> anyhow::Result<()> {
        let mut arena = arena(Spare::Nearby)?;
        let initial_resident = arena.bodies()[LANE];
        let mut guide = arena.worm_box.take().unwrap();
        let field = guide.field.as_mut().unwrap();
        let Lane::Filled(resident) = field.boxes[0].anchor.lanes[0][LANE] else {
            unreachable!()
        };
        let visitor = worm_id(FormationCell::from(&arena.cells()[cell(9, 2)]).segment);
        let request = PassageRequest {
            dock: Dock {
                box_index: 0,
                axis: WormBoxAxis::Horizontal,
                lane: LANE,
                increasing: true,
            },
            visitor,
            resident,
        };
        field.begin_handoffs(vec![request], 0);
        assert!(field.boxes[0].traffic.handoff.as_ref().unwrap().stage == Stage::Queue);
        arena.worm_box = Some(guide);
        arena.advance(ROUND_TICKS)?;
        assert_eq!(arena.bodies()[LANE], initial_resident);
        assert_eq!(arena.bodies()[REPLACEMENT][0], cell(8, 3));
        arena.advance(ROUND_TICKS)?;
        assert_eq!(arena.bodies()[LANE], initial_resident);
        assert_eq!(arena.bodies()[REPLACEMENT][0], cell(9, 3));
        check_crossing_and_refill(&mut arena)
    }

    #[test]
    fn a_waiting_neighbor_automatically_starts_a_handoff() -> anyhow::Result<()> {
        let mut arena = arena(Spare::AtMouth)?;
        // A few blocked commits advertise the request through the ordinary rule.
        arena.advance(3 * ROUND_TICKS)?;
        assert!(field(&arena).boxes[0].traffic.in_progress());
        check_crossing_and_refill(&mut arena)
    }

    #[test]
    fn leaves_the_resident_docked_when_no_replacement_can_answer() -> anyhow::Result<()> {
        let mut arena = arena(Spare::Absent)?;
        let initial_resident = arena.bodies()[LANE];
        arena.advance(24 * ROUND_TICKS)?;
        assert!(
            field(&arena).boxes[0].traffic.retry_at > 0,
            "the blocked worm should have requested a handoff"
        );
        assert!(!field(&arena).boxes[0].traffic.in_progress());
        assert_eq!(arena.bodies()[LANE], initial_resident);
        assert!(field(&arena).boxes[0].anchor.complete());
        Ok(())
    }
}
