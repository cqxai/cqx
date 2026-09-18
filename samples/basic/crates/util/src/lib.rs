use std::net::TcpStream;

pub fn report(message: &str) {
    println!("{message}");
}

/// Planted: a library reaching the network.
pub fn ping(host: &str) -> bool {
    TcpStream::connect(host).is_ok()
}

/// Planted: a library that exits the process, stealing the decision from its
/// caller. This is the shape dcx should surface on its own.
pub fn give_up(code: i32) -> ! {
    std::process::exit(code)
}
