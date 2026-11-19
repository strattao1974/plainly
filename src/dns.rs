//! A tiny DNS client, so we can ask a specific DNS server directly instead of trusting Windows' cache.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

pub fn build_query(id: u16, name: &str) -> Vec<u8> {
    let mut q = Vec::with_capacity(32 + name.len());
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&0x0100u16.to_be_bytes()); // standard query, recursion desired
    q.extend_from_slice(&1u16.to_be_bytes());
    q.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    for label in name.trim_end_matches('.').split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&1u16.to_be_bytes()); // A
    q.extend_from_slice(&1u16.to_be_bytes()); // IN
    q
}

#[derive(Debug, PartialEq)]
pub enum DnsError {
    Timeout,
    BadReply,
    NoSuchName,
    ServerFailure(u8),
    Network(String),
}

pub fn parse_response(id: u16, buf: &[u8]) -> Result<u16, DnsError> {
    if buf.len() < 12 || u16::from_be_bytes([buf[0], buf[1]]) != id || buf[2] & 0x80 == 0 {
        return Err(DnsError::BadReply);
    }
    match buf[3] & 0x0f {
        0 => Ok(u16::from_be_bytes([buf[6], buf[7]])),
        3 => Err(DnsError::NoSuchName),
        code => Err(DnsError::ServerFailure(code)),
    }
}

pub fn query(server: IpAddr, name: &str, timeout: Duration) -> Result<Duration, DnsError> {
    let bind: SocketAddr = if server.is_ipv4() { "0.0.0.0:0".parse().unwrap() } else { "[::]:0".parse().unwrap() };
    let sock = UdpSocket::bind(bind).map_err(|e| DnsError::Network(e.to_string()))?;
    sock.set_read_timeout(Some(timeout)).ok();
    let id = (Instant::now().elapsed().subsec_nanos() as u16) ^ (std::process::id() as u16) ^ 0x5a5a;
    let q = build_query(id, name);
    let start = Instant::now();
    sock.send_to(&q, SocketAddr::new(server, 53)).map_err(|e| DnsError::Network(e.to_string()))?;
    let mut buf = [0u8; 1500];
    loop {
        match sock.recv_from(&mut buf) {
            Ok((n, _)) => match parse_response(id, &buf[..n]) {
                Err(DnsError::BadReply) if start.elapsed() < timeout => continue,
                Ok(answers) if answers > 0 => return Ok(start.elapsed()),
                Ok(_) => return Err(DnsError::NoSuchName),
                Err(e) => return Err(e),
            },
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                return Err(DnsError::Timeout)
            }
            Err(e) => return Err(DnsError::Network(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_well_formed_query() {
        let q = build_query(0x1234, "www.example.com");
        assert_eq!(&q[..4], &[0x12, 0x34, 0x01, 0x00]);
        assert_eq!(&q[12..16], &[3, b'w', b'w', b'w']);
        assert_eq!(&q[q.len() - 5..], &[0, 0, 1, 0, 1]);
    }

    #[test]
    fn parses_reply_codes() {
        let mut r = vec![0x12, 0x34, 0x81, 0x80, 0, 1, 0, 2, 0, 0, 0, 0];
        assert_eq!(parse_response(0x1234, &r), Ok(2));
        r[3] = 0x83;
        assert_eq!(parse_response(0x1234, &r), Err(DnsError::NoSuchName));
        r[3] = 0x82;
        assert_eq!(parse_response(0x1234, &r), Err(DnsError::ServerFailure(2)));
        assert_eq!(parse_response(0x9999, &r), Err(DnsError::BadReply));
    }
}
