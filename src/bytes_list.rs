#![allow(clippy::comparison_chain)]
use std::collections::VecDeque;

/// A list of Bytes that can be read / treated as a single unit
#[derive(Default)]
pub struct BytesList(pub VecDeque<bytes::Bytes>);

impl From<Vec<u8>> for BytesList {
    #[inline(always)]
    fn from(v: Vec<u8>) -> Self {
        let mut q = VecDeque::with_capacity(1);
        q.push_back(v.into());
        Self(q)
    }
}

impl From<bytes::Bytes> for BytesList {
    #[inline(always)]
    fn from(b: bytes::Bytes) -> Self {
        let mut q = VecDeque::with_capacity(1);
        q.push_back(b);
        Self(q)
    }
}

impl bytes::Buf for BytesList {
    fn remaining(&self) -> usize {
        self.0.iter().map(|b| b.remaining()).sum()
    }

    fn chunk(&self) -> &[u8] {
        match self.0.front() {
            Some(b) => b.chunk(),
            None => &[],
        }
    }

    fn advance(&mut self, mut cnt: usize) {
        while cnt > 0 {
            if self.0.is_empty() {
                return;
            }

            let next_len = self.0.front().unwrap().remaining();
            if next_len == cnt {
                self.0.pop_front();
                return;
            } else if next_len < cnt {
                cnt -= next_len;
                self.0.pop_front();
            } else {
                self.0.front_mut().unwrap().advance(cnt);
                return;
            }
        }
    }
}
