//! Direct driver tests for #509: no scheduler-order assumptions.
use super::*;
use crate::primitives::{Channel, Selection};
use std::cell::RefCell;
use std::rc::Rc;

const PROGRAM: &str = r#"sparrow 18
module "selection-509" "1.0.0"
type t0 int
type t1 object obj0
type t2 array t1
type t3 object obj1
type t4 object obj2
object obj0 "Receiving<int>" {}
object obj1 "SelectedReceiving<int>" { field "receivers" t2 }
object obj2 "SelectedItem<int>" { field "index" t0 field "payload" t0 }
object obj3 "ChannelClosed" {}
object obj4 "ConcurrentChannelSelection" {}
function f0 "select" async intrinsic "okkama.channel.SelectedReceiving.item" [obj2, obj3, obj4] [] {
    registers [t3]
    params [r0]
    result t4
}
function f1 "send" async intrinsic "okkama.channel.Sending.send" [obj3] [] {
    registers [t1, t0]
    params [r0, r1]
    result t1
}
function f2 "receive" async intrinsic "okkama.channel.Receiving.item" [obj3] [] {
    registers [t1]
    params [r0]
    result t0
}
"#;

fn runtime() -> Box<Runtime> {
    let program = okkama_sparrow::assemble(PROGRAM).unwrap();
    Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap())
}

fn endpoint(r: &mut Runtime, capacity: usize) -> (Word, Rc<RefCell<Channel>>) {
    let channel = Rc::new(RefCell::new(Channel {
        capacity, closed: false, repr: None, messages: VecDeque::new(),
        senders: VecDeque::new(), receivers: VecDeque::new(), packet: UNBOUND, holders: 0,
    }));
    let object = r.heap.object_new(0, &[]);
    r.state_new(object, State::Channel(channel.clone()));
    (object, channel)
}

fn selector(r: &mut Runtime, endpoints: &[Word]) -> Word {
    let array = r.heap.allocate_array(Repr::Ref, endpoints);
    let object = r.heap.object_new(1, &[array]);
    let channels: Vec<_> = endpoints.iter().map(|&e| r.channel_of(e).unwrap()).collect();
    r.state_new(object, State::Selection(Selection { channels: channels.into(), ..Default::default() }));
    object
}

fn frame(r: &mut Runtime, function: usize, args: &[Word]) -> Word {
    let frame = r.future_new(function, args);
    tasks::set(frame, tasks::STATE, tasks::RUNNING);
    frame
}

fn run(r: &mut Runtime, f: Word) -> bool {
    r.ready.retain(|&queued| queued != f);
    drive(r, std::ptr::null_mut(), f)
}

fn number(r: &Runtime, value: Word) -> i64 { r.small_or_big(value).unwrap() }

fn outcome(r: &Runtime, f: Word) -> (i64, i64) {
    assert_eq!(tasks::get(f, tasks::STATE), DONE);
    let item = tasks::get(f, RESULT);
    (number(r, r.heap.field(item, 0)), number(r, r.heap.field(item, 1)))
}

#[test]
fn selection_buffer_refills_only_winning_channel() {
    let mut r = runtime();
    let (a, ca) = endpoint(&mut r, 1);
    let (b, cb) = endpoint(&mut r, 1);
    let one = r.heap.int(1);
    let two = r.heap.int(2);
    ca.borrow_mut().messages.push_back(one);
    cb.borrow_mut().messages.push_back(two);
    let sender = frame(&mut r, 1, &[a, two]);
    assert!(run(&mut r, sender));
    let s = selector(&mut r, &[a, b]);
    let f = frame(&mut r, 0, &[s]);
    assert!(!run(&mut r, f));
    assert_eq!(outcome(&r, f), (0, 1));
    assert_eq!(ca.borrow().messages.iter().copied().collect::<Vec<_>>(), vec![two]);
    assert_eq!(cb.borrow().messages.iter().copied().collect::<Vec<_>>(), vec![two]);
    assert_eq!(resume(sender), COMPLETED);
}

