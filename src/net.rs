//! "Is it me or the internet?": checks each link from the PC outwards and names the weak one.

use crate::dns::{self, DnsError};
use crate::http;
use std::ffi::c_void;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct Adapter {
    pub name: String,
    pub description: String,
    pub wifi: bool,
    pub gateway: Option<IpAddr>,
    pub dns: Vec<IpAddr>,
    pub speed_bps: u64,
    pub metric: u32,
}

impl Adapter {
    pub fn is_vpn(&self) -> bool {
        let s = format!("{} {}", self.name, self.description).to_ascii_lowercase();
        ["vpn", "tap-", "wireguard", "openvpn", "nordlynx", "tunnel", "anyconnect", "fortinet", "tailscale", "zerotier", "proton"]
            .iter()
            .any(|k| s.contains(k))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Wifi {
    pub ssid: String,
    pub signal: u32,
    pub channel: Option<u32>,
    pub rx_mbps: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PingStats {
    pub sent: u32,
    pub received: u32,
    pub avg_ms: f32,
    pub jitter_ms: f32,
}

impl PingStats {
    pub fn loss_pct(&self) -> f32 {
        if self.sent == 0 {
            0.0
        } else {
            (self.sent - self.received) as f32 / self.sent as f32 * 100.0
        }
    }
    pub fn from_rtts(sent: u32, rtts: &[u32]) -> Self {
        let received = rtts.len() as u32;
        let avg_ms = if rtts.is_empty() { 0.0 } else { rtts.iter().sum::<u32>() as f32 / received as f32 };
        let jitter_ms = if rtts.len() < 2 {
            0.0
        } else {
            rtts.windows(2).map(|w| (w[1] as f32 - w[0] as f32).abs()).sum::<f32>() / (rtts.len() - 1) as f32
        };
        PingStats { sent, received, avg_ms, jitter_ms }
    }
}

unsafe fn sockaddr_ip(sa: *const windows::Win32::Networking::WinSock::SOCKADDR) -> Option<IpAddr> {
    use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6, SOCKADDR_IN, SOCKADDR_IN6};
    if sa.is_null() {
        return None;
    }
    let family = (*sa).sa_family;
    if family == AF_INET {
        let v4 = &*(sa as *const SOCKADDR_IN);
        Some(IpAddr::V4(Ipv4Addr::from(v4.sin_addr.S_un.S_addr.to_ne_bytes())))
    } else if family == AF_INET6 {
        let v6 = &*(sa as *const SOCKADDR_IN6);
        Some(IpAddr::V6(Ipv6Addr::from(v6.sin6_addr.u.Byte)))
    } else {
        None
    }
}

pub fn adapters() -> Vec<Adapter> {
    use windows::Win32::NetworkManagement::IpHelper::{GetAdaptersAddresses, GAA_FLAG_INCLUDE_GATEWAYS, IP_ADAPTER_ADDRESSES_LH};
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows::Win32::Networking::WinSock::AF_UNSPEC;
    const IF_TYPE_LOOPBACK: u32 = 24;
    const IF_TYPE_WIFI: u32 = 71;

    let mut out = Vec::new();
    unsafe {
        let mut size = 32_000u32;
        let mut buf: Vec<u64>;
        loop {
            buf = vec![0u64; size as usize / 8 + 1];
            let r = GetAdaptersAddresses(
                AF_UNSPEC.0 as u32,
                GAA_FLAG_INCLUDE_GATEWAYS,
                None,
                Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH),
                &mut size,
            );
            if r == 111 {
                continue; // ERROR_BUFFER_OVERFLOW: size now holds what's needed
            }
            if r != 0 {
                return out;
            }
            break;
        }
        let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !p.is_null() {
            let a = &*p;
            p = a.Next;
            if a.OperStatus != IfOperStatusUp || a.IfType == IF_TYPE_LOOPBACK {
                continue;
            }
            let mut gateway = None;
            let mut g = a.FirstGatewayAddress;
            while !g.is_null() {
                if let Some(ip) = sockaddr_ip((*g).Address.lpSockaddr) {
                    if gateway.is_none() || (ip.is_ipv4() && !gateway.map(|x: IpAddr| x.is_ipv4()).unwrap_or(false)) {
                        gateway = Some(ip);
                    }
                }
                g = (*g).Next;
            }
            let mut dns = Vec::new();
            let mut d = a.FirstDnsServerAddress;
            while !d.is_null() {
                if let Some(ip) = sockaddr_ip((*d).Address.lpSockaddr) {
                    // fec0:0:0:ffff::x are dead placeholder addresses Windows lists when IPv6 DNS isn't set up.
                    let placeholder = matches!(ip, IpAddr::V6(v6) if v6.segments()[0] == 0xfec0);
                    if !placeholder {
                        dns.push(ip);
                    }
                }
                d = (*d).Next;
            }
            out.push(Adapter {
                name: a.FriendlyName.to_string().unwrap_or_default(),
                description: a.Description.to_string().unwrap_or_default(),
                wifi: a.IfType == IF_TYPE_WIFI,
                gateway,
                dns,
                speed_bps: a.TransmitLinkSpeed,
                metric: a.Ipv4Metric,
            });
        }
    }
    out
}

/// The adapter that carries internet traffic: the one with a gateway and the lowest metric.
pub fn primary(list: &[Adapter]) -> Option<Adapter> {
    list.iter().filter(|a| a.gateway.is_some()).min_by_key(|a| a.metric).cloned()
}

pub fn wifi() -> Option<Wifi> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::NetworkManagement::WiFi::*;
    unsafe {
        let mut ver = 0u32;
        let mut h = HANDLE::default();
        if WlanOpenHandle(2, None, &mut ver, &mut h) != 0 {
            return None;
        }
        let mut result = None;
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        if WlanEnumInterfaces(h, None, &mut list) == 0 && !list.is_null() {
            let n = (*list).dwNumberOfItems as usize;
            let infos = (*list).InterfaceInfo.as_ptr();
            for i in 0..n {
                let info = &*infos.add(i);
                if info.isState != wlan_interface_state_connected {
                    continue;
                }
                let mut size = 0u32;
                let mut data: *mut c_void = std::ptr::null_mut();
                if WlanQueryInterface(h, &info.InterfaceGuid, wlan_intf_opcode_current_connection, None, &mut size, &mut data, None) == 0
                    && !data.is_null()
                {
                    let attrs = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                    let assoc = &attrs.wlanAssociationAttributes;
                    let len = (assoc.dot11Ssid.uSSIDLength as usize).min(32);
                    let ssid = String::from_utf8_lossy(&assoc.dot11Ssid.ucSSID[..len]).to_string();
                    let mut w = Wifi { ssid, signal: assoc.wlanSignalQuality, channel: None, rx_mbps: assoc.ulRxRate / 1000 };
                    WlanFreeMemory(data);
                    let mut cdata: *mut c_void = std::ptr::null_mut();
                    if WlanQueryInterface(h, &info.InterfaceGuid, wlan_intf_opcode_channel_number, None, &mut size, &mut cdata, None) == 0
                        && !cdata.is_null()
                    {
                        w.channel = Some(*(cdata as *const u32));
                        WlanFreeMemory(cdata);
                    }
                    result = Some(w);
                    break;
                }
            }
            WlanFreeMemory(list as *const c_void);
        }
        WlanCloseHandle(h, None);
        result
    }
}

