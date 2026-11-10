//! IEEE 802.11 frame work for the Intel AX201 driver (`iwlwifi.rs`): what
//! mac80211 does for iwlmvm on Linux, cut down to one station interface on
//! one BSS, legacy (11a/b/g) rates, open or WPA2-PSK/CCMP. Pure functions on
//! byte slices -- no hardware, no statics -- so it builds and is tested on
//! the host as well (see the driver's notes on the offline tests).
//!
//! Frame formats: IEEE 802.11-2020 9.2-9.4 (MAC header, management frame
//! bodies, elements); LLC/SNAP framing of data: RFC 1042.

use alloc::string::String;
use alloc::vec::Vec;

// ---- frame control (first byte: subtype << 4 | type << 2) ----
pub const FC_ASSOC_REQ: u8 = 0x00;
pub const FC_ASSOC_RESP: u8 = 0x10;
pub const FC_REASSOC_RESP: u8 = 0x30;
pub const FC_PROBE_RESP: u8 = 0x50;
pub const FC_BEACON: u8 = 0x80;
pub const FC_DISASSOC: u8 = 0xA0;
pub const FC_AUTH: u8 = 0xB0;
pub const FC_DEAUTH: u8 = 0xC0;
/// second byte
pub const FC1_TODS: u8 = 0x01;
pub const FC1_FROMDS: u8 = 0x02;
pub const FC1_PROTECTED: u8 = 0x40;
pub const FC1_ORDER: u8 = 0x80;

pub const ETH_P_PAE: u16 = 0x888E;
const RFC1042: [u8; 6] = [0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00];

/// RSN cipher / AKM suite selectors (00-0F-AC:n)
pub const SUITE_TKIP: u32 = 0x000F_AC02;
pub const SUITE_CCMP: u32 = 0x000F_AC04;
pub const AKM_8021X: u32 = 0x000F_AC01;
pub const AKM_PSK: u32 = 0x000F_AC02;
pub const AKM_PSK_SHA256: u32 = 0x000F_AC06;
pub const AKM_SAE: u32 = 0x000F_AC08;

/// The legacy rates in iwlwifi's rate-index order (fw/api/rs.h IWL_RATE_*):
/// 0-3 CCK 1/2/5.5/11, 4-11 OFDM 6..54 -- in 500 kb/s units.
pub const RATES: [u8; 12] = [2, 4, 11, 22, 12, 18, 24, 36, 48, 72, 96, 108];
pub const FIRST_OFDM: usize = 4;

pub fn rate_index(units_500k: u8) -> Option<usize> {
    RATES.iter().position(|&r| r == units_500k)
}

/// One BSS heard during a scan (from a beacon or probe response).
#[derive(Clone)]
pub struct Bss {
    pub bssid: [u8; 6],
    pub ssid: Vec<u8>,
    pub channel: u8,
    pub band24: bool,
    pub rssi: i32,
    pub capab: u16,
    pub beacon_int: u16,
    /// raw rate bytes from Supported Rates + Extended Supported Rates (bit 7 = basic)
    pub rates: Vec<u8>,
    pub rsn: Option<Rsn>,
    /// a WPA1 vendor element (00-50-F2:1) is present
    pub wpa1: bool,
    pub dtim_period: u8,
    pub dtim_count: u8,
    /// the frame's TSF timestamp and the device time it arrived (gp2, us)
    pub tsf: u64,
    pub gp2: u32,
    pub ht: bool,
    pub last_ms: u64,
}

#[derive(Clone)]
pub struct Rsn {
    pub group: u32,
    pub pairwise: Vec<u32>,
    pub akm: Vec<u32>,
    pub caps: u16,
}

