//! M2.7-10: an exhaustive explorer of the park/wake handshake (`park.rs`),
//! independent of loom (`docs/testing-and-evaluation.md`, "The park/wake
//! handshake"). It enumerates every interleaving of one caller and 1–3
//! workers (1–2 in the `Buffered` mode below) over two consecutive callbacks
//! and a shutdown, and checks that no
//! wake is lost (a lost wake leaves the caller waiting for a join that never
//! comes: no step is enabled), that every join sees its own epoch's job, and
//! that every execution ends (no cycle in the state graph).
//!
//! Each step is one shared access of the code, at the line [`MAPPING`] names
//! (checked by `each_model_step_is_in_the_code`):
//!
//! - caller, per callback: under `semaphore` the callback-start wake of every
//!   parked worker; the job store; the open; the segment's wake of one. A
//!   wake is the SeqCst fence, then per flag a swap and, if it was set, an
//!   unpark. It closes once every worker has joined (the lost-wake detector,
//!   as in loom's P1 and P2). At the end the shutdown store and an unpark of
//!   every worker.
//! - worker: the shutdown check and a join attempt, then the joiner's wake of
//!   two; otherwise the flag store, the fence, the re-reads of the gate and
//!   of shutdown, the park (unless a re-read said not to), the flag cleared.
//!
//! A join is one step: the gate's load, CAS and the job's read (which cannot
//! change while the worker is joined); the leaves and the wait for them are
//! the gate's, which loom's G1–G3 check. An unpark leaves a token that the
//! next park consumes: one (`park`), a count (`semaphore`'s waiter), or, as a
//! broken blocker, none unless the worker is already parked. Requests
//! (`pending`), spinning and spurious returns from a park are left out.
//!
//! Two memory modes. `Sc`: every step is atomic in one total order, which is
//! what C11 gives these SeqCst accesses and fences, so it is the claim of
//! `park.rs`'s module documentation; a fence then changes nothing and is not
//! a step. `Buffered`: each thread's stores wait in a FIFO buffer that drains
//! one entry at a time, at any point; a fence waits until its thread's
//! buffer is empty, an unpark too (it releases), a read-modify-write acts on
//! memory once its thread has no store to the same place pending. That is
//! store buffering, accesses no stronger than Release/Acquire (as loom
//! treats SeqCst). It deliberately weakens the open, a SeqCst `swap`, to a
//! plain store: that is not an equivalence, and it is what lets a removed
//! fence fail here, which is not evidence that the hardware needs that fence.
//!
//! The number of interleavings is computed, not walked: each distinct state
//! is expanded once, and the count of complete executions from it is
//! memoised (`the_counts_are_interleavings` compares it with a plain walk).

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Variant {
    Correct,
    /// Without the worker's fence (`run_worker`).
    NoParkingFence,
    /// Without the waker's fence (`Parking::wake`).
    NoWakingFence,
    /// The worker re-reads the gate before it stores its flag.
    WorkerReadsFirst,
    /// The caller wakes before it opens.
    CallerWakesFirst,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    Binary,
    Counting,
    /// An unpark reaches only a worker already parked.
    Forgetful,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Sc,
    Buffered,
}

#[derive(Clone, Copy, Debug)]
struct Config {
    workers: usize,
    callbacks: u8,
    /// The callback-start wake of every parked worker (`semaphore`, P2).
    wake_all: bool,
    token: Token,
    mode: Mode,
    variant: Variant,
}

