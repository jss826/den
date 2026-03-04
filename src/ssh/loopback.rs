use std::collections::HashSet;
use std::net::SocketAddr;

/// Check if a socket address is local (loopback or one of this machine's IPs).
///
/// Uses the UDP bind trick: if we can bind to the IP, it's ours.
/// This catches both `127.0.0.1` and the machine's real IP addresses.
pub fn is_local_address(addr: &SocketAddr) -> bool {
    let ip = addr.ip();
    if ip.is_loopback() {
        return true;
    }
    // Unspecified (0.0.0.0 / ::) is not a real peer address — treat as non-local
    if ip.is_unspecified() {
        return false;
    }
    // Try to bind a UDP socket to this IP — succeeds only for local addresses
    let bind_addr = SocketAddr::new(ip, 0);
    std::net::UdpSocket::bind(bind_addr).is_ok()
}

/// Find the PID of the process that owns the TCP connection from `peer` to `local_port`.
///
/// Uses `GetExtendedTcpTable` to scan the IPv4 TCP table for a matching connection.
/// Handles IPv4-mapped IPv6 addresses (e.g. `::ffff:127.0.0.1`).
#[cfg(windows)]
pub fn find_tcp_peer_pid(peer: SocketAddr, local_port: u16) -> Option<u32> {
    use std::net::IpAddr;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_CONNECTIONS,
    };

    // AF_INET = 2 (avoid pulling in Win32_Networking_WinSock feature)
    const AF_INET: u32 = 2;

    // Extract IPv4 address (handle IPv4-mapped IPv6)
    let peer_ipv4 = match peer.ip() {
        IpAddr::V4(v4) => v4,
        IpAddr::V6(v6) => v6.to_ipv4_mapped()?,
    };
    let peer_port = peer.port();
    let peer_addr_be = u32::from_ne_bytes(peer_ipv4.octets());

    let mut buf: Vec<u8> = Vec::new();
    let mut size: u32 = 0;

    // First call to get required size
    // SAFETY: GetExtendedTcpTable with null buffer and valid size pointer returns
    // ERROR_INSUFFICIENT_BUFFER and sets size to the required buffer size.
    unsafe {
        GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut size,
            0, // no sort
            AF_INET,
            TCP_TABLE_OWNER_PID_CONNECTIONS,
            0,
        );
    }
    if size == 0 {
        return None;
    }

    buf.resize(size as usize, 0);

    // Second call to fill the buffer
    // SAFETY: buf is sized according to the first call's reported size.
    // GetExtendedTcpTable writes MIB_TCPTABLE_OWNER_PID into the buffer.
    let ret = unsafe {
        GetExtendedTcpTable(
            buf.as_mut_ptr().cast(),
            &mut size,
            0,
            AF_INET,
            TCP_TABLE_OWNER_PID_CONNECTIONS,
            0,
        )
    };
    if ret != 0 {
        return None;
    }

    // SAFETY: On success, buf contains a valid MIB_TCPTABLE_OWNER_PID header.
    let table = unsafe { &*(buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID) };
    let num_entries = table.dwNumEntries as usize;

    // Bounds check: verify the buffer is large enough for the reported number of entries.
    // The TCP table can grow between the size-query and fill calls, so dwNumEntries
    // could exceed what the buffer can hold.
    let header_size = std::mem::size_of::<MIB_TCPTABLE_OWNER_PID>();
    let row_size = std::mem::size_of_val(&table.table[0]);
    let required = header_size.saturating_add(num_entries.saturating_mul(row_size));
    if (size as usize) < required {
        return None;
    }

    // SAFETY: The table header is followed by `dwNumEntries` MIB_TCPROW_OWNER_PID entries.
    // We verified the buffer is large enough above.
    let rows = unsafe { std::slice::from_raw_parts(table.table.as_ptr(), num_entries) };

    for row in rows {
        // Remote = peer's address/port (the SSH client side)
        let remote_addr = row.dwRemoteAddr;
        let remote_port = (row.dwRemotePort as u16).to_be();
        // Local = our SSH server side
        let local = (row.dwLocalPort as u16).to_be();

        if remote_addr == peer_addr_be && remote_port == peer_port && local == local_port {
            return Some(row.dwOwningPid);
        }
    }

    None
}