fn suite(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// RSN element body (after id/len).
pub fn parse_rsn(b: &[u8]) -> Option<Rsn> {
    if b.len() < 2 || u16::from_le_bytes([b[0], b[1]]) != 1 {
        return None;
    }
    let mut r = Rsn { group: SUITE_CCMP, pairwise: alloc::vec![SUITE_CCMP], akm: alloc::vec![AKM_8021X], caps: 0 };
    let mut i = 2;
    if b.len() >= i + 4 {
        r.group = suite(&b[i..]);
        i += 4;
    } else {
        return Some(r);
    }
    let list = |i: &mut usize| -> Option<Vec<u32>> {
        if b.len() < *i + 2 {
            return None;
        }
        let n = u16::from_le_bytes([b[*i], b[*i + 1]]) as usize;
        *i += 2;
        if b.len() < *i + 4 * n {
            return None;
        }
        let v = (0..n).map(|k| suite(&b[*i + 4 * k..])).collect();
        *i += 4 * n;
        Some(v)
    };
    match list(&mut i) {
        Some(v) => r.pairwise = v,
        None => return Some(r),
    }
    match list(&mut i) {
        Some(v) => r.akm = v,
        None => return Some(r),
    }
    if b.len() >= i + 2 {
        r.caps = u16::from_le_bytes([b[i], b[i + 1]]);
    }
    Some(r)
}

/// Walk the elements: (id, body).
pub fn elements(mut b: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    core::iter::from_fn(move || {
        if b.len() < 2 {
            return None;
        }
        let (id, l) = (b[0], b[1] as usize);
        if b.len() < 2 + l {
            return None;
        }
        let body = &b[2..2 + l];
        b = &b[2 + l..];
        Some((id, body))
    })
}

/// A beacon or probe response (the whole 802.11 frame, no FCS) -> Bss.
/// `rx_channel` is the channel the frame arrived on (the RX descriptor's),
/// used when the frame carries no DS Parameter Set.
pub fn parse_bss(f: &[u8], rx_channel: u8, rssi: i32) -> Option<Bss> {
    if f.len() < 36 || (f[0] != FC_BEACON && f[0] != FC_PROBE_RESP) {
        return None;
    }
    let mut bssid = [0u8; 6];
    bssid.copy_from_slice(&f[16..22]);
    let tsf = u64::from_le_bytes(f[24..32].try_into().ok()?);
    let beacon_int = u16::from_le_bytes([f[32], f[33]]);
    let capab = u16::from_le_bytes([f[34], f[35]]);
    let mut b = Bss {
        bssid,
        ssid: Vec::new(),
        channel: rx_channel,
        band24: rx_channel <= 14,
        rssi,
        capab,
        beacon_int,
        rates: Vec::new(),
        rsn: None,
        wpa1: false,
        dtim_period: 1,
        dtim_count: 0,
        tsf,
        gp2: 0,
        ht: false,
        last_ms: 0,
    };
    for (id, body) in elements(&f[36..]) {
        match id {
            0 => b.ssid = body.to_vec(),
            1 | 50 => b.rates.extend_from_slice(body),
            3 if body.len() == 1 => {
                b.channel = body[0];
                b.band24 = body[0] <= 14;
            },
            5 if body.len() >= 2 => {
                b.dtim_count = body[0];
                b.dtim_period = body[1].max(1);
            },
            45 => b.ht = true,
            48 => b.rsn = parse_rsn(body),
            221 if body.len() >= 4 && body[..4] == [0x00, 0x50, 0xF2, 1] => b.wpa1 = true,
            _ => {},
        }
    }
    Some(b)
}

/// 0 open, 1 WEP, 2 WPA, 3 WPA2 (or WPA2/WPA3 mixed), 4 WPA3-only (wifi::Network.secure).
pub fn security(b: &Bss) -> u8 {
    if b.capab & 0x10 == 0 {
        return 0;
    }
    match &b.rsn {
        Some(r) => {
            let sae = r.akm.contains(&AKM_SAE);
            let psk = r.akm.iter().any(|&a| a != AKM_SAE);
            if sae && !psk { 4 } else { 3 }
        },
        None if b.wpa1 => 2,
        None => 1,
    }
}

/// The RSN element we associate with: WPA2-PSK, CCMP pairwise + group, no
/// capabilities (no MFP). Err says why this BSS cannot be joined that way.
pub fn rsn_ie_for(b: &Bss) -> Result<Vec<u8>, &'static str> {
    let r = b.rsn.as_ref().ok_or("no RSN element (WPA1/WEP network)")?;
    if !r.akm.contains(&AKM_PSK) {
        if r.akm.contains(&AKM_SAE) {
            return Err("WPA3-only network (SAE); set the router to WPA2/WPA3 mixed");
        }
        if r.akm.contains(&AKM_8021X) {
            return Err("WPA2-Enterprise (802.1X) is not supported");
        }
        return Err("no WPA2-PSK key management offered");
    }
    if !r.pairwise.contains(&SUITE_CCMP) {
        return Err("the access point offers no CCMP (AES) pairwise cipher");
    }
    if r.group != SUITE_CCMP {
        return Err("the group cipher is TKIP (WPA/WPA2 mixed mode); use AES-only WPA2");
    }
    if r.caps & 0x0040 != 0 {
        return Err("the access point requires management frame protection (802.11w)");
    }
    Ok(alloc::vec![0x30, 20, 1, 0, 0x00, 0x0F, 0xAC, 4, 1, 0, 0x00, 0x0F, 0xAC, 4, 1, 0, 0x00, 0x0F, 0xAC, 2, 0, 0])
}

