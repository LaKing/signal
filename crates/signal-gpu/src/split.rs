//! M2.7-2: the hook through which a large module splits one job across
//! devices without giving up determinism. The split is fixed: the module's
//! type decides the number of parts *P* (never the number of devices), part
//! *i* is always computed from the same slice of input and runs on device
//! (*first* + *i*) mod *n*; and the combine is fixed: it is called for parts
//! 0, 1, …, *P* − 1 in that order, only once every part is back, whatever
//! order they finished in. A job with any part not `Ok` fails as a whole,
//! with the first such part's outcome, and parts of different jobs are never
//! combined: a job a replaced part lost is `DeviceLost`. Each part is
//! reproducible on its own device (`same_machine`), so the combined result
//! is too.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::{DevicePool, GpuError, Job, KernelSpec, Outcome, PlacedChannel};

/// One channel per part, each on its device. Real-time safe like
/// [`Channel`](crate::Channel): nothing here blocks, locks or allocates.
#[derive(Debug)]
pub struct SplitChannel {
    parts: Vec<PlacedChannel>,
    /// The parts of the job being combined; capacity *P*, from `new`.
    taken: Vec<Option<Job>>,
    /// The jobs submitted and not yet taken, oldest first; capacity `jobs`,
    /// from `new` (no more can be in flight), so pushing never allocates.
    outstanding: VecDeque<u64>,
    /// How many of the oldest `outstanding` jobs are lost (a replaced part
    /// lost them, or a part's first finished job is a later one): each is
    /// taken as `DeviceLost` without waiting for its parts.
    lost: usize,
}

impl SplitChannel {
    /// `parts` channels of `jobs` jobs for `spec` (one part's kernel), part
    /// *i* on device (`first` + *i*) mod *n* of `pool`. Control thread only.
    pub fn new(
        pool: &Arc<DevicePool>,
        first: usize,
        spec: KernelSpec,
        parts: usize,
        jobs: usize,
    ) -> Result<SplitChannel, GpuError> {
        let n = pool.devices().len();
        if n == 0 {
            return Err(GpuError::NoDevice("the pool has no device".into()));
        }
        if parts == 0 {
            return Err(GpuError::Kernel("a split needs at least one part".into()));
        }
        let parts = (0..parts)
            .map(|i| DevicePool::lease(pool, (first + i) % n).channel(spec, jobs))
            .collect::<Result<Vec<_>, _>>()?;
        let taken = (0..parts.len()).map(|_| None).collect();
        Ok(SplitChannel {
            parts,
            taken,
            outstanding: VecDeque::with_capacity(jobs.max(1)),
            lost: 0,
        })
    }

    /// The number of parts.
    pub fn parts(&self) -> usize {
        self.parts.len()
    }

    /// Part `part`'s channel.
    pub fn part(&self, part: usize) -> Option<&PlacedChannel> {
        self.parts.get(part)
    }

    /// The device index part `part` runs on.
    pub fn device_of(&self, part: usize) -> Option<usize> {
        self.parts.get(part).map(PlacedChannel::device)
    }

    /// Submits job `seq` (sequence numbers rise): `fill(i, input)` writes
    /// part *i*'s input, for every part in order. All or nothing: `false`
    /// (nothing submitted) when some part has no free job. Parts of jobs
    /// already taken (a lost job's surviving parts) go back to their pools
    /// first.
    pub fn submit(&mut self, seq: u64, mut fill: impl FnMut(usize, &mut [f32])) -> bool {
        self.drop_stale();
        if self.outstanding.len() >= self.outstanding.capacity()
            || self.parts.iter().any(|p| p.free_jobs() == 0)
        {
            return false;
        }
        let mut all = true;
        for (i, part) in self.parts.iter_mut().enumerate() {
            all &= part.submit(seq, |input| fill(i, input));
        }
        self.outstanding.push_back(seq);
        all
    }

    /// Returns to their pools the finished parts of jobs already taken.
    fn drop_stale(&mut self) {
        let oldest = self.outstanding.front().copied().unwrap_or(u64::MAX);
        self.discard_before(oldest);
    }

    /// The oldest and newest of the parts' first finished jobs, once every
    /// part has one.
    fn heads(&self) -> Option<(u64, u64)> {
        let mut heads: Option<(u64, u64)> = None;
        for part in &self.parts {
            let done = part.peek_done()?;
            heads = Some(heads.map_or((done, done), |(lo, hi)| (lo.min(done), hi.max(done))));
        }
        heads
    }

