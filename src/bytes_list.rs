#![allow(clippy::comparison_chain)]

#[ouroboros::self_referencing]
struct BytesListInner {
    data: Vec<bytes::Bytes>,
    #[borrows(data)]
    #[covariant]
    io_slice: Vec<std::io::IoSlice<'this>>,
}

impl BytesListInner {
    fn priv_construct(data: Vec<bytes::Bytes>) -> Option<Self> {
        if data.is_empty() {
            None
        } else {
            Some(
                BytesListInnerBuilder {
                    data,
                    io_slice_builder: |data| {
                        let mut out = Vec::with_capacity(data.len());
                        for item in data.iter() {
                            out.push(std::io::IoSlice::new(&item[..]));
                        }
                        out
                    },
                }
                .build(),
            )
        }
    }

    fn priv_advance(self, mut amount: usize) -> Option<Self> {
        let mut data = self.into_heads().data;

        loop {
            if data.is_empty() {
                return None;
            }

            let next_len = data[0].len();
            if next_len < amount {
                amount -= next_len;
                data.remove(0);
            } else if next_len == amount {
                data.remove(0);
                break;
            } else {
                use bytes::Buf;
                data[0].advance(amount);
                break;
            }
        }

        if data.is_empty() {
            None
        } else {
            Self::priv_construct(data)
        }
    }
}

/// A list of Bytes that can be read / treated as a single unit
pub struct BytesList(Option<BytesListInner>);

impl BytesList {
    /// Construct a new BytesLis
    pub fn new(mut data: Vec<bytes::Bytes>) -> Self {
        data.retain(|b| !b.is_empty());
        BytesList(BytesListInner::priv_construct(data))
    }

    /// Extract the internal Vec<Bytes>
    pub fn into_inner(self) -> Vec<bytes::Bytes> {
        match self.0 {
            None => vec![],
            Some(inner) => inner.into_heads().data,
        }
    }

    /// Treat this BytesList as a slice of IoSlices
    pub fn as_io_slice(&self) -> &[std::io::IoSlice<'_>] {
        match &self.0 {
            None => &[],
            Some(inner) => inner.borrow_io_slice(),
        }
    }
}

impl bytes::Buf for BytesList {
    fn remaining(&self) -> usize {
        if let Some(inner) = &self.0 {
            inner.borrow_data().iter().map(|c| c.remaining()).sum()
        } else {
            0
        }
    }

    fn chunk(&self) -> &[u8] {
        if let Some(inner) = &self.0 {
            inner.borrow_data()[0].chunk()
        } else {
            &[]
        }
    }

    fn advance(&mut self, cnt: usize) {
        if let Some(inner) = self.0.take() {
            self.0 = inner.priv_advance(cnt);
        }
    }
}
