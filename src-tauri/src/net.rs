use netstat2::{get_sockets_info, AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    /// tcp | udp
    pub protocol: String,
    pub local_address: String,
    pub local_port: u16,
    pub remote_address: String,
    pub remote_port: u16,
    /// TCP 状态（LISTEN/ESTABLISHED/...）；UDP 为空
    pub state: String,
    pub pid: u32,
}

pub fn list_tcp() -> Result<Vec<Connection>, String> {
    list_family(AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6, ProtocolFlags::TCP, "tcp")
}

pub fn list_udp() -> Result<Vec<Connection>, String> {
    list_family(AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6, ProtocolFlags::UDP, "udp")
}

fn list_family(af: AddressFamilyFlags, pf: ProtocolFlags, protocol: &str) -> Result<Vec<Connection>, String> {
    let sockets = get_sockets_info(af, pf).map_err(|e| format!("PORT_QUERY_FAILED:{e}"))?;
    let mut out = Vec::new();
    for sock in sockets {
        let (local_addr, local_port, remote_addr, remote_port, state, pid) = match &sock.protocol_socket_info {
            ProtocolSocketInfo::Tcp(t) => (
                t.local_addr.to_string(),
                t.local_port,
                t.remote_addr.to_string(),
                t.remote_port,
                format!("{:?}", t.state).to_uppercase(),
                sock.associated_pids.first().copied().unwrap_or(0),
            ),
            ProtocolSocketInfo::Udp(u) => (
                u.local_addr.to_string(),
                u.local_port,
                String::new(),
                0,
                String::new(),
                sock.associated_pids.first().copied().unwrap_or(0),
            ),
        };
        out.push(Connection {
            protocol: protocol.to_string(),
            local_address: local_addr,
            local_port,
            remote_address: remote_addr,
            remote_port,
            state,
            pid,
        });
    }
    Ok(out)
}

