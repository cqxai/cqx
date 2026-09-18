//! H6 — the effect is a method on a field whose type carries the meaning.
//! Expected base: MISS as an effect, but the `has_field` edge does record that
//! this struct holds a TcpStream, which is a type-level hint at the same fact.
use std::net::TcpStream;

pub struct Conn {
    stream: TcpStream,
}

impl Conn {
    pub fn peer(&self) -> String {
        match self.stream.peer_addr() {
            Ok(addr) => addr.to_string(),
            Err(_) => String::new(),
        }
    }
}