    /// The sequence number of the job [`take_combined`](SplitChannel::take_combined)
    /// takes next: at once a job a replaced part lost (auditor-1-007),
    /// otherwise the oldest outstanding job once every part has a finished
    /// job at or after it (if they are not all that job, it is lost).
    /// Finished parts of jobs already taken go back to their pools first, so
    /// the answer is always the job `take_combined` takes (auditor-2-021).
    pub fn ready(&mut self) -> Option<u64> {
        let front = self.outstanding.front().copied()?;
        if self.lost > 0 {
            return Some(front);
        }
        self.drop_stale();
        // Every head is now at or after `front`.
        self.heads().map(|_| front)
    }

    /// Takes the oldest job whose every part is back: if every part is `Ok`,
    /// calls `combine(i, output)` for part 0, 1, … in order; then returns
    /// every part to its pool. Its sequence number and outcome: `Ok`, or the
    /// first part's (in part order) that is not. Parts of different jobs are
    /// never combined: when the parts' first finished jobs differ (a part
    /// lost older ones with its device, or the module discarded them), every
    /// outstanding job older than the newest of them is lost, and each is
    /// reported `DeviceLost` in turn, oldest first, with nothing combined
    /// (auditor-2-023: no job is forgotten without an outcome). A job a
    /// replaced part lost ([`refresh`](SplitChannel::refresh)) is taken as
    /// `DeviceLost` at once, without waiting for any part, and its surviving
    /// parts go back to their pools (now, or when they come back).
    pub fn take_combined(
        &mut self,
        mut combine: impl FnMut(usize, &[f32]),
    ) -> Option<(u64, Outcome)> {
        if self.lost == 0 {
            self.drop_stale();
            let (_, newest) = self.heads()?;
            self.lost = self.outstanding.iter().take_while(|s| **s < newest).count();
        }
        if self.lost > 0 {
            let seq = self.outstanding.pop_front()?;
            self.lost -= 1;
            self.drop_stale();
            return Some((seq, Outcome::DeviceLost));
        }
        // Every part's first finished job is the oldest outstanding one.
        let seq = self.outstanding.front().copied()?;
        self.forget_through(seq);
        for (part, slot) in self.parts.iter_mut().zip(self.taken.iter_mut()) {
            *slot = part.take_done();
        }
        let outcome = self
            .taken
            .iter()
            .flatten()
            .map(Job::outcome)
            .find(|o| *o != Outcome::Ok)
            .unwrap_or(Outcome::Ok);
        if outcome == Outcome::Ok {
            for (i, job) in self.taken.iter().enumerate() {
                if let Some(job) = job {
                    combine(i, job.output());
                }
            }
        }
        for (part, slot) in self.parts.iter_mut().zip(self.taken.iter_mut()) {
            if let Some(job) = slot.take()
                && let Err(job) = part.recycle(job)
            {
                // Never one of its own; never freed here.
                std::mem::forget(job);
            }
        }
        Some((seq, outcome))
    }

    /// Forgets the outstanding jobs up to `seq`, now taken.
    fn forget_through(&mut self, seq: u64) {
        while self.outstanding.front().is_some_and(|s| *s <= seq) {
            self.outstanding.pop_front();
        }
    }

    /// Returns to their pools the finished parts of jobs before `seq`: what
    /// a module does with parts whose other parts were lost.
    pub fn discard_before(&mut self, seq: u64) {
        for part in &mut self.parts {
            while part.peek_done().is_some_and(|done| done < seq) {
                if let Some(job) = part.take_done()
                    && let Err(job) = part.recycle(job)
                {
                    std::mem::forget(job);
                }
            }
        }
    }

    /// Takes every part's replacement after a device loss
    /// ([`PlacedChannel::refresh`]); whether any part changed. Jobs in
    /// flight on a replaced part never come back: every job outstanding then
    /// is lost, and [`take_combined`](SplitChannel::take_combined) reports
    /// each as `DeviceLost` without waiting for its parts.
    pub fn refresh(&mut self) -> bool {
        let mut any = false;
        for part in &mut self.parts {
            any |= part.refresh(|_| {});
        }
        if any {
            self.lost = self.outstanding.len();
        }
        any
    }
}
