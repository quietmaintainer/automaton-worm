//! Assemble the autonomous worm rule using the engine's standard opcodes.
//!
//! The four mutable words hold the body segment, scent/food/wait, movement
//! status or ticket, and a clock. The input word is constant after seeding.
//! Each tick reads only these words and the previous tick's four messages.

use crate::formation_protocol::*;
use crate::rules::{Opcode, register};
use crate::rules::{Opcode::*, RULE_VERSION, Row, RuleDefinition, RuleTable};

const INPUT: u32 = register::INPUT as u32;
const INBOX: u32 = register::INBOX as u32;
const BODY: u32 = register::MEMORY as u32;
const FIELD: u32 = BODY + 1;
const ACTION: u32 = BODY + 2;
const CLOCK: u32 = BODY + 3;
const OUTBOX: u32 = register::OUTBOX as u32;
const REPORT: u32 = register::ANSWER as u32;
const ONE: u32 = OUTBOX;
const TWO: u32 = OUTBOX + 1;
const LOW_BITS: u32 = OUTBOX + 2;
const MATCH_PHASE: u32 = OUTBOX + 3;
const ROLE: u32 = 15;
const ID: u32 = 16;
const PHASE: u32 = 17;
const WAIT: u32 = 18;
const RANDOM: u32 = 19;
const REQUEST_PORT: u32 = 20;
const GRANT_PORT: u32 = 21;
const GRANT_WORD: u32 = 22;
const TEST: u32 = 30;
const LITERAL: u32 = 31;

const HEAD: u64 = 1;
const TAIL: u64 = 5;
const GRANT_FIRST: u64 = ASK_FIRST + 1;
const GRANT_SECOND: u64 = ASK_SECOND + 1;
const SEGMENT_KEY: u64 = (ROLE_MASK << PORT_ROLE_SHIFT) | ID_MASK;
const TAIL_BID: u64 = 4 << PORT_ROLE_SHIFT;
// Indexed by (5 - body length, role). A larger rank means that the
// contacted site is nearer the end the neighbor wants freed.
const CONTACT_BY_LENGTH: [u32; 32] = [
    CONTACT_PRESSURE[0],
    CONTACT_PRESSURE[1],
    CONTACT_PRESSURE[2],
    CONTACT_PRESSURE[3],
    CONTACT_PRESSURE[4],
    CONTACT_PRESSURE[5],
    0,
    0,
    0,
    7,
    5,
    4,
    6,
    0,
    0,
    0,
    0,
    7,
    2,
    6,
    0,
    0,
    0,
    0,
    0,
    7,
    6,
    0,
    0,
    0,
    0,
    0,
];

pub(super) fn table() -> RuleTable<u64> {
    let mut p = Program { rows: Vec::new() };
    p.set(ONE, 1);
    p.set(TWO, 2);
    p.set(LOW_BITS, ROLE_MASK);
    p.imm(And, PHASE, CLOCK, PHASE_MASK);
    p.imm(And, ROLE, BODY, ROLE_MASK);
    p.imm(And, ID, BODY, ID_MASK);
    p.imm(ShiftRight, WAIT, FIELD, WAIT_SHIFT);
    p.imm(And, WAIT, WAIT, WAIT_MASK);
    p.imm(ShiftRight, RANDOM, CLOCK, CLOCK_SHIFT);
    p.op(Add, RANDOM, RANDOM, INPUT);
    // The seeded tie breaks use the original 32-bit xorshift.
    for (op, shift) in [(ShiftLeft, 13), (ShiftRight, 17), (ShiftLeft, 5)] {
        p.imm(op, TEST, RANDOM, shift);
        p.op(Xor, RANDOM, RANDOM, TEST);
        if op == ShiftLeft {
            p.imm(And, RANDOM, RANDOM, u64::from(u32::MAX));
        }
    }
    room_pressure(&mut p);
    request(&mut p);
    grant(&mut p);
    relay_result(&mut p);
    schedule_forward(&mut p);
    commit(&mut p);
    update_food_and_scent(&mut p);
    p.imm(Equal, TEST, PHASE, COMMIT);
    p.set(LITERAL, (1 << CLOCK_SHIFT) - COMMIT);
    p.select(TEST, LITERAL, ONE, TEST);
    p.op(Add, CLOCK, CLOCK, TEST);
    p.imm(And, CLOCK, CLOCK, u64::from(u32::MAX));
    broadcast(&mut p);

    let row_count = p.rows.len();
    RuleTable::with_word_size(RuleDefinition {
        version: RULE_VERSION,
        rows: p.rows,
        lookup: CONTACT_BY_LENGTH.into(),
    })
    .unwrap_or_else(|error| panic!("autonomous worm rule has {row_count} rows: {error}"))
}