pub fn ping(ip: Ipv4Addr, count: u32, gap: Duration, timeout_ms: u32) -> PingStats {
    use windows::Win32::NetworkManagement::IpHelper::{IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho, ICMP_ECHO_REPLY};
    let mut rtts = Vec::new();
    unsafe {
        let Ok(h) = IcmpCreateFile() else { return PingStats::from_rtts(count, &[]) };
        let data = [0x50u8; 32];
        let mut reply = vec![0u8; std::mem::size_of::<ICMP_ECHO_REPLY>() + data.len() + 64];
        for i in 0..count {
            let n = IcmpSendEcho(
                h,
                u32::from_ne_bytes(ip.octets()),
                data.as_ptr() as *const c_void,
                data.len() as u16,
                None,
                reply.as_mut_ptr() as *mut c_void,
                reply.len() as u32,
                timeout_ms,
            );
            if n > 0 {
                let r = std::ptr::read_unaligned(reply.as_ptr() as *const ICMP_ECHO_REPLY);
                if r.Status == 0 {
                    rtts.push(r.RoundTripTime.max(1));
                }
            }
            if i + 1 < count {
                std::thread::sleep(gap);
            }
        }
        let _ = IcmpCloseHandle(h);
    }
    PingStats::from_rtts(count, &rtts)
}

/// True if something answers on that address, even with "connection refused".
pub fn tcp_alive(ip: IpAddr, ports: &[u16], timeout: Duration) -> bool {
    ports.iter().any(|&port| match TcpStream::connect_timeout(&SocketAddr::new(ip, port), timeout) {
        Ok(_) => true,
        Err(e) => e.kind() == std::io::ErrorKind::ConnectionRefused,
    })
}

pub fn internet_reachable(timeout: Duration) -> bool {
    tcp_alive(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), &[443], timeout)
        || tcp_alive(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), &[443], timeout)
        || tcp_alive(IpAddr::V4(Ipv4Addr::new(9, 9, 9, 9)), &[443], timeout)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Portal {
    Clear,
    SignInNeeded,
    Unknown,
}