/// The legacy rates both sides support, as an iwl rate-index bitmap (bit n =
/// RATES[n]); on 5 GHz CCK is never used.
pub fn rate_bitmap(b: &Bss) -> u16 {
    let mut m = 0u16;
    for &r in &b.rates {
        if let Some(i) = rate_index(r & 0x7F) {
            if b.band24 || i >= FIRST_OFDM {
                m |= 1 << i;
            }
        }
    }
    if m == 0 {
        m = if b.band24 { 0x0F } else { 0x15 << FIRST_OFDM };
    }
    m
}

/// The BSS requires HT/VHT (a membership selector among the basic rates).
pub fn requires_ht(b: &Bss) -> bool {
    b.rates.iter().any(|&r| r == 0xFF || r == 0xFE)
}

/// mvm/mac-ctxt.c iwl_mvm_ack_rates: (cck, ofdm) ack-rate bitmaps from the
/// basic rates plus the mandatory lower ones.
pub fn ack_rates(b: &Bss) -> (u8, u8) {
    let mut cck = 0u8;
    let mut ofdm = 0u8;
    let mut low_cck = 100usize;
    let mut low_ofdm = 100usize;
    for &r in &b.rates {
        if r & 0x80 == 0 {
            continue;
        }
        if let Some(i) = rate_index(r & 0x7F) {
            if i >= FIRST_OFDM {
                ofdm |= 1 << (i - FIRST_OFDM);
                low_ofdm = low_ofdm.min(i);
            } else {
                cck |= 1 << i;
                low_cck = low_cck.min(i);
            }
        }
    }
    // IWL_RATE_24M_INDEX 8, 12M 6; 6M always
    if 8 < low_ofdm {
        ofdm |= 1 << (8 - FIRST_OFDM);
    }
    if 6 < low_ofdm {
        ofdm |= 1 << (6 - FIRST_OFDM);
    }
    ofdm |= 1;
    // 11M 3, 5.5M 2, 2M 1; 1M always
    if 3 < low_cck {
        cck |= 1 << 3;
    }
    if 2 < low_cck {
        cck |= 1 << 2;
    }
    if 1 < low_cck {
        cck |= 1 << 1;
    }
    cck |= 1;
    (cck, ofdm)
}

/// The lowest rate both sides support (management frames go out at it).
pub fn lowest_rate(b: &Bss) -> usize {
    let m = rate_bitmap(b);
    (0..12).find(|i| m & (1 << i) != 0).unwrap_or(if b.band24 { 0 } else { FIRST_OFDM })
}

