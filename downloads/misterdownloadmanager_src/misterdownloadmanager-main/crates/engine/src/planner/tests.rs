use super::*;
use proptest::prelude::*;

const MB: u64 = 1 << 20;

fn check_invariants(p: &Planner) {
    let mut expect = 0;
    let mut workers = std::collections::HashSet::new();
    for s in p.segments() {
        assert_eq!(s.start, expect, "segments contiguous: {:?}", p.segments());
        assert!(
            s.start <= s.cursor && s.cursor <= s.reserved && s.reserved <= s.end,
            "{s:?}"
        );
        assert!(s.start < s.end, "no empty segments: {s:?}");
        if let Some(w) = s.worker {
            assert!(workers.insert(w), "worker {w} holds two segments");
        }
        expect = s.end;
    }
    assert_eq!(expect, p.size());
}

#[test]
fn first_worker_gets_whole_file_second_splits_in_half() {
    let mut p = Planner::new(100 * MB, MB);
    assert_eq!(
        p.assign(0, None),
        Some(Assignment {
            from: 0,
            to: 100 * MB,
            written: 0,
            preempted: None
        })
    );
    assert_eq!(
        p.assign(1, None),
        Some(Assignment {
            from: 50 * MB,
            to: 100 * MB,
            written: 0,
            preempted: None
        })
    );
    assert_eq!(p.segments()[0].end, 50 * MB);
    check_invariants(&p);
}

#[test]
fn reassigned_segment_reports_the_bytes_already_written() {
    let mut p = Planner::new(10 * MB, MB);
    p.assign(0, None).unwrap();
    assert_eq!(p.claim(0, 0, 3 * MB), Claim::Granted(3 * MB));
    assert_eq!(p.commit(0, 3 * MB), None);
    p.release(0); // the connection failed
    let a = p.assign(0, None).unwrap();
    assert_eq!(
        (a.from, a.written),
        (3 * MB, 3 * MB),
        "resume point and data before it"
    );
    check_invariants(&p);
}

#[test]
fn ramp_up_always_halves_the_largest() {
    let mut p = Planner::new(64 * MB, MB);
    for w in 0..8 {
        p.assign(w, None).unwrap();
        check_invariants(&p);
    }
    let starts: Vec<u64> = p.segments().iter().map(|s| s.start / MB).collect();
    // Largest first, lowest position on ties: 0|32 → 0|16|32|48 → 0|8|16|24|32|40|48|56.
    assert_eq!(starts, vec![0, 8, 16, 24, 32, 40, 48, 56]);
}

#[test]
fn finished_worker_takes_over_next_segment_not_yet_receiving() {
    let mut p = Planner::new(10 * MB, MB);
    p.assign(0, None).unwrap(); // [0, 10)
    let a1 = p.assign(1, None).unwrap(); // [5, 10), worker 1 still connecting
    assert_eq!(p.claim(0, 0, 5 * MB), Claim::Granted(5 * MB));
    assert_eq!(
        p.commit(0, 5 * MB),
        Some(5 * MB),
        "worker 0 reached the start marker"
    );

    let a = p.assign(0, Some(5 * MB)).unwrap();
    assert_eq!(
        a,
        Assignment {
            from: a1.from,
            to: a1.to,
            written: 0,
            preempted: Some(1)
        }
    );
    assert_eq!(
        p.claim(1, a1.from, 1),
        Claim::Lost,
        "preempted worker must re-ask"
    );
    check_invariants(&p);
}