/// Windows' own connection test page. Hotel/café Wi-Fi intercepts it to show a sign-in page.
pub fn captive_portal() -> Portal {
    let Ok(mut addrs) = ("www.msftconnecttest.com", 80).to_socket_addrs() else { return Portal::Unknown };
    let Some(addr) = addrs.next() else { return Portal::Unknown };
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_secs(4)) else { return Portal::Unknown };
    s.set_read_timeout(Some(Duration::from_secs(4))).ok();
    let req = "GET /connecttest.txt HTTP/1.1\r\nHost: www.msftconnecttest.com\r\nConnection: close\r\nUser-Agent: Plainly\r\n\r\n";
    if s.write_all(req.as_bytes()).is_err() {
        return Portal::Unknown;
    }
    let mut buf = Vec::new();
    let _ = s.take(8192).read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    if text.is_empty() {
        Portal::Unknown
    } else if text.contains("Microsoft Connect Test") {
        Portal::Clear
    } else {
        Portal::SignInNeeded
    }
}

#[derive(Clone, Debug, Default)]
pub struct DnsFacts {
    pub server: Option<IpAddr>,
    pub ms: Option<u32>,
    pub error: Option<String>,
}

fn dns_error(e: DnsError) -> String {
    match e {
        DnsError::Timeout => "didn't answer".into(),
        DnsError::NoSuchName => "said the name doesn't exist".into(),
        DnsError::ServerFailure(_) => "reported an error".into(),
        DnsError::BadReply => "sent a garbled answer".into(),
        DnsError::Network(m) => m,
    }
}

pub fn check_dns(servers: &[IpAddr]) -> DnsFacts {
    let mut last = DnsFacts::default();
    for &server in servers.iter().take(3) {
        match dns::query(server, "www.wikipedia.org", Duration::from_secs(2)) {
            Ok(t) => return DnsFacts { server: Some(server), ms: Some(t.as_millis() as u32), error: None },
            Err(e) => last = DnsFacts { server: Some(server), ms: None, error: Some(dns_error(e)) },
        }
    }
    last
}

#[derive(Clone, Debug, Default)]
pub struct NetFacts {
    pub adapter: Option<Adapter>,
    pub wifi: Option<Wifi>,
    pub vpn: Option<String>,
    pub gateway: Option<PingStats>,
    pub gateway_alive: bool,
    pub internet: Option<PingStats>,
    pub internet_tcp: bool,
    pub dns_own: Option<DnsFacts>,
    pub dns_public_ok: bool,
    pub resolver_ok: bool,
    pub portal: Option<Portal>,
    pub https: Option<Result<u32, String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    NotConnected,
    LocalLink,
    Router,
    Provider,
    Dns,
    SignIn,
    Secure,
    Flaky,
    AllGood,
}

#[derive(Clone, Debug)]
pub struct Verdict {
    pub who: Who,
    pub headline: String,
    pub explanation: String,
    pub tips: Vec<String>,
}

fn link_name(f: &NetFacts) -> &'static str {
    if f.adapter.as_ref().map(|a| a.wifi).unwrap_or(false) { "Wi-Fi" } else { "network connection" }
}

pub fn verdict(f: &NetFacts) -> Verdict {
    let v = |who, h: &str, e: String, tips: &[&str]| Verdict {
        who,
        headline: h.to_string(),
        explanation: e,
        tips: tips.iter().map(|s| s.to_string()).collect(),
    };
    let mut out = verdict_inner(f, v);
    if let Some(vpn) = &f.vpn {
        out.tips.push(format!("You're connected through a VPN ({vpn}). If things are slow or broken, try disconnecting it and checking again."));
    }
    out
}