fn room_pressure(p: &mut Program) {
    const INGRESS: u32 = 23;
    const RANK: u32 = 24;
    const NEARER_HEAD: u32 = 25;
    const NEARER_TAIL: u32 = 26;
    const KEY: u32 = 27;
    const INCOMING: u32 = 28;
    const MATCH: u32 = 29;
    const CONTACT_HOP: u32 = REQUEST_PORT;
    const HOP: u32 = GRANT_PORT;

    p.imm(And, RANK, REPORT, ROOM_READY | ROOM_DISABLED | ROOM_WAVE);
    p.imm(Equal, TEST, PHASE, ROOM_SEND);
    p.select(REPORT, RANK, REPORT, TEST);
    p.imm(And, RANK, REPORT, ROOM_PRESSURE_MASK);
    p.imm(ShiftRight, RANK, RANK, ROOM_PRESSURE_SHIFT);
    p.imm(And, HOP, REPORT, ROOM_HOP_MASK);
    p.imm(ShiftRight, HOP, HOP, ROOM_HOP_SHIFT - 3);
    p.op(Or, RANK, RANK, HOP);
    p.adjacent_keys(NEARER_HEAD, NEARER_TAIL);
    p.copy(INGRESS, 0);
    p.copy(CONTACT_HOP, 0);
    for port in 0..4 {
        let message = INBOX + port;
        // Code zero with a nonzero worm ID is a directed make-room packet.
        p.imm(And, KEY, message, ID_MASK | ROLE_MASK);
        p.imm(And, MATCH, KEY, ROLE_MASK);
        p.op(Equal, MATCH, MATCH, 0);
        p.op(LessThan, TEST, 0, KEY);
        p.op(And, MATCH, MATCH, TEST);
        p.imm(ShiftRight, HOP, message, PORT_WAIT_SHIFT);
        p.imm(And, HOP, HOP, MAX_ROOM_FORWARDS);
        p.select(HOP, HOP, 0, MATCH);
        p.op(LessThan, TEST, CONTACT_HOP, HOP);
        p.select(CONTACT_HOP, HOP, CONTACT_HOP, TEST);
        p.set(LITERAL, 1 << (ROOM_INGRESS_SHIFT + u64::from(port)));
        p.select(MATCH, LITERAL, 0, MATCH);
        p.op(Or, INGRESS, INGRESS, MATCH);

        // The quiet relay phases put the hop in the low bits of the
        // occupied code. Same-body neighbors agree on the highest rank.
        p.imm(And, KEY, message, SEGMENT_KEY);
        p.op(Equal, TEST, KEY, NEARER_HEAD);
        p.op(Equal, MATCH, KEY, NEARER_TAIL);
        p.op(Or, MATCH, MATCH, TEST);
        p.imm(ShiftRight, INCOMING, message, PORT_WAIT_SHIFT);
        p.imm(And, HOP, message, MAX_ROOM_FORWARDS);
        p.imm(ShiftLeft, HOP, HOP, 3);
        p.op(Or, INCOMING, INCOMING, HOP);
        p.select(INCOMING, INCOMING, 0, MATCH);
        p.op(LessThan, TEST, RANK, INCOMING);
        p.select(RANK, INCOMING, RANK, TEST);
    }
    p.imm(Equal, TEST, PHASE, ROOM_RECEIVE);
    p.select(INGRESS, INGRESS, 0, ROLE);
    p.imm(And, KEY, FIELD, WORD_MASK & !ROOM_INGRESS_MASK);
    p.op(Or, KEY, KEY, INGRESS);
    p.select(FIELD, KEY, FIELD, TEST);
    p.select(INGRESS, INGRESS, 0, TEST);
    p.flag(ROOM_CONTACT, INGRESS);

    p.imm(And, INCOMING, BODY, BODY_LENGTH_MASK);
    p.imm(ShiftRight, INCOMING, INCOMING, BODY_LENGTH_SHIFT - 3);
    p.op(Or, INCOMING, INCOMING, ROLE);
    p.row(
        Lookup,
        INCOMING,
        INCOMING,
        0,
        CONTACT_BY_LENGTH.len() as u32,
    );
    // A request that has traveled farther wins before contact proximity.
    p.imm(ShiftLeft, CONTACT_HOP, CONTACT_HOP, 3);
    p.op(Or, INCOMING, INCOMING, CONTACT_HOP);
    p.select(RANK, INCOMING, RANK, INGRESS);
    // The send tick clears last round's preference. Start receiving on the
    // next tick so old broadcasts cannot refill the cleared preference.
    p.op(LessThan, MATCH, 0, ROLE);
    p.imm(LessThan, TEST, PHASE, ASK_FIRST);
    p.op(And, TEST, TEST, MATCH);
    p.op(LessThan, MATCH, 0, PHASE);
    p.op(And, TEST, TEST, MATCH);
    p.imm(And, INCOMING, RANK, ROLE_MASK);
    p.imm(ShiftLeft, INCOMING, INCOMING, ROOM_PRESSURE_SHIFT);
    p.imm(And, HOP, RANK, MAX_ROOM_FORWARDS << 3);
    p.imm(ShiftLeft, HOP, HOP, ROOM_HOP_SHIFT - 3);
    p.op(Or, RANK, INCOMING, HOP);
    p.imm(
        And,
        MATCH,
        REPORT,
        WORD_MASK & !(ROOM_PRESSURE_MASK | ROOM_HOP_MASK),
    );
    p.op(Or, RANK, RANK, MATCH);
    p.select(REPORT, RANK, REPORT, TEST);
}