#[test]
fn a_request_still_on_its_way_is_not_taken_over() {
    // With a grace, the pending connection keeps its segment until it has waited that long;
    // the free one splits instead.
    let grace = std::time::Duration::from_millis(60);
    let setup = || {
        let mut p = Planner::new(10 * MB, MB);
        p.set_preempt_after(grace);
        p.assign(0, None).unwrap(); // [0, 10)
        p.assign(1, None).unwrap(); // [5, 10), worker 1 still connecting
        assert_eq!(p.claim(0, 0, 5 * MB), Claim::Granted(5 * MB));
        assert_eq!(p.commit(0, 5 * MB), Some(5 * MB));
        p
    };
    let mut p = setup();
    let a = p.assign(0, Some(5 * MB)).unwrap();
    assert_eq!(a.preempted, None, "worker 1 only just got its segment");
    check_invariants(&p);
    // Once it has waited past the grace, IDM's rule applies.
    let mut p = setup();
    std::thread::sleep(grace + std::time::Duration::from_millis(10));
    let a = p.assign(0, Some(5 * MB)).unwrap();
    assert_eq!(a.preempted, Some(1));
    check_invariants(&p);
}

#[test]
fn finished_worker_splits_largest_when_next_already_receiving() {
    let mut p = Planner::new(10 * MB, MB);
    p.assign(0, None).unwrap();
    p.assign(1, None).unwrap(); // [5, 10)
    assert_eq!(p.claim(1, 5 * MB, 1024), Claim::Granted(1024)); // worker 1 is receiving
    p.claim(0, 0, 5 * MB);
    p.commit(0, 5 * MB);

    let a = p.assign(0, Some(5 * MB)).unwrap();
    assert_eq!(a.preempted, None);
    assert!(
        a.from > 5 * MB + 1024 && a.to == 10 * MB,
        "split worker 1's remainder: {a:?}"
    );
    check_invariants(&p);
}

#[test]
fn no_split_below_min_split() {
    let mut p = Planner::new(MB, MB);
    assert!(p.assign(0, None).is_some());
    assert_eq!(p.assign(1, None), None, "halves would be < min_split");
}

#[test]
fn split_shortens_the_owner_who_then_finishes_early() {
    let mut p = Planner::new(8 * MB, MB);
    p.assign(0, None).unwrap();
    assert_eq!(p.claim(0, 0, MB), Claim::Granted(MB)); // reserved = 1 MB, not committed yet
    let a = p.assign(1, None).unwrap();
    assert_eq!(a.from, MB + 7 * MB / 2 - (MB + 7 * MB / 2) % ALIGN); // half of the unclaimed part
    p.commit(0, MB);
    // Worker 0 asks for more than is left before the new boundary.
    assert_eq!(p.claim(0, MB, 100 * MB), Claim::Granted(a.from - MB));
    assert_eq!(p.commit(0, a.from), Some(a.from));
    check_invariants(&p);
}

#[test]
fn release_drops_uncommitted_claims() {
    let mut p = Planner::new(4 * MB, MB);
    p.assign(0, None).unwrap();
    p.claim(0, 0, 3000);
    p.commit(0, 1000);
    p.release(0);
    assert_eq!(p.segments()[0].reserved, 1000);
    assert_eq!(
        p.assign(7, None).unwrap().from,
        1000,
        "resumes from the committed cursor"
    );
}

#[test]
fn save_resume_round_trip() {
    let mut p = Planner::new(16 * MB, MB);
    p.assign(0, None).unwrap();
    p.assign(1, None).unwrap();
    p.claim(0, 0, 3 * MB);
    p.commit(0, 2 * MB);
    let saved = p.save();
    let mut r = Planner::resume(16 * MB, MB, &saved).unwrap();
    assert_eq!(r.downloaded(), 2 * MB);
    assert_eq!(r.active_workers(), 0);
    assert_eq!(r.assign(5, None).unwrap().from, 2 * MB);
    check_invariants(&r);
}

#[test]
fn resume_rejects_bad_state() {
    let s = |start, cursor, end| SavedSegment { start, cursor, end };
    assert!(Planner::resume(10, 1, &[s(0, 0, 5)]).is_err(), "short");
    assert!(
        Planner::resume(10, 1, &[s(0, 0, 5), s(6, 6, 10)]).is_err(),
        "gap"
    );
    assert!(
        Planner::resume(10, 1, &[s(0, 6, 5), s(5, 5, 10)]).is_err(),
        "cursor past end"
    );
    assert!(Planner::resume(10, 1, &[s(0, 0, 10)]).is_ok());
    assert!(Planner::resume(0, 1, &[]).is_ok());
}