fn verdict_inner(f: &NetFacts, v: impl Fn(Who, &str, String, &[&str]) -> Verdict) -> Verdict {
    let Some(adapter) = &f.adapter else {
        return v(
            Who::NotConnected,
            "It's your PC: it isn't connected to any network",
            "Wi-Fi is off or not joined to a network, or the network cable is unplugged.".into(),
            &["Click the network icon by the clock and pick your Wi-Fi.", "Check Airplane mode is off.", "If you use a cable, check both ends are plugged in firmly."],
        );
    };
    let signal = f.wifi.as_ref().map(|w| w.signal);
    let weak = signal.map(|s| s < 40).unwrap_or(false);

    if f.gateway.as_ref().map(|g| g.received == 0).unwrap_or(true) && !f.gateway_alive {
        if weak {
            return v(
                Who::LocalLink,
                "It's your Wi-Fi: the signal is too weak",
                format!("The signal is only {}%, and your PC can't reach the router through it.", signal.unwrap_or(0)),
                &["Move closer to the router, or remove things between you (walls, metal, fish tanks).", "Restart the router if it's been on for weeks."],
            );
        }
        return v(
            Who::Router,
            "It's your router (or the link to it)",
            format!("Your PC is connected to {} but the router isn't answering.", describe_link(adapter, f.wifi.as_ref())),
            &["Restart the router: unplug it for 30 seconds, plug it back in, wait 2 minutes.", "Turn Wi-Fi off and on again on the PC.", "If other devices work fine, restart this PC."],
        );
    }
    if f.portal == Some(Portal::SignInNeeded) {
        return v(
            Who::SignIn,
            "You need to sign in to this network",
            "This network (common in hotels, cafés and airports) wants you to accept terms or log in before it lets you onto the internet.".into(),
            &["Open your browser and try any website: the sign-in page should appear.", "If it doesn't, try going to http://neverssl.com"],
        );
    }
    let internet_down = f.internet.as_ref().map(|p| p.received == 0).unwrap_or(true) && !f.internet_tcp;
    if internet_down {
        return v(
            Who::Provider,
            "It's your internet connection, not your PC",
            "Your PC and router are talking fine, but nothing is getting past the router to the internet.".into(),
            &[
                "Check the lights on your modem/router: a red or blinking 'internet' light confirms it.",
                "Restart the modem and router: unplug for 30 seconds, then wait a couple of minutes.",
                "If that doesn't fix it, check your internet provider's outage page on your phone, or call them.",
            ],
        );
    }
    let own_dns_bad = f.dns_own.as_ref().map(|d| d.ms.is_none()).unwrap_or(false);
    if (!f.resolver_ok || own_dns_bad) && f.dns_public_ok {
        return v(
            Who::Dns,
            "It's the DNS server (the internet's address book)",
            "Your internet connection works, but the service that turns website names into addresses isn't answering properly. That makes every website look broken.".into(),
            &[
                "Restart the router; it usually provides the DNS.",
                "Or switch to a public DNS: Settings > Network > your connection > DNS server assignment > Edit, then use 1.1.1.1 and 8.8.8.8.",
            ],
        );
    }
    if let Some(Err(e)) = &f.https {
        return v(
            Who::Secure,
            "Secure websites are failing on this PC",
            format!("The internet is reachable, but secure (https) connections fail: {e}."),
            &[
                "Check the date and time on the PC are correct (Settings > Time & language).",
                "Antivirus 'web protection' or a proxy can cause this. Try turning it off briefly.",
            ],
        );
    }

    let mut problems = Vec::new();
    let mut tips: Vec<&str> = Vec::new();
    if let Some(g) = &f.gateway {
        if g.received > 0 && (g.loss_pct() >= 10.0 || g.avg_ms > 40.0) {
            problems.push(format!(
                "the {} to your router is unreliable ({:.0}% of checks lost, {:.0} ms)",
                link_name(f),
                g.loss_pct(),
                g.avg_ms
            ));
            tips.push("Move closer to the router, or use a network cable for important calls.");
        }
    }
    if weak {
        problems.push(format!("the Wi-Fi signal is weak ({}%)", signal.unwrap_or(0)));
        if !tips.contains(&"Move closer to the router, or use a network cable for important calls.") {
            tips.push("Move closer to the router, or use a network cable for important calls.");
        }
    }
    if let Some(i) = &f.internet {
        let gw_ok = f.gateway.as_ref().map(|g| g.loss_pct() < 10.0).unwrap_or(true);
        if i.received > 0 && gw_ok && i.loss_pct() >= 10.0 {
            problems.push(format!("the line from your router to the internet is dropping data ({:.0}% lost)", i.loss_pct()));
            tips.push("If this keeps happening, tell your internet provider: dropped data past the router is their side.");
        }
        if i.received > 0 && i.avg_ms > 150.0 {
            problems.push(format!("responses are slow ({:.0} ms; under 50 is normal)", i.avg_ms));
        }
    }
    if let Some(d) = &f.dns_own {
        if d.ms.map(|m| m > 300).unwrap_or(false) {
            problems.push(format!("looking up website names is slow ({} ms)", d.ms.unwrap_or(0)));
            tips.push("A faster public DNS (1.1.1.1) can make websites start loading sooner.");
        }
    }
    if !problems.is_empty() {
        let mut text = String::from("Everything connects, but ");
        text.push_str(&join_and(&problems));
        text.push('.');
        return v(Who::Flaky, "It works, but the connection is shaky", text, &tips);
    }
    v(
        Who::AllGood,
        "It's not you, and it's not your internet",
        "Your PC, network, router, internet connection and website lookups all check out.".into(),
        &[
            "If one website or app is broken, it's probably that site. Use the box below to check it.",
            "If everything feels slow, run the speed test.",
        ],
    )
}

fn join_and(items: &[String]) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        _ => format!("{} and {}", items[..items.len() - 1].join(", "), items[items.len() - 1]),
    }
}