#[test]
fn selection_rendezvous_commits_one_sender_and_removes_losers_eagerly() {
    let mut r = runtime();
    let (a, ca) = endpoint(&mut r, 0);
    let (b, cb) = endpoint(&mut r, 0);
    let s = selector(&mut r, &[a, b]);
    let f = frame(&mut r, 0, &[s]);
    assert!(run(&mut r, f));
    let value = r.heap.int(42);
    let sender = frame(&mut r, 1, &[b, value]);
    assert!(!run(&mut r, sender));
    assert!(!Runtime::parked(f));
    assert!(ca.borrow().receivers.is_empty());
    assert!(cb.borrow().receivers.is_empty());
    r.cancel(std::ptr::null_mut(), f);
    assert!(!run(&mut r, f));
    assert_eq!(outcome(&r, f), (1, 42));
    let losing_sender = frame(&mut r, 1, &[a, value]);
    assert!(run(&mut r, losing_sender));
    assert_eq!(ca.borrow().senders.len(), 1);
    let legitimate = frame(&mut r, 2, &[a]);
    assert!(!run(&mut r, legitimate));
    assert_eq!(resume(losing_sender), COMPLETED);
}

#[test]
fn selection_cancel_before_commit_skips_waiter_and_preserves_sender() {
    let mut r = runtime();
    let (a, ca) = endpoint(&mut r, 0);
    let (b, cb) = endpoint(&mut r, 0);
    let s = selector(&mut r, &[a, b]);
    let f = frame(&mut r, 0, &[s]);
    assert!(run(&mut r, f));
    r.cancel(std::ptr::null_mut(), f);
    r.cancel(std::ptr::null_mut(), f);
    let value = r.heap.int(5);
    let sender = frame(&mut r, 1, &[b, value]);
    assert!(run(&mut r, sender));
    assert!(!run(&mut r, f));
    assert_eq!(tasks::get(f, tasks::STATE), FAILURE_STATE);
    assert!(ca.borrow().receivers.is_empty());
    assert!(cb.borrow().receivers.is_empty());
    assert!(Runtime::parked(sender));
    let again = frame(&mut r, 0, &[s]);
    assert!(!run(&mut r, again));
    assert_eq!(outcome(&r, again), (1, 5));
}

#[test]
fn selection_close_one_keeps_parked_all_closed_refuses_and_cleans() {
    let mut r = runtime();
    let (a, ca) = endpoint(&mut r, 0);
    let (b, cb) = endpoint(&mut r, 0);
    let s = selector(&mut r, &[a, b]);
    let f = frame(&mut r, 0, &[s]);
    assert!(run(&mut r, f));
    r.channel_close(&ca);
    r.channel_close(&ca);
    assert!(Runtime::parked(f));
    assert_eq!(cb.borrow().receivers.len(), 1);
    r.channel_close(&cb);
    assert!(!Runtime::parked(f));
    assert_eq!(resume(f), REFUSED);
    assert!(!run(&mut r, f));
    assert_eq!(tasks::get(f, tasks::STATE), FAILURE_STATE);
    assert!(ca.borrow().receivers.is_empty() && cb.borrow().receivers.is_empty());
}

#[test]
fn selection_overlapping_selectors_respect_channel_fifo() {
    let mut r = runtime();
    let (a, ca) = endpoint(&mut r, 0);
    let (b, cb) = endpoint(&mut r, 0);
    let first = selector(&mut r, &[a, b]);
    let second = selector(&mut r, &[b, a]);
    let f = frame(&mut r, 0, &[first]);
    let g = frame(&mut r, 0, &[second]);
    assert!(run(&mut r, f));
    assert!(run(&mut r, g));
    for target in [b, a] {
        let value = r.heap.int(9);
        let sender = frame(&mut r, 1, &[target, value]);
        assert!(!run(&mut r, sender));
    }
    assert!(!run(&mut r, f));
    assert!(!run(&mut r, g));
    assert_eq!(outcome(&r, f), (1, 9));
    assert_eq!(outcome(&r, g), (1, 9));
    assert!(ca.borrow().receivers.is_empty() && cb.borrow().receivers.is_empty());
}