#[test]
fn make_single_returns_the_file_to_the_first_connection() {
    let mut p = Planner::new(8 * MB, MB);
    p.assign(0, None).unwrap();
    p.assign(1, None).unwrap(); // [4, 8) — its range request will come back 200
    p.claim(0, 0, MB);
    assert!(p.make_single(0));
    assert_eq!(p.segments().len(), 1);
    assert_eq!(p.segments()[0].end, 8 * MB);
    assert_eq!(p.claim(1, 4 * MB, 1), Claim::Lost);
    assert_eq!(p.assign(2, None), None, "no more splitting");
    assert_eq!(p.claim(0, MB, 7 * MB), Claim::Granted(7 * MB));
    check_invariants(&p);
}

#[test]
fn make_single_refuses_when_others_have_data() {
    let mut p = Planner::new(8 * MB, MB);
    p.assign(0, None).unwrap();
    p.assign(1, None).unwrap();
    p.claim(1, 4 * MB, 10);
    p.commit(1, 4 * MB + 10);
    assert!(!p.make_single(0));
    assert!(!p.make_single(1), "worker 1 doesn't own byte 0");
}

#[test]
fn the_probe_takes_over_the_split_off_segments_nothing_arrived_for() {
    // The probe's plain GET runs to the end of the file while connections 1 and 2 split
    // its segment; their range requests got a 200 and they stopped (CI saw the probe then end
    // at its shortened boundary and the whole file fetched again).
    let mut p = Planner::new(8 * MB, MB);
    p.assign(0, None).unwrap(); // [0, 8)
    p.assign(1, None).unwrap(); // [4, 8)
    p.assign(2, None).unwrap(); // [2, 4) or [6, 8)
    p.release(1);
    p.release(2);
    assert!(p.holds(0) && !p.holds(1));
    let end0 = p.segments()[0].end;
    assert_eq!(p.claim(0, 0, end0), Claim::Granted(end0));
    assert_eq!(p.room(0, end0), Some(0));
    while p.absorb_next(0) {
        check_invariants(&p);
    }
    assert_eq!(p.segments().len(), 1);
    assert_eq!(p.room(0, end0), Some(8 * MB - end0));
    assert_eq!(p.claim(0, end0, 8 * MB), Claim::Granted(8 * MB - end0));
    assert_eq!(p.commit(0, 8 * MB), Some(8 * MB));
    assert!(p.is_complete());
}

#[test]
fn a_segment_with_data_is_not_taken_over() {
    let mut p = Planner::new(8 * MB, MB);
    p.assign(0, None).unwrap();
    p.assign(1, None).unwrap(); // [4, 8)
    assert_eq!(p.claim(1, 4 * MB, 10), Claim::Granted(10));
    assert!(!p.absorb_next(0), "connection 1 is receiving it");
    assert_eq!(p.claim(1, 4 * MB + 10, 10), Claim::Granted(10));
    assert!(!p.absorb_next(1), "the last segment has nothing after it");
    assert!(!p.absorb_next(7), "no segment");
    check_invariants(&p);
}

#[test]
fn room_shrinks_when_split() {
    let mut p = Planner::new(8 * MB, MB);
    p.assign(0, None).unwrap();
    assert_eq!(p.room(0, MB), Some(7 * MB));
    let a = p.assign(1, None).unwrap();
    assert_eq!(p.room(0, MB), Some(a.from - MB));
    assert_eq!(p.room(0, a.from + 5), Some(0));
    assert_eq!(p.room(9, 0), None);
}

#[test]
fn empty_file_is_complete() {
    let mut p = Planner::new(0, 1);
    assert!(p.is_complete());
    assert_eq!(p.assign(0, None), None);
}