pub fn describe_link(a: &Adapter, w: Option<&Wifi>) -> String {
    match w {
        Some(w) if a.wifi => {
            let band = match w.channel {
                Some(c) if c <= 14 => ", 2.4 GHz",
                Some(c) if c >= 180 => ", 6 GHz",
                Some(_) => ", 5 GHz",
                None => "",
            };
            let rate = if w.rx_mbps > 0 { format!(", {} Mbps link", w.rx_mbps) } else { String::new() };
            format!("Wi-Fi \"{}\" (signal {}%{band}{rate})", w.ssid, w.signal)
        }
        _ if a.wifi => "Wi-Fi".into(),
        _ => {
            let speed = if a.speed_bps >= 1_000_000_000 {
                format!("{} Gbps", a.speed_bps / 1_000_000_000)
            } else if a.speed_bps > 0 {
                format!("{} Mbps", a.speed_bps / 1_000_000)
            } else {
                String::new()
            };
            if speed.is_empty() { "a network cable".into() } else { format!("a network cable ({speed})") }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    Waiting,
    Running,
    Ok,
    Warn,
    Fail,
    Skipped,
}

#[derive(Clone, Debug)]
pub struct Step {
    pub title: &'static str,
    pub state: StepState,
    pub detail: String,
}

pub const STEP_TITLES: [&str; 5] = ["Your PC's connection", "Router", "Internet", "Website names (DNS)", "Websites"];

pub fn fresh_steps() -> Vec<Step> {
    STEP_TITLES.iter().map(|t| Step { title: t, state: StepState::Waiting, detail: String::new() }).collect()
}

/// Runs every check in order, calling `update` after each step so the UI can show progress.
pub fn run_check(mut update: impl FnMut(&[Step])) -> (NetFacts, Vec<Step>) {
    let mut steps = fresh_steps();
    let mut f = NetFacts::default();
    let set = |steps: &mut Vec<Step>, i: usize, state: StepState, detail: String, update: &mut dyn FnMut(&[Step])| {
        steps[i].state = state;
        steps[i].detail = detail;
        if i + 1 < steps.len() && state != StepState::Running {
            steps[i + 1].state = StepState::Running;
        }
        update(steps);
    };
    steps[0].state = StepState::Running;
    update(&steps);

    let list = adapters();
    f.adapter = primary(&list);
    f.vpn = list
        .iter()
        .find(|a| a.is_vpn())
        .map(|a| if a.description.is_empty() { a.name.clone() } else { a.description.clone() });
    let Some(adapter) = f.adapter.clone() else {
        set(&mut steps, 0, StepState::Fail, "Not connected to any network.".into(), &mut update);
        for s in steps.iter_mut().skip(1) {
            s.state = StepState::Skipped;
        }
        update(&steps);
        return (f, steps);
    };
    if adapter.wifi {
        f.wifi = wifi();
    }
    let weak = f.wifi.as_ref().map(|w| w.signal < 40).unwrap_or(false);
    set(
        &mut steps,
        0,
        if weak { StepState::Warn } else { StepState::Ok },
        format!("Connected by {}.", describe_link(&adapter, f.wifi.as_ref())),
        &mut update,
    );

    let gw = adapter.gateway.unwrap();
    let gw_stats = match gw {
        IpAddr::V4(v4) => Some(ping(v4, 10, Duration::from_millis(100), 1000)),
        IpAddr::V6(_) => None,
    };
    f.gateway_alive = gw_stats.as_ref().map(|s| s.received > 0).unwrap_or(false)
        || tcp_alive(gw, &[80, 443, 53], Duration::from_millis(1500));
    f.gateway = gw_stats.clone();
    let (state, detail) = match (&gw_stats, f.gateway_alive) {
        (Some(s), _) if s.received > 0 => (
            if s.loss_pct() >= 10.0 || s.avg_ms > 40.0 { StepState::Warn } else { StepState::Ok },
            format!("Router at {gw} answered in {:.0} ms ({:.0}% lost).", s.avg_ms, s.loss_pct()),
        ),
        (_, true) => (StepState::Ok, format!("Router at {gw} is answering.")),
        _ => (StepState::Fail, format!("Router at {gw} isn't answering.")),
    };
    set(&mut steps, 1, state, detail, &mut update);
    if !f.gateway_alive {
        for s in steps.iter_mut().skip(2) {
            s.state = StepState::Skipped;
        }
        update(&steps);
        return (f, steps);
    }

    let probe = std::thread::spawn(|| (ping(Ipv4Addr::new(1, 1, 1, 1), 10, Duration::from_millis(100), 1500), ping(Ipv4Addr::new(8, 8, 8, 8), 10, Duration::from_millis(100), 1500)));
    f.internet_tcp = internet_reachable(Duration::from_secs(3));
    let (a, b) = probe.join().unwrap_or_default();
    let best = if a.received >= b.received { a } else { b };
    f.internet = Some(best.clone());
    let (state, detail) = if best.received > 0 {
        (
            if best.loss_pct() >= 10.0 || best.avg_ms > 150.0 { StepState::Warn } else { StepState::Ok },
            format!("Reached the internet in {:.0} ms ({:.0}% lost).", best.avg_ms, best.loss_pct()),
        )
    } else if f.internet_tcp {
        (StepState::Ok, "Reached the internet.".into())
    } else {
        (StepState::Fail, "Nothing gets past the router.".into())
    };
    let internet_down = best.received == 0 && !f.internet_tcp;
    if internet_down {
        f.portal = Some(captive_portal());
    }
    set(&mut steps, 2, state, detail, &mut update);
    if internet_down && f.portal != Some(Portal::SignInNeeded) {
        for s in steps.iter_mut().skip(3) {
            s.state = StepState::Skipped;
        }
        update(&steps);
        return (f, steps);
    }

    let own = check_dns(&adapter.dns);
    f.dns_public_ok = dns::query(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), "www.wikipedia.org", Duration::from_secs(2)).is_ok()
        || dns::query(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), "www.wikipedia.org", Duration::from_secs(2)).is_ok();
    f.resolver_ok = ("www.microsoft.com", 443).to_socket_addrs().map(|mut a| a.next().is_some()).unwrap_or(false);
    let (state, detail) = match (&own.ms, &own.server) {
        (Some(ms), Some(server)) => (
            if *ms > 300 { StepState::Warn } else { StepState::Ok },
            format!("Your DNS server ({server}) answered in {ms} ms."),
        ),
        (None, Some(server)) => (
            StepState::Fail,
            format!("Your DNS server ({server}) {}.", own.error.clone().unwrap_or_else(|| "didn't answer".into())),
        ),
        _ if f.resolver_ok => (StepState::Ok, "Website names are being looked up fine.".into()),
        _ => (StepState::Fail, "Website names can't be looked up.".into()),
    };
    f.dns_own = Some(own);
    set(&mut steps, 3, state, detail, &mut update);

    if f.portal.is_none() {
        f.portal = Some(captive_portal());
    }
    let https = http::get("www.cloudflare.com", "/cdn-cgi/trace", Duration::from_secs(8)).map(|r| r.status);
    let (state, detail) = match (&https, f.portal) {
        (_, Some(Portal::SignInNeeded)) => (StepState::Fail, "This network is showing a sign-in page instead of websites.".into()),
        (Ok(s), _) if *s >= 200 && *s < 400 => (StepState::Ok, "Websites load, including secure (https) ones.".into()),
        (Ok(s), _) => (StepState::Warn, format!("A test website answered with an unexpected code ({s}).")),
        (Err(e), _) => (StepState::Fail, format!("Secure websites fail: {e}.")),
    };
    f.https = Some(https);
    set(&mut steps, 4, state, detail, &mut update);
    (f, steps)
}