#[test]
fn selection_repeated_interleavings_have_bounded_queues() {
    // LCG seed is fixed and reported in the assertion; orders include cancel,
    // close, send-before-close and close-before-send, on 2..8 alternatives.
    let mut seed = 509_u64;
    for round in 0..512 {
        let mut r = runtime();
        let count = 2 + round % 7;
        let endpoints: Vec<_> = (0..count).map(|_| endpoint(&mut r, 1)).collect();
        let words: Vec<_> = endpoints.iter().map(|e| e.0).collect();
        let s = selector(&mut r, &words);
        let f = frame(&mut r, 0, &[s]);
        assert!(run(&mut r, f));
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let winner = (seed >> 32) as usize % count;
        let cancel_first = seed & 1 == 0;
        if cancel_first { r.cancel(std::ptr::null_mut(), f); }
        let value = r.heap.int(round as i64);
        let sender = frame(&mut r, 1, &[words[winner], value]);
        assert!(!run(&mut r, sender));
        for (_, c) in &endpoints { r.channel_close(c); }
        r.cancel(std::ptr::null_mut(), f);
        assert!(!run(&mut r, f));
        let messages: usize = endpoints.iter().map(|e| e.1.borrow().messages.len()).sum();
        if cancel_first {
            assert_eq!(tasks::get(f, tasks::STATE), FAILURE_STATE, "seed 509 round {round}");
            assert_eq!(messages, 1);
        } else {
            assert_eq!(outcome(&r, f), (winner as i64, round as i64));
            assert_eq!(messages, 0);
        }
        for (_, c) in &endpoints { assert!(c.borrow().receivers.is_empty()); }
    }
}

#[test]
fn selection_contracts_reject_bad_layout_and_mismatched_payload() {
    for source in [
        PROGRAM.replace("field \"index\" t0", "field \"index\" t1"),
        PROGRAM.replace("field \"payload\" t0", "field \"payload\" t1"),
        PROGRAM.replace("field \"receivers\" t2", "field \"receivers\" t1"),
        PROGRAM.replace("[obj2, obj3, obj4]", "[obj2, obj3]"),
    ] {
        let program = okkama_sparrow::assemble(&source).unwrap();
        assert!(Runtime::new(crate::program::Loaded::new(program)).is_err());
    }
    // Positive contract is exercised by every other direct-driver test.
    drop(runtime());
}

fn collect(r: &mut Runtime) {
    let ctx = Ctx { status: OK, stack_limit: 0, failure: UNBOUND,
        runtime: r, entry_frame: 0, scratch: UNBOUND };
    r.heap.set_stress(true);
    r.safepoint(0, &ctx, &[]);
}

#[test]
fn selection_gc_traces_parked_frames_and_reclaims_completed_configuration() {
    let mut r = runtime();
    let (a, ca) = endpoint(&mut r, 0);
    let (b, cb) = endpoint(&mut r, 0);
    r.roots.extend([(a, Repr::Ref), (b, Repr::Ref)]);
    let mut retained = 0;
    for round in 0..128 {
        let s = selector(&mut r, &[a, b]);
        let f = frame(&mut r, 0, &[s]);
        assert!(run(&mut r, f));
        // Only the channel registrations root the frame and its selector.
        collect(&mut r);
        assert!(r.states.contains_key(&s));
        assert_eq!(tasks::get(f, tasks::STATE), tasks::RUNNING);
        if round % 2 == 0 {
            r.cancel(std::ptr::null_mut(), f);
            assert!(!run(&mut r, f));
        } else {
            let value = r.heap.int(42);
            let sender = frame(&mut r, 1, &[b, value]);
            assert!(!run(&mut r, sender));
            r.roots.push((f, Repr::Ref));
            collect(&mut r);
            assert!(!run(&mut r, f));
            assert_eq!(outcome(&r, f), (1, 42));
            r.roots.pop();
        }
        assert!(ca.borrow().receivers.is_empty() && cb.borrow().receivers.is_empty());
        collect(&mut r);
        assert!(!r.states.contains_key(&s));
        assert_eq!(r.states.len(), 2);
        if round == 0 { retained = r.heap.stats().live_bytes; }
        assert_eq!(r.heap.stats().live_bytes, retained);
    }
}

