//! WebSocket transport for [`Session`], through the page's own script (`web/gsim.js`): each
//! message travels as one binary frame, in order.

use crate::session::{Controls, Session, SessionConfig};
use gsim_proto::*;

extern "C" {
    /// Open a socket to the server named by these bytes (empty: the one the page came from).
    fn gsim_ws_open(ptr: *const u8, len: usize);
    /// 0 while connecting, 1 once open, 2 when closed.
    fn gsim_ws_state() -> i32;
    fn gsim_ws_send(ptr: *const u8, len: usize);
    /// Length of the next message received, or -1; `gsim_ws_take` then copies it out.
    fn gsim_ws_next() -> i32;
    fn gsim_ws_take(ptr: *mut u8);
    fn gsim_ws_close();
    /// Seconds since the page loaded.
    fn gsim_now() -> f64;
}

pub struct NetClient {
    pub session: Session,
    start: f64,
    held: Vec<ClientMsg>,
    /// Bytes sent and received since `since`, and the rates over the window before.
    counted: (f64, f64),
    since: f64,
    rates: (f64, f64),
}

impl NetClient {
    pub fn connect(server: &str, cfg: SessionConfig) -> Result<Self, String> {
        // SAFETY: the script only reads the bytes during the call.
        let now = unsafe {
            gsim_ws_open(server.as_ptr(), server.len());
            gsim_now()
        };
        Ok(Self { session: Session::new(cfg), start: now, held: Vec::new(), counted: (0.0, 0.0), since: 0.0, rates: (0.0, 0.0) })
    }

    /// Seconds since this client was created (the session's clock).
    pub fn now(&self) -> f64 {
        unsafe { gsim_now() - self.start }
    }

    pub fn is_connected(&self) -> bool {
        unsafe { gsim_ws_state() == 1 }
    }

    pub fn disconnect_reason(&self) -> Option<String> {
        (unsafe { gsim_ws_state() } == 2).then(|| "connection to the server closed".to_string())
    }

    pub fn bytes_per_sec(&self) -> (f64, f64) {
        self.rates
    }

    /// Pump the network and advance the session. Call once per frame.
    pub fn update(&mut self, controls: Controls) {
        let now = self.now();
        loop {
            let len = unsafe { gsim_ws_next() };
            if len < 0 {
                break;
            }
            let mut bytes = vec![0u8; len as usize];
            // SAFETY: the buffer is as long as the script said the message is.
            unsafe { gsim_ws_take(bytes.as_mut_ptr()) };
            self.counted.1 += bytes.len() as f64;
            if let Some(msg) = decode::<ServerMsg>(&bytes) {
                self.session.handle(msg, now);
            }
        }
        self.session.update(now, controls);
        self.held.extend(self.session.drain_out());
        if self.is_connected() {
            for msg in self.held.drain(..) {
                let bytes = encode(&msg);
                self.counted.0 += bytes.len() as f64;
                unsafe { gsim_ws_send(bytes.as_ptr(), bytes.len()) };
            }
        }
        if now - self.since >= 1.0 {
            let span = now - self.since;
            self.rates = (self.counted.0 / span, self.counted.1 / span);
            (self.counted, self.since) = ((0.0, 0.0), now);
        }
    }

    pub fn disconnect(&mut self) {
        unsafe { gsim_ws_close() };
    }
}