#[derive(Clone, Debug)]
pub struct Speed {
    pub mbps: f64,
    pub idle_ms: Option<f32>,
    pub loaded_ms: Option<f32>,
}

pub fn speed_test() -> Result<Speed, String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let target = Ipv4Addr::new(1, 1, 1, 1);
    let idle = ping(target, 5, Duration::from_millis(100), 1000);
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    let loaded = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        let mut rtts = Vec::new();
        let mut sent = 0;
        while !s2.load(Ordering::Relaxed) {
            let p = ping(target, 1, Duration::ZERO, 1000);
            sent += 1;
            if p.received > 0 {
                rtts.push(p.avg_ms as u32);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        PingStats::from_rtts(sent, &rtts)
    });
    // Cloudflare refuses single downloads over 25 MB, so pull several in parallel until time or data runs out.
    const PER_REQUEST: u64 = 25_000_000;
    const DATA_CAP: u64 = 150_000_000;
    const TIME_CAP: Duration = Duration::from_secs(8);
    let total = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let first_error = Arc::new(std::sync::Mutex::new(None::<String>));
    let start = Instant::now();
    let streams: Vec<_> = (0..3)
        .map(|_| {
            let total = total.clone();
            let first_error = first_error.clone();
            std::thread::spawn(move || {
                while start.elapsed() < TIME_CAP && total.load(Ordering::Relaxed) < DATA_CAP {
                    let left = TIME_CAP.saturating_sub(start.elapsed());
                    match http::get("speed.cloudflare.com", &format!("/__down?bytes={PER_REQUEST}"), left) {
                        Ok(r) if r.status == 200 => {
                            total.fetch_add(r.bytes, Ordering::Relaxed);
                        }
                        Ok(r) => {
                            first_error.lock().unwrap().get_or_insert(format!("the test server answered with code {}", r.status));
                            break;
                        }
                        Err(e) => {
                            first_error.lock().unwrap().get_or_insert(e);
                            break;
                        }
                    }
                }
            })
        })
        .collect();
    for s in streams {
        let _ = s.join();
    }
    let elapsed = start.elapsed();
    stop.store(true, Ordering::Relaxed);
    let loaded = loaded.join().unwrap_or_default();
    let bytes = total.load(Ordering::Relaxed);
    if bytes < 1_000_000 {
        let err = first_error.lock().unwrap().clone();
        return Err(err.unwrap_or_else(|| "the speed test server didn't send enough data".into()));
    }
    Ok(Speed {
        mbps: bytes as f64 * 8.0 / elapsed.as_secs_f64() / 1e6,
        idle_ms: (idle.received > 0).then_some(idle.avg_ms),
        loaded_ms: (loaded.received > 0).then_some(loaded.avg_ms),
    })
}