#[test]
fn selection_rejects_concurrent_request_without_affecting_first() {
    let mut r = runtime();
    let (a, ca) = endpoint(&mut r, 0);
    let (b, cb) = endpoint(&mut r, 0);
    let s = selector(&mut r, &[a, b]);
    let first = frame(&mut r, 0, &[s]);
    let second = frame(&mut r, 0, &[s]);
    assert!(run(&mut r, first));
    assert!(!run(&mut r, second));
    assert_eq!(tasks::get(second, tasks::STATE), FAILURE_STATE);
    assert!(Runtime::parked(first));
    assert_eq!(ca.borrow().receivers.len(), 1);
    assert_eq!(cb.borrow().receivers.len(), 1);
    r.cancel(std::ptr::null_mut(), second);
    let value = r.heap.int(11);
    let sender = frame(&mut r, 1, &[a, value]);
    assert!(!run(&mut r, sender));
    // A committed request still reserves the selector until it publishes.
    let third = frame(&mut r, 0, &[s]);
    assert!(!run(&mut r, third));
    assert_eq!(tasks::get(third, tasks::STATE), FAILURE_STATE);
    assert!(!run(&mut r, first));
    assert_eq!(outcome(&r, first), (0, 11));
}

#[test]
fn selection_commit_wins_when_timeout_is_observed_before_origin_publication() {
    for publish_first in [false, true] {
        let mut r = runtime();
        let (a, _) = endpoint(&mut r, 0);
        let (b, _) = endpoint(&mut r, 0);
        let s = selector(&mut r, &[a, b]);
        let origin = frame(&mut r, 0, &[s]);
        assert!(run(&mut r, origin));
        let value = r.heap.int(42);
        let sender = frame(&mut r, 1, &[b, value]);
        assert!(!run(&mut r, sender));
        if publish_first { assert!(!run(&mut r, origin)); }
        let timer = r.settled_new(1, DONE, a);
        let bounded = frame(&mut r, 0, &[s]);
        set_state(&mut r, bounded, Box::new(Collection {
            strategy: Strategy::Timeout, started: true, entries: vec![origin, timer],
            consumed: vec![false; 2], cancelled: vec![false; 2], remaining: 2,
            ready: VecDeque::from([0]), inbox: vec![origin], positions: vec![],
            winner: None, winner_repr: Repr::Ref, failures: vec![], failed: FastSet::default(),
            external: None, finishing: false,
        }));
        // The timer is observed first, although receive has already committed.
        // Function 2 supplies an empty error layout for the timer failure.
        consume(&mut r, std::ptr::null_mut(), bounded, 2, 1);
        let c = collection(&mut r, bounded);
        assert!(c.winner.is_some(), "committed message must survive the timeout boundary");
        assert!(c.failures.is_empty());
        assert_eq!(c.remaining, 0);
    }
}

