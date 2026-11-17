use rusqlite::Connection;
use std::net::{IpAddr, Ipv4Addr, TcpListener, UdpSocket};

/// Discover the private IPv4 address used by the machine's default outbound route.
/// This does not send application data; UDP connect selects a route locally.
pub fn lan_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(("8.8.8.8", 80)).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(address) if address.is_private() => Some(address),
        _ => None,
    }
}

/// Reserve an unused ephemeral TCP port and avoid ports already assigned to hosted apps.
pub fn allocate_lan_port(connection: &Connection) -> Result<u16, String> {
    for _ in 0..64 {
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
            .map_err(|e| format!("Could not find an available LAN port: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let in_use: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM websites WHERE port=?1 OR lan_port=?1)",
                [port],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        drop(listener);
        if !in_use {
            return Ok(port);
        }
    }
    Err("Could not allocate a unique LAN port after 64 attempts.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_a_port_not_already_in_registry() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE websites(port INTEGER, lan_port INTEGER);").unwrap();
        let port = allocate_lan_port(&db).unwrap();
        assert_ne!(port, 0);
        db.execute("INSERT INTO websites(lan_port) VALUES (?1)", [port]).unwrap();
        let next = allocate_lan_port(&db).unwrap();
        assert_ne!(port, next);
    }
}