impl Speed {
    pub fn describe(&self) -> (String, String) {
        let verdict = match self.mbps {
            m if m < 2.0 => "Very slow. Pages will crawl and video won't play smoothly.",
            m if m < 10.0 => "Slow. Fine for email and browsing; video may drop to low quality.",
            m if m < 30.0 => "OK. Enough for HD video and video calls.",
            m if m < 100.0 => "Good. Enough for several people streaming at once.",
            _ => "Fast. Plenty for anything most homes do.",
        };
        let lag = match (self.idle_ms, self.loaded_ms) {
            (Some(i), Some(l)) => {
                let extra = (l - i).max(0.0);
                if extra < 30.0 {
                    format!("Stays responsive while busy (delay {i:.0} ms idle, {l:.0} ms while downloading).")
                } else if extra < 100.0 {
                    format!("Gets a bit laggy while something downloads (delay goes from {i:.0} ms to {l:.0} ms).")
                } else {
                    format!(
                        "Gets very laggy while something downloads (delay jumps from {i:.0} ms to {l:.0} ms). Video calls and games will stutter whenever someone downloads. A router with \"Smart Queue\" or \"SQM\" fixes this."
                    )
                }
            }
            _ => String::new(),
        };
        (format!("{:.0} Mbps download. {verdict}", self.mbps), lag)
    }
}

#[derive(Clone, Debug)]
pub struct SiteResult {
    pub ok: bool,
    pub text: String,
}

pub fn host_from_input(input: &str) -> Option<String> {
    let s = input.trim();
    let s = s.split("://").nth(1).unwrap_or(s);
    let host = s.split(['/', '?', '#']).next()?.split(':').next()?.trim().to_ascii_lowercase();
    (!host.is_empty() && host.contains('.') && !host.contains(' ')).then_some(host)
}

pub fn check_site(input: &str) -> SiteResult {
    let Some(host) = host_from_input(input) else {
        return SiteResult { ok: false, text: "That doesn't look like a web address. Try something like bbc.co.uk".into() };
    };
    let start = Instant::now();
    let addr = match (host.as_str(), 443).to_socket_addrs() {
        Ok(mut a) => a.next(),
        Err(_) => None,
    };
    let Some(addr) = addr else {
        return SiteResult {
            ok: false,
            text: format!("\"{host}\" couldn't be found. Check the spelling. If it's right, the site may have closed or its name has expired."),
        };
    };
    let lookup_ms = start.elapsed().as_millis();
    if TcpStream::connect_timeout(&addr, Duration::from_secs(5)).is_err() {
        return SiteResult {
            ok: false,
            text: format!("{host} exists, but its server isn't answering. The site itself is probably down. It's not you."),
        };
    }
    match http::get(&host, "/", Duration::from_secs(6)) {
        Ok(r) if r.status < 500 => SiteResult {
            ok: true,
            text: format!(
                "{host} is working from this PC (answered in {} ms; name lookup {lookup_ms} ms). If it won't load in your browser, try refreshing, a private window, or clearing the browser's cache.",
                r.elapsed.as_millis()
            ),
        },
        Ok(r) => SiteResult {
            ok: false,
            text: format!("{host} is answering but reporting a server error ({}). That's a problem on the website's side.", r.status),
        },
        Err(e) => SiteResult { ok: false, text: format!("{host} answers, but loading the page failed: {e}.") },
    }
}