fn request(p: &mut Program) {
    const BEST: u32 = 23;
    const SCORE: u32 = 24;
    const HEADING: u32 = 25;
    const TIE: u32 = 26;
    const ELIGIBLE: u32 = 27;
    const STRAIGHT_BONUS: u32 = 28;
    const RETURN_PORTS: u32 = 29;
    const RETURN_FRESHNESS: u32 = GRANT_PORT;
    const CLEAR_HEAD: u32 = GRANT_WORD;
    const CAN_RETURN: u32 = MATCH_PHASE;

    p.imm(ShiftRight, HEADING, BODY, HEADING_SHIFT);
    p.imm(And, HEADING, HEADING, 3);
    p.imm(ShiftRight, RETURN_PORTS, BODY, RETURN_PORT_SHIFT);
    p.imm(And, RETURN_FRESHNESS, BODY, RETURN_FRESH);
    p.imm(And, CLEAR_HEAD, REPORT, ROOM_PRESSURE_MASK);
    p.imm(Equal, CLEAR_HEAD, CLEAR_HEAD, ROOM_PRESSURE_MASK);
    p.imm(And, TIE, RANDOM, 3);
    p.imm(And, STRAIGHT_BONUS, RANDOM, 4);
    p.imm(Equal, TEST, ROLE, HEAD);
    p.select(STRAIGHT_BONUS, STRAIGHT_BONUS, 0, TEST);
    p.copy(BEST, 0);
    p.copy(REQUEST_PORT, 0);
    for port in 0..4 {
        let message = INBOX + port;
        p.imm(And, SCORE, message, SCENT_MASK << HEADING_SHIFT);
        p.imm(Xor, TEST, TIE, u64::from(port));
        p.op(Add, SCORE, SCORE, TEST);
        p.imm(Equal, TEST, HEADING, u64::from(port));
        // The seed breaks equal scent ties. A straight step often wins those ties.
        p.select(TEST, STRAIGHT_BONUS, 0, TEST);
        p.op(Add, SCORE, SCORE, TEST);
        p.imm(Add, SCORE, SCORE, 2);

        // Returning through the old tail waits a round and needs an exit,
        // food, or a current request that would free the head's tile.
        p.imm(And, CAN_RETURN, message, OPEN_USEFUL_RETURN);
        p.op(Or, CAN_RETURN, CAN_RETURN, CLEAR_HEAD);
        p.select(CAN_RETURN, 0, CAN_RETURN, RETURN_FRESHNESS);
        p.imm(And, TEST, RETURN_PORTS, 1 << port);
        p.select(CAN_RETURN, CAN_RETURN, ONE, TEST);
        p.select(SCORE, ONE, SCORE, TEST);
        p.select(SCORE, SCORE, 0, CAN_RETURN);

        p.imm(And, TEST, message, ROLE_MASK);
        p.imm(Equal, TEST, TEST, OPEN);
        p.select(SCORE, SCORE, 0, TEST);
        p.op(LessThan, TEST, BEST, SCORE);
        p.select(BEST, SCORE, BEST, TEST);
        p.set(LITERAL, u64::from(port + 1));
        p.select(REQUEST_PORT, LITERAL, REQUEST_PORT, TEST);
    }
    p.leader(HEADING);
    p.role_at(ELIGIBLE, ASK_FIRST, HEADING);
    p.tail_role(TEST);
    p.imm(Xor, TEST, TEST, HEAD);
    p.op(Xor, HEADING, HEADING, TEST);
    p.role_at(TEST, ASK_SECOND, HEADING);
    p.imm(Equal, SCORE, ACTION, TRY_OTHER);
    p.op(And, TEST, TEST, SCORE);
    p.imm(And, SCORE, FIELD, REVERSALS_DISABLED);
    p.select(TEST, 0, TEST, SCORE);
    p.op(Or, ELIGIBLE, ELIGIBLE, TEST);
    p.select(REQUEST_PORT, REQUEST_PORT, 0, ELIGIBLE);
}

