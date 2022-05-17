#![allow(dead_code)]
use crate::types::*;
use crate::*;
use std::pin::Pin;
use std::sync::atomic;
use std::task::Poll;

#[derive(Clone)]
pub(crate) struct PoolCon {
    con_term: Term,
    out_data_limit: Arc<tokio::sync::Semaphore>,
    out_send: tokio::sync::mpsc::UnboundedSender<Message>,
    idle_timeout_s: Arc<atomic::AtomicU8>,
}

impl PoolCon {
    pub fn new<T: Tx3Transport>(
        remote: Arc<Id<T>>,
        pool_term: Term,
        con_term: Term,
        con: <<T as Tx3Transport>::Common as Tx3TransportCommon>::Connection,
        in_send: tokio::sync::mpsc::UnboundedSender<(Arc<Id<T>>, Message)>,
        max_in_bytes: usize,
        max_out_bytes: usize,
    ) -> Self {
        let (out_send, out_recv) = tokio::sync::mpsc::unbounded_channel();
        let idle_timeout_s = Arc::new(atomic::AtomicU8::new(20));

        poll_con::<T>(
            remote,
            pool_term,
            con_term.clone(),
            con,
            out_recv,
            in_send,
            max_in_bytes,
            idle_timeout_s.clone(),
        );

        let out_data_limit =
            Arc::new(tokio::sync::Semaphore::new(max_out_bytes));

        Self {
            con_term,
            out_data_limit,
            out_send,
            idle_timeout_s,
        }
    }

    pub fn terminate(&self) {
        self.con_term.term();
    }

    pub fn set_idle_timeout_s(&self, idle_timeout_s: u8) {
        self.idle_timeout_s
            .store(idle_timeout_s, atomic::Ordering::Release);
    }

    pub async fn acquire_send_permit(
        &self,
        byte_count: u32,
    ) -> Result<tokio::sync::OwnedSemaphorePermit> {
        self.out_data_limit
            .clone()
            .acquire_many_owned(byte_count)
            .await
            .map_err(|_| other_err("ConClosed"))
    }

    pub fn send(&self, msg: Message) -> std::result::Result<(), Message> {
        self.out_send.send(msg).map_err(|err| err.0)
    }
}

