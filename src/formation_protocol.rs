// The rule program and the snapshot reader share the cell word layout.
pub const TICKS_PER_MOVE: u32 = 24;
pub const ROOM_SEND: u64 = 0;
pub const ROOM_RECEIVE: u64 = 1;
pub const ASK_FIRST: u64 = 6;
pub const ASK_SECOND: u64 = 13;
pub const COMMIT: u64 = TICKS_PER_MOVE as u64 - 1;
pub const CLOCK_SHIFT: u64 = 5;
pub const PHASE_MASK: u64 = (1 << CLOCK_SHIFT) - 1;
pub const ROLE_MASK: u64 = 7;

pub const OPEN: u64 = 1;
pub const REQUEST: u64 = 2;
pub const TICKET: u64 = 3;
pub const OCCUPIED: u64 = 4;
pub const FORWARD: u64 = 5;
pub const REVERSE: u64 = 6;
pub const TRY_OTHER: u64 = 7;

pub const SCENT_MASK: u64 = 63;
pub const FOOD_SCENT: u64 = 48;
pub const FOOD_SHIFT: u64 = 6;
pub const FOOD_MASK: u64 = 63;
pub const RIPE: u64 = 1;
pub const WAIT_SHIFT: u64 = 12;
pub const WAIT_MASK: u64 = 15;

pub const PICKED_UP: u64 = 1;
pub const MOVED: u64 = 2;
pub const REVERSED: u64 = 4;
pub const BLOCKED: u64 = 8;
pub const CONTESTED: u64 = 16;

pub const ROOM_PRESSURE_SHIFT: u64 = 5;
pub const ROOM_PRESSURE_MASK: u64 = 7 << ROOM_PRESSURE_SHIFT;
pub const ROOM_CONTACT: u64 = 1 << 8;
pub const ROOM_READY: u64 = 1 << 9;
pub const ROOM_PORT_SHIFT: u64 = 10;
// Set at initialization and preserved across rounds. Both runs use the same table.
pub const ROOM_DISABLED: u64 = 1 << 14;
pub const ROOM_WAVE: u64 = 1 << 15;
pub const YIELDED_ID_SHIFT: u64 = 16;
pub const MAX_ROOM_FORWARDS: u64 = 3;
// Odd pressures choose the tail first. Stronger pressures win the relay.
pub const CONTACT_PRESSURE: [u32; 6] = [0, 7, 5, 2, 4, 6];
// These two fields share the upper bits of the scent/food/age word.
pub const ROOM_INGRESS_SHIFT: u64 = 16;
pub const ROOM_INGRESS_MASK: u64 = 15 << ROOM_INGRESS_SHIFT;
pub const ROOM_NEXT_HOP_SHIFT: u64 = 20;
pub const ROOM_NEXT_HOP_MASK: u64 = 3 << ROOM_NEXT_HOP_SHIFT;
// Fixed per-site setting in the same word, independent of the input seed.
pub const REVERSALS_DISABLED: u64 = 1 << 22;

pub const HEADING_SHIFT: u64 = 20;
pub const ID_MASK: u64 = ((1 << HEADING_SHIFT) - 1) & !ROLE_MASK;
// A tail remembers the port back to the last tile its worm left.
pub const RETURN_PORT_SHIFT: u64 = HEADING_SHIFT + 2;
pub const RETURN_FRESH: u64 = 1 << (RETURN_PORT_SHIFT + 4);
pub const REVERSED_TICKET: u64 = 1 << RETURN_PORT_SHIFT;
pub const PORT_ROLE_SHIFT: u64 = HEADING_SHIFT + 2 * FOOD_SHIFT;
pub const PORT_WAIT_SHIFT: u64 = PORT_ROLE_SHIFT + 3;
pub const OPEN_USEFUL_RETURN: u64 = 1 << (PORT_WAIT_SHIFT + 3);
pub const ROOM_HOP_SHIFT: u64 = YIELDED_ID_SHIFT + 17;
pub const ROOM_HOP_MASK: u64 = 3 << ROOM_HOP_SHIFT;
pub const ROOM_FORWARD_PORT_SHIFT: u64 = ROOM_HOP_SHIFT + 2;
pub const WORD_MASK: u64 = (1 << (ROOM_FORWARD_PORT_SHIFT + 4)) - 1;
// A short worm stores its missing segment count in the top of the
// body word and carries it to the destination in its request.
pub const BODY_LENGTH_SHIFT: u64 = RETURN_PORT_SHIFT + 5;
pub const BODY_LENGTH_MASK: u64 = 3 << BODY_LENGTH_SHIFT;
pub const REQUEST_LENGTH_SHIFT: u64 = ROOM_FORWARD_PORT_SHIFT + 4;
pub const REQUEST_LENGTH_MASK: u64 = 3 << REQUEST_LENGTH_SHIFT;