#[test]
fn selection_costs_registration_cleanup_allocation_and_retained_bytes() {
    for count in [2, 8, 32] {
        let mut r = runtime();
        r.heap.allocations = Some(Box::default());
        let endpoints: Vec<_> = (0..count).map(|_| endpoint(&mut r, 1)).collect();
        let words: Vec<_> = endpoints.iter().map(|e| e.0).collect();
        let s = selector(&mut r, &words);
        let mut registration = 0;
        let mut cleanup = 0;
        for _ in 0..512 {
            let f = frame(&mut r, 0, &[s]);
            let start = std::time::Instant::now();
            assert!(run(&mut r, f));
            registration += start.elapsed().as_nanos();
            r.cancel(std::ptr::null_mut(), f);
            let start = std::time::Instant::now();
            assert!(!run(&mut r, f));
            cleanup += start.elapsed().as_nanos();
        }
        let queue_bytes: usize = endpoints.iter().map(|e| e.1.borrow().receivers.capacity() * size_of::<Word>()).sum();
        let allocations = |r: &Runtime| -> u64 {
            r.heap.allocations.as_ref().unwrap().kinds.values().map(|&(count, _)| count).sum()
        };
        let before = allocations(&r);
        let bytes_before = r.heap.stats().allocated_bytes;
        let value = r.heap.int(42);
        for _ in 0..512 {
            endpoints[0].1.borrow_mut().messages.push_back(value);
            let f = frame(&mut r, 0, &[s]);
            assert!(!run(&mut r, f));
            assert_eq!(outcome(&r, f).1, 42);
        }
        let per_round = (allocations(&r) - before) / 512;
        assert_eq!(per_round, 2, "one frame and one result, independent of alternatives");
        assert!(endpoints.iter().all(|e| e.1.borrow().receivers.is_empty()));
        println!("selection-cost n={count}: register={}ns cleanup={}ns heap_allocations/ready={per_round} heap_bytes/ready={} queue_capacity_bytes={queue_bytes} config_bytes={}",
            registration / 512, cleanup / 512, (r.heap.stats().allocated_bytes - bytes_before) / 512,
            count * size_of::<Rc<RefCell<Channel>>>());
    }
}

#[test]
fn selection_cold_cancel_registers_nothing_and_close_before_send_commits_nothing() {
    let mut r = runtime();
    let (a, ca) = endpoint(&mut r, 0);
    let (b, cb) = endpoint(&mut r, 0);
    let s = selector(&mut r, &[a, b]);
    let cold = r.future_new(0, &[s]);
    r.cancel(std::ptr::null_mut(), cold);
    assert_eq!(tasks::get(cold, tasks::STATE), FAILURE_STATE);
    assert!(ca.borrow().receivers.is_empty() && cb.borrow().receivers.is_empty());
    let waiting = frame(&mut r, 0, &[s]);
    assert!(run(&mut r, waiting));
    r.channel_close(&ca);
    let value = r.heap.int(7);
    let sender = frame(&mut r, 1, &[a, value]);
    assert!(!run(&mut r, sender));
    assert_eq!(tasks::get(sender, tasks::STATE), FAILURE_STATE);
    assert!(Runtime::parked(waiting));
    r.cancel(std::ptr::null_mut(), waiting);
    r.channel_close(&cb);
    assert!(!run(&mut r, waiting));
    assert!(r.is_cancelled(tasks::get(waiting, tasks::FAILURE)));
    assert!(ca.borrow().receivers.is_empty() && cb.borrow().receivers.is_empty());
}

#[test]
fn selection_commit_survives_cancellation_of_timeout_wrapper() {
    let mut r = runtime();
    let (a, _) = endpoint(&mut r, 0);
    let s = selector(&mut r, &[a]);
    let origin = frame(&mut r, 0, &[s]);
    assert!(run(&mut r, origin));
    let value = r.heap.int(42);
    let sender = frame(&mut r, 1, &[a, value]);
    assert!(!run(&mut r, sender));
    let timer = r.settled_new(1, DONE, a);
    let bounded = frame(&mut r, 0, &[s]);
    set_state(&mut r, bounded, Box::new(Collection {
        strategy: Strategy::Timeout, started: true, entries: vec![origin, timer],
        consumed: vec![false; 2], cancelled: vec![false; 2], remaining: 2,
        ready: VecDeque::from([1]), inbox: vec![], positions: vec![],
        winner: None, winner_repr: Repr::Ref, failures: vec![], failed: FastSet::default(),
        external: None, finishing: false,
    }));
    match super::collect(&mut r, std::ptr::null_mut(), bounded, 0, Intrinsic::TimeoutWithin, true) {
        Done(item) => assert_eq!(number(&r, r.heap.field(item, 1)), 42),
        other => panic!("committed selection lost through timeout wrapper: {other:?}"),
    }
}