#[allow(clippy::too_many_arguments)]
fn poll_con<T: Tx3Transport>(
    remote: Arc<Id<T>>,
    pool_term: Term,
    con_term: Term,
    mut con: <<T as Tx3Transport>::Common as Tx3TransportCommon>::Connection,
    mut out_recv: tokio::sync::mpsc::UnboundedReceiver<Message>,
    in_send: tokio::sync::mpsc::UnboundedSender<(Arc<Id<T>>, Message)>,
    max_in_bytes: usize,
    idle_timeout_s: Arc<atomic::AtomicU8>,
) {
    let mut pool_term_fut = pool_term.on_term();
    let mut con_term_fut = con_term.on_term();
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));

    let mut out_recv_done = false;
    let mut out_cur_msg = None;

    let in_data_limit = Arc::new(tokio::sync::Semaphore::new(max_in_bytes));
    let mut in_buf = [0; 4096];
    let mut in_bytes_list = BytesList::new();
    let mut in_next_len = None;
    let mut in_cur_permit = None;
    let mut in_cur_permit_fut = None;

    struct TermGuard(Term);

    impl Drop for TermGuard {
        fn drop(&mut self) {
            self.0.term();
        }
    }

    let term_guard = TermGuard(con_term);

    let mut last_action = tokio::time::Instant::now();
    tokio::task::spawn(futures::future::poll_fn(move |cx| {
        let _term_guard = &term_guard;

        let mut did_something = false;

        // check to see if we've been terminated, and register waker
        // for termination signal
        match Pin::new(&mut pool_term_fut).poll(cx) {
            Poll::Pending => (),
            Poll::Ready(_) => return Poll::Ready(()),
        }

        // check to see if we've been terminated, and register waker
        // for termination signal
        match Pin::new(&mut con_term_fut).poll(cx) {
            Poll::Pending => (),
            Poll::Ready(_) => return Poll::Ready(()),
        }

        // make sure we are scheduled to wake up at least once per second
        // to check our timeouts, etc
        'interval_loop: loop {
            match interval.poll_tick(cx) {
                Poll::Pending => break 'interval_loop,
                Poll::Ready(_) => (),
            }
        }

        // process all outgoing data until we've reached pending status
        'send_loop: loop {
            if !out_recv_done && out_cur_msg.is_none() {
                match out_recv.poll_recv(cx) {
                    Poll::Pending => (),
                    Poll::Ready(None) => out_recv_done = true,
                    Poll::Ready(Some(msg)) => {
                        out_cur_msg = Some(msg);
                        did_something = true;
                    }
                }
            }

            if out_cur_msg.is_none() {
                break 'send_loop;
            }

            if let Some(mut msg) = out_cur_msg.take() {
                while !msg.content.has_remaining() {
                    // TODO - vectored?
                    match Pin::new(&mut con).poll_write(cx, msg.content.chunk())
                    {
                        Poll::Pending => {
                            out_cur_msg = Some(msg);
                            break 'send_loop;
                        }
                        Poll::Ready(Err(err)) => {
                            tracing::debug!(?err);
                            return Poll::Ready(());
                        }
                        Poll::Ready(Ok(cnt)) => {
                            msg.content.advance(cnt);
                        }
                    }
                }
            }
        }

        // process all incoming data until we've reached pending status
        'recv_loop: loop {
            let mut in_buf = tokio::io::ReadBuf::new(&mut in_buf[..]);

            if in_next_len.is_none() && in_bytes_list.remaining() < 4 {
                in_buf.clear();
                match Pin::new(&mut con).poll_read(cx, &mut in_buf) {
                    Poll::Pending => break 'recv_loop,
                    Poll::Ready(Err(err)) => {
                        tracing::debug!(?err);
                        return Poll::Ready(());
                    }
                    Poll::Ready(Ok(_)) => {
                        did_something = true;
                        in_bytes_list.push(bytes::Bytes::copy_from_slice(
                            in_buf.filled(),
                        ));
                    }
                }
                if in_bytes_list.remaining() < 4 {
                    continue 'recv_loop;
                }
            }

            if in_next_len.is_none() {
                let next_len = in_bytes_list.get_u32_le();
                // TODO - FIXME - validate next_len bitfield here
                in_next_len = Some(next_len);
            }

            let next_len = in_next_len.unwrap();

            if in_cur_permit.is_none() {
                if in_cur_permit_fut.is_none() {
                    in_cur_permit_fut = Some(Box::pin(
                        in_data_limit.clone().acquire_many_owned(next_len),
                    ));
                }
                match Pin::new(in_cur_permit_fut.as_mut().unwrap()).poll(cx) {
                    Poll::Pending => break 'recv_loop,
                    Poll::Ready(Err(_)) => return Poll::Ready(()),
                    Poll::Ready(Ok(permit)) => {
                        did_something = true;
                        in_cur_permit = Some(permit);
                    }
                }
            }

            while in_bytes_list.remaining() < next_len as usize {
                in_buf.clear();
                match Pin::new(&mut con).poll_read(cx, &mut in_buf) {
                    Poll::Pending => break 'recv_loop,
                    Poll::Ready(Err(err)) => {
                        tracing::debug!(?err);
                        return Poll::Ready(());
                    }
                    Poll::Ready(Ok(_)) => {
                        did_something = true;
                        in_bytes_list.push(bytes::Bytes::copy_from_slice(
                            in_buf.filled(),
                        ));
                    }
                }
            }

            // we got a full message!
            in_next_len.take();
            let permit = in_cur_permit.take().unwrap();
            let msg = Message::new(
                in_bytes_list.take_front(next_len as usize),
                permit,
            );
            if in_send.send((remote.clone(), msg)).is_err() {
                return Poll::Ready(());
            }
        }

        if did_something {
            last_action = tokio::time::Instant::now();
        }

        let cur_idle_timeout_s = idle_timeout_s.load(atomic::Ordering::Acquire);

        if last_action.elapsed().as_millis() as u64
            >= (cur_idle_timeout_s as u64) * 1000
        {
            return Poll::Ready(());
        }

        Poll::Pending
    }));
}