#[cfg(not(windows))]
pub fn find_tcp_peer_pid(_peer: SocketAddr, _local_port: u16) -> Option<u32> {
    None
}

/// Check if the peer process is a descendant of any Den PTY child process.
///
/// Walks the process tree upward from `pid` using `CreateToolhelp32Snapshot`,
/// checking if any ancestor is in `child_pids`. Limits to 32 hops to prevent
/// infinite loops from circular parent references.
#[cfg(windows)]
pub fn is_self_connection(peer: SocketAddr, local_port: u16, child_pids: &HashSet<u32>) -> bool {
    if child_pids.is_empty() {
        return false;
    }

    let Some(pid) = find_tcp_peer_pid(peer, local_port) else {
        return false;
    };

    // Build parent map from process snapshot
    let parent_map = build_parent_map();

    // Walk ancestors
    const MAX_HOPS: usize = 32;
    let mut current = pid;
    for _ in 0..MAX_HOPS {
        if child_pids.contains(&current) {
            return true;
        }
        match parent_map.get(&current) {
            Some(&parent) if parent != current && parent != 0 => {
                current = parent;
            }
            _ => break,
        }
    }

    false
}

/// Build a PID → parent PID map using CreateToolhelp32Snapshot.
#[cfg(windows)]
fn build_parent_map() -> std::collections::HashMap<u32, u32> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32, Process32First, Process32Next, TH32CS_SNAPPROCESS,
    };

    let mut map = std::collections::HashMap::new();

    // SAFETY: TH32CS_SNAPPROCESS with 0 takes a snapshot of all processes.
    // Returns INVALID_HANDLE_VALUE on failure.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return map;
    }

    let mut entry = PROCESSENTRY32 {
        dwSize: std::mem::size_of::<PROCESSENTRY32>() as u32,
        ..unsafe { std::mem::zeroed() }
    };

    // SAFETY: Process32First reads the first process entry from a valid snapshot handle.
    if unsafe { Process32First(snap, &mut entry) } != 0 {
        loop {
            map.insert(entry.th32ProcessID, entry.th32ParentProcessID);
            // SAFETY: Process32Next reads the next process entry. Returns 0 when done.
            if unsafe { Process32Next(snap, &mut entry) } == 0 {
                break;
            }
        }
    }

    // SAFETY: snap is a valid handle from CreateToolhelp32Snapshot.
    unsafe { CloseHandle(snap) };

    map
}

#[cfg(not(windows))]
pub fn is_self_connection(_peer: SocketAddr, _local_port: u16, _child_pids: &HashSet<u32>) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_is_local() {
        let addr: SocketAddr = "127.0.0.1:12345".parse().unwrap();
        assert!(is_local_address(&addr));
    }

    #[test]
    fn ipv6_loopback_is_local() {
        let addr: SocketAddr = "[::1]:12345".parse().unwrap();
        assert!(is_local_address(&addr));
    }

    #[test]
    fn unspecified_is_not_local() {
        let addr: SocketAddr = "0.0.0.0:12345".parse().unwrap();
        assert!(!is_local_address(&addr));
    }

    #[test]
    fn remote_is_not_local() {
        // 192.0.2.1 is TEST-NET-1 (RFC 5737), guaranteed not to be a local address
        let addr: SocketAddr = "192.0.2.1:12345".parse().unwrap();
        assert!(!is_local_address(&addr));
    }

    #[test]
    fn empty_child_pids_returns_false() {
        let addr: SocketAddr = "127.0.0.1:12345".parse().unwrap();
        let empty = HashSet::new();
        assert!(!is_self_connection(addr, 22, &empty));
    }
}