fn grant(p: &mut Program) {
    const BEST: u32 = 23;
    const BID: u32 = 24;
    const BIDS: u32 = 25;
    const ELIGIBLE: u32 = 26;
    const RESERVATION: u32 = 27;
    const LENGTH_BITS: u32 = 28;

    p.copy(BEST, 0);
    p.copy(BIDS, 0);
    p.copy(LENGTH_BITS, 0);
    p.copy(GRANT_PORT, 0);
    for port in 0..4 {
        let message = INBOX + port;
        p.imm(And, TEST, message, ROLE_MASK);
        p.imm(Equal, TEST, TEST, REQUEST);
        p.op(Add, BIDS, BIDS, TEST);
        p.imm(
            And,
            BID,
            message,
            (WAIT_MASK << PORT_WAIT_SHIFT) | ID_MASK | TAIL_BID,
        );
        p.select(BID, BID, 0, TEST);
        p.op(LessThan, TEST, BEST, BID);
        p.select(BEST, BID, BEST, TEST);
        p.imm(And, RESERVATION, message, REQUEST_LENGTH_MASK);
        p.select(LENGTH_BITS, RESERVATION, LENGTH_BITS, TEST);
        p.set(LITERAL, u64::from(port + 1));
        p.select(GRANT_PORT, LITERAL, GRANT_PORT, TEST);
    }
    p.imm(Equal, ELIGIBLE, PHASE, GRANT_FIRST);
    p.imm(Equal, TEST, PHASE, GRANT_SECOND);
    p.op(Or, ELIGIBLE, ELIGIBLE, TEST);
    p.op(Or, TEST, BODY, ACTION);
    p.op(Equal, TEST, TEST, 0);
    p.op(And, ELIGIBLE, ELIGIBLE, TEST);
    p.select(GRANT_PORT, GRANT_PORT, 0, ELIGIBLE);
    p.set(LITERAL, 1);
    p.op(LessThan, TEST, LITERAL, BIDS);
    p.op(And, TEST, TEST, ELIGIBLE);
    p.flag(CONTESTED, TEST);

    p.imm(And, GRANT_WORD, BEST, ID_MASK);
    p.imm(Or, GRANT_WORD, GRANT_WORD, TICKET);
    p.imm(Subtract, RESERVATION, GRANT_PORT, 1);
    p.imm(Xor, RESERVATION, RESERVATION, 1); // heading from the sender to this cell
    p.imm(ShiftLeft, RESERVATION, RESERVATION, HEADING_SHIFT);
    p.imm(And, TEST, BEST, TAIL_BID);
    p.set(LITERAL, REVERSED_TICKET);
    p.select(TEST, LITERAL, 0, TEST);
    p.op(Or, RESERVATION, RESERVATION, TEST);
    p.imm(And, BEST, BEST, ID_MASK);
    p.op(Or, RESERVATION, RESERVATION, BEST);
    p.imm(Or, RESERVATION, RESERVATION, HEAD);
    p.imm(
        ShiftRight,
        LENGTH_BITS,
        LENGTH_BITS,
        REQUEST_LENGTH_SHIFT - BODY_LENGTH_SHIFT,
    );
    p.op(Or, RESERVATION, RESERVATION, LENGTH_BITS);
    p.select(ACTION, RESERVATION, ACTION, GRANT_PORT);
}

fn relay_result(p: &mut Program) {
    const NEARER_HEAD: u32 = 23;
    const NEARER_TAIL: u32 = 24;
    const EXPECT_TICKET: u32 = 25;
    const WAVE: u32 = 26;
    const ACCEPTED: u32 = 27;
    const CODE: u32 = 28;
    const MATCH: u32 = 29;
    const NONE: u64 = 8;

    p.adjacent_keys(NEARER_HEAD, NEARER_TAIL);
    p.imm(Or, NEARER_HEAD, NEARER_HEAD, FORWARD);
    p.imm(Or, NEARER_TAIL, NEARER_TAIL, REVERSE);
    p.imm(Or, EXPECT_TICKET, ID, TICKET);
    p.copy(ACCEPTED, 0);
    p.set(WAVE, NONE);
    for port in 0..4 {
        let message = INBOX + port;
        p.imm(And, MATCH, message, ID_MASK | ROLE_MASK);
        p.op(Equal, MATCH, MATCH, EXPECT_TICKET);
        p.op(Or, ACCEPTED, ACCEPTED, MATCH);

        // Failure can travel through the body in either direction. A
        // successful move may relay only away from its leading end.
        p.imm(And, MATCH, message, SEGMENT_KEY | FORWARD);
        p.op(Equal, MATCH, MATCH, NEARER_HEAD);
        p.imm(And, MATCH_PHASE, message, ROLE_MASK);
        p.set(LITERAL, NONE);
        p.select(CODE, MATCH_PHASE, LITERAL, MATCH);
        p.imm(And, MATCH, message, SEGMENT_KEY | REVERSE);
        p.op(Equal, MATCH, MATCH, NEARER_TAIL);
        p.select(CODE, MATCH_PHASE, CODE, MATCH);
        p.op(Min, WAVE, WAVE, CODE);
    }
    p.tail_role(TEST);
    p.op(Equal, CODE, ROLE, TEST);
    p.imm(Add, CODE, CODE, FORWARD);
    p.leader(NEARER_HEAD);
    p.role_at(TEST, GRANT_FIRST + 1, NEARER_HEAD);
    p.set(LITERAL, TRY_OTHER);
    p.select(MATCH, CODE, LITERAL, ACCEPTED);
    p.select(WAVE, MATCH, WAVE, TEST);
    p.tail_role(NEARER_TAIL);
    p.imm(Xor, NEARER_TAIL, NEARER_TAIL, HEAD);
    p.op(Xor, NEARER_TAIL, NEARER_TAIL, NEARER_HEAD);
    p.role_at(TEST, GRANT_SECOND + 1, NEARER_TAIL);
    p.op(And, TEST, TEST, ACCEPTED);
    p.select(WAVE, CODE, WAVE, TEST);

    p.imm(LessThan, TEST, WAVE, NONE);
    p.op(LessThan, MATCH, 0, ROLE);
    p.op(And, TEST, TEST, MATCH);
    p.imm(Equal, MATCH, ACTION, TRY_OTHER);
    p.op(Equal, ACCEPTED, ACTION, 0);
    p.op(Or, MATCH, MATCH, ACCEPTED);
    p.op(And, TEST, TEST, MATCH);
    // Occupied packets borrow result codes during the earlier room relay.
    p.imm(LessThan, MATCH, PHASE, GRANT_FIRST + 1);
    p.select(TEST, 0, TEST, MATCH);
    p.select(ACTION, WAVE, ACTION, TEST);
}