impl Config {
    /// Whether the variant's thread fences (and the mode makes it a step).
    fn fences(&self, without: Variant) -> bool {
        self.mode == Mode::Buffered && self.variant != without
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Store {
    Job(u8),
    Open(u8),
    Shutdown,
    Parked(usize, bool),
}

/// Memory: the job slot, the gate word, the flags and shutdown.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Mem {
    job: u8,
    epoch: u8,
    open: bool,
    parked: [bool; 3],
    shutdown: bool,
}

/// `Parking::wake(n)` in progress: fenced, the next flag, those woken, and
/// the worker to unpark next.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Wake {
    n: u8,
    fenced: bool,
    next: u8,
    woken: u8,
    unpark: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum CallerPc {
    WakeAll,
    StoreJob,
    Open,
    Wake,
    Close,
    Shutdown,
    Signal(u8),
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum WorkerPc {
    Top,
    StoreFlag,
    Fence,
    ReadGate,
    ReadShutdown,
    Park,
    ClearFlag,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Worker {
    pc: WorkerPc,
    seen: u8,
    tokens: u8,
    /// Bit `e`: joined epoch `e`.
    joins: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct State {
    mem: Mem,
    caller: CallerPc,
    callback: u8,
    /// Three workers; those beyond the configured number never run.
    workers: [Worker; 3],
    /// Per thread (the caller first): its wake in progress and its buffer.
    wakes: [Option<Wake>; 4],
    buffers: [Vec<Store>; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    LostWake,
    WrongEpoch,
    Cycle,
}

#[derive(Debug)]
struct Violation {
    kind: Kind,
    trace: Vec<String>,
}

type Step = (usize, &'static str);

impl State {
    fn new(cfg: &Config) -> State {
        let worker = |w| Worker {
            pc: if w < cfg.workers {
                WorkerPc::Top
            } else {
                WorkerPc::Done
            },
            seen: 0,
            tokens: 0,
            joins: if w < cfg.workers { 0 } else { u8::MAX },
        };
        let mut s = State {
            mem: Mem {
                job: 0,
                epoch: 0,
                open: false,
                parked: [false; 3],
                shutdown: false,
            },
            caller: CallerPc::WakeAll,
            callback: 1,
            workers: std::array::from_fn(worker),
            wakes: [None; 4],
            buffers: Default::default(),
        };
        s.begin(cfg);
        s
    }

    fn begin(&mut self, cfg: &Config) {
        self.caller = if cfg.wake_all {
            CallerPc::WakeAll
        } else {
            CallerPc::StoreJob
        };
    }

    fn store(&mut self, t: usize, store: Store, cfg: &Config) {
        match cfg.mode {
            Mode::Sc => self.apply(store),
            Mode::Buffered => self.buffers[t].push(store),
        }
    }

    fn apply(&mut self, store: Store) {
        match store {
            Store::Job(e) => self.mem.job = e,
            Store::Open(e) => (self.mem.epoch, self.mem.open) = (e, true),
            Store::Shutdown => self.mem.shutdown = true,
            Store::Parked(w, v) => self.mem.parked[w] = v,
        }
    }

    fn start_wake(&mut self, t: usize, n: u8, cfg: &Config) {
        self.wakes[t] = Some(Wake {
            n,
            fenced: !cfg.fences(Variant::NoWakingFence),
            next: 0,
            woken: 0,
            unpark: None,
        });
    }

    fn unpark(&mut self, w: usize, cfg: &Config) {
        let worker = &mut self.workers[w];
        worker.tokens = match cfg.token {
            Token::Binary => 1,
            Token::Counting => worker.tokens + 1,
            Token::Forgetful if worker.pc == WorkerPc::Park => 1,
            Token::Forgetful => worker.tokens,
        };
    }

    /// The next step of thread `t`'s wake (`Parking::wake`).
    fn wake_step(&mut self, t: usize, mut wake: Wake, cfg: &Config) -> Option<&'static str> {
        let label = if !wake.fenced {
            if !self.buffers[t].is_empty() {
                return None;
            }
            wake.fenced = true;
            "wake: fence"
        } else if let Some(w) = wake.unpark {
            if !self.buffers[t].is_empty() {
                return None;
            }
            self.unpark(usize::from(w), cfg);
            wake.unpark = None;
            "wake: unpark"
        } else {
            let w = usize::from(wake.next);
            if self.buffers[t]
                .iter()
                .any(|s| matches!(s, Store::Parked(p, _) if *p == w))
            {
                return None;
            }
            if std::mem::take(&mut self.mem.parked[w]) {
                wake.unpark = Some(wake.next);
                wake.woken += 1;
            }
            wake.next += 1;
            "wake: swap flag"
        };
        let done = wake.unpark.is_none()
            && (wake.woken >= wake.n || usize::from(wake.next) == cfg.workers);
        self.wakes[t] = (!done).then_some(wake);
        Some(label)
    }

    /// The caller's step after `pc` in a callback, in the variant's order.
    fn then(pc: CallerPc, cfg: &Config) -> CallerPc {
        use CallerPc::*;
        let order = match cfg.variant {
            Variant::CallerWakesFirst => [WakeAll, StoreJob, Wake, Open, Close],
            _ => [WakeAll, StoreJob, Open, Wake, Close],
        };
        let at = order.iter().position(|&p| p == pc);
        at.and_then(|i| order.get(i + 1)).copied().unwrap_or(Close)
    }

    fn caller_step(&mut self, cfg: &Config) -> Option<&'static str> {
        let callback = self.callback;
        let then = State::then(self.caller, cfg);
        let (label, next) = match self.caller {
            CallerPc::WakeAll => {
                self.start_wake(0, u8::MAX, cfg);
                ("callback-start wake", then)
            }
            CallerPc::StoreJob => {
                self.store(0, Store::Job(callback), cfg);
                ("store job", then)
            }
            CallerPc::Open => {
                self.store(0, Store::Open(callback), cfg);
                ("open", then)
            }
            CallerPc::Wake => {
                self.start_wake(0, 1, cfg);
                ("wake one", then)
            }
            CallerPc::Close => {
                let joined = self.workers.iter().all(|w| w.joins & (1 << callback) != 0);
                if !joined || self.buffers[0].contains(&Store::Open(callback)) {
                    return None;
                }
                self.mem.open = false;
                if callback < cfg.callbacks {
                    self.callback += 1;
                    self.begin(cfg);
                    return Some("close");
                }
                ("close", CallerPc::Shutdown)
            }
            CallerPc::Shutdown => {
                self.store(0, Store::Shutdown, cfg);
                ("store shutdown", CallerPc::Signal(0))
            }
            CallerPc::Signal(w) => {
                if !self.buffers[0].is_empty() {
                    return None;
                }
                self.unpark(usize::from(w), cfg);
                let next = if usize::from(w) + 1 < cfg.workers {
                    CallerPc::Signal(w + 1)
                } else {
                    CallerPc::Done
                };
                ("shutdown unpark", next)
            }
            CallerPc::Done => return None,
        };
        self.caller = next;
        Some(label)
    }

    /// The step after `pc` on the way to parking, in the variant's order.
    fn after(pc: WorkerPc, cfg: &Config) -> WorkerPc {
        use WorkerPc::*;
        let reads_first = cfg.variant == Variant::WorkerReadsFirst;
        let order: &[WorkerPc] = match (reads_first, cfg.fences(Variant::NoParkingFence)) {
            (false, true) => &[StoreFlag, Fence, ReadGate, ReadShutdown, Park],
            (false, false) => &[StoreFlag, ReadGate, ReadShutdown, Park],
            (true, true) => &[ReadGate, ReadShutdown, StoreFlag, Fence, Park],
            (true, false) => &[ReadGate, ReadShutdown, StoreFlag, Park],
        };
        match order.iter().position(|&p| p == pc) {
            None => order[0],
            Some(i) => order.get(i + 1).copied().unwrap_or(ClearFlag),
        }
    }

    /// Where a worker goes when a re-read says it must not park: it clears
    /// its flag, or, if it re-read before storing it, starts again.
    fn not_parking(cfg: &Config) -> WorkerPc {
        match cfg.variant {
            Variant::WorkerReadsFirst => WorkerPc::Top,
            _ => WorkerPc::ClearFlag,
        }
    }

    fn worker_step(&mut self, w: usize, cfg: &Config) -> Result<Option<&'static str>, Kind> {
        use WorkerPc::*;
        let t = w + 1;
        let mem = &mut self.mem;
        let worker = &mut self.workers[w];
        let (label, next) = match worker.pc {
            Top if mem.shutdown => ("sees shutdown", Done),
            Top if mem.open && mem.epoch != worker.seen => {
                worker.seen = mem.epoch;
                if mem.job != worker.seen || worker.joins & (1 << worker.seen) != 0 {
                    return Err(Kind::WrongEpoch);
                }
                worker.joins |= 1 << worker.seen;
                worker.pc = Top;
                self.start_wake(t, 2, cfg);
                return Ok(Some("join, wake two"));
            }
            Top => ("nothing to join", State::after(Top, cfg)),
            StoreFlag => {
                worker.pc = State::after(StoreFlag, cfg);
                self.store(t, Store::Parked(w, true), cfg);
                return Ok(Some("store flag"));
            }
            Fence if !self.buffers[t].is_empty() => return Ok(None),
            Fence => ("fence", State::after(Fence, cfg)),
            ReadGate if mem.open && mem.epoch != worker.seen => {
                ("re-read gate: open", State::not_parking(cfg))
            }
            ReadGate => ("re-read gate: closed", ReadShutdown),
            ReadShutdown if mem.shutdown => ("re-read shutdown: set", State::not_parking(cfg)),
            ReadShutdown => ("re-read shutdown", State::after(ReadShutdown, cfg)),
            Park if worker.tokens == 0 => return Ok(None),
            Park => {
                worker.tokens = match cfg.token {
                    Token::Counting => worker.tokens - 1,
                    _ => 0,
                };
                ("park returns", ClearFlag)
            }
            ClearFlag => {
                worker.pc = Top;
                self.store(t, Store::Parked(w, false), cfg);
                return Ok(Some("clear flag"));
            }
            Done => return Ok(None),
        };
        worker.pc = next;
        Ok(Some(label))
    }

    /// Every state one step away, with the step.
    fn successors(&self, cfg: &Config) -> Result<Vec<(Step, State)>, Kind> {
        let mut next = Vec::new();
        for t in 0..=cfg.workers {
            let mut s = self.clone();
            let label = match self.wakes[t] {
                Some(wake) => s.wake_step(t, wake, cfg),
                None if t == 0 => s.caller_step(cfg),
                None => s.worker_step(t - 1, cfg)?,
            };
            if let Some(label) = label {
                next.push(((t, label), s));
            }
            if !self.buffers[t].is_empty() {
                let mut s = self.clone();
                let store = s.buffers[t].remove(0);
                s.apply(store);
                next.push(((t, "store drains"), s));
            }
        }
        Ok(next)
    }

    fn finished(&self) -> bool {
        self.caller == CallerPc::Done && self.workers.iter().all(|w| w.pc == WorkerPc::Done)
    }
}

#[derive(Debug)]
struct Report {
    interleavings: f64,
    states: usize,
    transitions: u64,
}

/// A multiplicative hash (as rustc's `FxHasher`): std's SipHash dominates
/// the explorer's time in the unoptimised test profile.
#[derive(Default)]
struct Fx(u64);

impl Hasher for Fx {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }

    fn write_u8(&mut self, n: u8) {
        self.write_u64(u64::from(n));
    }

    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }

    fn write_isize(&mut self, n: isize) {
        self.write_u64(n as u64);
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

struct Explorer<'c> {
    cfg: &'c Config,
    /// Per state reached: the executions from it, or `None` while it is
    /// on the current path (reaching it again is a cycle).
    memo: HashMap<State, Option<f64>, BuildHasherDefault<Fx>>,
    path: Vec<Step>,
    transitions: u64,
}

impl Explorer<'_> {
    fn violation(&self, kind: Kind) -> Violation {
        let trace = self.path.iter().map(|&(t, step)| match t {
            0 => format!("caller: {step}"),
            t => format!("worker {}: {step}", t - 1),
        });
        Violation {
            kind,
            trace: trace.collect(),
        }
    }

    /// The number of complete executions from `s`.
    fn visit(&mut self, s: State) -> Result<f64, Violation> {
        match self.memo.get(&s) {
            Some(Some(n)) => return Ok(*n),
            Some(None) => return Err(self.violation(Kind::Cycle)),
            None => {}
        }
        let next = s.successors(self.cfg).map_err(|k| self.violation(k))?;
        if next.is_empty() && !s.finished() {
            return Err(self.violation(Kind::LostWake));
        }
        self.memo.insert(s.clone(), None);
        let mut total = if next.is_empty() { 1.0 } else { 0.0 };
        for (step, child) in next {
            self.transitions += 1;
            self.path.push(step);
            total += self.visit(child)?;
            self.path.pop();
        }
        self.memo.insert(s, Some(total));
        Ok(total)
    }
}

fn explore(cfg: &Config) -> Result<Report, Violation> {
    let mut explorer = Explorer {
        cfg,
        memo: HashMap::default(),
        path: Vec::new(),
        transitions: 0,
    };
    let interleavings = explorer.visit(State::new(cfg))?;
    Ok(Report {
        interleavings,
        states: explorer.memo.len(),
        transitions: explorer.transitions,
    })
}

fn config(workers: usize, callbacks: u8, wake_all: bool, token: Token, mode: Mode) -> Config {
    Config {
        workers,
        callbacks,
        wake_all,
        token,
        mode,
        variant: Variant::Correct,
    }
}

/// AC-explorer, AC1: the handshake loses no wake, joins only its own epoch
/// and ends, over two callbacks, for 1–3 workers in `Sc` and 1–2 in
/// `Buffered` (3 there take about 45 s unoptimised), printing what it
/// explored.
fn loses_no_wake(policy: &str, wake_all: bool, token: Token) {
    let start = Instant::now();
    let (mut interleavings, mut states) = (0.0, 0);
    for (mode, most) in [(Mode::Sc, 3), (Mode::Buffered, 2)] {
        for workers in 1..=most {
            let cfg = config(workers, 2, wake_all, token, mode);
            let at = Instant::now();
            let report = explore(&cfg).unwrap_or_else(|v| panic!("{cfg:?}: {v:#?}"));
            println!(
                "handshake {policy} {mode:?} workers={workers}: {:.3e} interleavings, {} states, \
                 {} transitions, {:.2?}",
                report.interleavings,
                report.states,
                report.transitions,
                at.elapsed()
            );
            interleavings += report.interleavings;
            states += report.states;
        }
    }
    println!(
        "handshake {policy}: {interleavings:.3e} interleavings through {states} states in {:.2?}",
        start.elapsed()
    );
}

/// `park50` and `budget-spin`: no callback-start wake, `park`'s token.
#[test]
fn park_loses_no_wake() {
    loses_no_wake("park", false, Token::Binary);
}

/// Loom's P2: the callback-start wake of every parked worker before the
/// open, with `park`'s token.
#[test]
fn a_wake_before_the_open_loses_no_wake() {
    loses_no_wake("park + wake-all (P2)", true, Token::Binary);
}

/// `semaphore`: the callback-start wake, with the waiter's count.
#[test]
fn semaphore_loses_no_wake() {
    loses_no_wake("semaphore", true, Token::Counting);
}

/// AC2: a broken handshake fails. Swapping the flag and the gate on either
/// side loses a wake in both modes; a removed fence loses one only once
/// accesses are weaker than SC (`Buffered`): under `Sc` the SeqCst accesses
/// alone order the handshake, as `park.rs` says. A blocker that forgets an
/// early unpark loses one too.
#[test]
fn a_broken_handshake_is_caught() {
    let cases = [
        (Variant::WorkerReadsFirst, Token::Binary, Mode::Sc, true),
        (
            Variant::WorkerReadsFirst,
            Token::Binary,
            Mode::Buffered,
            true,
        ),
        (Variant::CallerWakesFirst, Token::Binary, Mode::Sc, true),
        (
            Variant::CallerWakesFirst,
            Token::Binary,
            Mode::Buffered,
            true,
        ),
        (Variant::NoParkingFence, Token::Binary, Mode::Sc, false),
        (Variant::NoParkingFence, Token::Binary, Mode::Buffered, true),
        (Variant::NoWakingFence, Token::Binary, Mode::Sc, false),
        (Variant::NoWakingFence, Token::Binary, Mode::Buffered, true),
        (Variant::Correct, Token::Forgetful, Mode::Sc, true),
    ];
    for (variant, token, mode, fails) in cases {
        let cfg = Config {
            variant,
            ..config(2, 1, true, token, mode)
        };
        match explore(&cfg) {
            Ok(report) => {
                assert!(!fails, "{cfg:?} passed: {report:?}");
                println!("{variant:?} {token:?} {mode:?}: holds");
            }
            Err(v) => {
                assert!(fails, "{cfg:?}: {v:#?}");
                assert_eq!(v.kind, Kind::LostWake, "{cfg:?}: {v:#?}");
                println!(
                    "{variant:?} {token:?} {mode:?}: lost wake after {} steps: {}",
                    v.trace.len(),
                    v.trace.join("; ")
                );
            }
        }
    }
}

/// The memoised count is the number of executions: a plain walk of every
/// execution agrees, for one worker and one callback in `Sc` (in `Buffered`
/// that is already 6.2e9 executions).
#[test]
fn the_counts_are_interleavings() {
    fn walk(s: &State, cfg: &Config) -> u64 {
        let next = s.successors(cfg).unwrap();
        assert!(!next.is_empty() || s.finished());
        next.iter().map(|(_, c)| walk(c, cfg)).sum::<u64>().max(1)
    }
    let cfg = config(1, 1, false, Token::Binary, Mode::Sc);
    let walked = walk(&State::new(&cfg), &cfg);
    println!("{walked} executions walked");
    assert_eq!(explore(&cfg).unwrap().interleavings, walked as f64);
}

/// Each model step: the file under `src/` and its line, and the text on that
/// line (AC-mapping; `docs/testing-and-evaluation.md` has the same table).
const MAPPING: &str = "
callback-start wake   | pool.rs:406   | shared.wake(usize::MAX)
wake: fence           | park.rs:44    | fence(Ordering::SeqCst);
wake: swap flag       | park.rs:50    | parked.swap(false, Ordering::SeqCst)
wake: unpark          | park.rs:51    | signal(w);
wake: unpark (park)   | pool.rs:236   | thread.unpark();
wake: unpark (waiter) | pool.rs:239   | waiter.signal(w)
store job             | shared.rs:1395 | *slot = (job as *const Job<'_>)
open                  | gate.rs:101   | (epoch << EPOCH_SHIFT) | OPEN, Ordering::SeqCst
wake one              | pool.rs:490   | shared.wake(1)
close                 | gate.rs:130   | fetch_and(!OPEN, Ordering::AcqRel)
store shutdown        | pool.rs:498   | shutdown.store(true, Ordering::SeqCst)
shutdown unpark       | pool.rs:500   | self.shared.signal(w);
shutdown check        | park.rs:100   | pool.shutdown(Ordering::Acquire)
join                  | park.rs:104   | pool.join(w, &mut seen)
join (CAS)            | gate.rs:85    | .compare_exchange(word, word + 1
join (read job)       | shared.rs:1368 | self.job.with(|job| *job)
wake two              | pool.rs:665   | self.wake(2);
store flag            | park.rs:117   | parked.store(true, Ordering::SeqCst);
fence                 | park.rs:120   | fence(Ordering::SeqCst);
re-read               | park.rs:121   | pool.joinable(seen) || pool.shutdown
re-read gate          | gate.rs:60    | self.word.load(Ordering::SeqCst)
park                  | park.rs:123   | pool.block(w);
park (park)           | pool.rs:246   | Blocker::Park => thread::park()
park (waiter)         | pool.rs:247   | waiter.wait(w)
clear flag            | park.rs:125   | parked.store(false, Ordering::SeqCst);
";

/// AC-mapping: every model step is on the line [`MAPPING`] names, and the
/// steps of `park.rs` are in the model's order: the wake's fence, swap and
/// unpark; the worker's shutdown check, join, flag, fence, re-read, park and
/// clear. Swapping the flag and the gate in `run_worker` fails here too.
#[test]
fn each_model_step_is_in_the_code() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut line_of = HashMap::new();
    for row in MAPPING.trim().lines() {
        let [step, at, text] = row.splitn(3, " | ").map(str::trim).collect::<Vec<_>>()[..] else {
            panic!("{row:?}");
        };
        let (file, line) = at.split_once(':').unwrap();
        let line: usize = line.parse().unwrap();
        let source = std::fs::read_to_string(src.join(file)).unwrap();
        let found = source.lines().nth(line - 1).unwrap_or("");
        assert!(
            found.contains(text),
            "{step}: {at} is {found:?}, not {text:?}"
        );
        line_of.insert(step, line);
    }
    let wake = ["wake: fence", "wake: swap flag", "wake: unpark"];
    let worker = [
        "shutdown check",
        "join",
        "store flag",
        "fence",
        "re-read",
        "park",
        "clear flag",
    ];
    for order in [&wake[..], &worker[..]] {
        let lines: Vec<usize> = order.iter().map(|s| line_of[s]).collect();
        assert!(lines.is_sorted(), "{order:?} at {lines:?}");
    }
}