fn mgmt_header(f: &mut Vec<u8>, fc: u8, da: &[u8; 6], sa: &[u8; 6], bssid: &[u8; 6], seq: u16) {
    f.extend_from_slice(&[fc, 0, 0, 0]);
    f.extend_from_slice(da);
    f.extend_from_slice(sa);
    f.extend_from_slice(bssid);
    f.extend_from_slice(&(seq << 4).to_le_bytes());
}

/// Open System authentication, transaction 1.
pub fn build_auth(bssid: &[u8; 6], own: &[u8; 6], seq: u16) -> Vec<u8> {
    let mut f = Vec::with_capacity(30);
    mgmt_header(&mut f, FC_AUTH, bssid, own, bssid, seq);
    f.extend_from_slice(&[0, 0, 1, 0, 0, 0]);
    f
}

pub fn build_deauth(bssid: &[u8; 6], own: &[u8; 6], seq: u16, reason: u16) -> Vec<u8> {
    let mut f = Vec::with_capacity(26);
    mgmt_header(&mut f, FC_DEAUTH, bssid, own, bssid, seq);
    f.extend_from_slice(&reason.to_le_bytes());
    f
}

/// Association request: capability, listen interval, SSID, rates (both
/// elements), the RSN element when joining with WPA2. No HT/VHT/WMM: a
/// legacy (11a/b/g) non-QoS station.
pub fn build_assoc_req(b: &Bss, own: &[u8; 6], seq: u16, rsn: Option<&[u8]>, listen: u16) -> Vec<u8> {
    let mut f = Vec::with_capacity(128);
    mgmt_header(&mut f, FC_ASSOC_REQ, &b.bssid, own, &b.bssid, seq);
    let mut capab: u16 = 0x0001; // ESS
    if b.capab & 0x0010 != 0 {
        capab |= 0x0010; // privacy
    }
    if b.band24 {
        capab |= 0x0020 | 0x0400; // short preamble, short slot time
    }
    f.extend_from_slice(&capab.to_le_bytes());
    f.extend_from_slice(&listen.to_le_bytes());
    f.push(0);
    f.push(b.ssid.len() as u8);
    f.extend_from_slice(&b.ssid);
    let m = rate_bitmap(b);
    let rates: Vec<u8> = (0..12).filter(|i| m & (1 << i) != 0).map(|i| RATES[i]).collect();
    let n1 = rates.len().min(8);
    f.push(1);
    f.push(n1 as u8);
    f.extend_from_slice(&rates[..n1]);
    if rates.len() > 8 {
        f.push(50);
        f.push((rates.len() - 8) as u8);
        f.extend_from_slice(&rates[8..]);
    }
    if let Some(ie) = rsn {
        f.extend_from_slice(ie);
    }
    f
}

/// Authentication response: Ok(()) when it is transaction 2 of Open System
/// with status 0, Err(status) otherwise.
pub fn auth_result(f: &[u8]) -> Option<Result<(), u16>> {
    if f.len() < 30 || f[0] != FC_AUTH {
        return None;
    }
    let alg = u16::from_le_bytes([f[24], f[25]]);
    let tr = u16::from_le_bytes([f[26], f[27]]);
    let st = u16::from_le_bytes([f[28], f[29]]);
    if alg != 0 || tr != 2 {
        return None;
    }
    Some(if st == 0 { Ok(()) } else { Err(st) })
}

/// Association response: Ok(aid) or Err(status).
pub fn assoc_result(f: &[u8]) -> Option<Result<u16, u16>> {
    if f.len() < 30 || (f[0] != FC_ASSOC_RESP && f[0] != FC_REASSOC_RESP) {
        return None;
    }
    let st = u16::from_le_bytes([f[26], f[27]]);
    let aid = u16::from_le_bytes([f[28], f[29]]) & 0x3FFF;
    Some(if st == 0 { Ok(aid) } else { Err(st) })
}