fn schedule_forward(p: &mut Program) {
    const HOP: u32 = 23;
    const LEADER: u32 = 24;
    const SEND: u32 = 25;

    // A worm that could not move passes the request from the end that
    // tried to make the requested space. The hop advances once per round.
    p.imm(And, HOP, REPORT, ROOM_HOP_MASK);
    p.imm(ShiftRight, HOP, HOP, ROOM_HOP_SHIFT);
    p.imm(LessThan, SEND, HOP, MAX_ROOM_FORWARDS);
    p.imm(And, TEST, REPORT, ROOM_PRESSURE_MASK);
    p.select(SEND, SEND, 0, TEST);
    p.imm(And, TEST, REPORT, ROOM_WAVE);
    p.select(SEND, SEND, 0, TEST);
    p.leader(LEADER);
    p.op(Equal, TEST, LEADER, ROLE);
    p.op(And, SEND, SEND, TEST);
    p.imm(Equal, TEST, ACTION, TRY_OTHER);
    p.op(And, SEND, SEND, TEST);
    p.imm(Add, HOP, HOP, 1);
    p.imm(ShiftLeft, HOP, HOP, ROOM_NEXT_HOP_SHIFT);
    p.select(HOP, HOP, 0, SEND);
    p.imm(And, LEADER, FIELD, WORD_MASK & !ROOM_NEXT_HOP_MASK);
    p.op(Or, HOP, HOP, LEADER);
    p.imm(Equal, TEST, PHASE, COMMIT);
    p.select(FIELD, HOP, FIELD, TEST);
}

fn commit(p: &mut Program) {
    const DONE: u32 = 23;
    const ARRIVED: u32 = 24;
    const BLOCKED_HEAD: u32 = 25;
    const SLID: u32 = 26;
    const SEGMENT: u32 = 27;
    const NEW_ROLE: u32 = 28;
    const BACKWARD: u32 = 29;

    p.imm(Equal, DONE, PHASE, COMMIT);
    p.op(Equal, ARRIVED, BODY, 0);
    p.op(LessThan, TEST, 0, ACTION);
    p.op(And, ARRIVED, ARRIVED, TEST);
    p.op(And, ARRIVED, ARRIVED, DONE);
    p.flag(MOVED, ARRIVED);
    p.imm(And, TEST, ACTION, REVERSED_TICKET);
    p.select(TEST, ARRIVED, 0, TEST);
    p.flag(REVERSED, TEST);
    p.imm(Equal, SLID, ACTION, FORWARD);
    p.imm(Equal, BACKWARD, ACTION, REVERSE);
    p.op(Or, TEST, SLID, BACKWARD);
    p.imm(Equal, BLOCKED_HEAD, ROLE, HEAD);
    p.select(BLOCKED_HEAD, 0, BLOCKED_HEAD, TEST);
    p.op(And, BLOCKED_HEAD, BLOCKED_HEAD, DONE);
    p.flag(BLOCKED, BLOCKED_HEAD);

    p.imm(And, TEST, REPORT, WORD_MASK & !ROOM_READY);
    p.select(REPORT, TEST, REPORT, DONE);
    p.tail_role(TEST);
    p.op(Equal, TEST, ROLE, TEST);
    p.imm(Equal, SEGMENT, ROLE, HEAD);
    p.op(Or, TEST, TEST, SEGMENT);
    p.imm(Equal, SEGMENT, ACTION, TRY_OTHER);
    p.op(And, TEST, TEST, SEGMENT);
    p.op(And, TEST, TEST, DONE);
    p.flag(ROOM_READY, TEST);

    p.tail_role(SEGMENT);
    p.imm(Add, NEW_ROLE, ROLE, 1);
    p.imm(Add, TEST, SEGMENT, 2);
    p.op(Subtract, TEST, TEST, ROLE);
    p.select(NEW_ROLE, TEST, NEW_ROLE, BACKWARD);
    p.op(Or, TEST, ID, NEW_ROLE);
    p.imm(And, MATCH_PHASE, BODY, BODY_LENGTH_MASK);
    p.op(Or, TEST, TEST, MATCH_PHASE);
    p.op(LessThan, MATCH_PHASE, SEGMENT, NEW_ROLE);
    p.select(TEST, 0, TEST, MATCH_PHASE);
    p.op(Equal, NEW_ROLE, NEW_ROLE, SEGMENT);
    p.imm(ShiftLeft, MATCH_PHASE, SEGMENT, PORT_ROLE_SHIFT);
    p.copy(SEGMENT, TEST);
    p.op(Or, SLID, SLID, BACKWARD);

    // The new tail can identify the departing segment in its inbox.
    // On a wait, retain that port and clear the one-round delay.
    p.op(And, NEW_ROLE, NEW_ROLE, SLID);
    p.set(LITERAL, HEAD << PORT_ROLE_SHIFT);
    p.select(TEST, LITERAL, MATCH_PHASE, BACKWARD);
    p.op(Or, TEST, TEST, ID);
    p.copy(BACKWARD, 0);
    for port in 0..4 {
        p.imm(And, MATCH_PHASE, INBOX + port, SEGMENT_KEY);
        p.op(Equal, MATCH_PHASE, MATCH_PHASE, TEST);
        p.set(
            LITERAL,
            (1 << (RETURN_PORT_SHIFT + u64::from(port))) | RETURN_FRESH,
        );
        p.select(MATCH_PHASE, LITERAL, 0, MATCH_PHASE);
        p.op(Or, BACKWARD, BACKWARD, MATCH_PHASE);
    }
    p.select(BACKWARD, BACKWARD, 0, NEW_ROLE);
    p.op(Or, SEGMENT, SEGMENT, BACKWARD);
    p.imm(And, TEST, BODY, WORD_MASK & !RETURN_FRESH);
    p.select(SEGMENT, SEGMENT, TEST, SLID);
    p.imm(And, TEST, ACTION, WORD_MASK & !REVERSED_TICKET);
    p.select(SEGMENT, TEST, SEGMENT, ARRIVED);
    p.select(BODY, SEGMENT, BODY, DONE);

    // Credit the previous occupant only when its contacted tile is free.
    p.op(Equal, TEST, BODY, 0);
    p.op(And, TEST, TEST, DONE);
    p.imm(And, SEGMENT, REPORT, ROOM_CONTACT);
    p.select(TEST, TEST, 0, SEGMENT);
    p.imm(ShiftLeft, SEGMENT, ID, YIELDED_ID_SHIFT - 3);
    p.select(SEGMENT, SEGMENT, 0, TEST);
    p.op(Or, REPORT, REPORT, SEGMENT);

    p.imm(Add, TEST, WAIT, 1);
    p.imm(Min, TEST, TEST, WAIT_MASK);
    p.select(TEST, TEST, 0, BLOCKED_HEAD);
    p.select(WAIT, TEST, WAIT, DONE);
    p.select(ACTION, 0, ACTION, DONE);
    p.imm(And, ROLE, BODY, ROLE_MASK);
    p.imm(And, ID, BODY, ID_MASK);
}

