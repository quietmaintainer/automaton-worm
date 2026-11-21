pub const WORM_LENGTH: usize = 5;
pub const MAX_WORMS: usize = 1_000;
pub const MAX_PELLETS: usize = 32;
pub const MAX_GRID_SIZE: usize = 128;

pub(crate) type Body = [usize; WORM_LENGTH];

pub(crate) fn initial_pellets(
    cells: usize,
    bodies: &[Body],
    count: usize,
    seed: &mut u32,
) -> Vec<usize> {
    let mut occupied = vec![false; cells];
    for &cell in bodies.iter().flatten() {
        occupied[cell] = true;
    }
    let mut available: Vec<_> = (0..cells).filter(|&cell| !occupied[cell]).collect();
    (0..count)
        .map(|_| {
            let index = crate::world::random_index(seed, available.len());
            available.swap_remove(index)
        })
        .collect()
}

const CORNER_BODY: [(usize, usize); WORM_LENGTH] = [(1, 1), (1, 0), (0, 0), (0, 1), (0, 2)];
const FIRST_LANE_SIZE: usize = 11;

pub fn worm_capacity(size: usize) -> usize {
    match size {
        0..=2 => 0,
        3..=5 => 1,
        6..=10 => 4,
        _ => MAX_WORMS.min((size / WORM_LENGTH).saturating_mul(size.div_ceil(2))),
    }
}

pub fn minimum_size(count: usize) -> usize {
    match count {
        0 | 1 => 3,
        2..=4 => 6,
        _ => (FIRST_LANE_SIZE..=MAX_GRID_SIZE)
            .find(|&size| worm_capacity(size) >= count.min(MAX_WORMS))
            .expect("the maximum population fits in the arena"),
    }
}

pub(crate) fn initial_bodies(size: usize, count: usize) -> Vec<Body> {
    if count <= 4 {
        let last = size - 1;
        let corners = [(false, false), (true, true), (true, false), (false, true)];
        return corners[..count]
            .iter()
            .map(|&(right, bottom)| {
                CORNER_BODY.map(|(x, y)| {
                    let x = if right { last - x } else { x };
                    let y = if bottom { last - y } else { y };
                    y * size + x
                })
            })
            .collect();
    }

    // Horizontal bodies fill alternating rows. This leaves space for their
    // heads to turn even when every slot in the starting row is taken.
    let columns = size / WORM_LENGTH;
    let slots = columns * size.div_ceil(2);
    (0..count)
        .map(|index| {
            let slot = index * slots / count;
            let y = 2 * (slot / columns);
            let x = WORM_LENGTH * (slot % columns);
            std::array::from_fn(|segment| {
                let offset = if (y / 2).is_multiple_of(2) {
                    WORM_LENGTH - 1 - segment
                } else {
                    segment
                };
                y * size + x + offset
            })
        })
        .collect()
}