/// ieee80211_hdrlen for the frames we handle.
pub fn hdrlen(f: &[u8]) -> usize {
    if f.len() < 2 {
        return 24;
    }
    let ty = (f[0] >> 2) & 3;
    if ty == 2 {
        let mut n = 24;
        if f[1] & (FC1_TODS | FC1_FROMDS) == (FC1_TODS | FC1_FROMDS) {
            n += 6;
        }
        if f[0] & 0x80 != 0 {
            n += 2; // QoS control
            if f[1] & FC1_ORDER != 0 {
                n += 4; // HT control
            }
        }
        n
    } else if ty == 1 {
        10
    } else {
        24
    }
}

/// An Ethernet frame (dst, src, type, payload) -> a To-DS non-QoS 802.11 data
/// frame with LLC/SNAP: (frame, header length).
pub fn eth_to_wlan(eth: &[u8], bssid: &[u8; 6], own: &[u8; 6], seq: u16, protect: bool) -> Option<(Vec<u8>, usize)> {
    if eth.len() < 14 {
        return None;
    }
    let mut f = Vec::with_capacity(eth.len() + 24 + 8 - 14);
    f.push(0x08);
    f.push(FC1_TODS | if protect { FC1_PROTECTED } else { 0 });
    f.extend_from_slice(&[0, 0]);
    f.extend_from_slice(bssid);
    f.extend_from_slice(own);
    f.extend_from_slice(&eth[0..6]);
    f.extend_from_slice(&(seq << 4).to_le_bytes());
    f.extend_from_slice(&RFC1042);
    f.extend_from_slice(&eth[12..]);
    Some((f, 24))
}

/// A From-DS data frame's header + its (decrypted, de-padded) LLC payload ->
/// an Ethernet frame. None for null-data frames and non-SNAP payloads.
pub fn wlan_to_eth(hdr: &[u8], payload: &[u8]) -> Option<Vec<u8>> {
    if hdr.len() < 24 || (hdr[0] >> 2) & 3 != 2 || hdr[0] & 0x40 != 0 {
        return None; // not data, or a (QoS) null function
    }
    let (da, sa) = match hdr[1] & (FC1_TODS | FC1_FROMDS) {
        FC1_FROMDS => (&hdr[4..10], &hdr[16..22]),
        0 => (&hdr[4..10], &hdr[10..16]),
        _ => return None,
    };
    if payload.len() < 8 || payload[..6] != RFC1042 {
        return None;
    }
    let mut e = Vec::with_capacity(payload.len() + 14 - 6);
    e.extend_from_slice(da);
    e.extend_from_slice(sa);
    e.extend_from_slice(&payload[6..]);
    Some(e)
}

pub fn mac_str(m: &[u8]) -> String {
    let mut s = String::new();
    for (i, b) in m.iter().enumerate() {
        if i > 0 {
            s.push(':');
        }
        let h = b"0123456789abcdef";
        s.push(h[(b >> 4) as usize] as char);
        s.push(h[(b & 15) as usize] as char);
    }
    s
}

/// 802.11 reason / status codes the user may see.
pub fn status_text(code: u16) -> &'static str {
    match code {
        1 => "unspecified failure",
        2 => "previous authentication no longer valid",
        3 => "station is leaving",
        4 => "inactivity",
        5 => "access point is full",
        6 | 7 => "frame from a station not authenticated/associated",
        10 => "capabilities not supported",
        12 => "association denied (outside the standard)",
        13 => "authentication algorithm not supported",
        15 => "challenge failure",
        17 => "access point is full (too many stations)",
        18 => "data rates not supported (the network may require 11n/ac)",
        14 | 16 | 19..=26 => "required capability missing",
        27 => "requires HT (802.11n); this driver connects with legacy rates",
        30 => "rejected temporarily (try again)",
        31 => "robust management frame policy violation",
        40 => "invalid element",
        41 => "invalid group cipher",
        42 => "invalid pairwise cipher",
        43 => "invalid AKMP",
        45 => "cipher suite rejected by policy",
        104 => "requires VHT (802.11ac)",
        _ => "",
    }
}
