// SPDX-License-Identifier: MPL-2.0
//! What the app knows of a network, from net.rs's scan and join, for the screens (screens.rs) to show: no hardware
//! here, so the screens and the frame parser (frames.rs) build on a computer too. Moved from net.h.

/// What a network asks of a station: WPA2-PSK (CCMP, without required PMF) and open networks can be joined here, and
/// nothing else, WPA3-only among them.
pub const WPA2: u8 = 0;
pub const OPEN: u8 = 1;
pub const OTHER: u8 = 2;

/// A network heard.
#[derive(Clone, Copy)]
pub struct Ap {
    /// Its name, ending in a zero.
    pub ssid: [u8; 33],
    pub bssid: [u8; 6],
    pub channel: u8,
    pub security: u8,
    /// dBm, the strongest heard.
    pub rssi: i8,
}

impl Ap {
    pub const fn new() -> Ap {
        Ap { ssid: [0; 33], bssid: [0; 6], channel: 0, security: 0, rssi: 0 }
    }

    pub fn ssid(&self) -> &[u8] {
        let n = self.ssid.iter().position(|&c| c == 0).unwrap_or(self.ssid.len());
        &self.ssid[..n]
    }
}

impl Default for Ap {
    fn default() -> Ap {
        Ap::new()
    }
}

/// The steps of a join, for its status.
pub const STEP_RADIO: u8 = 0;
pub const STEP_FINDING: u8 = 1;
pub const STEP_KEY: u8 = 2;
pub const STEP_JOINING: u8 = 3;
pub const STEP_ADDRESS: u8 = 4;
pub const STEPS: u8 = 5;

/// How a join went.
#[derive(Clone, Copy, Default)]
pub struct Joined {
    pub rssi: i8,
    pub channel: u8,
    /// Host order.
    pub ip: u32,
    pub gateway: u32,
    pub dns: u32,
    /// From the start to an address.
    pub ms: u32,
    /// The step it got to: where it stopped, if it failed.
    pub step: u8,
}