pub fn report_text(steps: &[Step], verdict: &Verdict) -> String {
    let mut s = format!("{}\n{}\n\n", verdict.headline, verdict.explanation);
    for st in steps {
        let mark = match st.state {
            StepState::Ok => "OK",
            StepState::Warn => "Warning",
            StepState::Fail => "Problem",
            StepState::Skipped => "Skipped",
            _ => "-",
        };
        s.push_str(&format!("- {}: {} {}\n", st.title, mark, st.detail));
    }
    if !verdict.tips.is_empty() {
        s.push_str("\nWhat to try:\n");
        for t in &verdict.tips {
            s.push_str(&format!("- {t}\n"));
        }
    }
    s.push_str("\n(Report from Plainly)");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> NetFacts {
        NetFacts {
            adapter: Some(Adapter { name: "Wi-Fi".into(), wifi: true, gateway: Some("192.168.1.1".parse().unwrap()), ..Default::default() }),
            wifi: Some(Wifi { ssid: "Home".into(), signal: 80, channel: Some(36), rx_mbps: 400 }),
            vpn: None,
            gateway: Some(PingStats { sent: 10, received: 10, avg_ms: 3.0, jitter_ms: 1.0 }),
            gateway_alive: true,
            internet: Some(PingStats { sent: 10, received: 10, avg_ms: 18.0, jitter_ms: 2.0 }),
            internet_tcp: true,
            dns_own: Some(DnsFacts { server: Some("192.168.1.1".parse().unwrap()), ms: Some(20), error: None }),
            dns_public_ok: true,
            resolver_ok: true,
            portal: Some(Portal::Clear),
            https: Some(Ok(200)),
        }
    }

    #[test]
    #[ignore = "uses the real network; run with --ignored --nocapture"]
    fn live_speed_and_sites() {
        match speed_test() {
            Ok(s) => println!("speed {:?} -> {:?}", s, s.describe()),
            Err(e) => println!("speed failed: {e}"),
        }
        for site in ["bbc.co.uk", "https://github.com/rust-lang", "this-site-does-not-exist-plainly.com", "not a site"] {
            println!("{site}: {:?}", check_site(site));
        }
        println!("portal: {:?}", captive_portal());
    }

    #[test]
    fn healthy_connection_is_all_good() {
        assert_eq!(verdict(&good()).who, Who::AllGood);
    }

    #[test]
    fn no_adapter_means_not_connected() {
        assert_eq!(verdict(&NetFacts::default()).who, Who::NotConnected);
    }

    #[test]
    fn dead_router_with_weak_signal_blames_wifi() {
        let mut f = good();
        f.gateway = Some(PingStats { sent: 10, received: 0, ..Default::default() });
        f.gateway_alive = false;
        assert_eq!(verdict(&f).who, Who::Router);
        f.wifi.as_mut().unwrap().signal = 20;
        assert_eq!(verdict(&f).who, Who::LocalLink);
    }

    #[test]
    fn router_fine_internet_dead_blames_provider() {
        let mut f = good();
        f.internet = Some(PingStats { sent: 10, received: 0, ..Default::default() });
        f.internet_tcp = false;
        f.portal = Some(Portal::Unknown);
        assert_eq!(verdict(&f).who, Who::Provider);
        f.portal = Some(Portal::SignInNeeded);
        assert_eq!(verdict(&f).who, Who::SignIn);
    }

    #[test]
    fn broken_dns_with_working_public_dns() {
        let mut f = good();
        f.dns_own = Some(DnsFacts { server: Some("192.168.1.1".parse().unwrap()), ms: None, error: Some("didn't answer".into()) });
        f.resolver_ok = false;
        assert_eq!(verdict(&f).who, Who::Dns);
    }

    #[test]
    fn lossy_wifi_is_flaky() {
        let mut f = good();
        f.gateway = Some(PingStats { sent: 10, received: 7, avg_ms: 60.0, jitter_ms: 30.0 });
        let v = verdict(&f);
        assert_eq!(v.who, Who::Flaky);
        assert!(v.explanation.contains("30% of checks lost"));
    }

    #[test]
    fn vpn_adds_a_tip() {
        let mut f = good();
        f.vpn = Some("NordLynx".into());
        assert!(verdict(&f).tips.iter().any(|t| t.contains("NordLynx")));
    }

    #[test]
    fn ping_stats_maths() {
        let s = PingStats::from_rtts(4, &[10, 20, 10]);
        assert_eq!(s.received, 3);
        assert!((s.avg_ms - 13.333).abs() < 0.01);
        assert_eq!(s.jitter_ms, 10.0);
        assert_eq!(s.loss_pct(), 25.0);
    }

    #[test]
    fn host_parsing() {
        assert_eq!(host_from_input("https://www.BBC.co.uk/news?x=1").as_deref(), Some("www.bbc.co.uk"));
        assert_eq!(host_from_input("example.com:8080").as_deref(), Some("example.com"));
        assert_eq!(host_from_input("not a site"), None);
    }

    #[test]
    fn speed_descriptions() {
        let s = Speed { mbps: 45.0, idle_ms: Some(15.0), loaded_ms: Some(220.0) };
        let (a, b) = s.describe();
        assert!(a.starts_with("45 Mbps download. Good"));
        assert!(b.contains("very laggy"));
    }
}