// ---- randomized simulation -------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Op {
    Assign(u32),
    Claim(u32, u64),
    Commit(u32),
    Release(u32),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..8u32).prop_map(Op::Assign),
        (0..8u32, 1..5000u64).prop_map(|(w, n)| Op::Claim(w, n)),
        (0..8u32).prop_map(Op::Commit),
        (0..8u32).prop_map(Op::Release),
    ]
}

#[derive(Default, Clone)]
struct Sim {
    pos: Option<u64>,
    pending: Option<(u64, u64)>,
    after: Option<u64>,
}

struct World {
    p: Planner,
    written: Vec<bool>,
    w: Vec<Sim>,
}

impl World {
    fn write(&mut self, from: u64, n: u64) {
        for b in from..from + n {
            assert!(!self.written[b as usize], "byte {b} written twice");
            self.written[b as usize] = true;
        }
    }

    fn commit(&mut self, w: u32) {
        let s = &mut self.w[w as usize];
        if let Some((pos, n)) = s.pending.take() {
            s.pos = Some(pos + n);
            if let Some(end) = self.p.commit(w, pos + n) {
                s.after = Some(end);
                s.pos = None;
            }
        }
    }

    fn apply(&mut self, op: &Op) {
        match *op {
            Op::Assign(w) => {
                self.commit(w);
                let after = self.w[w as usize].after.take();
                self.w[w as usize].pos = self.p.assign(w, after).map(|a| a.from);
            }
            Op::Claim(w, n) => {
                let s = self.w[w as usize].clone();
                if let (Some(pos), None) = (s.pos, s.pending) {
                    match self.p.claim(w, pos, n) {
                        Claim::Granted(k) => {
                            self.write(pos, k);
                            self.w[w as usize].pending = Some((pos, k));
                        }
                        Claim::Lost => self.w[w as usize].pos = None,
                    }
                }
            }
            Op::Commit(w) => self.commit(w),
            Op::Release(w) => {
                // A failed write: the claimed bytes never reached disk.
                if let Some((pos, n)) = self.w[w as usize].pending.take() {
                    for b in pos..pos + n {
                        self.written[b as usize] = false;
                    }
                }
                self.p.release(w);
                self.w[w as usize] = Sim::default();
            }
        }
        check_invariants(&self.p);
    }

    fn finish(&mut self) {
        for _ in 0..100_000 {
            if self.p.is_complete() {
                break;
            }
            for w in 0..8u32 {
                self.commit(w);
                if self.w[w as usize].pos.is_none() {
                    self.apply(&Op::Assign(w));
                }
                self.apply(&Op::Claim(w, 4096));
                self.commit(w);
            }
        }
        assert!(self.p.is_complete(), "stuck: {:?}", self.p.segments());
        assert!(self.written.iter().all(|b| *b), "every byte written");
        assert_eq!(self.p.downloaded(), self.p.size());
    }
}

proptest! {
    #[test]
    fn random_workers_write_every_byte_exactly_once(
        size in 1..60_000u64,
        min_split in 1..3000u64,
        ops in prop::collection::vec(op(), 0..300),
        resume_at in prop::option::of(0..300usize),
    ) {
        let mut world = World {
            p: Planner::new(size, min_split),
            written: vec![false; size as usize],
            w: vec![Sim::default(); 8],
        };
        for (i, op) in ops.iter().enumerate() {
            if resume_at == Some(i) {
                // Simulated restart: uncommitted claims are lost.
                for w in 0..8 {
                    if let Some((pos, n)) = world.w[w].pending.take() {
                        for b in pos..pos + n {
                            world.written[b as usize] = false;
                        }
                    }
                }
                world.p = Planner::resume(size, min_split, &world.p.save()).unwrap();
                world.w = vec![Sim::default(); 8];
            }
            world.apply(op);
        }
        world.finish();
    }
}
