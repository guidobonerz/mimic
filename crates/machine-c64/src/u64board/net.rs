//! The cable in the board's Ethernet socket: at its other end a virtual
//! router ([`devices_net::vrouter`]) that takes the firmware's Ethernet
//! frames to the host's network through `mimic_core::host` (user-mode
//! NAT: DHCP and DNS for the board, its connections opened as host
//! sockets, host ports forwarded to the board's servers).
//!
//! The board gets 10.0.2.15 by DHCP, the router is 10.0.2.2 (connections
//! to it reach the PC itself), DNS 10.0.2.3. From the host and its LAN
//! the board's servers are reached on the PC's ports of [`FORWARDS`]
//! (`options["netfwd"]` gives others).

use chips_ultimate::EtherLink;
use devices_net::vrouter::{Forward, Router, RouterConfig};
use mimic_core::host::HostServices;
use mimic_core::{StateError, StateReader, StateWriter};

/// Host port → board port, unless `options["netfwd"]` says otherwise:
/// the firmware's web interface, FTP, Telnet and its socket DMA port.
pub const FORWARDS: [(u16, u16); 4] = [(6480, 80), (6421, 21), (6423, 23), (6464, 64)];

/// Microseconds between two looks of the router at its sockets.
const POLL_US: u64 = 1000;

pub struct Cable {
    /// `None`: nothing plugged in (`options["net"] = "off"`).
    router: Option<Router>,
    /// Emulated time for the router, microseconds.
    now_us: u64,
    /// The polling period last served, plus one (0: none yet).
    next_poll: u64,
}

/// `"8080:80,2121:21"` → pairs; `""` → none.
pub fn parse_forwards(s: &str) -> Result<Vec<(u16, u16)>, String> {
    s.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (h, g) = p
                .split_once(':')
                .ok_or_else(|| format!("netfwd '{p}': HOSTPORT:BOARDPORT expected"))?;
            match (h.parse::<u16>(), g.parse::<u16>()) {
                (Ok(h), Ok(g)) if h != 0 && g != 0 => Ok((h, g)),
                _ => Err(format!("netfwd '{p}': ports are 1 to 65535")),
            }
        })
        .collect()
}

impl Cable {
    pub fn new(plugged: bool, forwards: &[(u16, u16)]) -> Self {
        let router = plugged.then(|| {
            Router::new(RouterConfig {
                forwards: forwards
                    .iter()
                    .map(|&(host_port, guest_port)| Forward {
                        host_port,
                        guest_port,
                    })
                    .collect(),
                ..RouterConfig::default()
            })
        });
        Self {
            router,
            now_us: 0,
            next_poll: 0,
        }
    }

    pub fn attach_host(&mut self, host: HostServices) {
        if let Some(r) = &mut self.router {
            r.attach_host(host);
        }
    }

    /// The board's clock moved on (`clock`: 100 MHz periods since its
    /// power-on): the router gets its time and looks at its sockets
    /// every [`POLL_US`].
    pub fn advance(&mut self, clock: u64) {
        self.now_us = clock / 100;
        // At the same times whatever happened before (a loaded state
        // must go on as the saved machine did): once per period, with
        // the period's start as the time.
        let period = self.now_us / POLL_US + 1;
        if period != self.next_poll {
            self.next_poll = period;
            if let Some(r) = &mut self.router {
                r.poll((period - 1) * POLL_US);
            }
        }
    }

    /// The board was switched on anew: its clock starts at 0 again.
    pub fn power_on(&mut self) {
        self.next_poll = 0;
        self.now_us = 0;
    }

    /// Host port, board port and whether the host port could be opened.
    pub fn forwards(&self) -> Vec<(u16, u16, bool)> {
        self.router.as_ref().map_or_else(Vec::new, |r| {
            r.forwards_status()
                .into_iter()
                .map(|(f, up)| (f.host_port, f.guest_port, up))
                .collect()
        })
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.bool(self.router.is_some());
        w.u64(self.next_poll);
        if let Some(r) = &self.router {
            r.save(w);
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        if r.bool()? != self.router.is_some() {
            return Err(StateError::Invalid("network cable differs"));
        }
        let next_poll = r.u64()?;
        if let Some(router) = &mut self.router {
            router.load(r)?;
        }
        self.next_poll = next_poll;
        Ok(())
    }
}

impl EtherLink for Cable {
    fn link_up(&self) -> bool {
        self.router.is_some()
    }

    fn transmit(&mut self, frame: &[u8]) {
        if let Some(r) = &mut self.router {
            r.guest_frame(frame, self.now_us);
        }
    }

    fn receive(&mut self) -> Option<Vec<u8>> {
        self.router.as_mut()?.take_frame()
    }

    fn pending(&self) -> bool {
        self.router.as_ref().is_some_and(Router::has_frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_parse() {
        assert_eq!(
            parse_forwards("8080:80, 2121:21"),
            Ok(vec![(8080, 80), (2121, 21)])
        );
        assert_eq!(parse_forwards(""), Ok(vec![]));
        assert!(parse_forwards("80").is_err());
        assert!(parse_forwards("0:80").is_err());
        assert!(parse_forwards("a:b").is_err());
    }

    #[test]
    fn unplugged_has_no_link() {
        let mut c = Cable::new(false, &FORWARDS);
        assert!(!c.link_up());
        c.transmit(&[0; 64]);
        c.advance(1_000_000);
        assert!(c.receive().is_none() && !c.pending());
        assert!(Cable::new(true, &[]).link_up());
    }
}
