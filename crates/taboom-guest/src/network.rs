use serde::{Deserialize, Serialize};
use taboom_proto::{ProxyProtocol, RouteState, RouteStateReport};

const GUESTFWD_PROXY_ADDR: &str = "10.0.2.100";
const GUESTFWD_PROXY_PORT: u16 = 1080;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tun2socksConfig {
    pub proxy_addr: String,
    pub proxy_port: u16,
    pub protocol: ProxyProtocol,
    pub tun_device: String,
    pub tun_addr: String,
    pub tun_mask: String,
}

impl Default for Tun2socksConfig {
    fn default() -> Self {
        Self {
            proxy_addr: GUESTFWD_PROXY_ADDR.into(),
            proxy_port: GUESTFWD_PROXY_PORT,
            protocol: ProxyProtocol::Socks5,
            tun_device: "tun0".into(),
            tun_addr: "198.18.0.1".into(),
            tun_mask: "255.254.0.0".into(),
        }
    }
}

impl Tun2socksConfig {
    pub fn proxy_url(&self) -> String {
        let scheme = match self.protocol {
            ProxyProtocol::Socks5 => "socks5",
            ProxyProtocol::Http => "http",
        };
        format!("{scheme}://{}:{}", self.proxy_addr, self.proxy_port)
    }

    pub fn build_args(&self) -> Vec<String> {
        vec![
            "-device".into(),
            self.tun_device.clone(),
            "-proxy".into(),
            self.proxy_url(),
        ]
    }
}

#[derive(Debug, Clone)]
pub struct NetworkSetup {
    pub config: Tun2socksConfig,
    pub dns_through_proxy: bool,
}

impl NetworkSetup {
    pub fn for_proxy(protocol: ProxyProtocol) -> Self {
        Self {
            config: Tun2socksConfig {
                protocol,
                ..Default::default()
            },
            dns_through_proxy: true,
        }
    }

    pub fn direct() -> Self {
        Self {
            config: Tun2socksConfig::default(),
            dns_through_proxy: false,
        }
    }

    pub fn resolv_conf_content(&self) -> String {
        if self.dns_through_proxy {
            // When proxied, DNS resolves through the SOCKS proxy.
            // tun2socks handles DNS via the proxy tunnel.
            // Use a local DNS forwarder address that tun2socks provides.
            "nameserver 198.18.0.2\n".into()
        } else {
            // Direct mode: use QEMU's built-in DNS forwarder
            "nameserver 10.0.2.3\n".into()
        }
    }

    pub fn sysctl_rules() -> Vec<(&'static str, &'static str)> {
        vec![
            // Disable UDP (kills QUIC, forces TCP fallback)
            ("net.ipv4.conf.all.disable_policy", "0"),
        ]
    }

    pub fn iptables_drop_udp_rules() -> Vec<Vec<&'static str>> {
        vec![
            // Drop all outbound UDP except DNS to the tun2socks DNS forwarder.
            // This kills QUIC; Chrome falls back to HTTP/2 over TCP.
            vec![
                "iptables", "-A", "OUTPUT", "-p", "udp",
                "--dport", "53", "-d", "198.18.0.2", "-j", "ACCEPT",
            ],
            vec![
                "iptables", "-A", "OUTPUT", "-p", "udp", "-j", "DROP",
            ],
        ]
    }
}

pub fn current_route_state(proxy_reachable: bool) -> RouteStateReport {
    if proxy_reachable {
        RouteStateReport {
            state: RouteState::Up,
            exit_ip: None,
            asn: None,
            country: None,
        }
    } else {
        RouteStateReport {
            state: RouteState::Down {
                reason: "proxy unreachable".into(),
            },
            exit_ip: None,
            asn: None,
            country: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_url_socks5() {
        let cfg = Tun2socksConfig::default();
        assert_eq!(cfg.proxy_url(), "socks5://10.0.2.100:1080");
    }

    #[test]
    fn proxy_url_http() {
        let cfg = Tun2socksConfig {
            protocol: ProxyProtocol::Http,
            ..Default::default()
        };
        assert_eq!(cfg.proxy_url(), "http://10.0.2.100:1080");
    }

    #[test]
    fn resolv_conf_proxy_vs_direct() {
        let proxied = NetworkSetup::for_proxy(ProxyProtocol::Socks5);
        assert!(proxied.resolv_conf_content().contains("198.18.0.2"));

        let direct = NetworkSetup::direct();
        assert!(direct.resolv_conf_content().contains("10.0.2.3"));
    }
}
