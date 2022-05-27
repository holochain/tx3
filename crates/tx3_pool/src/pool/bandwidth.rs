use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;

const ONE_SEC: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Clone)]
pub(crate) struct Bandwidth(Arc<Mutex<BwInner>>);

impl Bandwidth {
    pub fn new(
        bucket_start: tokio::time::Instant,
        bucket_count: usize,
    ) -> Self {
        Self(Arc::new(Mutex::new(BwInner::new(
            bucket_start,
            bucket_count,
        ))))
    }

    pub fn add(&self, byte_count: usize) -> Option<usize> {
        let mut inner = self.0.lock();
        let out = inner.tick();
        inner.add(byte_count);
        out
    }

    pub fn get(&self) -> Option<usize> {
        self.0.lock().tick()
    }
}

struct BwInner {
    bucket_start: tokio::time::Instant,
    bucket_count: usize,
    cur_bucket: usize,
    buckets: VecDeque<usize>,
}

impl BwInner {
    fn new(bucket_start: tokio::time::Instant, bucket_count: usize) -> Self {
        Self {
            bucket_start,
            bucket_count,
            cur_bucket: 0,
            // if we tick more frequently than once per second
            // we should only ever need bucket_count + 1
            buckets: VecDeque::with_capacity(bucket_count + 1),
        }
    }

    /// only call this *after* tick, because the cur_bucket
    /// may need to be adjusted first.
    fn add(&mut self, byte_count: usize) {
        self.cur_bucket += byte_count;
    }

    fn tick(&mut self) -> Option<usize> {
        let now = tokio::time::Instant::now();
        while now - self.bucket_start > ONE_SEC {
            self.bucket_start += ONE_SEC;
            self.buckets.push_back(self.cur_bucket);
            self.cur_bucket = 0;
        }

        while self.buckets.len() > self.bucket_count {
            self.buckets.pop_front();
        }

        if self.buckets.len() == self.bucket_count {
            Some(self.buckets.iter().sum::<usize>() / self.buckets.len())
        } else {
            None
        }
    }
}
