#![allow(dead_code)]
use crate::types::*;
use crate::*;
use std::pin::Pin;
use std::task::Poll;

struct PoolCon<T: Tx3Transport> {
    con_term: Term,
    remote:
        Arc<<<T as Tx3Transport>::Common as Tx3TransportCommon>::EndpointId>,
    out_data_limit: Arc<tokio::sync::Semaphore>,
    out_send: tokio::sync::mpsc::UnboundedSender<Message<T>>,
}

impl<T: Tx3Transport> PoolCon<T> {
    pub fn new(
        pool_term: Term,
        remote: Arc<
            <<T as Tx3Transport>::Common as Tx3TransportCommon>::EndpointId,
        >,
        con: <<T as Tx3Transport>::Common as Tx3TransportCommon>::Connection,
        in_send: tokio::sync::mpsc::UnboundedSender<Message<T>>,
        max_in_bytes: usize,
        max_out_bytes: usize,
    ) -> Self {
        let con_term = Term::new();

        let (out_send, out_recv) = tokio::sync::mpsc::unbounded_channel();

        poll_con(
            pool_term,
            con_term.clone(),
            remote.clone(),
            con,
            out_recv,
            in_send,
            max_in_bytes,
        );

        let out_data_limit =
            Arc::new(tokio::sync::Semaphore::new(max_out_bytes));

        Self {
            con_term,
            remote,
            out_data_limit,
            out_send,
        }
    }

    pub fn terminate(&self) {
        self.con_term.term();
    }

    pub async fn send(&self, msg_res: MsgRes, data: BytesList) -> Result<()> {
        let permit = self
            .out_data_limit
            .clone()
            .acquire_many_owned(data.remaining() as u32)
            .await
            .map_err(|_| other_err("ConClosed"))?;
        let msg =
            <Message<T>>::new(self.remote.clone(), data, permit, Some(msg_res));
        self.out_send.send(msg).map_err(|_| other_err("ConClosed"))
    }
}

fn poll_con<T: Tx3Transport>(
    pool_term: Term,
    con_term: Term,
    remote: Arc<
        <<T as Tx3Transport>::Common as Tx3TransportCommon>::EndpointId,
    >,
    mut con: <<T as Tx3Transport>::Common as Tx3TransportCommon>::Connection,
    mut out_recv: tokio::sync::mpsc::UnboundedReceiver<Message<T>>,
    in_send: tokio::sync::mpsc::UnboundedSender<Message<T>>,
    max_in_bytes: usize,
) {
    let mut pool_term = pool_term.on_term();
    let mut con_term = con_term.on_term();
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));

    let mut out_recv_done = false;
    let mut out_cur_msg = None;

    let in_data_limit = Arc::new(tokio::sync::Semaphore::new(max_in_bytes));
    let mut in_buf = [0; 4096];
    let mut in_bytes_list = BytesList::new();
    let mut in_next_len = None;
    let mut in_cur_permit = None;
    let mut in_cur_permit_fut = None;
    tokio::task::spawn(futures::future::poll_fn(move |cx| {
        let mut did_something = false;

        // check to see if we've been terminated, and register waker
        // for termination signal
        match Pin::new(&mut pool_term).poll(cx) {
            Poll::Pending => (),
            Poll::Ready(_) => return Poll::Ready(()),
        }

        // check to see if we've been terminated, and register waker
        // for termination signal
        match Pin::new(&mut con_term).poll(cx) {
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
                remote.clone(),
                in_bytes_list.take_front(next_len as usize),
                permit,
                None,
            );
            if in_send.send(msg).is_err() {
                return Poll::Ready(());
            }
        }

        if did_something {
            // TODO _ FIXME
            tracing::trace!("TODO FIXME");
        }

        Poll::Pending
    }));
}