fn update_food_and_scent(p: &mut Program) {
    const PARTNER: u32 = 23;
    const AXIS: u32 = 24;
    const FOOD: u32 = 25;
    const OTHER: u32 = 26;
    const SCENT: u32 = 27;
    const PICKUP: u32 = 28;
    const READY: u32 = 29;

    // A different set of disjoint edges is matched each tick. Both sides of
    // an edge see the same token transfer, so two seeds never merge or copy.
    p.imm(ShiftRight, AXIS, PHASE, 1);
    p.imm(And, AXIS, AXIS, 1);
    p.op(ShiftRight, PARTNER, INPUT, AXIS);
    p.op(Xor, PARTNER, PARTNER, PHASE);
    p.imm(Xor, PARTNER, PARTNER, 1);
    p.imm(And, PARTNER, PARTNER, 1);
    p.imm(ShiftLeft, AXIS, AXIS, 1);
    p.op(Or, PARTNER, PARTNER, AXIS);
    p.copy(OTHER, 0);
    for port in 0..4 {
        p.imm(Equal, TEST, PARTNER, u64::from(port));
        p.select(OTHER, INBOX + port, OTHER, TEST);
    }
    p.op(LessThan, READY, 0, OTHER); // A missing edge supplies zero.
    p.imm(ShiftRight, OTHER, OTHER, HEADING_SHIFT + FOOD_SHIFT);
    p.imm(And, OTHER, OTHER, FOOD_MASK);
    p.imm(ShiftRight, FOOD, FIELD, FOOD_SHIFT);
    p.imm(And, FOOD, FOOD, FOOD_MASK);

    p.imm(Equal, PICKUP, FOOD, RIPE);
    p.imm(And, TEST, REPORT, MOVED);
    p.imm(Equal, AXIS, PHASE, COMMIT);
    p.select(TEST, AXIS, 0, TEST);
    p.op(And, PICKUP, PICKUP, TEST);
    p.flag(PICKED_UP, PICKUP);

    p.op(Equal, TEST, OTHER, 0);
    p.op(And, READY, READY, TEST);
    p.set(LITERAL, RIPE);
    p.op(LessThan, TEST, LITERAL, FOOD);
    p.op(And, READY, READY, TEST);
    p.op(Equal, TEST, FOOD, 0);
    p.op(LessThan, PARTNER, LITERAL, OTHER);
    p.op(And, TEST, TEST, PARTNER);
    p.select(FOOD, OTHER, FOOD, TEST);
    p.select(FOOD, 0, FOOD, READY);

    p.imm(And, TEST, RANDOM, 15);
    p.imm(Add, TEST, TEST, 14);
    p.select(FOOD, TEST, FOOD, PICKUP);
    p.set(LITERAL, 2);
    p.op(LessThan, TEST, LITERAL, FOOD);
    p.op(Subtract, FOOD, FOOD, TEST);
    p.op(Equal, TEST, FOOD, LITERAL);
    p.op(Equal, READY, BODY, 0);
    p.op(And, READY, READY, TEST);
    p.set(LITERAL, RIPE);
    p.select(FOOD, LITERAL, FOOD, READY);

    p.imm(And, SCENT, FIELD, SCENT_MASK);
    for port in 0..4 {
        p.imm(ShiftRight, OTHER, INBOX + port, HEADING_SHIFT);
        p.imm(And, OTHER, OTHER, SCENT_MASK);
        p.op(LessThan, TEST, SCENT, OTHER);
        p.select(SCENT, OTHER, SCENT, TEST);
    }
    p.op(LessThan, TEST, 0, SCENT);
    p.op(Subtract, SCENT, SCENT, TEST);
    p.imm(Equal, TEST, FOOD, RIPE);
    p.set(LITERAL, FOOD_SCENT);
    p.select(SCENT, LITERAL, SCENT, TEST);
    p.imm(
        And,
        PARTNER,
        FIELD,
        ROOM_INGRESS_MASK | ROOM_NEXT_HOP_MASK | REVERSALS_DISABLED,
    );
    p.imm(ShiftLeft, FIELD, FOOD, FOOD_SHIFT);
    p.op(Or, FIELD, FIELD, SCENT);
    p.imm(ShiftLeft, TEST, WAIT, WAIT_SHIFT);
    p.op(Or, FIELD, FIELD, TEST);
    p.op(Or, FIELD, FIELD, PARTNER);
}