fn cancel_timeout_during_timer_cleanup(selected: bool, intrinsic: Intrinsic) {
    let mut r = runtime();
    let (a, channel) = endpoint(&mut r, 0);
    let s = selector(&mut r, &[a]);
    let origin = if selected { frame(&mut r, 0, &[s]) } else { frame(&mut r, 2, &[a]) };
    assert!(run(&mut r, origin));
    let value = r.heap.int(42);
    let sender = frame(&mut r, 1, &[a, value]);
    assert!(!run(&mut r, sender));
    assert!(!run(&mut r, origin));
    let committed = tasks::get(origin, RESULT);
    assert!(channel.borrow().messages.is_empty());
    assert!(channel.borrow().senders.is_empty());

    // A parked driver stands in for a timer whose cancellation cleanup has
    // not run yet. Run it explicitly only after cancelling the wrapper.
    let (clock, timer_channel) = endpoint(&mut r, 0);
    let timer = frame(&mut r, 2, &[clock]);
    assert!(run(&mut r, timer));
    let bounded = frame(&mut r, 0, &[s]);
    set_state(&mut r, bounded, Box::new(Collection {
        strategy: Strategy::Timeout, started: true, entries: vec![origin, timer],
        consumed: vec![false; 2], cancelled: vec![false; 2], remaining: 2,
        ready: VecDeque::from([0]), inbox: vec![], positions: vec![(timer, 1)],
        winner: None, winner_repr: Repr::Ref, failures: vec![], failed: FastSet::default(),
        external: None, finishing: false,
    }));
    tasks::set(bounded, FLAGS, tasks::get(bounded, FLAGS) | tasks::INBOX);
    r.listen(timer, bounded);
    assert!(matches!(super::collect(&mut r, std::ptr::null_mut(), bounded, 0, intrinsic, false), Pending));
    let c = collection(&mut r, bounded);
    assert!(c.finishing && c.consumed[0] && c.cancelled[1]);
    assert_eq!(c.winner, Some(committed));
    assert_eq!(c.remaining, 1);
    assert_eq!(tasks::get(timer, tasks::STATE), tasks::RUNNING);

    // Deliver cancellation while the wrapper is waiting for timer cleanup.
    // Re-entry must still wait, and repeated delivery must preserve the winner.
    for _ in 0..2 {
        assert!(matches!(super::collect(&mut r, std::ptr::null_mut(), bounded, 0, intrinsic, true), Pending));
    }
    assert!(!run(&mut r, timer));
    assert!(r.is_cancelled(tasks::get(timer, tasks::FAILURE)));
    assert!(timer_channel.borrow().receivers.is_empty());
    let result = super::collect(&mut r, std::ptr::null_mut(), bounded, 0, intrinsic, false);
    if selected {
        match result {
            Done(item) => {
                assert_eq!(item, committed);
                assert_eq!(number(&r, r.heap.field(item, 0)), 0);
                assert_eq!(number(&r, r.heap.field(item, 1)), 42);
            }
            other => panic!("committed selection lost during timer cleanup ({intrinsic:?}): {other:?}"),
        }
    } else {
        match result {
            Failed(failure) => assert!(r.is_cancelled(failure)),
            other => panic!("ordinary future must still lose to cancellation ({intrinsic:?}): {other:?}"),
        }
    }
    assert_eq!(collection(&mut r, bounded).remaining, 0);
}

#[test]
fn selection_commit_survives_within_cancellation_during_timer_cleanup() {
    cancel_timeout_during_timer_cleanup(true, Intrinsic::TimeoutWithin);
}

#[test]
fn selection_commit_survives_before_cancellation_during_timer_cleanup() {
    cancel_timeout_during_timer_cleanup(true, Intrinsic::TimeoutBefore);
}

#[test]
fn timeout_cancellation_during_timer_cleanup_still_cancels_ordinary_future() {
    for intrinsic in [Intrinsic::TimeoutWithin, Intrinsic::TimeoutBefore] {
        cancel_timeout_during_timer_cleanup(false, intrinsic);
    }
}
