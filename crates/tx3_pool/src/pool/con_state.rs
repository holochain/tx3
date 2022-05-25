//! [wait_send_s] = 3 sec
//! [max_msg_count] = 10
//! [max_msg_bytes] = # bytes
//! [max_msg_time_s] = 20 sec
//!
//! [rate_min_byte_per_s] = 65_536 bytes
//! [rate_min_window_s] = 5 seconds
//!
//! - [rate_min_byte_per_s] bytes over [rate_min_window_s] seconds
//!   i.e. if we haven't received+sent at least 2,621,440 bytes
//!   in the last sliding window of 5 seconds (twice, see below),
//!   we'll consider the connection idle and close it.
//!   Equates to 524.288 Kbps up+down.
//! - total bytes read/written are recorded once per second.
//!   when [rate_min_window_s] is reached, if below [rate_min_byte_per_s]:
//!   - if ![want_close]: post [want_close], clear data to reset idle timer.
//!   - if [want_close]: stop read/write, do NOT send [close], drop whole con.
//!
//! write -> if con has been open at least [wait_send_s] && no more data:
//!          send [close] (zero len msg) && shutdown write half.
//! write -> if [max_msg_count] / [max_msg_bytes] / [max_msg_time_s] passed:
//!          send [close] (zero len msg) && shutdown write half.
//!          (checked after completing current message)
//! write -> if [want_close]:
//!          send [close] (zero len msg) && shutdown write half.
//!          (checked after completing current message)
//! write -> on [error]: drop whole connection.
//!
//! read <- on [close]: close read half + post [want_close].
//! read <- if [max_msg_count] / [max_msg_bytes] / [max_msg_time_s] passed:
//!         close read half.
//!         Since [max_msg_time_s] is not entirely objective, read side SHOULD
//!         use ([max_msg_time_s] * 1.5).
//! read <- on [error]: drop whole connection.