fn broadcast(p: &mut Program) {
    const COMMON: u32 = 23;
    const IDLE: u32 = 24;
    const REQUEST_WORD: u32 = 25;
    const MESSAGE: u32 = 26;

    p.imm(And, COMMON, FIELD, SCENT_MASK | (FOOD_MASK << FOOD_SHIFT));
    p.imm(ShiftLeft, COMMON, COMMON, HEADING_SHIFT);
    p.imm(ShiftLeft, TEST, ROLE, PORT_ROLE_SHIFT);
    p.op(Or, COMMON, COMMON, TEST);
    p.imm(And, TEST, REPORT, ROOM_PRESSURE_MASK);
    p.imm(ShiftLeft, TEST, TEST, PORT_WAIT_SHIFT - ROOM_PRESSURE_SHIFT);
    p.imm(ShiftLeft, MATCH_PHASE, WAIT, PORT_WAIT_SHIFT);
    p.select(TEST, MATCH_PHASE, TEST, REQUEST_PORT);
    p.op(Or, COMMON, COMMON, TEST);

    p.imm(Or, REQUEST_WORD, ID, REQUEST);
    p.imm(And, TEST, BODY, BODY_LENGTH_MASK);
    p.imm(
        ShiftLeft,
        TEST,
        TEST,
        REQUEST_LENGTH_SHIFT - BODY_LENGTH_SHIFT,
    );
    p.op(Or, REQUEST_WORD, REQUEST_WORD, TEST);
    // The stock priority bit identifies a tail even for a short body.
    p.op(LessThan, TEST, ONE, ROLE);
    p.set(LITERAL, TAIL_BID);
    p.select(TEST, LITERAL, 0, TEST);
    p.op(Or, REQUEST_WORD, REQUEST_WORD, TEST);
    p.imm(And, MESSAGE, FIELD, FOOD_MASK << FOOD_SHIFT);
    p.imm(Equal, MESSAGE, MESSAGE, RIPE << FOOD_SHIFT);
    for port in 0..4 {
        p.imm(And, TEST, INBOX + port, ROLE_MASK);
        p.imm(Equal, TEST, TEST, OPEN);
        p.op(Or, MESSAGE, MESSAGE, TEST);
    }
    p.set(LITERAL, OPEN | OPEN_USEFUL_RETURN);
    p.select(MESSAGE, LITERAL, ONE, MESSAGE);
    p.op(Or, TEST, BODY, ACTION);
    p.op(Equal, TEST, TEST, 0);
    p.imm(Or, IDLE, ID, OCCUPIED);
    p.imm(And, MATCH_PHASE, REPORT, ROOM_HOP_MASK);
    p.imm(ShiftRight, MATCH_PHASE, MATCH_PHASE, ROOM_HOP_SHIFT);
    p.imm(LessThan, LITERAL, PHASE, ASK_FIRST);
    p.select(MATCH_PHASE, MATCH_PHASE, 0, LITERAL);
    p.op(Or, IDLE, IDLE, MATCH_PHASE);
    p.select(IDLE, MESSAGE, IDLE, TEST);
    p.select(TEST, ACTION, 0, ROLE);
    p.op(Or, MESSAGE, ID, ACTION);
    p.select(IDLE, MESSAGE, IDLE, TEST);

    // Hold the packets in scratch until all four are ready. The output
    // registers have served as literal and scratch registers up to here.
    // The waiting age has already been packed into the common word.
    p.imm(And, WAIT, REPORT, ROOM_READY | ROOM_DISABLED);
    p.imm(Equal, WAIT, WAIT, ROOM_READY);
    p.imm(Equal, TEST, PHASE, ROOM_SEND);
    p.select(WAIT, WAIT, 0, TEST);
    p.imm(And, MATCH_PHASE, FIELD, ROOM_NEXT_HOP_MASK);
    p.imm(
        ShiftLeft,
        MATCH_PHASE,
        MATCH_PHASE,
        PORT_WAIT_SHIFT - ROOM_NEXT_HOP_SHIFT,
    );
    const PACKETS: [u32; 4] = [27, 28, 29, RANDOM];
    for (port, packet) in PACKETS.into_iter().enumerate() {
        let port = port as u32;
        p.imm(Equal, TEST, REQUEST_PORT, u64::from(port + 1));
        p.select(MESSAGE, REQUEST_WORD, IDLE, TEST);
        p.imm(Equal, TEST, GRANT_PORT, u64::from(port + 1));
        p.select(MESSAGE, GRANT_WORD, MESSAGE, TEST);
        p.imm(And, packet, INBOX + port, ID_MASK);
        p.op(Equal, TEST, packet, ID);
        p.select(TEST, 0, packet, TEST);
        p.select(TEST, TEST, 0, WAIT);
        p.imm(
            And,
            ROLE,
            FIELD,
            1 << (ROOM_INGRESS_SHIFT + u64::from(port)),
        );
        p.select(ROLE, ROLE, 0, MATCH_PHASE);
        p.select(TEST, 0, TEST, ROLE);
        p.select(MESSAGE, ID, MESSAGE, TEST);
        p.select(packet, MATCH_PHASE, 0, TEST);
        p.select(ROLE, 0, TEST, MATCH_PHASE);
        p.flag(1 << (ROOM_PORT_SHIFT + u64::from(port)), ROLE);
        p.select(ROLE, TEST, 0, MATCH_PHASE);
        p.flag(1 << (ROOM_FORWARD_PORT_SHIFT + u64::from(port)), ROLE);
        p.op(Or, packet, packet, MESSAGE);
        p.op(Or, packet, packet, COMMON);
    }
    for (port, packet) in PACKETS.into_iter().enumerate() {
        p.copy(OUTBOX + port as u32, packet);
    }
}

