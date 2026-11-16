// The rule program and the snapshot reader share the cell word layout.
pub const TICKS_PER_MOVE: u32 = 24;
pub const ROOM_SEND: u32 = 0;
pub const ROOM_RECEIVE: u32 = 1;
pub const ASK_FIRST: u32 = 6;
pub const COMMIT: u32 = TICKS_PER_MOVE - 1;
pub const CLOCK_SHIFT: u32 = 5;
pub const PHASE_MASK: u32 = (1 << CLOCK_SHIFT) - 1;
pub const ROLE_MASK: u32 = 7;
pub const ID_MASK: u32 = 0x1ff8;
pub const HEADING_SHIFT: u32 = 13;
// A tail remembers the port back to the last tile its worm left.
pub const RETURN_PORT_SHIFT: u32 = 15;
pub const RETURN_FRESH: u32 = 1 << (RETURN_PORT_SHIFT + 4);

pub const OPEN: u32 = 1;
pub const REQUEST: u32 = 2;
pub const TICKET: u32 = 3;
pub const OCCUPIED: u32 = 4;
pub const FORWARD: u32 = 5;
pub const REVERSE: u32 = 6;
pub const TRY_OTHER: u32 = 7;
pub const REVERSED_TICKET: u32 = 1 << 15;

pub const SCENT_MASK: u32 = 63;
pub const FOOD_SCENT: u32 = 48;
pub const FOOD_SHIFT: u32 = 6;
pub const FOOD_MASK: u32 = 63;
pub const RIPE: u32 = 1;
pub const WAIT_SHIFT: u32 = 12;
pub const WAIT_MASK: u32 = 15;

pub const PORT_ROLE_SHIFT: u32 = 25;
pub const PORT_WAIT_SHIFT: u32 = 28;
// This bit has this meaning only in a packet advertising an open tile.
pub const OPEN_USEFUL_RETURN: u32 = 1 << 31;

pub const PICKED_UP: u32 = 1;
pub const MOVED: u32 = 2;
pub const REVERSED: u32 = 4;
pub const BLOCKED: u32 = 8;
pub const CONTESTED: u32 = 16;

pub const ROOM_PRESSURE_SHIFT: u32 = 5;
pub const ROOM_PRESSURE_MASK: u32 = 7 << ROOM_PRESSURE_SHIFT;
pub const ROOM_CONTACT: u32 = 1 << 8;
pub const ROOM_READY: u32 = 1 << 9;
pub const ROOM_PORT_SHIFT: u32 = 10;
// Set at initialization and preserved across rounds. Both runs use the same table.
pub const ROOM_DISABLED: u32 = 1 << 14;
pub const ROOM_WAVE: u32 = 1 << 15;
pub const YIELDED_ID_SHIFT: u32 = 16;
pub const ROOM_HOP_SHIFT: u32 = 26;
pub const ROOM_HOP_MASK: u32 = 3 << ROOM_HOP_SHIFT;
pub const ROOM_FORWARD_PORT_SHIFT: u32 = 28;
pub const MAX_ROOM_FORWARDS: u32 = 3;
// Odd pressures choose the tail first. Stronger pressures win the relay.
pub const CONTACT_PRESSURE: [u32; 6] = [0, 7, 5, 2, 4, 6];
// These two fields share the upper bits of the scent/food/age word.
pub const ROOM_INGRESS_SHIFT: u32 = 16;
pub const ROOM_INGRESS_MASK: u32 = 15 << ROOM_INGRESS_SHIFT;
pub const ROOM_NEXT_HOP_SHIFT: u32 = 20;
pub const ROOM_NEXT_HOP_MASK: u32 = 3 << ROOM_NEXT_HOP_SHIFT;