struct Program {
    rows: Vec<Row<u64>>,
}

impl Program {
    fn row(&mut self, op: Opcode, dst: u32, a: u32, b: u32, c: u32) {
        self.rows.push(Row {
            op,
            dst,
            a: u64::from(a),
            b,
            c,
        });
    }

    fn op(&mut self, op: Opcode, dst: u32, a: u32, b: u32) {
        self.row(op, dst, a, b, 0);
    }

    fn set(&mut self, dst: u32, value: u64) {
        self.rows.push(Row {
            op: Constant,
            dst,
            a: value,
            b: 0,
            c: 0,
        });
    }

    fn copy(&mut self, dst: u32, source: u32) {
        self.op(Copy, dst, source, 0);
    }

    fn imm(&mut self, op: Opcode, dst: u32, a: u32, value: u64) {
        let operand = match value {
            0 => 0,
            1 => ONE,
            2 => TWO,
            ROLE_MASK => LOW_BITS,
            _ => {
                self.set(LITERAL, value);
                LITERAL
            }
        };
        self.op(op, dst, a, operand);
    }

    fn select(&mut self, dst: u32, yes: u32, no: u32, condition: u32) {
        self.row(Select, dst, yes, no, condition);
    }

    fn role_at(&mut self, dst: u32, phase: u64, role_register: u32) {
        self.imm(Equal, MATCH_PHASE, PHASE, phase);
        self.op(Equal, dst, ROLE, role_register);
        self.op(And, dst, dst, MATCH_PHASE);
    }

    fn adjacent_keys(&mut self, nearer_head: u32, nearer_tail: u32) {
        self.imm(Subtract, nearer_head, ROLE, 1);
        self.imm(ShiftLeft, nearer_head, nearer_head, PORT_ROLE_SHIFT);
        self.op(Or, nearer_head, nearer_head, ID);
        self.imm(Add, nearer_tail, ROLE, 1);
        self.imm(ShiftLeft, nearer_tail, nearer_tail, PORT_ROLE_SHIFT);
        self.op(Or, nearer_tail, nearer_tail, ID);
    }

    fn leader(&mut self, dst: u32) {
        self.imm(ShiftRight, dst, REPORT, ROOM_PRESSURE_SHIFT);
        self.imm(And, dst, dst, 1);
        self.imm(And, TEST, FIELD, REVERSALS_DISABLED);
        self.select(dst, 0, dst, TEST);
        self.tail_role(TEST);
        self.select(dst, TEST, ONE, dst);
    }

    fn tail_role(&mut self, dst: u32) {
        // The body length is the top field of every occupied body word.
        self.imm(ShiftRight, dst, BODY, BODY_LENGTH_SHIFT);
        self.set(LITERAL, TAIL);
        self.op(Subtract, dst, LITERAL, dst);
    }

    fn flag(&mut self, value: u64, condition: u32) {
        self.set(LITERAL, value);
        self.select(LITERAL, LITERAL, 0, condition);
        self.op(Or, REPORT, REPORT, LITERAL);
    }
}
