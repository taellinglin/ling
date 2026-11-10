//! Intel Wi-Fi 6 AX201 driver (x86-64): the Tiger Lake PCH CNVi MAC (PCI
//! 8086:43F0 / 8086:A0F0, CSR_HW_REV 0x351 "QuZ") with the HR (AX201) CRF
//! module. A port of the parts of Linux's iwlwifi (pcie gen2 transport + mvm
//! op-mode, v6.1 -- the kernel whose 22000-family maximum firmware API is 72,
//! the API of the firmware embedded here) this hardware needs. Every register
//! offset, bit and command id below is copied from that source; the file it
//! came from is named next to it (paths relative to
//! drivers/net/wireless/intel/iwlwifi/).
//!
//! QEMU cannot emulate this device, so the driver is a staged state machine:
//! each stage records ok / FAIL plus a short reason, the boot log prints them
//! (`ling_kernel_net_init`), and the console / SSH command `wifi-intel` shows
//! them -- short lines meant to be photographed off an HDMI screen. Every
//! hardware wait is bounded by a timeout, nothing writes a register the Linux
//! driver does not write in the same situation, and a failed stage stops the
//! sequence (the later stages are not attempted).
//!
//! Stages:
//!  1. detect   -- PCI find, power state D0, memory decode + bus master on,
//!                 INTx/MSI/MSI-X off (the driver polls), BAR0 sized and
//!                 mapped (it is above 4 GiB on the target PC: map_mmio),
//!                 CSR_HW_REV read -> which firmware file this MAC needs.
//!  2. wake     -- iwl_trans_pcie_start_hw: NIC_READY handshake
//!                 (prepare_card_hw), persistence bit, SW reset, the
//!                 integrated-22000 forced power gating, APM init (INIT_DONE,
//!                 wait for MAC_CLOCK_READY), a MAC_ACCESS_REQ grab; then
//!                 CSR_HW_RF_ID, the CNVi mailbox (RF step) and the MAC
//!                 address straps from CSR 0x380.
//!  3. firmware -- parse the .ucode TLV file (SEC_RT sections split into
//!                 LMAC / UMAC / paging by the separator sections), copy the
//!                 sections into DMA memory, build the gen2 context info (RX
//!                 queue 0: free/used RBD rings + status; command queue 0: 32
//!                 long-format TFDs), write CSR_CTXT_INFO_BA, kick
//!                 UREG_CPU_INIT_RUN, wait for the ALIVE interrupt bit, stock
//!                 the RX ring (the firmware has configured the RFH by then)
//!                 and wait for the UCODE_ALIVE_NTFY packet; status 0xCAFE.
//!  4. init     -- the unified-image mvm start (iwl_run_unified_mvm_ucode):
//!                 INIT_EXTENDED_CFG_CMD (NVM), NVM_ACCESS_COMPLETE, wait for
//!                 INIT_COMPLETE_NOTIF, NVM_GET_INFO (bands, chains, LAR), the
//!                 MAC address (iwl_set_hw_address_from_csr).
//!  5. scan     -- the rest of iwl_mvm_up (TX_ANT_CONFIGURATION, BT_CONFIG,
//!                 SOC_CONFIGURATION, 3 x PHY_CONTEXT + RLC_CONFIG, empty
//!                 TEMP_REPORTING_THRESHOLDS, POWER_TABLE (CAM), MCC_UPDATE
//!                 "ZZ" when LAR, SCAN_CFG v5), the station MAC context, then
//!                 a passive SCAN_REQ_UMAC v15 over the regulatory channel
//!                 list; beacons / probe responses are parsed out of
//!                 REPLY_RX_MPDU_CMD until SCAN_COMPLETE_UMAC.
//!  6. join     -- PHY context to the AP's channel, MAC context (BSSID),
//!                 BINDING, ADD_STA (sta 0), two SCD_QUEUE_CFG queues
//!                 (management tid 15, data tid 0), SESSION_PROTECTION, Open
//!                 System auth + association as TX_CMD frames (legacy rates,
//!                 no HT/WMM), ADD_STA/MAC "associated", TLC_MNG_CONFIG, the
//!                 WPA2 4-way handshake (arch/aarch64/wpa.rs) over EAPOL data
//!                 frames, ADD_STA_KEY for the PTK and GTK (the firmware does
//!                 CCMP). Then 802.3 <-> 802.11 + LLC/SNAP in the driver and
//!                 drivers/netdev.rs routes netstack over it (DHCP runs as is).
//!
//! The firmware is NEVER started at boot (stage 1 only): the first scan or
//! join starts it (`ensure_up`), `wifi-intel up` does it by hand.
//!
//! The original plan for stages 5-6 (kept as the map of what was ported):
//!  5. scan: finish iwl_mvm_up's command list after stage 4 --
//!     TX_ANT_CONFIGURATION_CMD (0x98, valid tx ant from the PHY_SKU TLV),
//!     PHY_CONFIGURATION_CMD (0x6a v3: phy_cfg + calib triggers from the
//!     DEF_CALIB TLVs), BT_CONFIG (0x9b), SOC_CONFIGURATION_CMD (system 0x01,
//!     the capa says SOC_LATENCY_SUPPORT: low_latency_xtal + ltr_delay 2500us
//!     for iwl_qu_long_latency_trans_cfg), RFH_QUEUE_CONFIG_CMD / RSS only if
//!     more RX queues are used (keep one), DQA_ENABLE_CMD (data path 0x00,
//!     cmd_queue 0), ADD_STA for the aux station (ADD_STA v12 here: the fw
//!     uses an internal aux sta, so skip), PHY_CONTEXT_CMD x NUM_PHY_CTX,
//!     POWER_TABLE_CMD (0x77 v6, CAM), LARI_CONFIG_CHANGE (optional),
//!     MCC_UPDATE_CMD (0xc8: "ZZ" world, or the user's country) and
//!     SCAN_CFG_CMD (umac scan config, v5 here). Then SCAN_REQ_UMAC v15
//!     (mvm/scan.c iwl_mvm_scan_umac_v14_and_above: general params v11,
//!     channel params v6/v7, probe params v4) and collect the frames: every
//!     beacon / probe response arrives on RX queue 0 as REPLY_RX_MPDU_CMD
//!     (0xc1, iwl_rx_mpdu_desc v3 here) during the scan; parse SSID / RSN /
//!     WPA IEs + the RSSI from the descriptor, until
//!     SCAN_COMPLETE_UMAC (0x0f) arrives. Wire wifi::scan() to it. ~1-2 days.
//!  6. join: MAC_CONTEXT_CMD (0x28 v... add, station type), BINDING_CONTEXT
//!     _CMD (0x2b), TIME_EVENT_CMD (0x29, session protection on this fw:
//!     SESSION_PROTECTION_CMD mac group 0x05), ADD_STA (0x18 v12) for the AP,
//!     send auth / assoc frames as TX_CMD (0x1c v9, gen2 TFDs on a data
//!     queue allocated with SCD_QUEUE_CONFIG_CMD data path 0x17 v3 -- each
//!     data queue needs its own TFD ring + byte-count table), parse the
//!     responses from REPLY_RX_MPDU_CMD, then the 4-way handshake: the
//!     supplicant in arch/aarch64/wpa.rs is pure Rust (PBKDF2 / PRF-SHA1 /
//!     HMAC-SHA1 MIC / AES key unwrap) and can be shared; install the PTK
//!     and GTK with ADD_STA_KEY (0x17) so the firmware does CCMP. Then the
//!     data path: 802.3 <-> 802.11 + LLC/SNAP framing in the driver (the 22000
//!     family has no header conversion offload), a "wifi" arm in
//!     drivers/netdev.rs exactly like the Pi's arch/aarch64/netdev.rs, and
//!     netstack's DHCP runs unchanged. ~1-2 weeks of on-hardware iteration.
//!
//! Firmware: iwlwifi-QuZ-a0-hr-b0-72.ucode (linux-firmware, Intel
//! redistributable licence, see firmware/README.md) is embedded (+1.3 MiB).
//! Qu-b0 / Qu-c0 MACs need iwlwifi-Qu-b0-hr-b0-72.ucode /
//! iwlwifi-Qu-c0-hr-b0-72.ucode, read from `firmware/<name>` on storage
//! when present (they are not embedded).

use crate::arch::pci::{self, Location};
use crate::arch::timer;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// 802.11 frames (beacons, auth/assoc, LLC/SNAP data): what mac80211 does.
#[allow(dead_code)]
#[path = "iwlwifi_wlan.rs"]
mod wlan;
/// The WPA2-PSK supplicant the Pi's Wi-Fi uses (pure computation; host-tested).
#[allow(dead_code)]
#[path = "../arch/aarch64/wpa.rs"]
mod wpa;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};

static FW_QUZ_HR: &[u8] = include_bytes!("../../firmware/iwlwifi-QuZ-a0-hr-b0-72.ucode");
const FW_QUZ_HR_NAME: &str = "iwlwifi-QuZ-a0-hr-b0-72.ucode";

// ---- PCI ids (pcie/drv.c iwl_hw_card_ids: CNVi Qu-family MACs, cfg/22000.c) ----
const INTEL: u16 = 0x8086;
/// 0x43F0 Tiger Lake-H PCH (the target PC), 0xA0F0 Tiger Lake-LP, 0x34F0
/// Ice Lake, 0x02F0 / 0x06F0 Comet Lake, 0x3DF0 / 0x4DF0: all
/// IWL_DEVICE_FAMILY_22000 gen2 CNVi (pcie/drv.c).
const QU_IDS: [u16; 7] = [0x43F0, 0xA0F0, 0x34F0, 0x02F0, 0x06F0, 0x3DF0, 0x4DF0];

// ---- CSR (iwl-csr.h) ----
const CSR_HW_IF_CONFIG_REG: u32 = 0x000;
const CSR_INT_COALESCING: u32 = 0x004;
const CSR_INT: u32 = 0x008;
const CSR_INT_MASK: u32 = 0x00c;
const CSR_FH_INT_STATUS: u32 = 0x010;
const CSR_RESET: u32 = 0x020;
const CSR_GP_CNTRL: u32 = 0x024;
const CSR_HW_REV: u32 = 0x028;
const CSR_GIO_REG: u32 = 0x03C;
const CSR_UCODE_DRV_GP1_CLR: u32 = 0x05c;
const CSR_MBOX_SET_REG: u32 = 0x88;
const CSR_HW_RF_ID: u32 = 0x09c;
const CSR_MAC_SHADOW_REG_CTRL: u32 = 0x0A8;
const CSR_GIO_CHICKEN_BITS: u32 = 0x100;
const CSR_DBG_HPET_MEM_REG: u32 = 0x240;
const CSR_DBG_LINK_PWR_MGMT_REG: u32 = 0x250;
/// iwl-context-info.h
const CSR_CTXT_INFO_BA: u32 = 0x40;
/// cfg/22000.c IWL_DEVICE_22000_COMMON .mac_addr_from_csr; iwl-csr.h CSR_MAC_ADDR*
const CSR_ADDR_BASE: u32 = 0x380;

const CSR_HW_IF_CONFIG_REG_MSK_MAC_STEP_DASH: u32 = 0x0000000F;
const CSR_HW_IF_CONFIG_REG_BIT_MAC_SI: u32 = 0x00000100;
const CSR_HW_IF_CONFIG_REG_BIT_RADIO_SI: u32 = 0x00000200;
const CSR_HW_IF_CONFIG_REG_D3_DEBUG: u32 = 0x00000200;
const CSR_HW_IF_CONFIG_REG_MSK_PHY_TYPE: u32 = 0x00000C00;
const CSR_HW_IF_CONFIG_REG_MSK_PHY_DASH: u32 = 0x00003000;
const CSR_HW_IF_CONFIG_REG_MSK_PHY_STEP: u32 = 0x0000C000;
const CSR_HW_IF_CONFIG_REG_POS_PHY_TYPE: u32 = 10;
const CSR_HW_IF_CONFIG_REG_POS_PHY_DASH: u32 = 12;
const CSR_HW_IF_CONFIG_REG_POS_PHY_STEP: u32 = 14;
const CSR_HW_IF_CONFIG_REG_BIT_HAP_WAKE_L1A: u32 = 0x00080000;
const CSR_HW_IF_CONFIG_REG_BIT_NIC_READY: u32 = 0x00400000;
const CSR_HW_IF_CONFIG_REG_PREPARE: u32 = 0x08000000;
const CSR_MBOX_SET_REG_OS_ALIVE: u32 = 1 << 5;

const CSR_INT_BIT_FH_RX: u32 = 1 << 31;
const CSR_INT_BIT_HW_ERR: u32 = 1 << 29;
const CSR_INT_BIT_RX_PERIODIC: u32 = 1 << 28;
const CSR_INT_BIT_FH_TX: u32 = 1 << 27;
const CSR_INT_BIT_SW_ERR: u32 = 1 << 25;
const CSR_INT_BIT_RF_KILL: u32 = 1 << 7;
const CSR_INT_BIT_SW_RX: u32 = 1 << 3;
const CSR_INT_BIT_WAKEUP: u32 = 1 << 1;
const CSR_INT_BIT_ALIVE: u32 = 1 << 0;
/// iwl-csr.h CSR_INI_SET_MASK
const CSR_INI_SET_MASK: u32 = CSR_INT_BIT_FH_RX
    | CSR_INT_BIT_HW_ERR
    | CSR_INT_BIT_FH_TX
    | CSR_INT_BIT_SW_ERR
    | CSR_INT_BIT_RF_KILL
    | CSR_INT_BIT_SW_RX
    | CSR_INT_BIT_WAKEUP
    | CSR_INT_BIT_ALIVE
    | CSR_INT_BIT_RX_PERIODIC;

const CSR_RESET_REG_FLAG_SW_RESET: u32 = 0x00000080;
const CSR_RESET_REG_FLAG_MASTER_DISABLED: u32 = 0x00000100;
const CSR_RESET_REG_FLAG_STOP_MASTER: u32 = 0x00000200;
const CSR_RESET_LINK_PWR_MGMT_DISABLED: u32 = 0x80000000;

const CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY: u32 = 0x00000001;
const CSR_GP_CNTRL_REG_FLAG_INIT_DONE: u32 = 0x00000004;
const CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ: u32 = 0x00000008;
const CSR_GP_CNTRL_REG_FLAG_GOING_TO_SLEEP: u32 = 0x00000010;
const CSR_GP_CNTRL_REG_VAL_MAC_ACCESS_EN: u32 = 0x00000001;
const CSR_GP_CNTRL_REG_FLAG_HW_RF_KILL_SW: u32 = 0x08000000;
const CSR_GP_CNTRL_REG_FLAG_RFKILL_WAKE_L1A_EN: u32 = 0x04000000;

const CSR_UCODE_SW_BIT_RFKILL: u32 = 0x00000002;
const CSR_UCODE_DRV_GP1_BIT_CMD_BLOCKED: u32 = 0x00000004;
const CSR_GIO_REG_VAL_L0S_DISABLED: u32 = 0x00000002;
const CSR_GIO_CHICKEN_BITS_REG_BIT_L1A_NO_L0S_RX: u32 = 0x00800000;
const CSR_DBG_HPET_MEM_REG_VAL: u32 = 0xFFFF0000;
/// iwl-csr.h IWL_HOST_INT_TIMEOUT_DEF
const IWL_HOST_INT_TIMEOUT_DEF: u8 = 0x40;

const CSR_HW_REV_TYPE_QU_B0: u32 = 0x0000331;
const CSR_HW_REV_TYPE_QU_C0: u32 = 0x0000332;
const CSR_HW_REV_TYPE_QUZ: u32 = 0x0000351;
/// CSR_HW_RFID_TYPE(CSR_HW_RF_ID_TYPE_HR / _HR1)
const RF_TYPE_HR: u32 = 0x10A;
const RF_TYPE_HR1: u32 = 0x10C;

// HBUS (iwl-csr.h, HBUS_BASE 0x400)
const HBUS_TARG_MEM_RADDR: u32 = 0x400 + 0x00c;
const HBUS_TARG_MEM_RDAT: u32 = 0x400 + 0x01c;
const HBUS_TARG_PRPH_WADDR: u32 = 0x400 + 0x044;
const HBUS_TARG_PRPH_RADDR: u32 = 0x400 + 0x048;
const HBUS_TARG_PRPH_WDAT: u32 = 0x400 + 0x04c;
const HBUS_TARG_PRPH_RDAT: u32 = 0x400 + 0x050;
const HBUS_TARG_WRPTR: u32 = 0x400 + 0x060;
/// pcie/trans.c iwl_trans_pcie_prph_msk (pre-AX210)
const PRPH_MASK: u32 = 0x000FFFFF;

// ---- periphery (iwl-prph.h; umac_prph_offset is 0 on the 22000 family) ----
const UREG_CPU_INIT_RUN: u32 = 0xa05c44;
const HPM_MAC_LTR_CSR: u32 = 0xa0348c;
const HPM_MAC_LRT_ENABLE_ALL: u32 = 0xf;
const HPM_UMAC_LTR: u32 = 0xa03480;
const PREG_PRPH_WPROT_22000: u32 = 0xA04D00;
const UMAG_SB_CPU_1_STATUS: u32 = 0xA038C0;
const UMAG_SB_CPU_2_STATUS: u32 = 0xA038C4;
const UREG_UMAC_CURRENT_PC: u32 = 0xa05c18;
const UREG_LMAC1_CURRENT_PC: u32 = 0xa05c1c;
const WFPM_LMAC1_PD_NOTIFICATION: u32 = 0xa0338c;
const HPM_DEBUG: u32 = 0xA03440;
const PERSISTENCE_BIT: u32 = 1 << 12;
const PREG_WFPM_ACCESS: u32 = 1 << 12;
const HPM_HIPM_GEN_CFG: u32 = 0xA03458;
const HPM_HIPM_GEN_CFG_CR_PG_EN: u32 = 1 << 0;
const HPM_HIPM_GEN_CFG_CR_SLP_EN: u32 = 1 << 1;
const HPM_HIPM_GEN_CFG_CR_FORCE_ACTIVE: u32 = 1 << 10;
/// iwl-prph.h WFPM_CTRL_REG / ENABLE_WFPM: pcie/drv.c get_crf_id sets this
/// at probe ("Enable access to peripheral registers") before reading
/// SD_REG_VER / CNVI_AUX_MISC_CHIP. Without it the Z590's AX201 answers
/// only the HPM block; UMAG/UREG/CNVI read 0xa5a5a5a2 and the
/// UREG_CPU_INIT_RUN write is dropped (no ALIVE).
const WFPM_CTRL_REG: u32 = 0xA03030;
const ENABLE_WFPM: u32 = 1 << 31;
const SD_REG_VER: u32 = 0xA29600;
const CNVI_MBOX_C: u32 = 0xA3400C;
/// iwl-prph.h CNVI_AUX_MISC_CHIP: the CNVi chip id, readable whenever the
/// periphery is up (a cheap "is PRPH alive" probe).
const CNVI_AUX_MISC_CHIP: u32 = 0xA200B0;
/// Bring-up variant: 0 = Linux order (forced power gating for integrated
/// 22000), 1 = the same without the power-gating step (fallback when the
/// periphery is unreachable after it).
static mut WAKE_VARIANT: u8 = 0;
/// iwl-fh.h (mq RX, pre-AX210)
const RFH_RXF_DMA_CFG: u32 = 0xA09820;
const RFH_GEN_STATUS: u32 = 0xA09808;
const RXF_DMA_IDLE: u32 = 1 << 31;
const RFH_Q0_FRBDCB_WIDX_TRG: u32 = 0x1C80;

/// pcie/trans-gen2.c iwl_pcie_set_ltr: NO_SNOOP_REQ | scale USEC (2) | 250,
/// SNOOP_REQ | scale USEC (2) | 250 (iwl-csr.h CSR_LTR_LONG_VAL_AD_*).
const LTR_VAL: u32 = 0x8000_0000 | (2 << 26) | (250 << 16) | 0x8000 | (2 << 10) | 250;

// ---- context info (iwl-context-info.h) ----
const IWL_CTXT_INFO_TFD_FORMAT_LONG: u32 = 0x0100;
const IWL_CTXT_INFO_RB_CB_SIZE_POS: u32 = 4; // mask 0x00f0
const IWL_CTXT_INFO_RB_SIZE_POS: u32 = 9; // mask 0x1e00
const IWL_CTXT_INFO_RB_SIZE_4K: u32 = 0x4;
const IWL_MAX_DRAM_ENTRY: usize = 64;
const CTXT_SIZE: usize = 1792; // sizeof(struct iwl_context_info)
const CTXT_OFF_FREE_RBD: usize = 24;
const CTXT_OFF_USED_RBD: usize = 32;
const CTXT_OFF_STATUS_WR: usize = 40;
const CTXT_OFF_CMDQ_ADDR: usize = 48;
const CTXT_OFF_CMDQ_SIZE: usize = 56;
const CTXT_OFF_UMAC_IMG: usize = 192;
const CTXT_OFF_LMAC_IMG: usize = 192 + 8 * IWL_MAX_DRAM_ENTRY;
const CTXT_OFF_VIRT_IMG: usize = 192 + 16 * IWL_MAX_DRAM_ENTRY;

// ---- queues ----
/// RBD ring entries (a power of two, 8..2048: IWL_CTXT_INFO_RB_CB_SIZE).
/// Linux uses 2048 (IWL_NUM_RBDS_22000_HE); one RX queue at our rates needs
/// far fewer.
const RX_RING: usize = 256;
const RX_RING_LOG2: u32 = 8;
/// Buffers in flight: ring - 1 (Linux: num_rx_bufs - 1), 4 KiB each
/// (IWL_AMSDU_4K, mvm's default rx_buf_size).
const RX_BUFS: usize = RX_RING - 1;
const RB_SIZE: usize = 4096;
/// iwl-trans.h
const FH_RSCSR_FRAME_SIZE_MSK: u32 = 0x00003FFF;
const FH_RSCSR_FRAME_INVALID: u32 = 0x55550000;
const FH_RSCSR_FRAME_ALIGN: usize = 0x40;
/// fw/api/cmdhdr.h
const SEQ_RX_FRAME: u16 = 0x8000;
/// fw/api/txq.h IWL_CMD_QUEUE_SIZE; queue 0 = IWL_MVM_DQA_CMD_QUEUE
const CMD_SLOTS: usize = 32;
const TFD_SIZE: usize = 256; // sizeof(struct iwl_tfh_tfd)
/// iwl-trans.h IWL_FIRST_TB_SIZE
const FIRST_TB: usize = 20;
/// cfg/22000.c iwl_22000_base_params.max_tfd_queue_size (write-pointer wrap)
const MAX_TFD_QUEUE: u32 = 256;
const CMD_BUF: usize = 4096;

// ---- commands (fw/api/commands.h, alive.h, nvm-reg.h) ----
const LEGACY_GROUP: u8 = 0x0;
const SYSTEM_GROUP: u8 = 0x2;
const REGULATORY_AND_NVM_GROUP: u8 = 0xc;
const UCODE_ALIVE_NTFY: u8 = 0x1;
const INIT_COMPLETE_NOTIF: u8 = 0x4;
const INIT_EXTENDED_CFG_CMD: u8 = 0x03;
const NVM_ACCESS_COMPLETE: u8 = 0x0;
const NVM_GET_INFO: u8 = 0x2;
/// fw/api/alive.h enum iwl_extended_cfg_flags: BIT(IWL_INIT_NVM)
const IWL_INIT_NVM: u32 = 1 << 1;
const IWL_ALIVE_STATUS_OK: u16 = 0xCAFE;
/// fw/api/cmdhdr.h DEF_ID: legacy (group 0) commands go out in the "always
/// long" group 1 once the firmware has wide command headers (iwl-trans.c
/// iwl_trans_send_cmd); notifications may carry either.
const LONG_GROUP: u8 = 0x1;
const MAC_CONF_GROUP: u8 = 0x3;
const PHY_OPS_GROUP: u8 = 0x4;
const DATA_PATH_GROUP: u8 = 0x5;
// legacy / long-group command ids (fw/api/commands.h)
const PHY_CONTEXT_CMD: u8 = 0x08;
const SCAN_REQ_UMAC: u8 = 0x0d;
const SCAN_ABORT_UMAC: u8 = 0x0e;
const SCAN_COMPLETE_UMAC: u8 = 0x0f;
const ADD_STA_KEY: u8 = 0x17;
const ADD_STA: u8 = 0x18;
const REMOVE_STA: u8 = 0x19;
const TX_CMD: u8 = 0x1c;
const SCD_QUEUE_CFG: u8 = 0x1d;
const MAC_CONTEXT_CMD: u8 = 0x28;
const BINDING_CONTEXT_CMD: u8 = 0x2b;
const POWER_TABLE_CMD: u8 = 0x77;
const TX_ANT_CONFIGURATION_CMD: u8 = 0x98;
const BT_CONFIG: u8 = 0x9b;
const MISSED_BEACONS_NOTIFICATION: u8 = 0xa2;
const REPLY_RX_MPDU_CMD: u8 = 0xc1;
const MCC_UPDATE_CMD: u8 = 0xc8;
const SCAN_CFG_CMD: u8 = 0x0c;
// grouped commands
const SOC_CONFIGURATION_CMD: u8 = 0x01; // SYSTEM_GROUP
const TEMP_REPORTING_THRESHOLDS_CMD: u8 = 0x04; // PHY_OPS_GROUP
const RLC_CONFIG_CMD: u8 = 0x08; // DATA_PATH_GROUP
const TLC_MNG_CONFIG_CMD: u8 = 0x0f; // DATA_PATH_GROUP
const SESSION_PROTECTION_CMD: u8 = 0x05; // MAC_CONF_GROUP
const SESSION_PROTECTION_NOTIF: u8 = 0xfb; // MAC_CONF_GROUP
/// fw/api/context.h
const FW_CTXT_ACTION_ADD: u32 = 1;
const FW_CTXT_ACTION_MODIFY: u32 = 2;
const FW_CTXT_ACTION_REMOVE: u32 = 3;
const FW_CTXT_INVALID: u32 = 0xffff_ffff;
/// fw/api/mac.h
const FW_MAC_TYPE_BSS_STA: u32 = 5;
const MAC_FILTER_ACCEPT_GRP: u32 = 1 << 2;
const MAC_FILTER_IN_BEACON: u32 = 1 << 6;
const MAC_FLG_SHORT_SLOT: u32 = 1 << 4;
const MAC_FLG_SHORT_PREAMBLE: u32 = 1 << 5;
/// fw/api/phy-ctxt.h
const PHY_BAND_5: u8 = 0;
const PHY_BAND_24: u8 = 1;
/// fw/api/sta.h
const STA_FLG_FAT_EN_MSK: u32 = 3 << 26;
const STA_FLG_MIMO_EN_MSK: u32 = 3 << 28;
const STA_FLG_RTS_MIMO_PROT: u32 = 1 << 17;
const ADD_STA_SUCCESS: u32 = 0x1;
const STA_KEY_FLG_CCM: u16 = 2;
const STA_KEY_FLG_WEP_KEY_MAP: u16 = 1 << 3;
const STA_KEY_MULTICAST: u16 = 1 << 14;
/// fw/api/tx.h, iwl-trans.h
const IWL_TX_FLAGS_CMD_RATE: u32 = 1 << 0;
const IWL_TX_FLAGS_ENCRYPT_DIS: u32 = 1 << 1;
const TX_CMD_OFFLD_PAD: u16 = 1 << 13;
const TX_QUEUE_CFG_ENABLE_QUEUE: u16 = 1 << 0;
const IWL_MGMT_TID: u8 = 15;
const IWL_TID_NON_QOS: u8 = 0;
/// fw/api/rs.h (rate_n_flags v2: TX_CMD version > 8)
const RATE_MCS_LEGACY_OFDM_MSK: u32 = 1 << 8;
const RATE_MCS_ANT_A_MSK: u32 = 1 << 14;
const RATE_MCS_ANT_B_MSK: u32 = 2 << 14;
/// fw/api/rx.h
const RX_DESC_SIZE_V1: usize = 48;
const RX_MPDU_STATUS_CRC_OK: u32 = 1 << 0;
const RX_MPDU_STATUS_OVERRUN_OK: u32 = 1 << 1;
const RX_MPDU_STATUS_MIC_OK: u32 = 1 << 6;
const RX_MPDU_STATUS_SEC_MASK: u32 = 7 << 8;
const RX_MPDU_STATUS_SEC_NONE: u32 = 0;
const RX_MPDU_STATUS_SEC_CCM: u32 = 2 << 8;
const RX_MPDU_MFLG2_PAD: u8 = 0x20;
/// iwl-trans.h: the RX packet's len_n_flags
const FH_RSCSR_RADA_EN: u32 = 1 << 26;
/// iwl-prph.h: the interrupt mode the driver uses (iwl_pcie_conf_msix_hw)
const UREG_CHICK: u32 = 0xA05C00;
const UREG_CHICK_MSI_ENABLE: u32 = 1 << 24;
/// iwl-csr.h: where the causes go when the device is in MSI-X mode
const CSR_MSIX_FH_INT_CAUSES_AD: u32 = 0x2800;
const CSR_MSIX_HW_INT_CAUSES_AD: u32 = 0x2808;
const MSIX_HW_INT_CAUSES_REG_ALIVE: u32 = 1 << 0;
/// fw/file.h capability / API bits used below
const CAPA_LAR_SUPPORT: u32 = 1;
const CAPA_UMAC_SCAN: u32 = 2;
const CAPA_SOC_LATENCY_SUPPORT: u32 = 37;
const CAPA_BINDING_CDB_SUPPORT: u32 = 39;
const CAPA_ULTRA_HB_CHANNELS: u32 = 48;
const CAPA_SESSION_PROT_CMD: u32 = 54;
const CAPA_BT_MPLUT_SUPPORT: u32 = 67;
const CAPA_CT_KILL_BY_FW: u32 = 74;
const CAPA_TEMP_THS_REPORT_SUPPORT: u32 = 75;
const CAPA_MCC_UPDATE_11AX_SUPPORT: u32 = 89;
const API_WIFI_MCC_UPDATE: u32 = 9;
const API_STA_TYPE: u32 = 30;
const API_REDUCED_SCAN_CONFIG: u32 = 56;
/// TLV types (fw/file.h)
const TLV_API_CHANGES_SET: u32 = 29;
const TLV_ENABLED_CAPABILITIES: u32 = 30;
const TLV_CMD_VERSIONS: u32 = 48;
/// iwl-nvm-parse.c iwl_ext_nvm_channels (22000 family)
const NVM_CHANNELS: [u8; 51] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 36, 40, 44, 48, 52, 56, 60, 64, 68, 72, 76, 80, 84, 88, 92, 96, 100, 104, 108,
    112, 116, 120, 124, 128, 132, 136, 140, 144, 149, 153, 157, 161, 165, 169, 173, 177, 181,
];
/// One MAC (id 0, color 0), one PHY context (0) in use, one station (the AP, 0).
const MAC_ID: u32 = 0;
const PHY_ID: u32 = 0;
const AP_STA: u8 = 0;
/// Wi-Fi TX queue memory: 16 slots for management (IWL_MGMT_QUEUE_SIZE),
/// 64 for data; each slot one 4 KiB page holding header + payload.
const MGMT_Q_SIZE: usize = 16;
const DATA_Q_SIZE: usize = 64;
const RX_FRAMES_MAX: usize = 128;

// ---- firmware file (fw/file.h) ----
const IWL_TLV_UCODE_MAGIC: u32 = 0x0a4c5749;
const TLV_SEC_RT: u32 = 19;
const TLV_SECURE_SEC_RT: u32 = 24;
const TLV_PHY_SKU: u32 = 23;
const TLV_NUM_OF_CPU: u32 = 27;
const TLV_PAGING: u32 = 32;
const TLV_FW_VERSION: u32 = 36;
const CPU1_CPU2_SEPARATOR_SECTION: u32 = 0xFFFFCCCC;
const PAGING_SEPARATOR_SECTION: u32 = 0xAAAABBBB;
const MAX_SECTION: usize = 32 * 1024;
/// fw/file.h FW_PHY_CFG_RADIO_* / *_CHAIN
const FW_PHY_CFG_RADIO_TYPE: u32 = 0x3;
const FW_PHY_CFG_RADIO_STEP_POS: u32 = 2;
const FW_PHY_CFG_RADIO_DASH_POS: u32 = 4;

// ---- timeouts (us) ----
/// pcie/trans.c HW_READY_TIMEOUT
const HW_READY_TIMEOUT: u64 = 50;
const NIC_INIT_TIMEOUT: u64 = 25_000;
const GRAB_TIMEOUT: u64 = 15_000;
/// mvm/fw.c MVM_UCODE_ALIVE_TIMEOUT is 1 s; twice that for the interrupt bit
const ALIVE_IRQ_TIMEOUT: u64 = 2_000_000;
const ALIVE_PKT_TIMEOUT: u64 = 1_000_000;
/// iwl-trans.h HOST_COMPLETE_TIMEOUT (2 s)
const CMD_TIMEOUT: u64 = 2_000_000;
const INIT_COMPLETE_TIMEOUT: u64 = 2_000_000;

// ======================================================================

#[derive(Clone, Copy, PartialEq, Eq)]
enum St {
    NotRun,
    Ok,
    Fail,
    Todo,
}

struct Stage {
    st: St,
    text: String,
}

const STAGE_NAMES: [&str; 6] = ["detect", "wake", "firmware", "init", "scan", "join"];

struct Pkt {
    group: u8,
    cmd: u8,
    seq: u16,
    data: Vec<u8>,
}

/// One gen2 TX queue (queue/tx.c): a ring of long-format TFDs, its byte-count
/// table, one 4 KiB page per slot (TX command + 802.11 header, payload at +128).
struct TxQ {
    /// the firmware's queue number (from the SCD_QUEUE_CFG response)
    qid: u16,
    size: usize,
    tfds: u64,
    bc: u64,
    bufs: Vec<u64>,
    /// write / read pointers, wrapping at MAX_TFD_QUEUE
    wp: u32,
    rp: u32,
    active: bool,
}

/// The association with one access point.
struct Link {
    bss: wlan::Bss,
    aid: u16,
    seq: u16,
    assoc: bool,
    /// keys installed (the firmware then does CCMP on TX and RX)
    ptk: bool,
    authorized: bool,
    /// firmware rate scaling configured (TLC_MNG_CONFIG_CMD accepted)
    tlc: bool,
    sup: Option<wpa::Handshake>,
    /// data frames for netstack, as Ethernet frames
    rx: VecDeque<Vec<u8>>,
    /// management frames from the AP (auth / assoc responses)
    mgmt: Vec<Vec<u8>>,
    /// EAPOL frames (Ethernet framing) for the supplicant
    eapol: VecDeque<Vec<u8>>,
    /// the last beacon: (TSF, device time, DTIM count)
    beacon: Option<(u64, u32, u8)>,
    /// why the link went away (deauth / disassoc / missed beacons)
    lost: Option<String>,
    rssi: i32,
    tx_fail_run: u32,
    tx_ok: u32,
    tx_err: u32,
    rx_data: u32,
}

struct Dma {
    ctxt: u64,
    rx_free: u64,
    rx_used: u64,
    rx_stts: u64,
    rx_bufs: Vec<u64>,
    cmd_tfds: u64,
    cmd_bufs: u64,
    secs: Vec<u64>,
}

struct Dev {
    loc: Location,
    device_id: u16,
    subsys: u32,
    bar0_phys: u64,
    bar0_len: u64,
    base: u64,
    hw_rev: u32,
    rf_id: u32,
    cnvi_mbox: u32,
    phy_config: u32,
    fw_version: String,
    fw_name: &'static str,
    mac: [u8; 6],
    dma: Option<Dma>,
    rx_read: usize,
    rx_write: usize,
    rx_write_actual: usize,
    cmd_wp: u32,
    inbox: Vec<Pkt>,
    log: Vec<(u8, u8)>,
    alive_ms: u64,
    lmac_err: u32,
    umac_err: u32,
    // ---- from the firmware file (TLVs 29, 30, 48) ----
    capa: [u32; 4],
    api: [u32; 3],
    cmd_ver: Vec<(u8, u8, u8)>,
    // ---- from NVM_GET_INFO ----
    nvm_sku: u32,
    nvm_tx: u32,
    nvm_rx: u32,
    nvm_lar: bool,
    nvm_ch: Vec<u32>,
    // ---- the mvm "up" state (iwl_mvm_up + the interface's MAC context) ----
    configured: bool,
    channels: Vec<u8>,
    mcc: u16,
    bss: Vec<wlan::Bss>,
    scanning: bool,
    scan_ms: u64,
    mpdus: u32,
    txq: [TxQ; 2],
    link: Option<Link>,
    /// a command timed out or the firmware asserted: the next bring-up
    /// restarts it from scratch
    wedged: bool,
    last_up_try_ms: u64,
    /// TX status of the last frame per queue slot we wait on: (qid, idx, status)
    tx_done: Vec<(u16, u8, u8)>,
}

static mut DEV: Option<Dev> = None;
static mut STAGES: Option<Vec<Stage>> = None;
static mut PROBED: bool = false;
/// The PCI scan found no supported device.
static mut ABSENT: bool = false;

fn stages() -> &'static mut Vec<Stage> {
    let s = unsafe { &mut *(&raw mut STAGES) };
    if s.is_none() {
        *s = Some((0..6).map(|_| Stage { st: St::NotRun, text: String::new() }).collect());
    }
    s.as_mut().unwrap()
}

fn set_stage(i: usize, st: St, text: String) {
    let s = &mut stages()[i];
    s.st = st;
    s.text = text;
}

fn dev() -> Option<&'static mut Dev> {
    unsafe { (*(&raw mut DEV)).as_mut() }
}

// ---- small helpers ----

fn delay_us(us: u64) {
    timer::delay_us(us);
}

fn now_us() -> u64 {
    timer::now_us()
}

fn mac_str(m: &[u8; 6]) -> String {
    format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5])
}

fn rd16(b: &[u8], o: usize) -> u16 {
    if o + 2 > b.len() {
        return 0;
    }
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn rd32(b: &[u8], o: usize) -> u32 {
    if o + 4 > b.len() {
        return 0;
    }
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// 2^order contiguous zeroed frames below 4 GiB (identity-mapped, coherent).
fn dma_alloc(bytes: usize) -> Option<u64> {
    let pages = bytes.div_ceil(4096).max(1);
    let order = pages.next_power_of_two().trailing_zeros() as usize;
    let phys = crate::mm::frame::alloc_frames(order)?;
    let len = 4096u64 << order;
    if phys == 0 || phys + len > 1 << 32 {
        // outside the identity map / beyond 32-bit DMA comfort: leak it
        return None;
    }
    unsafe { core::ptr::write_bytes(phys as *mut u8, 0, len as usize) };
    Some(phys)
}

unsafe fn dw64(addr: u64, v: u64) {
    write_volatile(addr as *mut u64, v);
}
unsafe fn dw32(addr: u64, v: u32) {
    write_volatile(addr as *mut u32, v);
}
unsafe fn dw16(addr: u64, v: u16) {
    write_volatile(addr as *mut u16, v);
}

// ---- register access (iwl-io.c / pcie/trans.c) ----

impl Dev {
    fn r32(&self, off: u32) -> u32 {
        unsafe { read_volatile((self.base + off as u64) as *const u32) }
    }
    fn w32(&self, off: u32, v: u32) {
        unsafe { write_volatile((self.base + off as u64) as *mut u32, v) }
    }
    fn w8(&self, off: u32, v: u8) {
        unsafe { write_volatile((self.base + off as u64) as *mut u8, v) }
    }
    fn set_bit(&self, off: u32, mask: u32) {
        let v = self.r32(off);
        self.w32(off, v | mask);
    }
    fn clear_bit(&self, off: u32, mask: u32) {
        let v = self.r32(off);
        self.w32(off, v & !mask);
    }
    /// iwl_poll_bit: (reg & mask) == (bits & mask) within timeout_us (10 us steps).
    fn poll_bit(&self, off: u32, bits: u32, mask: u32, timeout_us: u64) -> bool {
        let mut t = 0;
        loop {
            if self.r32(off) & mask == bits & mask {
                return true;
            }
            if t >= timeout_us {
                return false;
            }
            delay_us(10);
            t += 10;
        }
    }

    fn read_prph_no_grab(&self, reg: u32) -> u32 {
        self.w32(HBUS_TARG_PRPH_RADDR, (reg & PRPH_MASK) | (3 << 24));
        self.r32(HBUS_TARG_PRPH_RDAT)
    }
    fn write_prph_no_grab(&self, reg: u32, v: u32) {
        self.w32(HBUS_TARG_PRPH_WADDR, (reg & PRPH_MASK) | (3 << 24));
        self.w32(HBUS_TARG_PRPH_WDAT, v);
    }

    /// __iwl_trans_pcie_grab_nic_access (no forced NMI on failure).
    fn grab(&self) -> bool {
        self.set_bit(CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ);
        delay_us(2);
        self.poll_bit(
            CSR_GP_CNTRL,
            CSR_GP_CNTRL_REG_VAL_MAC_ACCESS_EN,
            CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY | CSR_GP_CNTRL_REG_FLAG_GOING_TO_SLEEP,
            GRAB_TIMEOUT,
        )
    }
    fn release(&self) {
        self.clear_bit(CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ);
    }
    fn read_prph(&self, reg: u32) -> Option<u32> {
        if !self.grab() {
            self.release();
            return None;
        }
        let v = self.read_prph_no_grab(reg);
        self.release();
        Some(v)
    }
    fn write_prph(&self, reg: u32, v: u32) -> bool {
        if !self.grab() {
            self.release();
            return false;
        }
        self.write_prph_no_grab(reg, v);
        self.release();
        true
    }
    fn set_bits_prph(&self, reg: u32, mask: u32) -> bool {
        if !self.grab() {
            self.release();
            return false;
        }
        let v = self.read_prph_no_grab(reg);
        self.write_prph_no_grab(reg, v | mask);
        self.release();
        true
    }
    fn clear_bits_prph(&self, reg: u32, mask: u32) -> bool {
        if !self.grab() {
            self.release();
            return false;
        }
        let v = self.read_prph_no_grab(reg);
        self.write_prph_no_grab(reg, v & !mask);
        self.release();
        true
    }
    /// iwl_trans_pcie_read_mem, one dword of device SRAM.
    fn read_mem32(&self, addr: u32) -> Option<u32> {
        if !self.grab() {
            self.release();
            return None;
        }
        self.w32(HBUS_TARG_MEM_RADDR, addr);
        let v = self.r32(HBUS_TARG_MEM_RDAT);
        self.release();
        Some(v)
    }

    /// iwl_disable_interrupts (non-MSI-X): mask everything, ack everything.
    fn disable_interrupts(&self) {
        self.w32(CSR_INT_MASK, 0);
        self.w32(CSR_INT, 0xFFFF_FFFF);
        self.w32(CSR_FH_INT_STATUS, 0xFFFF_FFFF);
    }

    /// iwl_enable_rfkill_int (non-MSI-X, family >= 9000)
    fn enable_rfkill_int(&self) {
        self.w32(CSR_INT_MASK, CSR_INT_BIT_RF_KILL);
        self.set_bit(CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_RFKILL_WAKE_L1A_EN);
    }

    fn hw_rfkill(&self) -> bool {
        // pcie/internal.h iwl_is_rfkill_set
        self.r32(CSR_GP_CNTRL) & CSR_GP_CNTRL_REG_FLAG_HW_RF_KILL_SW == 0
    }

    // ---- stage 2 pieces (pcie/trans.c, iwl-io.c) ----

    /// iwl_pcie_set_hw_ready
    fn set_hw_ready(&self) -> bool {
        self.set_bit(CSR_HW_IF_CONFIG_REG, CSR_HW_IF_CONFIG_REG_BIT_NIC_READY);
        let ok = self.poll_bit(
            CSR_HW_IF_CONFIG_REG,
            CSR_HW_IF_CONFIG_REG_BIT_NIC_READY,
            CSR_HW_IF_CONFIG_REG_BIT_NIC_READY,
            HW_READY_TIMEOUT,
        );
        if ok {
            self.set_bit(CSR_MBOX_SET_REG, CSR_MBOX_SET_REG_OS_ALIVE);
        }
        ok
    }

    /// iwl_pcie_prepare_card_hw (bounded: 10 x 150 ms + 25 ms at worst).
    fn prepare_card_hw(&self) -> Result<(), String> {
        if self.set_hw_ready() {
            return Ok(());
        }
        self.set_bit(CSR_DBG_LINK_PWR_MGMT_REG, CSR_RESET_LINK_PWR_MGMT_DISABLED);
        delay_us(1500);
        for _ in 0..10 {
            self.set_bit(CSR_HW_IF_CONFIG_REG, CSR_HW_IF_CONFIG_REG_PREPARE);
            let mut t = 0;
            while t < 150_000 {
                if self.set_hw_ready() {
                    return Ok(());
                }
                delay_us(200);
                t += 200;
            }
            delay_us(25_000);
        }
        Err(format!("NIC_READY never set (HW_IF_CONFIG 0x{:08x}; owned by ME/CSME?)", self.r32(CSR_HW_IF_CONFIG_REG)))
    }

    /// iwl_finish_nic_init (pre-BZ): INIT_DONE, wait for MAC_CLOCK_READY.
    fn finish_nic_init(&self) -> Result<(), String> {
        self.set_bit(CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_INIT_DONE);
        if self.poll_bit(CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY, CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY, NIC_INIT_TIMEOUT) {
            Ok(())
        } else {
            Err(format!("MAC clock never ready (GP_CNTRL 0x{:08x})", self.r32(CSR_GP_CNTRL)))
        }
    }

    /// iwl_trans_pcie_sw_reset (pre-BZ)
    fn sw_reset(&self, retake: bool) -> Result<(), String> {
        self.set_bit(CSR_RESET, CSR_RESET_REG_FLAG_SW_RESET);
        delay_us(5_000);
        if retake {
            self.prepare_card_hw()
        } else {
            Ok(())
        }
    }

    /// iwl_trans_pcie_clear_persistence_bit (IWL_DEVICE_FAMILY_22000)
    fn clear_persistence_bit(&self) -> Result<(), String> {
        let hpm = self.read_prph_no_grab(HPM_DEBUG);
        if hpm != 0xa5a5_a5a0 && hpm & PERSISTENCE_BIT != 0 {
            let wprot = self.read_prph_no_grab(PREG_PRPH_WPROT_22000);
            if wprot & PREG_WFPM_ACCESS != 0 {
                return Err(String::from("persistence bit set and write-protected"));
            }
            self.write_prph_no_grab(HPM_DEBUG, hpm & !PERSISTENCE_BIT);
        }
        Ok(())
    }

    /// iwl_pcie_gen2_force_power_gating (22000 + integrated)
    fn force_power_gating(&self) -> Result<(), String> {
        self.finish_nic_init()?;
        if !self.set_bits_prph(HPM_HIPM_GEN_CFG, HPM_HIPM_GEN_CFG_CR_FORCE_ACTIVE) {
            return Err(String::from("power gating: no NIC access"));
        }
        delay_us(20);
        self.set_bits_prph(HPM_HIPM_GEN_CFG, HPM_HIPM_GEN_CFG_CR_PG_EN | HPM_HIPM_GEN_CFG_CR_SLP_EN);
        delay_us(20);
        self.clear_bits_prph(HPM_HIPM_GEN_CFG, HPM_HIPM_GEN_CFG_CR_FORCE_ACTIVE);
        self.sw_reset(true)
    }

    /// pcie/drv.c get_crf_id: set ENABLE_WFPM under one NIC-access grab and
    /// log what the periphery says afterwards (WFPM_CTRL, crf, cnv ids).
    fn enable_wfpm(&self, step: &str) -> bool {
        if !self.grab() {
            self.release();
            crate::drivers::serial::write_str(&format!("wifi-intel: enable_wfpm [{step}]: no NIC access\n"));
            return false;
        }
        let before = self.read_prph_no_grab(WFPM_CTRL_REG);
        self.write_prph_no_grab(WFPM_CTRL_REG, before | ENABLE_WFPM);
        let after = self.read_prph_no_grab(WFPM_CTRL_REG);
        let crf = self.read_prph_no_grab(SD_REG_VER);
        let cnv = self.read_prph_no_grab(CNVI_AUX_MISC_CHIP);
        self.release();
        crate::drivers::serial::write_str(&format!(
            "wifi-intel: enable_wfpm [{step}]: WFPM_CTRL {before:08x} -> {after:08x}, crf {crf:08x}, cnv {cnv:08x}\n"
        ));
        cnv & 0xFFFF_FFF0 != 0xa5a5_a5a0
    }

    /// iwl_pcie_apm_config: L0s off (we never touch the link's ASPM).
    fn apm_config(&self) {
        self.set_bit(CSR_GIO_REG, CSR_GIO_REG_VAL_L0S_DISABLED);
    }

    /// iwl_pcie_apm_init as it runs for the 22000 family (no PLL cfg, no
    /// APMG), and iwl_pcie_gen2_apm_init (the same register writes).
    fn apm_init(&self) -> Result<(), String> {
        self.set_bit(CSR_GIO_CHICKEN_BITS, CSR_GIO_CHICKEN_BITS_REG_BIT_L1A_NO_L0S_RX);
        self.set_bit(CSR_DBG_HPET_MEM_REG, CSR_DBG_HPET_MEM_REG_VAL);
        self.set_bit(CSR_HW_IF_CONFIG_REG, CSR_HW_IF_CONFIG_REG_BIT_HAP_WAKE_L1A);
        self.apm_config();
        self.finish_nic_init()
    }

    /// iwl_pcie_apm_stop_master + RX DMA stop + SW reset: quiesce the device
    /// (before a retry, after a failure). Leaves our DMA memory allocated.
    fn stop_device(&self) {
        self.disable_interrupts();
        // iwl_pcie_rx_stop (mq, pre-AX210)
        if self.write_prph(RFH_RXF_DMA_CFG, 0) {
            if self.grab() {
                let mut t = 0;
                while t < 1000 && self.read_prph_no_grab(RFH_GEN_STATUS) & RXF_DMA_IDLE == 0 {
                    delay_us(10);
                    t += 10;
                }
            }
            self.release();
        }
        self.set_bit(CSR_RESET, CSR_RESET_REG_FLAG_STOP_MASTER);
        self.poll_bit(CSR_RESET, CSR_RESET_REG_FLAG_MASTER_DISABLED, CSR_RESET_REG_FLAG_MASTER_DISABLED, 100);
        let _ = self.sw_reset(false);
        self.clear_bit(CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_INIT_DONE);
        self.disable_interrupts();
    }

    fn mac_from_csr(&self) -> [u8; 6] {
        // iwl-nvm-parse.c iwl_set_hw_address_from_csr + iwl_flip_hw_address
        let flip = |a0: u32, a1: u32| {
            let h0 = a0.to_le_bytes();
            let h1 = a1.to_le_bytes();
            [h0[3], h0[2], h0[1], h0[0], h1[1], h1[0]]
        };
        let m = flip(self.r32(CSR_ADDR_BASE + 0x08), self.r32(CSR_ADDR_BASE + 0x0c));
        if valid_ether(&m) {
            return m;
        }
        flip(self.r32(CSR_ADDR_BASE), self.r32(CSR_ADDR_BASE + 0x04))
    }

    fn regs_line(&self) -> String {
        format!(
            "regs GP_CNTRL={:08x} INT={:08x} FH={:08x} RESET={:08x} IFCFG={:08x}",
            self.r32(CSR_GP_CNTRL),
            self.r32(CSR_INT),
            self.r32(CSR_FH_INT_STATUS),
            self.r32(CSR_RESET),
            self.r32(CSR_HW_IF_CONFIG_REG)
        )
    }

    /// One line of periphery reads for the boot log (`report` carries it): on
    /// a real Z590 every PRPH read came back 0xa5a5a5a2 ("not reachable"), so
    /// the start sequence logs where the periphery stops answering.
    fn trace(&self, step: &str) {
        let p = |r| self.read_prph(r).map(|v| format!("{v:08x}")).unwrap_or_else(|| String::from("nograb"));
        let line = format!(
            "wifi-intel trace [{step}] GP_CNTRL={:08x} IFCFG={:08x} RESET={:08x} pci_cmd={:08x} | hpm_dbg={} hipm={} mbox={} aux_chip={} chick={} sb1={}\n",
            self.r32(CSR_GP_CNTRL),
            self.r32(CSR_HW_IF_CONFIG_REG),
            self.r32(CSR_RESET),
            self.loc.read32(0x04),
            p(HPM_DEBUG),
            p(HPM_HIPM_GEN_CFG),
            p(CNVI_MBOX_C),
            p(CNVI_AUX_MISC_CHIP),
            p(UREG_CHICK),
            p(UMAG_SB_CPU_1_STATUS),
        );
        crate::drivers::serial::write_str(&line);
    }

    /// The periphery answers (a read is not the 0xa5a5a5aX "no access" fill).
    fn prph_alive(&self) -> bool {
        matches!(self.read_prph(CNVI_AUX_MISC_CHIP), Some(v) if v & 0xFFFF_FFF0 != 0xa5a5_a5a0 && v != 0xFFFF_FFFF)
    }

    /// The SecBoot / PC registers mvm/fw.c prints when ALIVE does not come.
    fn boot_diag(&self) -> String {
        let p = |r| self.read_prph(r).map(|v| format!("{v:x}")).unwrap_or_else(|| String::from("?"));
        format!(
            "sb1={} sb2={} umacPC={} lmacPC={} pd={}",
            p(UMAG_SB_CPU_1_STATUS),
            p(UMAG_SB_CPU_2_STATUS),
            p(UREG_UMAC_CURRENT_PC),
            p(UREG_LMAC1_CURRENT_PC),
            p(WFPM_LMAC1_PD_NOTIFICATION)
        )
    }

    /// When the firmware asserted (CSR_INT SW_ERR): the error ids from the
    /// LMAC / UMAC error tables the ALIVE notification pointed at
    /// (fw/dump.c: valid, error_id are the first two dwords of both).
    fn fw_error_line(&self) -> String {
        let int = self.r32(CSR_INT);
        let mut s = format!("CSR_INT {int:08x}");
        for (name, at) in [("lmac", self.lmac_err), ("umac", self.umac_err)] {
            if at == 0 {
                s.push_str(&format!(" {name} err table unknown"));
                continue;
            }
            match (self.read_mem32(at), self.read_mem32(at + 4), self.read_mem32(at + 8)) {
                (Some(v), Some(id), Some(pc)) => s.push_str(&format!(" {name} err valid={v:x} id=0x{id:x} pc=0x{pc:x}")),
                _ => s.push_str(&format!(" {name} err table @0x{at:x} unreadable")),
            }
        }
        // the boot log keeps it for `report` (the stage text gets truncated on screen)
        crate::drivers::serial::write_str(&format!("wifi-intel: firmware error: {s}\n"));
        s
    }

    fn fw_errored(&self) -> bool {
        self.r32(CSR_INT) & (CSR_INT_BIT_SW_ERR | CSR_INT_BIT_HW_ERR) != 0
    }
}

fn valid_ether(m: &[u8; 6]) -> bool {
    m[0] & 1 == 0 && m.iter().any(|&b| b != 0) && m.iter().any(|&b| b != 0xFF)
}

// ======================================================================
// Stage 1: detect

fn pci_cap(loc: &Location, id: u8) -> Option<u8> {
    if (loc.read32(0x04) >> 16) & 0x10 == 0 {
        return None;
    }
    let mut p = (loc.read32(0x34) & 0xFC) as u8;
    let mut guard = 0;
    while p != 0 && guard < 48 {
        let h = loc.read32(p);
        if h & 0xFF == id as u32 {
            return Some(p);
        }
        p = ((h >> 8) & 0xFC) as u8;
        guard += 1;
    }
    None
}

fn find() -> Option<(Location, u16)> {
    for loc in pci::find_all(INTEL, 0x02) {
        let did = (loc.read32(0) >> 16) as u16;
        if QU_IDS.contains(&did) {
            return Some((loc, did));
        }
    }
    None
}

/// The memory BAR0 (64-bit on this device): (base, size). Sized with memory
/// decoding off, then restored (pcim_iomap_regions does the same dance).
fn bar0(loc: &Location) -> (u64, u64) {
    let w = |off: u8, v: u32| pci::write32(loc.bus, loc.device, loc.function, off, v);
    let cmd = loc.read32(0x04) & 0xFFFF;
    let lo = loc.read32(0x10);
    if lo & 1 != 0 {
        return (0, 0);
    }
    let is64 = (lo >> 1) & 3 == 2;
    let hi = if is64 { loc.read32(0x14) } else { 0 };
    let base = ((hi as u64) << 32) | (lo & 0xFFFF_FFF0) as u64;
    let size = unsafe {
        let flags = crate::arch::cpu::read_flags();
        crate::arch::cpu::cli();
        w(0x04, cmd & !0x0002);
        w(0x10, 0xFFFF_FFFF);
        let mlo = loc.read32(0x10) & 0xFFFF_FFF0;
        w(0x10, lo);
        let mut mask = mlo as u64 | 0xFFFF_FFFF_0000_0000;
        if is64 {
            w(0x14, 0xFFFF_FFFF);
            let mhi = loc.read32(0x14);
            w(0x14, hi);
            mask = ((mhi as u64) << 32) | mlo as u64;
        }
        w(0x04, cmd);
        crate::arch::cpu::write_flags(flags);
        if mlo == 0 && !is64 { 0 } else { (!mask).wrapping_add(1) }
    };
    (base, size)
}

/// Reset the PCI function (bring-up variant 2): FLR when the device offers it
/// (PCIe DevCap bit 28, or the Advanced Features capability), else a PCI-PM
/// D3hot -> D0 cycle. The config header is saved and written back after.
fn pci_function_reset(loc: &Location) -> String {
    let w = |off: u8, v: u32| pci::write32(loc.bus, loc.device, loc.function, off, v);
    let saved: Vec<u32> = (0..16u8).map(|i| loc.read32(i * 4)).collect();
    let mut how = String::from("none");
    if let Some(pcie) = pci_cap(loc, 0x10) {
        if loc.read32(pcie + 4) & (1 << 28) != 0 {
            let ctl = loc.read32(pcie + 8);
            w(pcie + 8, (ctl & 0xFFFF) | (1 << 15)); // DevCtl: initiate FLR
            delay_us(100_000);
            how = String::from("FLR (pcie)");
        }
    }
    if how == "none" {
        if let Some(af) = pci_cap(loc, 0x13) {
            if (loc.read32(af) >> 24) & 0x02 != 0 {
                w(af + 4, 1); // AF control: initiate FLR (byte 4 bit 0)
                delay_us(100_000);
                how = String::from("FLR (af)");
            }
        }
    }
    if how == "none" {
        if let Some(pm) = pci_cap(loc, 0x01) {
            let pmcsr = loc.read32(pm + 4);
            w(pm + 4, (pmcsr & !0x8003) | 3); // D3hot
            delay_us(20_000);
            w(pm + 4, pmcsr & !0x8003); // D0
            delay_us(20_000);
            how = format!("D3hot->D0 (no_soft_reset={})", (pmcsr >> 3) & 1);
        }
    }
    // restore BARs, cache line / latency, interrupt line; command last
    for i in [3u8, 4, 5, 6, 7, 8, 9, 15] {
        w(i * 4, saved[i as usize]);
    }
    w(0x04, (saved[1] & 0xFFFF) | 0x0002 | 0x0004 | 0x0400);
    let r40 = loc.read32(0x40);
    w(0x40, r40 & !0x0000_FF00);
    format!("{how}, hw_rev after 0x{:x}", loc.read32(0x08))
}

fn stage_detect() -> Result<(), String> {
    let Some((loc, device_id)) = find() else {
        unsafe { ABSENT = true };
        return Err(String::from("no Intel Wi-Fi (CNVi AX201) on the PCI bus"));
    };
    let w = |off: u8, v: u32| pci::write32(loc.bus, loc.device, loc.function, off, v);
    let subsys = loc.read32(0x2C);
    // power state D0 (pcim_enable_device)
    if let Some(pm) = pci_cap(&loc, 0x01) {
        let pmcsr = loc.read32(pm + 4);
        if pmcsr & 3 != 0 {
            w(pm + 4, pmcsr & !0x8003);
            delay_us(10_000);
        }
    }
    // the driver polls: MSI and MSI-X off (legacy CSR_INT mode), INTx masked
    if let Some(msi) = pci_cap(&loc, 0x05) {
        let v = loc.read32(msi);
        if v & (1 << 16) != 0 {
            w(msi, v & !(1 << 16));
        }
    }
    if let Some(msix) = pci_cap(&loc, 0x11) {
        let v = loc.read32(msix);
        if v & (1 << 31) != 0 {
            w(msix, v & !(1 << 31));
        }
    }
    let (b0, b0_len) = bar0(&loc);
    if b0 == 0 || b0_len < 0x2000 {
        return Err(format!("BAR0 unusable (0x{b0:x}, {b0_len} bytes)"));
    }
    let Some(base) = crate::arch::paging::map_mmio(b0, b0_len) else {
        return Err(format!("BAR0 0x{b0:x} could not be mapped"));
    };
    // memory decode + bus master, INTx disable (bit 10); status half written as 0
    let cmd = loc.read32(0x04) & 0xFFFF;
    w(0x04, cmd | 0x0002 | 0x0004 | 0x0400);
    // PCI_CFG_RETRY_TIMEOUT (0x41) = 0 (pcie/trans.c iwl_trans_pcie_alloc)
    let r40 = loc.read32(0x40);
    w(0x40, r40 & !0x0000_FF00);
    let mut d = Dev {
        loc,
        device_id,
        subsys,
        bar0_phys: b0,
        bar0_len: b0_len,
        base,
        hw_rev: 0,
        rf_id: 0,
        cnvi_mbox: 0,
        phy_config: 0,
        fw_version: String::new(),
        fw_name: "",
        mac: [0; 6],
        dma: None,
        rx_read: 0,
        rx_write: 0,
        rx_write_actual: 0,
        cmd_wp: 0,
        inbox: Vec::new(),
        log: Vec::new(),
        alive_ms: 0,
        lmac_err: 0,
        umac_err: 0,
        capa: [0; 4],
        api: [0; 3],
        cmd_ver: Vec::new(),
        nvm_sku: 0,
        nvm_tx: 0,
        nvm_rx: 0,
        nvm_lar: false,
        nvm_ch: Vec::new(),
        configured: false,
        channels: Vec::new(),
        mcc: 0,
        bss: Vec::new(),
        scanning: false,
        scan_ms: 0,
        mpdus: 0,
        txq: [TxQ::empty(), TxQ::empty()],
        link: None,
        wedged: false,
        last_up_try_ms: 0,
        tx_done: Vec::new(),
    };
    d.disable_interrupts();
    d.hw_rev = d.r32(CSR_HW_REV);
    if d.hw_rev == 0xFFFF_FFFF {
        return Err(String::from("CSR_HW_REV reads 0xffffffff (device not decoding / in D3)"));
    }
    d.fw_name = match d.hw_rev {
        CSR_HW_REV_TYPE_QUZ => FW_QUZ_HR_NAME,
        CSR_HW_REV_TYPE_QU_C0 => "iwlwifi-Qu-c0-hr-b0-72.ucode",
        _ => "iwlwifi-Qu-b0-hr-b0-72.ucode",
    };
    unsafe { *(&raw mut DEV) = Some(d) };
    let d = dev().unwrap();
    let kind = match d.hw_rev {
        CSR_HW_REV_TYPE_QUZ => "QuZ",
        CSR_HW_REV_TYPE_QU_C0 => "Qu-c0",
        CSR_HW_REV_TYPE_QU_B0 => "Qu-b0",
        _ => "Qu?",
    };
    set_stage(
        0,
        St::Ok,
        format!("rev 0x{:x} {kind}, bar0 0x{:x} {}K, fw {}", d.hw_rev, d.bar0_phys, d.bar0_len >> 10, d.fw_name),
    );
    Ok(())
}

// ======================================================================
// Stage 2: wake (iwl_trans_pcie_start_hw + the probe-time NIC access check)

fn stage_wake(d: &mut Dev) -> Result<String, String> {
    let variant = unsafe { WAKE_VARIANT };
    let mut reset_note = String::new();
    if variant == 2 {
        reset_note = pci_function_reset(&d.loc);
        crate::drivers::serial::write_str(&format!("wifi-intel: pci function reset: {reset_note}\n"));
    }
    d.disable_interrupts();
    d.trace("start");
    d.prepare_card_hw()?;
    // pcie/drv.c iwl_pci_probe: finish_nic_init, grab, get_crf_id (ENABLE_WFPM)
    d.finish_nic_init()?;
    d.enable_wfpm("probe");
    d.trace("prepare_card_hw");
    d.clear_persistence_bit()?;
    d.sw_reset(true)?;
    d.trace("sw_reset");
    if variant != 1 {
        d.force_power_gating()?;
        d.trace("force_power_gating");
    }
    d.apm_init()?;
    // the SW resets above may clear it again: set it once more (a no-op if kept)
    d.enable_wfpm("apm_init");
    d.trace("apm_init");
    // iwl_pcie_init_msix -> iwl_pcie_conf_msix_hw without MSI-X: tell the
    // device the driver takes its causes through CSR_INT (MSI/legacy mode).
    // Linux normally runs this device in MSI-X mode, where every cause goes
    // to the CSR_MSIX_* registers instead; if the mode bit is left as the
    // previous OS (Windows: MSI-X) set it, CSR_INT never shows ALIVE.
    if !d.write_prph(UREG_CHICK, UREG_CHICK_MSI_ENABLE) {
        return Err(format!("UREG_CHICK write: no NIC access (GP_CNTRL 0x{:08x})", d.r32(CSR_GP_CNTRL)));
    }
    d.enable_rfkill_int();
    if !d.grab() {
        d.release();
        return Err(format!("MAC access request never granted (GP_CNTRL 0x{:08x})", d.r32(CSR_GP_CNTRL)));
    }
    d.release();
    d.rf_id = d.r32(CSR_HW_RF_ID);
    d.cnvi_mbox = d.read_prph(CNVI_MBOX_C).unwrap_or(0);
    d.mac = d.mac_from_csr();
    let rf_type = (d.rf_id >> 12) & 0xFFF;
    let rf = match rf_type {
        RF_TYPE_HR => "HR",
        RF_TYPE_HR1 => "HR1",
        0x105 => "JF",
        0x10D => "GF",
        _ => "?",
    };
    // pcie/trans-gen2.c iwl_pcie_get_rf_name
    let step = match d.cnvi_mbox {
        0x20000 => "B3",
        0x120000 => "B5",
        _ => "",
    };
    let rfk = if d.hw_rfkill() { "RFKILL ON" } else { "rfkill off" };
    d.trace("wake done");
    let prph = if d.prph_alive() { "prph ok" } else { "PRPH UNREACHABLE" };
    let s = format!(
        "rf 0x{:x} {rf} {step} (mbox {:x}), nic access ok, {prph}, {rfk}, csr mac {}{}",
        d.rf_id,
        d.cnvi_mbox,
        mac_str(&d.mac),
        match variant {
            1 => String::from(", no power gating"),
            2 => format!(", after pci reset: {reset_note}"),
            _ => String::new(),
        }
    );
    if rf_type != RF_TYPE_HR && rf_type != RF_TYPE_HR1 {
        return Err(format!("{s} -- RF is not HR (AX201): no firmware for it here"));
    }
    Ok(s)
}

// ======================================================================
// Stage 3: firmware

struct FwImage<'a> {
    version: String,
    human: String,
    phy_sku: u32,
    lmac: Vec<&'a [u8]>,
    umac: Vec<&'a [u8]>,
    paging: Vec<&'a [u8]>,
    capa: [u32; 4],
    api: [u32; 3],
    cmd_ver: Vec<(u8, u8, u8)>,
}

/// iwl-drv.c iwl_parse_tlv_firmware, the parts the context-info boot uses.
fn parse_fw(fw: &[u8]) -> Result<FwImage<'_>, String> {
    if fw.len() < 88 || rd32(fw, 0) != 0 || rd32(fw, 4) != IWL_TLV_UCODE_MAGIC {
        return Err(String::from("not a TLV .ucode file"));
    }
    let human_raw = &fw[8..72];
    let hl = human_raw.iter().position(|&b| b == 0).unwrap_or(64);
    let human = String::from_utf8_lossy(&human_raw[..hl]).into_owned();
    let mut img = FwImage {
        version: String::new(),
        human,
        phy_sku: 0,
        lmac: Vec::new(),
        umac: Vec::new(),
        paging: Vec::new(),
        capa: [0; 4],
        api: [0; 3],
        cmd_ver: Vec::new(),
    };
    let mut secs: Vec<(u32, &[u8])> = Vec::new();
    let mut off = 88;
    while off + 8 <= fw.len() {
        let t = rd32(fw, off);
        let l = rd32(fw, off + 4) as usize;
        let start = off + 8;
        if start + l > fw.len() {
            return Err(format!("TLV {t} runs past the end"));
        }
        let data = &fw[start..start + l];
        match t {
            TLV_SEC_RT | TLV_SECURE_SEC_RT => {
                if l < 4 {
                    return Err(String::from("short section TLV"));
                }
                secs.push((rd32(data, 0), &data[4..]));
            },
            TLV_PHY_SKU if l == 4 => img.phy_sku = rd32(data, 0),
            TLV_FW_VERSION if l == 12 => {
                let (maj, min, lc) = (rd32(data, 0), rd32(data, 4), rd32(data, 8) as u8);
                img.version = if maj >= 35 { format!("{maj}.{min:08x}.{lc}") } else { format!("{maj}.{min}.{lc}") };
            },
            // iwl-drv.c iwl_set_ucode_api_flags / iwl_set_ucode_capabilities
            TLV_API_CHANGES_SET if l >= 8 => {
                let i = rd32(data, 0) as usize;
                if i < img.api.len() {
                    img.api[i] = rd32(data, 4);
                }
            },
            TLV_ENABLED_CAPABILITIES if l >= 8 => {
                let i = rd32(data, 0) as usize;
                if i < img.capa.len() {
                    img.capa[i] = rd32(data, 4);
                }
            },
            // fw/img.c iwl_fw_lookup_cmd_ver: {cmd, group, cmd_ver, notif_ver}
            TLV_CMD_VERSIONS => {
                for e in data.chunks_exact(4) {
                    img.cmd_ver.push((e[1], e[0], e[2]));
                }
            },
            TLV_NUM_OF_CPU | TLV_PAGING => {},
            _ => {},
        }
        off = start + ((l + 3) & !3);
    }
    // pcie/ctxt-info.c iwl_pcie_init_fw_sec + pcie/internal.h iwl_pcie_get_num_sections
    let is_sep = |o: u32| o == CPU1_CPU2_SEPARATOR_SECTION || o == PAGING_SEPARATOR_SECTION;
    let count = |from: usize| secs.iter().skip(from).take_while(|s| !is_sep(s.0)).count();
    let n_lmac = count(0);
    let n_umac = count(n_lmac + 1);
    let n_paging = count(n_lmac + n_umac + 2);
    for (i, s) in secs.iter().enumerate() {
        let d = s.1;
        if i < n_lmac {
            img.lmac.push(d);
        } else if i > n_lmac && i <= n_lmac + n_umac {
            img.umac.push(d);
        } else if i > n_lmac + n_umac + 1 && i <= n_lmac + n_umac + 1 + n_paging {
            img.paging.push(d);
        }
    }
    if img.lmac.is_empty() || img.umac.is_empty() {
        return Err(format!("no LMAC/UMAC sections ({} sections)", secs.len()));
    }
    if img.lmac.len() > IWL_MAX_DRAM_ENTRY || img.umac.len() > IWL_MAX_DRAM_ENTRY || img.paging.len() > IWL_MAX_DRAM_ENTRY {
        return Err(String::from("too many firmware sections"));
    }
    if img.lmac.iter().chain(&img.umac).chain(&img.paging).any(|s| s.len() > MAX_SECTION || s.is_empty()) {
        return Err(String::from("a firmware section is over 32 KiB"));
    }
    Ok(img)
}

fn firmware_bytes(name: &str) -> Option<Vec<u8>> {
    if name == FW_QUZ_HR_NAME {
        return None; // the embedded one (borrowed, not copied)
    }
    if crate::vfs::ready() {
        for p in [format!("firmware/{name}"), format!("media/usb0/firmware/{name}"), format!("media/disk1/firmware/{name}")] {
            if let Some(b) = crate::vfs::read(&p) {
                return Some(b);
            }
        }
    }
    None
}

fn alloc_dma(d: &mut Dev, n_secs: usize) -> Result<(), String> {
    let oom = || String::from("out of DMA memory (below 4 GiB)");
    if d.dma.is_none() {
        let ctxt = dma_alloc(CTXT_SIZE).ok_or_else(oom)?;
        let rx_free = dma_alloc(RX_RING * 8).ok_or_else(oom)?;
        let rx_used = dma_alloc(RX_RING * 4).ok_or_else(oom)?;
        let rx_stts = dma_alloc(16).ok_or_else(oom)?;
        let mut rx_bufs = Vec::with_capacity(RX_BUFS);
        for _ in 0..RX_BUFS {
            rx_bufs.push(dma_alloc(RB_SIZE).ok_or_else(oom)?);
        }
        let cmd_tfds = dma_alloc(CMD_SLOTS * TFD_SIZE).ok_or_else(oom)?;
        let cmd_bufs = dma_alloc(CMD_SLOTS * CMD_BUF).ok_or_else(oom)?;
        d.dma = Some(Dma { ctxt, rx_free, rx_used, rx_stts, rx_bufs, cmd_tfds, cmd_bufs, secs: Vec::new() });
    }
    let dma = d.dma.as_mut().unwrap();
    while dma.secs.len() < n_secs {
        dma.secs.push(dma_alloc(MAX_SECTION).ok_or_else(oom)?);
    }
    Ok(())
}

impl Dev {
    // ---- RX queue 0 (pcie/rx.c, mq / pre-AX210 formats) ----

    fn rx_reset(&mut self) {
        let dma = self.dma.as_ref().unwrap();
        unsafe {
            core::ptr::write_bytes(dma.rx_free as *mut u8, 0, RX_RING * 8);
            core::ptr::write_bytes(dma.rx_used as *mut u8, 0, RX_RING * 4);
            core::ptr::write_bytes(dma.rx_stts as *mut u8, 0, 16);
        }
        self.rx_read = 0;
        self.rx_write = 0;
        self.rx_write_actual = 0;
        self.inbox.clear();
    }

    /// iwl_pcie_restock_bd (22000: a __le64 "page_dma | vid" per free RBD)
    fn rx_restock(&mut self, vid: usize) {
        let dma = self.dma.as_ref().unwrap();
        let addr = dma.rx_bufs[vid - 1];
        unsafe { dw64(dma.rx_free + (self.rx_write * 8) as u64, addr | vid as u64) };
        self.rx_write = (self.rx_write + 1) & (RX_RING - 1);
    }

    /// iwl_pcie_rxq_inc_wr_ptr: the write index goes to the RFH in multiples of 8.
    fn rx_kick(&mut self) {
        let wa = self.rx_write & !7;
        if wa != self.rx_write_actual {
            fence(Ordering::SeqCst);
            self.rx_write_actual = wa;
            self.w32(RFH_Q0_FRBDCB_WIDX_TRG, wa as u32);
        }
    }

    /// iwl_pcie_rx_handle: walk closed RBs, split each into packets
    /// (iwl_pcie_rx_handle_rb), hand the buffer back.
    fn rx_poll(&mut self) -> usize {
        let Some(dma) = self.dma.as_ref() else { return 0 };
        let stts = dma.rx_stts;
        let used = dma.rx_used;
        let closed = (unsafe { read_volatile(stts as *const u16) } & 0x0FFF) as usize & (RX_RING - 1);
        fence(Ordering::SeqCst);
        let mut n = 0;
        while self.rx_read != closed {
            let vid = (unsafe { read_volatile((used + (self.rx_read * 4) as u64) as *const u32) } & 0x0FFF) as usize;
            if vid >= 1 && vid <= RX_BUFS {
                let buf = self.dma.as_ref().unwrap().rx_bufs[vid - 1];
                self.parse_rb(buf);
                unsafe { dw32(buf, FH_RSCSR_FRAME_INVALID) };
                self.rx_restock(vid);
                n += 1;
            } else {
                self.note(0xFF, 0xFF);
            }
            self.rx_read = (self.rx_read + 1) & (RX_RING - 1);
        }
        self.rx_kick();
        n
    }

    fn parse_rb(&mut self, buf: u64) {
        let mut off = 0usize;
        while off + 4 + 4 < RB_SIZE {
            let p = buf + off as u64;
            let lnf = unsafe { read_volatile(p as *const u32) };
            if lnf == FH_RSCSR_FRAME_INVALID {
                break;
            }
            let flen = (lnf & FH_RSCSR_FRAME_SIZE_MSK) as usize; // header + payload
            let len = flen + 4; // + the status word
            if len < 8 || off + len > RB_SIZE {
                break;
            }
            let hdr = unsafe { read_volatile((p + 4) as *const u32) };
            let cmd = hdr as u8;
            let group = (hdr >> 8) as u8;
            let seq = (hdr >> 16) as u16;
            let plen = flen - 4;
            let data = unsafe { core::slice::from_raw_parts((p + 8) as *const u8, plen) };
            let legacy = group == LEGACY_GROUP || group == LONG_GROUP;
            if legacy && cmd == REPLY_RX_MPDU_CMD {
                // received 802.11 frames: handled right here, never queued
                self.on_mpdu(data, lnf);
            } else if legacy && cmd == TX_CMD && seq & SEQ_RX_FRAME == 0 && (seq >> 8) & 0x1f != 0 {
                self.on_tx_resp(seq, data);
            } else {
                self.note(group, cmd);
                if legacy && cmd == MISSED_BEACONS_NOTIFICATION {
                    self.on_missed_beacons(data);
                }
                if self.inbox.len() >= 64 {
                    self.inbox.remove(0);
                }
                self.inbox.push(Pkt { group, cmd, seq, data: data.to_vec() });
            }
            off += len.div_ceil(FH_RSCSR_FRAME_ALIGN) * FH_RSCSR_FRAME_ALIGN;
        }
    }

    fn note(&mut self, group: u8, cmd: u8) {
        if self.log.len() >= 10 {
            self.log.remove(0);
        }
        self.log.push((group, cmd));
    }

    fn take(&mut self, f: impl Fn(&Pkt) -> bool) -> Option<Pkt> {
        let i = self.inbox.iter().position(|p| f(p))?;
        Some(self.inbox.remove(i))
    }

    /// Poll RX until a packet matches, the firmware asserts, or the time is up.
    fn wait_pkt(&mut self, timeout_us: u64, f: impl Fn(&Pkt) -> bool) -> Result<Pkt, String> {
        let end = now_us() + timeout_us;
        loop {
            self.rx_poll();
            if let Some(p) = self.take(&f) {
                return Ok(p);
            }
            if self.fw_errored() {
                return Err(format!("firmware error: {}", self.fw_error_line()));
            }
            if now_us() >= end {
                return Err(String::from("timeout"));
            }
            delay_us(50);
        }
    }

    // ---- command queue 0 (pcie/tx-gen2.c iwl_pcie_gen2_enqueue_hcmd) ----

    /// Send a host command (wide header) and wait for its response.
    fn send_cmd(&mut self, group: u8, cmd: u8, payload: &[u8]) -> Result<Vec<u8>, String> {
        self.send_cmd_v(group, cmd, 0, payload)
    }

    /// A command whose response is `struct iwl_cmd_response { __le32 status }`.
    fn send_cmd_status(&mut self, group: u8, cmd: u8, payload: &[u8]) -> Result<u32, String> {
        let r = self.send_cmd(group, cmd, payload)?;
        Ok(rd32(&r, 0))
    }

    /// `ver` goes in the wide header's version byte (iwl_cmd_id(.., ver)).
    fn send_cmd_v(&mut self, group: u8, cmd: u8, ver: u8, payload: &[u8]) -> Result<Vec<u8>, String> {
        let r = self.send_cmd_inner(group, cmd, ver, payload);
        if r.is_err() {
            self.wedged = true;
        }
        r
    }

    fn send_cmd_inner(&mut self, group: u8, cmd: u8, ver: u8, payload: &[u8]) -> Result<Vec<u8>, String> {
        let total = 8 + payload.len();
        if total > CMD_BUF {
            return Err(String::from("command too large"));
        }
        let dma = self.dma.as_ref().ok_or_else(|| String::from("no queues"))?;
        let wp = self.cmd_wp;
        let idx = (wp as usize) & (CMD_SLOTS - 1);
        let buf = dma.cmd_bufs + (idx * CMD_BUF) as u64;
        let tfd = dma.cmd_tfds + (idx * TFD_SIZE) as u64;
        let seq: u16 = (wp & 0xff) as u16; // QUEUE_TO_SEQ(0) | INDEX_TO_SEQ(wp)
        unsafe {
            // struct iwl_cmd_header_wide (fw/api/cmdhdr.h)
            let h = buf as *mut u8;
            write_volatile(h, cmd);
            write_volatile(h.add(1), group);
            dw16(buf + 2, seq);
            dw16(buf + 4, payload.len() as u16);
            write_volatile(h.add(6), 0); // reserved
            write_volatile(h.add(7), ver); // version
            core::ptr::copy_nonoverlapping(payload.as_ptr(), h.add(8), payload.len());
            // struct iwl_tfh_tfd: __le16 num_tbs; struct iwl_tfh_tb { __le16 tb_len; __le64 addr } tbs[25]
            core::ptr::write_bytes(tfd as *mut u8, 0, TFD_SIZE);
            let tb0 = total.min(FIRST_TB);
            dw16(tfd + 2, tb0 as u16);
            core::ptr::write_unaligned((tfd + 4) as *mut u64, buf);
            let mut n = 1u16;
            if total > tb0 {
                dw16(tfd + 12, (total - tb0) as u16);
                core::ptr::write_unaligned((tfd + 14) as *mut u64, buf + tb0 as u64);
                n = 2;
            }
            dw16(tfd, n);
        }
        fence(Ordering::SeqCst);
        self.cmd_wp = (wp + 1) & (MAX_TFD_QUEUE - 1);
        // queue/tx.c iwl_txq_inc_wr_ptr
        self.w32(HBUS_TARG_WRPTR, self.cmd_wp);
        let p = self
            .wait_pkt(CMD_TIMEOUT, |p| p.seq & SEQ_RX_FRAME == 0 && p.seq == seq)
            .map_err(|e| format!("cmd {group:02x}.{cmd:02x}: {e}"))?;
        Ok(p.data)
    }

    // ---- context info + firmware start (pcie/ctxt-info.c, trans-gen2.c) ----

    fn build_ctxt(&mut self, img: &FwImage) {
        let dma = self.dma.as_ref().unwrap();
        let c = dma.ctxt;
        unsafe {
            core::ptr::write_bytes(c as *mut u8, 0, 4096);
            // version: mac_id = (u16)CSR_HW_REV, version 0, size in DWs
            dw16(c, self.r32(CSR_HW_REV) as u16);
            dw16(c + 2, 0);
            dw16(c + 4, (CTXT_SIZE / 4) as u16);
            let flags = IWL_CTXT_INFO_TFD_FORMAT_LONG
                | ((RX_RING_LOG2 << IWL_CTXT_INFO_RB_CB_SIZE_POS) & 0x00f0)
                | ((IWL_CTXT_INFO_RB_SIZE_4K << IWL_CTXT_INFO_RB_SIZE_POS) & 0x1e00);
            dw32(c + 8, flags);
            dw64(c + CTXT_OFF_FREE_RBD as u64, dma.rx_free);
            dw64(c + CTXT_OFF_USED_RBD as u64, dma.rx_used);
            dw64(c + CTXT_OFF_STATUS_WR as u64, dma.rx_stts);
            dw64(c + CTXT_OFF_CMDQ_ADDR as u64, dma.cmd_tfds);
            // TFD_QUEUE_CB_SIZE(IWL_CMD_QUEUE_SIZE) = ilog2(32) - 3
            write_volatile((c + CTXT_OFF_CMDQ_SIZE as u64) as *mut u8, (CMD_SLOTS.trailing_zeros() - 3) as u8);
            let mut k = 0;
            for (i, s) in img.lmac.iter().enumerate() {
                let a = dma.secs[k];
                core::ptr::copy_nonoverlapping(s.as_ptr(), a as *mut u8, s.len());
                dw64(c + (CTXT_OFF_LMAC_IMG + i * 8) as u64, a);
                k += 1;
            }
            for (i, s) in img.umac.iter().enumerate() {
                let a = dma.secs[k];
                core::ptr::copy_nonoverlapping(s.as_ptr(), a as *mut u8, s.len());
                dw64(c + (CTXT_OFF_UMAC_IMG + i * 8) as u64, a);
                k += 1;
            }
            for (i, s) in img.paging.iter().enumerate() {
                let a = dma.secs[k];
                core::ptr::copy_nonoverlapping(s.as_ptr(), a as *mut u8, s.len());
                dw64(c + (CTXT_OFF_VIRT_IMG + i * 8) as u64, a);
                k += 1;
            }
        }
        fence(Ordering::SeqCst);
    }

    /// iwl_mvm_nic_config (the op-mode hook iwl_pcie_gen2_nic_init calls)
    fn nic_config(&self) {
        let pc = self.phy_config;
        let ty = pc & FW_PHY_CFG_RADIO_TYPE;
        let step = (pc >> FW_PHY_CFG_RADIO_STEP_POS) & 3;
        let dash = (pc >> FW_PHY_CFG_RADIO_DASH_POS) & 3;
        let val = (self.hw_rev & CSR_HW_IF_CONFIG_REG_MSK_MAC_STEP_DASH)
            | (ty << CSR_HW_IF_CONFIG_REG_POS_PHY_TYPE)
            | (step << CSR_HW_IF_CONFIG_REG_POS_PHY_STEP)
            | (dash << CSR_HW_IF_CONFIG_REG_POS_PHY_DASH);
        let mask = CSR_HW_IF_CONFIG_REG_MSK_MAC_STEP_DASH
            | CSR_HW_IF_CONFIG_REG_MSK_PHY_TYPE
            | CSR_HW_IF_CONFIG_REG_MSK_PHY_STEP
            | CSR_HW_IF_CONFIG_REG_MSK_PHY_DASH
            | CSR_HW_IF_CONFIG_REG_BIT_RADIO_SI
            | CSR_HW_IF_CONFIG_REG_BIT_MAC_SI
            | CSR_HW_IF_CONFIG_REG_D3_DEBUG;
        let v = self.r32(CSR_HW_IF_CONFIG_REG);
        self.w32(CSR_HW_IF_CONFIG_REG, (v & !mask) | val);
    }
}

fn stage_firmware(d: &mut Dev) -> Result<String, String> {
    let owned = firmware_bytes(d.fw_name);
    let bytes: &[u8] = match &owned {
        Some(b) => b,
        None if d.fw_name == FW_QUZ_HR_NAME => FW_QUZ_HR,
        None => return Err(format!("{} is not embedded; put it in firmware/ on storage", d.fw_name)),
    };
    let img = parse_fw(bytes)?;
    d.phy_config = img.phy_sku;
    d.capa = img.capa;
    d.api = img.api;
    d.cmd_ver = img.cmd_ver.clone();
    d.fw_version = if img.version.is_empty() { img.human.clone() } else { img.version.clone() };
    let n = img.lmac.len() + img.umac.len() + img.paging.len();
    alloc_dma(d, n)?;

    // iwl_trans_pcie_gen2_start_fw
    d.prepare_card_hw()?;
    d.enable_rfkill_int();
    d.w32(CSR_INT, 0xFFFF_FFFF);
    d.disable_interrupts();
    let rfkill = d.hw_rfkill(); // unified image: run_in_rfkill
    d.w32(CSR_UCODE_DRV_GP1_CLR, CSR_UCODE_SW_BIT_RFKILL);
    d.w32(CSR_UCODE_DRV_GP1_CLR, CSR_UCODE_DRV_GP1_BIT_CMD_BLOCKED);
    d.w32(CSR_INT, 0xFFFF_FFFF);
    // iwl_pcie_gen2_nic_init
    d.apm_init()?;
    d.enable_wfpm("fw: nic_init");
    d.trace("fw: nic_init");
    d.nic_config();
    d.w8(CSR_INT_COALESCING, IWL_HOST_INT_TIMEOUT_DEF); // iwl_pcie_gen2_rx_init
    d.rx_reset();
    {
        let dma = d.dma.as_ref().unwrap();
        unsafe { core::ptr::write_bytes(dma.cmd_tfds as *mut u8, 0, CMD_SLOTS * TFD_SIZE) };
        for &b in &dma.rx_bufs {
            unsafe { dw32(b, FH_RSCSR_FRAME_INVALID) };
        }
    }
    d.cmd_wp = 0;
    d.set_bit(CSR_MAC_SHADOW_REG_CTRL, 0x800FFFFF);
    // iwl_pcie_ctxt_info_init
    d.build_ctxt(&img);
    d.w32(CSR_INT_MASK, CSR_INT_BIT_ALIVE | CSR_INT_BIT_FH_RX); // iwl_enable_fw_load_int_ctx_info
    let c = d.dma.as_ref().unwrap().ctxt;
    d.w32(CSR_CTXT_INFO_BA, c as u32);
    d.w32(CSR_CTXT_INFO_BA + 4, (c >> 32) as u32);
    // iwl_pcie_set_ltr (integrated 22000)
    d.write_prph(HPM_MAC_LTR_CSR, HPM_MAC_LRT_ENABLE_ALL);
    d.write_prph(HPM_UMAC_LTR, LTR_VAL);
    d.trace("fw: ctxt written");
    let t0 = now_us();
    if !d.write_prph(UREG_CPU_INIT_RUN, 1) {
        return Err(String::from("no NIC access to start the CPU"));
    }
    delay_us(20_000);
    d.trace("fw: cpu started +20ms");

    // ALIVE interrupt bit, then stock the RX ring (the ISR's gen2 restock).
    // The cause is looked for in CSR_INT (MSI mode, what UREG_CHICK asked
    // for) and in the MSI-X HW cause register (if the device stayed in MSI-X
    // mode); either way the RX ring is stocked so the ALIVE packet can land.
    let mut got_irq = false;
    let mut msix_alive = false;
    while now_us() - t0 < ALIVE_IRQ_TIMEOUT {
        let int = d.r32(CSR_INT);
        if int & CSR_INT_BIT_ALIVE != 0 {
            d.w32(CSR_INT, CSR_INT_BIT_ALIVE);
            got_irq = true;
            break;
        }
        let hw = d.r32(CSR_MSIX_HW_INT_CAUSES_AD);
        if hw != 0xFFFF_FFFF && hw & MSIX_HW_INT_CAUSES_REG_ALIVE != 0 {
            d.w32(CSR_MSIX_HW_INT_CAUSES_AD, MSIX_HW_INT_CAUSES_REG_ALIVE);
            msix_alive = true;
            break;
        }
        if int & (CSR_INT_BIT_SW_ERR | CSR_INT_BIT_HW_ERR) != 0 {
            break;
        }
        delay_us(100);
    }
    let irq_ms = (now_us() - t0) / 1000;
    for vid in 1..=RX_BUFS {
        d.rx_restock(vid);
    }
    d.rx_kick();
    let pkt = d.wait_pkt(ALIVE_PKT_TIMEOUT, |p| p.group == LEGACY_GROUP && p.cmd == UCODE_ALIVE_NTFY);
    let pkt = match pkt {
        Ok(p) => p,
        Err(e) => {
            d.trace("fw: no ALIVE");
            let irq = if got_irq {
                format!("ALIVE irq after {irq_ms} ms but no packet, ")
            } else if msix_alive {
                format!("ALIVE cause in MSI-X reg after {irq_ms} ms (MSI mode not taken), ")
            } else {
                format!("no ALIVE irq in {irq_ms} ms, ")
            };
            let why = format!(
                "no ALIVE ({irq}{e}) {} | {} | rx closed={} msix fh={:08x} hw={:08x} | ctxt 0x{:x}",
                d.boot_diag(),
                d.regs_line(),
                d.dma.as_ref().map(|m| unsafe { read_volatile(m.rx_stts as *const u16) } & 0xFFF).unwrap_or(0),
                d.r32(CSR_MSIX_FH_INT_CAUSES_AD),
                d.r32(CSR_MSIX_HW_INT_CAUSES_AD),
                d.dma.as_ref().map(|m| m.ctxt).unwrap_or(0)
            );
            d.stop_device();
            return Err(why);
        },
    };
    d.alive_ms = (now_us() - t0) / 1000;
    // fw/api/alive.h: v6 144 bytes, v5 128, v4 116, v3 68
    let a = &pkt.data;
    let status = rd16(a, 0);
    let two_lmacs = a.len() >= 116;
    let umac_off = if two_lmacs { 4 + 96 } else { 4 + 48 };
    let (lmaj, lmin) = (rd32(a, 4), rd32(a, 8));
    d.lmac_err = rd32(a, 20);
    d.umac_err = rd32(a, umac_off + 8) & !0xC000_0000; // FW_ADDR_CACHE_CONTROL
    let (umaj, umin) = (rd32(a, umac_off), rd32(a, umac_off + 4));
    if status != IWL_ALIVE_STATUS_OK {
        let why = format!("ALIVE status 0x{status:04x} (not 0xCAFE), len {}", a.len());
        d.stop_device();
        return Err(why);
    }
    // iwl_trans_pcie_gen2_fw_alive: interrupts back on (still masked at the
    // PCI level: INTx disabled, no MSI), RF name
    d.w32(CSR_INT_MASK, CSR_INI_SET_MASK);
    Ok(format!(
        "{} alive {} ms{}, lmac {lmaj}.{lmin:x} umac {umaj}.{umin:x}{}",
        d.fw_version,
        d.alive_ms,
        if got_irq {
            ""
        } else if msix_alive {
            " (irq via MSI-X reg)"
        } else {
            " (no irq bit)"
        },
        if rfkill { ", RFKILL ON" } else { "" }
    ))
}

// ======================================================================
// Stage 4: init (mvm/fw.c iwl_run_unified_mvm_ucode, iwl-nvm-parse.c iwl_get_nvm)

fn stage_init(d: &mut Dev) -> Result<String, String> {
    d.send_cmd(SYSTEM_GROUP, INIT_EXTENDED_CFG_CMD, &IWL_INIT_NVM.to_le_bytes())?;
    d.send_cmd(REGULATORY_AND_NVM_GROUP, NVM_ACCESS_COMPLETE, &[0; 4])?;
    d.wait_pkt(INIT_COMPLETE_TIMEOUT, |p| p.group == LEGACY_GROUP && p.cmd == INIT_COMPLETE_NOTIF)
        .map_err(|e| format!("INIT_COMPLETE: {e}"))?;
    let r = d.send_cmd(REGULATORY_AND_NVM_GROUP, NVM_GET_INFO, &[0; 4])?;
    // struct iwl_nvm_get_info_rsp (v4: 468 bytes) / _v3 (128 bytes)
    if r.len() != 468 && r.len() != 128 {
        return Err(format!("NVM_GET_INFO reply is {} bytes (expected 468 or 128)", r.len()));
    }
    let gflags = rd32(&r, 0);
    let nvm_ver = rd16(&r, 4);
    let n_addrs = r[7];
    let sku = rd32(&r, 8);
    let (tx, rx) = (rd32(&r, 12), rd32(&r, 16));
    let lar = rd32(&r, 20);
    d.nvm_sku = sku;
    d.nvm_tx = tx;
    d.nvm_rx = rx;
    // iwl-nvm-parse.c iwl_get_nvm: LAR only when both the NVM and the TLV say so
    d.nvm_lar = lar != 0 && d.has_capa(CAPA_LAR_SUPPORT);
    d.nvm_ch.clear();
    if r.len() == 468 {
        let n = (rd32(&r, 24) as usize).min(110);
        for i in 0..n {
            d.nvm_ch.push(rd32(&r, 28 + 4 * i));
        }
    } else {
        for i in 0..51 {
            d.nvm_ch.push(rd16(&r, 24 + 2 * i) as u32);
        }
    }
    d.mac = d.mac_from_csr();
    if !valid_ether(&d.mac) {
        return Err(format!("no valid MAC address (csr {})", mac_str(&d.mac)));
    }
    let mut bands = String::new();
    if sku & 1 != 0 {
        bands.push_str("2.4");
    }
    if sku & 2 != 0 {
        bands.push_str(if bands.is_empty() { "5" } else { "+5" });
    }
    let modes = if sku & 0x10 != 0 { "ax" } else if sku & 0x08 != 0 { "ac" } else { "n" };
    Ok(format!(
        "MAC {}, {bands} GHz {modes}, tx {:x} rx {:x}, nvm {nvm_ver:x} addrs {n_addrs}{}{}",
        mac_str(&d.mac),
        tx,
        rx,
        if lar != 0 { ", LAR" } else { "" },
        if gflags & 1 != 0 { ", OTP EMPTY" } else { "" }
    ))
}

// ======================================================================
// Stage 5 (part 1): the rest of iwl_mvm_up + the station interface's MAC
// context (mvm/fw.c iwl_mvm_up, mvm/mac80211.c iwl_mvm_mac_add_interface)

impl TxQ {
    const fn empty() -> TxQ {
        TxQ { qid: 0, size: 0, tfds: 0, bc: 0, bufs: Vec::new(), wp: 0, rp: 0, active: false }
    }
}

fn now_ms() -> u64 {
    timer::now_ms()
}

fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

/// "<step>: <error>" for the stage line.
fn step<T>(r: Result<T, String>, what: &str) -> Result<T, String> {
    r.map_err(|e| format!("{what}: {e}"))
}

impl Dev {
    fn has_capa(&self, n: u32) -> bool {
        let (i, b) = ((n / 32) as usize, n % 32);
        i < self.capa.len() && self.capa[i] & (1 << b) != 0
    }
    fn has_api(&self, n: u32) -> bool {
        let (i, b) = ((n / 32) as usize, n % 32);
        i < self.api.len() && self.api[i] & (1 << b) != 0
    }
    /// fw/img.c iwl_fw_lookup_cmd_ver (group 0 is looked up as the long group)
    fn cmd_version(&self, group: u8, cmd: u8, def: u8) -> u8 {
        let g = if group == 0 { LONG_GROUP } else { group };
        match self.cmd_ver.iter().find(|e| e.0 == g && e.1 == cmd) {
            Some(e) if e.2 != 99 => e.2,
            _ => def,
        }
    }
    /// iwl_mvm_get_valid_tx_ant / _rx_ant: the firmware's chains (PHY_SKU
    /// TLV) masked by the NVM's.
    fn valid_tx_ant(&self) -> u32 {
        let fw = (self.phy_config >> 16) & 0xF;
        let fw = if fw == 0 { 3 } else { fw };
        if self.nvm_tx & 0xF != 0 { fw & self.nvm_tx } else { fw }
    }
    fn valid_rx_ant(&self) -> u32 {
        let fw = (self.phy_config >> 20) & 0xF;
        let fw = if fw == 0 { 3 } else { fw };
        if self.nvm_rx & 0xF != 0 { fw & self.nvm_rx } else { fw }
    }

    /// PHY_CONTEXT_CMD v3/v4 (mvm/phy-ctxt.c iwl_mvm_phy_ctxt_apply) and the
    /// RLC_CONFIG_CMD v2 that follows it on this firmware.
    fn phy_ctxt(&mut self, id: u32, action: u32, channel: u8) -> Result<(), String> {
        let ver = self.cmd_version(LONG_GROUP, PHY_CONTEXT_CMD, 1);
        if ver != 3 && ver != 4 {
            return Err(format!("PHY_CONTEXT_CMD v{ver} is not written (3/4 are)"));
        }
        let rlc = self.cmd_version(DATA_PATH_GROUP, RLC_CONFIG_CMD, 0) >= 2;
        // valid rx chains, 1 idle / 1 active (mvm/phy-ctxt.c iwl_mvm_phy_ctxt_set_rxchain)
        let rxchain = (self.valid_rx_ant() << 1) | (1 << 10) | (1 << 12);
        let band = if channel <= 14 { PHY_BAND_24 } else { PHY_BAND_5 };
        let mut c = [0u8; 32];
        put32(&mut c, 0, id);
        put32(&mut c, 4, action);
        if self.has_capa(CAPA_ULTRA_HB_CHANNELS) {
            put32(&mut c, 8, channel as u32);
            c[12] = band;
            c[13] = 0; // PHY_VHT_CHANNEL_MODE20
            c[14] = 0; // PHY_VHT_CTRL_POS_1_BELOW
        } else {
            c[8] = channel;
            c[9] = band;
        }
        put32(&mut c, 16, 0); // lmac_id: LMAC_24G_INDEX (no CDB)
        if !rlc {
            put32(&mut c, 20, rxchain);
        }
        self.send_cmd(LONG_GROUP, PHY_CONTEXT_CMD, &c)?;
        if action != FW_CTXT_ACTION_REMOVE && rlc {
            let mut r = [0u8; 32];
            put32(&mut r, 0, id);
            put32(&mut r, 4, rxchain);
            self.send_cmd_v(DATA_PATH_GROUP, RLC_CONFIG_CMD, 2, &r)?;
        }
        Ok(())
    }

    /// MAC_CONTEXT_CMD for the station interface (mvm/mac-ctxt.c
    /// iwl_mvm_mac_ctxt_cmd_common + _cmd_sta), struct iwl_mac_ctx_cmd.
    fn mac_ctxt(&mut self, action: u32) -> Result<(), String> {
        let mut c = [0u8; 148];
        put32(&mut c, 0, MAC_ID);
        put32(&mut c, 4, action);
        put32(&mut c, 8, FW_MAC_TYPE_BSS_STA);
        put32(&mut c, 12, 0); // tsf_id
        c[16..22].copy_from_slice(&self.mac);
        let (bssid, (cck, ofdm), preamble, slot) = match &self.link {
            Some(l) => {
                let b = &l.bss;
                (b.bssid, wlan::ack_rates(b), b.band24 && b.capab & 0x20 != 0, !b.band24 || b.capab & 0x400 != 0)
            },
            None => ([0xFF; 6], (0x0F, 0x15), false, false),
        };
        c[24..30].copy_from_slice(&bssid);
        put32(&mut c, 32, cck as u32);
        put32(&mut c, 36, ofdm as u32);
        put32(&mut c, 44, if preamble { MAC_FLG_SHORT_PREAMBLE } else { 0 });
        put32(&mut c, 48, if slot { MAC_FLG_SHORT_SLOT } else { 0 });
        let assoc = self.link.as_ref().filter(|l| l.assoc).map(|l| (l.aid, l.bss.beacon_int, l.bss.dtim_period, l.beacon));
        let filter = MAC_FILTER_ACCEPT_GRP | if assoc.is_some() { 0 } else { MAC_FILTER_IN_BEACON };
        put32(&mut c, 52, filter);
        put32(&mut c, 56, 0); // qos_flags: a non-QoS (no WMM) station, 20 MHz no-HT
        // non-QoS EDCA (mac80211 ieee80211_set_wmm_default, enable_qos false):
        // CWmin 15, CWmax 1023, AIFSN 2, no TXOP; ucode AC n -> gen2 EDCA FIFO n+1
        for ac in 0..4 {
            let o = 60 + ac * 8;
            put16(&mut c, o, 15);
            put16(&mut c, o + 2, 1023);
            c[o + 4] = 2;
            c[o + 5] = 1 << (ac + 1);
            put16(&mut c, o + 6, 0);
        }
        // struct iwl_mac_data_sta at 100
        if let Some((aid, bi, dtim, beacon)) = assoc {
            let bi = bi.max(1) as u32;
            let (tsf, gp2, count) = beacon.unwrap_or((0, 0, 0));
            let offs = count as u64 * bi as u64 * 1024;
            put32(&mut c, 100, 1);
            put32(&mut c, 104, gp2.wrapping_add(offs as u32));
            put64(&mut c, 108, tsf.wrapping_add(offs));
            put32(&mut c, 116, bi);
            put32(&mut c, 124, bi * dtim.max(1) as u32);
            put32(&mut c, 136, aid as u32);
            put32(&mut c, 140, gp2);
        } else if let Some(l) = &self.link {
            put32(&mut c, 116, l.bss.beacon_int as u32);
            put32(&mut c, 124, l.bss.beacon_int as u32 * l.bss.dtim_period.max(1) as u32);
        }
        put32(&mut c, 132, 10); // listen interval (IWL_CONN_MAX_LISTEN_INTERVAL)
        self.send_cmd(LONG_GROUP, MAC_CONTEXT_CMD, &c).map(|_| ())
    }

    /// iwl_mvm_up's command list after the NVM (this firmware: unified image,
    /// no DQA_ENABLE (capa off), no aux station (ADD_STA v12), one RX queue
    /// so no RSS/RFH config), then the interface's MAC context.
    fn mvm_config(&mut self) -> Result<String, String> {
        let tx_ant = self.valid_tx_ant();
        step(self.send_cmd(LONG_GROUP, TX_ANT_CONFIGURATION_CMD, &tx_ant.to_le_bytes()), "TX_ANT_CONFIGURATION_CMD")?;
        // mvm/coex.c iwl_mvm_send_bt_init_conf: BT_COEX_NW, SYNC2SCO, MPLUT, HIGH_BAND_RET
        let mut modules: u32 = (1 << 2) | (1 << 4);
        if self.has_capa(CAPA_BT_MPLUT_SUPPORT) {
            modules |= 1 << 0;
        }
        let mut bt = [0u8; 8];
        put32(&mut bt, 0, 1);
        put32(&mut bt, 4, modules);
        step(self.send_cmd(LONG_GROUP, BT_CONFIG, &bt), "BT_CONFIG")?;
        // fw/init.c iwl_set_soc_latency with iwl_qu_long_latency_trans_cfg
        // (8086:43F0 / A0F0): integrated, LTR delay 2500 us, low-latency xtal, 12000 us
        if self.has_capa(CAPA_SOC_LATENCY_SUPPORT) {
            let mut flags: u32 = 2 << 2;
            if self.cmd_version(LONG_GROUP, SCAN_REQ_UMAC, 0) >= 2 {
                flags |= 1 << 1;
            }
            let mut s = [0u8; 8];
            put32(&mut s, 0, flags);
            put32(&mut s, 4, 12000);
            step(self.send_cmd(SYSTEM_GROUP, SOC_CONFIGURATION_CMD, &s), "SOC_CONFIGURATION_CMD")?;
        }
        // channels: the regulatory list (LAR: from the MCC answer below)
        self.channels = channels_from(&self.nvm_ch, self.nvm_sku & 2 != 0);
        let first = self.channels.first().copied().unwrap_or(1);
        for id in 0..3 {
            step(self.phy_ctxt(id, FW_CTXT_ACTION_ADD, first), "PHY_CONTEXT_CMD add")?;
        }
        // thermal: the firmware owns CT-kill / TX backoff (empty threshold list)
        if self.has_capa(CAPA_CT_KILL_BY_FW) && self.has_capa(CAPA_TEMP_THS_REPORT_SUPPORT) {
            step(self.send_cmd(PHY_OPS_GROUP, TEMP_REPORTING_THRESHOLDS_CMD, &[0u8; 20]), "TEMP_REPORTING_THRESHOLDS_CMD")?;
        }
        // iwl_mvm_power_update_device: power save off (CAM)
        step(self.send_cmd(LONG_GROUP, POWER_TABLE_CMD, &[0u8; 4]), "POWER_TABLE_CMD")?;
        // iwl_mvm_init_mcc: with LAR the firmware wants a regulatory domain
        // before it scans ("ZZ" = world, current source asks it to keep its own)
        let mut reg = String::from("nvm channels");
        if self.nvm_lar {
            let mut m = [0u8; 28];
            put16(&mut m, 0, (b'Z' as u16) << 8 | b'Z' as u16);
            m[2] = if self.has_api(API_WIFI_MCC_UPDATE) { 0x10 } else { 0 }; // MCC_SOURCE_GET_CURRENT / OLD_FW
            let r = step(self.send_cmd(LONG_GROUP, MCC_UPDATE_CMD, &m), "MCC_UPDATE_CMD")?;
            let (mcc, n, at) = if self.has_capa(CAPA_MCC_UPDATE_11AX_SUPPORT) {
                (rd16(&r, 4), rd32(&r, 16) as usize, 20)
            } else {
                (rd16(&r, 4), rd32(&r, 12) as usize, 16)
            };
            self.mcc = if mcc == 0 { 0x3030 } else { mcc };
            let list: Vec<u32> = (0..n.min(110)).map(|i| rd32(&r, at + 4 * i)).collect();
            let ch = channels_from(&list, self.nvm_sku & 2 != 0);
            if !ch.is_empty() {
                self.channels = ch;
            }
            reg = format!("regdomain {}{} (status {})", (self.mcc >> 8) as u8 as char, self.mcc as u8 as char, rd32(&r, 0));
        }
        // iwl_mvm_config_scan (reduced config, SCAN_CFG_CMD v5: no bcast sta)
        if self.has_capa(CAPA_UMAC_SCAN) {
            if !self.has_api(API_REDUCED_SCAN_CONFIG) {
                return Err(String::from("firmware wants the legacy scan config (not written)"));
            }
            let mut s = [0u8; 12];
            if self.cmd_version(LONG_GROUP, ADD_STA, 0) >= 12 && self.cmd_version(LONG_GROUP, SCAN_CFG_CMD, 0) < 5 {
                s[2] = 0xFF;
            }
            put32(&mut s, 4, tx_ant);
            put32(&mut s, 8, self.valid_rx_ant());
            step(self.send_cmd(LONG_GROUP, SCAN_CFG_CMD, &s), "SCAN_CFG_CMD")?;
        }
        // the station interface (iwl_mvm_mac_ctxt_add)
        self.link = None;
        step(self.mac_ctxt(FW_CTXT_ACTION_ADD), "MAC_CONTEXT_CMD add")?;
        self.configured = true;
        Ok(format!("configured: tx ant {tx_ant:x}, {} channels, {reg}", self.channels.len()))
    }
}

/// Channel numbers whose NVM / MCC flags say NVM_CHANNEL_VALID (bit 0), in
/// iwl_ext_nvm_channels order; 5 GHz only with the SKU's 5 GHz band.
fn channels_from(flags: &[u32], band5: bool) -> Vec<u8> {
    let mut v = Vec::new();
    for (i, &f) in flags.iter().enumerate().take(NVM_CHANNELS.len()) {
        let ch = NVM_CHANNELS[i];
        if f & 1 == 0 || ch == 14 || (ch > 14 && !band5) {
            continue;
        }
        v.push(ch);
    }
    v
}

// ======================================================================
// Stage 5 (part 2): scanning (mvm/scan.c iwl_mvm_scan_umac_v14_and_above)

impl Dev {
    /// One passive UMAC scan over every allowed channel; the beacons and
    /// probe responses heard meanwhile land in `self.bss` (on_mpdu).
    fn scan_umac(&mut self) -> Result<String, String> {
        let ver = self.cmd_version(LONG_GROUP, SCAN_REQ_UMAC, 0);
        if ver != 14 && ver != 15 {
            return Err(format!("SCAN_REQ_UMAC v{ver} is not written (v14/v15 are)"));
        }
        if self.channels.is_empty() {
            return Err(String::from("no allowed channels (regulatory list empty)"));
        }
        if self.hw_rfkill() {
            return Err(String::from("the radio is switched off (hardware RF-kill: BIOS setting or airplane switch)"));
        }
        // struct iwl_scan_req_umac_v15: uid, ooc_priority, general v11 (36),
        // channel params v6 (4 + 67 x 8), periodic (12), probe params v4 (1344)
        let mut c = alloc::vec![0u8; 1940];
        put32(&mut c, 0, 0); // uid
        put32(&mut c, 4, 6); // ooc_priority IWL_SCAN_PRIORITY_EXT_6
        let g = 8;
        // FORCE_PASSIVE (no SSIDs given) | PASS_ALL | ADAPTIVE_DWELL
        put16(&mut c, g, (1 << 11) | (1 << 1) | (1 << 7));
        c[g + 3] = MAC_ID as u8; // scan_start_mac_id
        c[g + 4] = 10; // active dwell, LB / HB
        c[g + 5] = 10;
        c[g + 6] = 2; // adwell_default_2g
        c[g + 7] = 8; // adwell_default_5g
        c[g + 8] = 10; // adwell_default_social_chn
        put16(&mut c, g + 10, 300); // adwell_max_budget (full scan)
        // max_out_of_time / suspend_time stay 0 (IWL_SCAN_TYPE_UNASSOC)
        put32(&mut c, g + 28, 6); // scan_priority
        c[g + 32] = 110; // passive dwell
        c[g + 33] = 110;
        let ch = 44;
        c[ch] = 1 << 5; // IWL_SCAN_CHANNEL_FLAG_ENABLE_CHAN_ORDER
        let n = self.channels.len().min(67);
        c[ch + 1] = n as u8;
        c[ch + 2] = 10; // n_aps_override: GO-friendly, social
        c[ch + 3] = 2;
        for (i, &chan) in self.channels.iter().take(n).enumerate() {
            let o = ch + 4 + i * 8;
            put32(&mut c, o, 0);
            c[o + 4] = chan;
            c[o + 5] = if chan <= 14 { PHY_BAND_24 } else { PHY_BAND_5 };
            c[o + 6] = 1; // iter_count
            c[o + 7] = 0;
        }
        let per = 44 + 4 + 67 * 8;
        put16(&mut c, per, 0); // schedule[0].interval
        c[per + 2] = 1; // schedule[0].iter_count
        // probe request template (iwl_mvm_build_scan_probe; unused by a passive
        // scan): struct iwl_scan_probe_req = mac_header, band_data[3],
        // common_data (offset, len pairs) then buf[512]
        let pr = per + 12;
        let mut t: Vec<u8> = Vec::new();
        t.extend_from_slice(&[0x40, 0, 0, 0]);
        t.extend_from_slice(&[0xFF; 6]);
        t.extend_from_slice(&self.mac);
        t.extend_from_slice(&[0xFF; 6]);
        t.extend_from_slice(&[0, 0, 0, 0]);
        let mh = t.len();
        let b24: [u8; 19] = [1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24, 50, 4, 0x30, 0x48, 0x60, 0x6c, 3, 1, 0];
        let b5: [u8; 10] = [1, 8, 0x8c, 0x12, 0x98, 0x24, 0xb0, 0x48, 0x60, 0x6c];
        t.extend_from_slice(&b24);
        t.extend_from_slice(&b5);
        put16(&mut c, pr, 0); // mac_header
        put16(&mut c, pr + 2, mh as u16);
        put16(&mut c, pr + 4, mh as u16); // band_data[0]: 2.4 GHz
        put16(&mut c, pr + 6, b24.len() as u16);
        put16(&mut c, pr + 8, (mh + b24.len()) as u16); // band_data[1]: 5 GHz
        put16(&mut c, pr + 10, b5.len() as u16);
        put16(&mut c, pr + 12, t.len() as u16); // band_data[2]: 6 GHz, empty
        put16(&mut c, pr + 16, t.len() as u16); // common_data: empty
        let buf = pr + 20;
        c[buf..buf + t.len()].copy_from_slice(&t);

        self.bss.clear();
        self.rx_poll();
        self.inbox.retain(|p| p.cmd != SCAN_COMPLETE_UMAC);
        self.scanning = true;
        let t0 = now_ms();
        let r = self.send_cmd(LONG_GROUP, SCAN_REQ_UMAC, &c);
        if let Err(e) = r {
            self.scanning = false;
            return Err(format!("SCAN_REQ_UMAC: {e}"));
        }
        let budget = 2_000_000 + n as u64 * 200_000;
        let done = self.wait_pkt(budget, |p| (p.group == LEGACY_GROUP || p.group == LONG_GROUP) && p.cmd == SCAN_COMPLETE_UMAC);
        let note = match done {
            Ok(p) => {
                let st = p.data.get(6).copied().unwrap_or(0);
                if st == 1 { String::new() } else { format!(" (scan status {st})") }
            },
            Err(e) if e.starts_with("firmware error") => {
                self.scanning = false;
                self.wedged = true;
                return Err(format!("during the scan: {e}"));
            },
            Err(_) => {
                // SCAN_ABORT_UMAC, then the (aborted) completion
                let _ = self.send_cmd(LONG_GROUP, SCAN_ABORT_UMAC, &[0u8; 8]);
                let _ = self.wait_pkt(2_000_000, |p| (p.group == LEGACY_GROUP || p.group == LONG_GROUP) && p.cmd == SCAN_COMPLETE_UMAC);
                String::from(" (timed out, aborted)")
            },
        };
        self.scanning = false;
        self.scan_ms = now_ms();
        Ok(format!(
            "{} networks on {n} channels in {} ms, {} frames{note}",
            self.bss.len(),
            now_ms() - t0,
            self.mpdus
        ))
    }
}

// ======================================================================
// Stage 6 (part 1): TX queues and the data path (queue/tx.c gen2,
// mvm/tx.c, mvm/rxmq.c)

impl Dev {
    /// iwl_txq_dyn_alloc with SCD_QUEUE_CFG (TX_QUEUE_CFG_CMD v2): a queue for
    /// (AP station, tid). `which` 0 = management (tid 15), 1 = data (tid 0).
    fn enable_txq(&mut self, which: usize, tid: u8) -> Result<(), String> {
        let size = if which == 0 { MGMT_Q_SIZE } else { DATA_Q_SIZE };
        let oom = || String::from("out of DMA memory for a TX queue");
        {
            let q = &mut self.txq[which];
            if q.tfds == 0 {
                q.tfds = dma_alloc(size * TFD_SIZE).ok_or_else(oom)?;
                q.bc = dma_alloc(4096).ok_or_else(oom)?;
                while q.bufs.len() < size {
                    q.bufs.push(dma_alloc(4096).ok_or_else(oom)?);
                }
                q.size = size;
            }
            unsafe {
                core::ptr::write_bytes(q.tfds as *mut u8, 0, size * TFD_SIZE);
                core::ptr::write_bytes(q.bc as *mut u8, 0, 4096);
            }
        }
        let (tfds, bc) = (self.txq[which].tfds, self.txq[which].bc);
        let mut c = [0u8; 24];
        c[0] = AP_STA;
        c[1] = tid;
        put16(&mut c, 2, TX_QUEUE_CFG_ENABLE_QUEUE);
        put32(&mut c, 4, size.trailing_zeros() - 3); // TFD_QUEUE_CB_SIZE
        put64(&mut c, 8, bc);
        put64(&mut c, 16, tfds);
        let r = self.send_cmd(LONG_GROUP, SCD_QUEUE_CFG, &c)?;
        if r.len() < 6 {
            return Err(format!("queue config answer is {} bytes", r.len()));
        }
        let q = &mut self.txq[which];
        q.qid = rd16(&r, 0);
        let wp = rd16(&r, 4) as u32 & (MAX_TFD_QUEUE - 1);
        q.wp = wp;
        q.rp = wp;
        q.active = true;
        if q.qid == 0 || q.qid >= 32 {
            q.active = false;
            return Err(format!("firmware gave queue {}", q.qid));
        }
        Ok(())
    }

    /// iwl_txq_gen2_tx: one 802.11 frame. TB0 = the first 20 bytes of
    /// (command header + iwl_tx_cmd_gen2 + 802.11 header), TB1 the rest of
    /// them (dword-padded), TB2 the frame body. Returns (queue, index).
    fn tx_frame(&mut self, which: usize, frame: &[u8], flags: u32, rate: u32) -> Result<(u16, u8), String> {
        let hl = wlan::hdrlen(frame).min(frame.len());
        if frame.len() > 2304 + 64 {
            return Err(String::from("frame too large"));
        }
        if !self.txq[which].active {
            return Err(String::from("TX queue not set up"));
        }
        // room? (reclaimed by TX responses)
        let end = now_us() + 100_000;
        loop {
            let q = &self.txq[which];
            let used = q.wp.wrapping_sub(q.rp) & (MAX_TFD_QUEUE - 1);
            if (used as usize) < q.size - 1 {
                break;
            }
            if now_us() >= end {
                return Err(String::from("TX queue full (no TX responses)"));
            }
            self.rx_poll();
            delay_us(50);
        }
        let q = &mut self.txq[which];
        let mask = q.size as u32 - 1;
        let idx = (q.wp & mask) as usize;
        let buf = q.bufs[idx];
        let tfd = q.tfds + (idx * TFD_SIZE) as u64;
        let seq: u16 = ((q.qid & 0x1f) << 8) | idx as u16;
        let body = &frame[hl..];
        let tb1 = (4 + hl).div_ceil(4) * 4; // (4 + 20 + hl) - 20, ALIGN 4
        unsafe {
            core::ptr::write_bytes(buf as *mut u8, 0, 128);
            let h = buf as *mut u8;
            // struct iwl_cmd_header (legacy)
            write_volatile(h, TX_CMD);
            write_volatile(h.add(1), 0);
            dw16(buf + 2, seq);
            // struct iwl_tx_cmd_gen2
            dw16(buf + 4, frame.len() as u16);
            dw16(buf + 6, if hl % 4 != 0 { TX_CMD_OFFLD_PAD } else { 0 });
            dw32(buf + 8, flags);
            dw32(buf + 20, rate);
            core::ptr::copy_nonoverlapping(frame.as_ptr(), h.add(24), hl);
            core::ptr::copy_nonoverlapping(body.as_ptr(), h.add(128), body.len());
            core::ptr::write_bytes(tfd as *mut u8, 0, TFD_SIZE);
            dw16(tfd + 2, FIRST_TB as u16);
            core::ptr::write_unaligned((tfd + 4) as *mut u64, buf);
            dw16(tfd + 12, tb1 as u16);
            core::ptr::write_unaligned((tfd + 14) as *mut u64, buf + FIRST_TB as u64);
            let mut n = 2u16;
            if !body.is_empty() {
                dw16(tfd + 22, body.len() as u16);
                core::ptr::write_unaligned((tfd + 24) as *mut u64, buf + 128);
                n = 3;
            }
            dw16(tfd, n);
            // byte-count table (pre-AX210: dwords | fetch chunks << 12; 3 TBs fit one 64-byte chunk)
            dw16(q.bc + (idx * 2) as u64, frame.len().div_ceil(4) as u16);
        }
        fence(Ordering::SeqCst);
        q.wp = (q.wp + 1) & (MAX_TFD_QUEUE - 1);
        let wr = q.wp | ((q.qid as u32) << 16);
        let qid = q.qid;
        self.w32(HBUS_TARG_WRPTR, wr);
        Ok((qid, idx as u8))
    }

    /// TX_CMD response (mvm/tx.c iwl_mvm_rx_tx_cmd_single): reclaim the
    /// slot, keep the status for whoever waits on it.
    fn on_tx_resp(&mut self, seq: u16, d: &[u8]) {
        let qid = (seq >> 8) & 0x1f;
        let idx = (seq & 0xff) as u32;
        let status = if d.len() >= 42 { d[40] } else { 0 };
        for q in self.txq.iter_mut() {
            if q.active && q.qid == qid {
                let mask = q.size as u32 - 1;
                let mut guard = 0;
                while q.rp != q.wp && (q.rp & mask) != ((idx + 1) & mask) && guard < 256 {
                    q.rp = (q.rp + 1) & (MAX_TFD_QUEUE - 1);
                    guard += 1;
                }
            }
        }
        if self.tx_done.len() >= 16 {
            self.tx_done.remove(0);
        }
        self.tx_done.push((qid, idx as u8, status));
        if let Some(l) = self.link.as_mut() {
            // TX_STATUS_SUCCESS 1, TX_STATUS_DIRECT_DONE 2
            if status == 1 || status == 2 {
                l.tx_ok += 1;
                l.tx_fail_run = 0;
            } else {
                l.tx_err += 1;
                l.tx_fail_run += 1;
            }
        }
    }

    /// Wait for the TX status of (queue, index): Some(status) or None on timeout.
    fn wait_tx(&mut self, qid: u16, idx: u8, timeout_us: u64) -> Option<u8> {
        let end = now_us() + timeout_us;
        loop {
            if let Some(i) = self.tx_done.iter().position(|e| e.0 == qid && e.1 == idx) {
                let s = self.tx_done[i].2;
                self.tx_done.remove(i);
                return Some(s);
            }
            if now_us() >= end || self.fw_errored() {
                return None;
            }
            self.rx_poll();
            delay_us(50);
        }
    }

    fn on_missed_beacons(&mut self, d: &[u8]) {
        // struct iwl_missed_beacons_notif v3; > IWL_MVM_MISSED_BEACONS_THRESHOLD_LONG
        if rd32(d, 0) == MAC_ID && rd32(d, 8) > 16 {
            if let Some(l) = self.link.as_mut() {
                if l.assoc && l.lost.is_none() {
                    l.lost = Some(format!("{} beacons missed in a row (out of range?)", rd32(d, 8)));
                }
            }
        }
    }

    /// REPLY_RX_MPDU_CMD (mvm/rxmq.c iwl_mvm_rx_mpdu_mq + iwl_mvm_create_skb):
    /// struct iwl_rx_mpdu_desc (v1 layout, 48 bytes) then the 802.11 frame.
    fn on_mpdu(&mut self, d: &[u8], lnf: u32) {
        self.mpdus = self.mpdus.wrapping_add(1);
        if d.len() < RX_DESC_SIZE_V1 + 10 {
            return;
        }
        let mpdu_len = rd16(d, 0) as usize;
        let (mf1, mf2) = (d[2], d[3]);
        let status = rd32(d, 12);
        if status & (RX_MPDU_STATUS_CRC_OK | RX_MPDU_STATUS_OVERRUN_OK) != (RX_MPDU_STATUS_CRC_OK | RX_MPDU_STATUS_OVERRUN_OK) {
            return;
        }
        let (ea, eb, channel) = (d[32], d[33], d[34]);
        let gp2 = rd32(d, 36);
        let f = &d[RX_DESC_SIZE_V1..];
        if mpdu_len > f.len() || mpdu_len < 10 {
            return;
        }
        let f = &f[..mpdu_len];
        let ea = if ea != 0 { -(ea as i32) } else { -128 };
        let eb = if eb != 0 { -(eb as i32) } else { -128 };
        let rssi = ea.max(eb);
        let ty = (f[0] >> 2) & 3;
        if ty == 0 {
            if f[0] == wlan::FC_BEACON || f[0] == wlan::FC_PROBE_RESP {
                if let Some(mut b) = wlan::parse_bss(f, channel, rssi) {
                    b.gp2 = gp2;
                    b.last_ms = now_ms();
                    if let Some(l) = self.link.as_mut() {
                        if b.bssid == l.bss.bssid && f[0] == wlan::FC_BEACON {
                            l.beacon = Some((b.tsf, gp2, b.dtim_count));
                            l.rssi = (l.rssi * 3 + rssi) / 4;
                        }
                    }
                    if self.scanning || self.link.is_none() {
                        match self.bss.iter().position(|x| x.bssid == b.bssid) {
                            Some(i) => {
                                let best = self.bss[i].rssi.max(b.rssi);
                                self.bss[i] = b;
                                self.bss[i].rssi = best;
                            },
                            None if self.bss.len() < 128 => self.bss.push(b),
                            None => {},
                        }
                    }
                }
                return;
            }
            let mac = self.mac;
            let Some(l) = self.link.as_mut() else { return };
            // to us (or a broadcast deauth / disassoc) from the AP
            let bcast = f[4..10] == [0xFF; 6] && (f[0] == wlan::FC_DEAUTH || f[0] == wlan::FC_DISASSOC);
            if f.len() < 24 || f[10..16] != l.bss.bssid || (f[4..10] != mac && !bcast) {
                return;
            }
            match f[0] {
                wlan::FC_AUTH | wlan::FC_ASSOC_RESP | wlan::FC_REASSOC_RESP => {
                    if l.mgmt.len() < 8 {
                        l.mgmt.push(f.to_vec());
                    }
                },
                wlan::FC_DEAUTH | wlan::FC_DISASSOC if f.len() >= 26 => {
                    let code = rd16(f, 24);
                    let what = if f[0] == wlan::FC_DEAUTH { "deauthenticated" } else { "disassociated" };
                    l.lost = Some(format!("{what} by the access point: reason {code} {}", wlan::status_text(code)));
                    l.mgmt.push(f.to_vec());
                },
                _ => {},
            }
            return;
        }
        if ty != 2 {
            return;
        }
        let Some(l) = self.link.as_mut() else { return };
        if f.len() < 24 || f[10..16] != l.bss.bssid {
            return;
        }
        let hl = wlan::hdrlen(f);
        let protected = f[1] & wlan::FC1_PROTECTED != 0;
        let sec = status & RX_MPDU_STATUS_SEC_MASK;
        let mut crypt = 0;
        if protected {
            if sec == RX_MPDU_STATUS_SEC_CCM && status & RX_MPDU_STATUS_MIC_OK != 0 {
                crypt = 8; // the CCMP header stays in the frame
            } else {
                return; // not decrypted (no key yet / wrong key / other cipher)
            }
        } else if sec != RX_MPDU_STATUS_SEC_NONE && sec != RX_MPDU_STATUS_SEC_MASK {
            return;
        }
        let pad = if mf2 & RX_MPDU_MFLG2_PAD != 0 { 2 } else { 0 };
        // Trailing MIC / FCS bytes (create_skb + mac80211): with RADA the
        // hardware stripped the MIC and mic_crc counts what it left; without
        // RADA create_skb drops (mic_crc - 8) and mac80211 the 8-byte CCMP
        // MIC -- mic_crc bytes off the end either way.
        let _rada = lnf & FH_RSCSR_RADA_EN != 0;
        let mic_crc = (((mf1 >> 4) & 0xF) as usize) << 1;
        let start = hl + crypt + pad;
        let end = mpdu_len.saturating_sub(mic_crc);
        if start > end || end > f.len() {
            return;
        }
        let Some(eth) = wlan::wlan_to_eth(&f[..hl], &f[start..end]) else { return };
        l.rssi = (l.rssi * 7 + rssi) / 8;
        let ethertype = u16::from_be_bytes([eth[12], eth[13]]);
        if ethertype == wlan::ETH_P_PAE {
            if l.eapol.len() < 8 {
                l.eapol.push_back(eth);
            }
            return;
        }
        // nothing but EAPOL before the port is authorized; nothing in clear after the keys
        if !l.authorized || (l.ptk && !protected) {
            return;
        }
        l.rx_data = l.rx_data.wrapping_add(1);
        if l.rx.len() >= RX_FRAMES_MAX {
            l.rx.pop_front();
        }
        l.rx.push_back(eth);
    }

    /// rate_n_flags for frames sent at a fixed rate (mvm/tx.c
    /// iwl_mvm_get_tx_rate + _tx_ant, TX_CMD v9: legacy rates by index; on
    /// 2.4 GHz the antenna not shared with Bluetooth, ANT_B for the 22000 family).
    fn fixed_rate(b: &wlan::Bss, idx: usize) -> u32 {
        let ant = if b.band24 { RATE_MCS_ANT_B_MSK } else { RATE_MCS_ANT_A_MSK };
        if idx >= wlan::FIRST_OFDM {
            (idx - wlan::FIRST_OFDM) as u32 | RATE_MCS_LEGACY_OFDM_MSK | ant
        } else {
            idx as u32 | ant
        }
    }

    /// A management frame to the AP (management queue, lowest rate, clear).
    fn tx_mgmt(&mut self, frame: &[u8]) -> Result<(u16, u8), String> {
        let rate = match &self.link {
            Some(l) => Self::fixed_rate(&l.bss, wlan::lowest_rate(&l.bss)),
            None => return Err(String::from("no link")),
        };
        self.tx_frame(0, frame, IWL_TX_FLAGS_CMD_RATE | IWL_TX_FLAGS_ENCRYPT_DIS, rate)
    }

    /// An Ethernet frame as an 802.11 data frame. EAPOL goes in the clear
    /// before the pairwise key is in; everything else needs the port open.
    fn tx_eth(&mut self, eth: &[u8], force_clear: bool) -> Result<(u16, u8), String> {
        let mac = self.mac;
        let (frame, flags, rate) = {
            let Some(l) = self.link.as_mut() else { return Err(String::from("not joined")) };
            let eapol = eth.len() >= 14 && u16::from_be_bytes([eth[12], eth[13]]) == wlan::ETH_P_PAE;
            if !l.authorized && !eapol {
                return Err(String::from("port not authorized yet"));
            }
            let protect = l.ptk && !force_clear;
            l.seq = (l.seq + 1) & 0x0FFF;
            let (frame, _) = wlan::eth_to_wlan(eth, &l.bss.bssid, &mac, l.seq, protect).ok_or_else(|| String::from("runt frame"))?;
            let mut flags = if protect { 0 } else { IWL_TX_FLAGS_ENCRYPT_DIS };
            let mut rate = 0;
            // the firmware's rate scaling once authorized (and configured)
            if !(l.authorized && l.tlc) || eapol {
                flags |= IWL_TX_FLAGS_CMD_RATE;
                rate = Self::fixed_rate(&l.bss, wlan::lowest_rate(&l.bss));
            }
            (frame, flags, rate)
        };
        self.tx_frame(1, &frame, flags, rate)
    }
}

// ======================================================================
// Stage 6 (part 2): joining (mvm/mac80211.c sta_state + bss_info_changed,
// mvm/sta.c iwl_mvm_add_sta / iwl_mvm_send_sta_key, mvm/binding.c,
// mvm/time-event.c session protection, mvm/rs-fw.c rs_fw_rate_init)

impl Dev {
    fn binding(&mut self, action: u32) -> Result<(), String> {
        let mut c = [0u8; 28];
        put32(&mut c, 0, PHY_ID);
        put32(&mut c, 4, action);
        put32(&mut c, 8, MAC_ID);
        put32(&mut c, 12, FW_CTXT_INVALID);
        put32(&mut c, 16, FW_CTXT_INVALID);
        put32(&mut c, 20, PHY_ID);
        put32(&mut c, 24, 0); // lmac_id (LMAC_24G_INDEX, no CDB)
        let len = if self.has_capa(CAPA_BINDING_CDB_SUPPORT) { 28 } else { 24 };
        let st = self.send_cmd_status(LONG_GROUP, BINDING_CONTEXT_CMD, &c[..len])?;
        if st != 0 {
            return Err(format!("status {st}"));
        }
        Ok(())
    }

    /// ADD_STA (struct iwl_mvm_add_sta_cmd, 48 bytes; mvm/sta.c iwl_mvm_sta_send_to_fw)
    fn add_sta(&mut self, update: bool, aid: u16) -> Result<(), String> {
        let bssid = self.link.as_ref().map(|l| l.bss.bssid).ok_or_else(|| String::from("no link"))?;
        let mut c = [0u8; 48];
        c[0] = update as u8;
        put16(&mut c, 2, 0xFFFF); // tid_disable_tx: no aggregation
        put32(&mut c, 4, MAC_ID);
        if !update {
            c[8..14].copy_from_slice(&bssid);
        }
        c[16] = AP_STA;
        put32(&mut c, 20, 0); // non-HT, single stream
        put32(&mut c, 24, STA_FLG_FAT_EN_MSK | STA_FLG_MIMO_EN_MSK | STA_FLG_RTS_MIMO_PROT);
        if self.has_api(API_STA_TYPE) {
            c[35] = 0; // IWL_STA_LINK
        }
        put16(&mut c, 36, aid);
        let st = self.send_cmd_status(LONG_GROUP, ADD_STA, &c)?;
        if st & 0xFF != ADD_STA_SUCCESS {
            return Err(format!("status 0x{st:x}"));
        }
        Ok(())
    }

    /// ADD_STA_KEY v2/v3 (struct iwl_mvm_add_sta_key_cmd, 76 bytes) for CCMP.
    fn add_key(&mut self, offset: u8, keyidx: u8, key: &[u8], mcast: bool) -> Result<(), String> {
        let ver = self.cmd_version(LONG_GROUP, ADD_STA_KEY, 2);
        if ver < 2 {
            return Err(format!("ADD_STA_KEY v{ver} not written"));
        }
        if key.len() != 16 {
            return Err(format!("{}-byte key (CCMP keys are 16)", key.len()));
        }
        let mut c = [0u8; 76];
        c[0] = AP_STA;
        c[1] = offset;
        let mut flags = ((keyidx as u16) << 8) & 0x300 | STA_KEY_FLG_WEP_KEY_MAP | STA_KEY_FLG_CCM;
        if mcast {
            flags |= STA_KEY_MULTICAST;
        }
        put16(&mut c, 2, flags);
        c[4..20].copy_from_slice(key);
        // rx_secur_seq_cnt / transmit_seq_cnt start at 0
        let st = self.send_cmd_status(LONG_GROUP, ADD_STA_KEY, &c)?;
        if st != ADD_STA_SUCCESS {
            return Err(format!("status 0x{st:x}"));
        }
        Ok(())
    }

    /// TLC_MNG_CONFIG_CMD v4: firmware rate scaling over the legacy rates.
    fn tlc_config(&mut self) -> Result<(), String> {
        let ver = self.cmd_version(DATA_PATH_GROUP, TLC_MNG_CONFIG_CMD, 0);
        if ver != 4 {
            return Err(format!("TLC_MNG_CONFIG_CMD v{ver} not written"));
        }
        let rates = self.link.as_ref().map(|l| wlan::rate_bitmap(&l.bss)).unwrap_or(0x0FF0);
        let mut c = [0u8; 28];
        c[0] = AP_STA;
        c[4] = 0; // IWL_TLC_MNG_CH_WIDTH_20MHZ
        c[5] = 0; // IWL_TLC_MNG_MODE_NON_HT
        c[6] = (self.valid_tx_ant() & 3) as u8;
        put16(&mut c, 10, rates);
        self.send_cmd(DATA_PATH_GROUP, TLC_MNG_CONFIG_CMD, &c).map(|_| ())
    }

    /// SESSION_PROTECTION_CMD: stay on the AP's channel while joining.
    fn session_protect(&mut self, ms: u32) -> Result<bool, String> {
        if !self.has_capa(CAPA_SESSION_PROT_CMD) {
            return Ok(false);
        }
        let mut c = [0u8; 24];
        put32(&mut c, 0, MAC_ID);
        put32(&mut c, 4, FW_CTXT_ACTION_ADD);
        put32(&mut c, 8, 0); // SESSION_PROTECT_CONF_ASSOC
        put32(&mut c, 12, ms * 1000 / 1024);
        self.inbox.retain(|p| !(p.group == MAC_CONF_GROUP && p.cmd == SESSION_PROTECTION_NOTIF));
        self.send_cmd(MAC_CONF_GROUP, SESSION_PROTECTION_CMD, &c)?;
        let started = self
            .wait_pkt(1_000_000, |p| p.group == MAC_CONF_GROUP && p.cmd == SESSION_PROTECTION_NOTIF)
            .map(|p| rd32(&p.data, 4) == 1 && rd32(&p.data, 8) == 1)
            .unwrap_or(false);
        Ok(started)
    }

    /// Wait for a management frame from the AP that `f` recognizes.
    fn wait_mgmt<T>(&mut self, timeout_us: u64, f: impl Fn(&[u8]) -> Option<T>) -> Option<T> {
        let end = now_us() + timeout_us;
        loop {
            self.rx_poll();
            if let Some(l) = self.link.as_mut() {
                while let Some(fr) = (!l.mgmt.is_empty()).then(|| l.mgmt.remove(0)) {
                    if let Some(v) = f(&fr) {
                        return Some(v);
                    }
                }
            }
            if now_us() >= end || self.fw_errored() {
                return None;
            }
            delay_us(100);
        }
    }

    /// Undo a (partial) join: deauth when associated, then the station (its
    /// queues go with it on this firmware), the binding, a clean MAC context.
    fn teardown(&mut self, bound: bool, sta: bool) {
        if self.wedged || self.fw_errored() {
            // the firmware is not answering: the next scan / join restarts it
            self.link = None;
            self.wedged = true;
            for q in self.txq.iter_mut() {
                q.active = false;
            }
            return;
        }
        let mac = self.mac;
        let deauth = match self.link.as_mut() {
            Some(l) if l.assoc && l.lost.is_none() => {
                l.seq = (l.seq + 1) & 0x0FFF;
                Some(wlan::build_deauth(&l.bss.bssid, &mac, l.seq, 3))
            },
            _ => None,
        };
        if let Some(f) = deauth {
            if let Ok((q, i)) = self.tx_mgmt(&f) {
                let _ = self.wait_tx(q, i, 200_000);
            }
        }
        if sta {
            let _ = self.send_cmd(LONG_GROUP, REMOVE_STA, &[AP_STA, 0, 0, 0]);
        }
        for q in self.txq.iter_mut() {
            q.active = false;
        }
        if bound {
            let _ = self.binding(FW_CTXT_ACTION_REMOVE);
        }
        self.link = None;
        let _ = self.mac_ctxt(FW_CTXT_ACTION_MODIFY);
    }

    fn join(&mut self, ssid: &str, psk: &str) -> Result<String, String> {
        // the network: from the last scan, else scan now
        // A dual-band network: its 5 GHz access point when that is usable
        // (-80 dBm or better), else the strongest. On the user's Z590 the
        // 5 GHz join works while a 2.4 GHz one made the firmware assert
        // during authentication (CSR_INT 82000000) -- and 5 GHz is the
        // faster, less crowded band anyway.
        let pick = |d: &Dev| {
            let same: Vec<&wlan::Bss> = d.bss.iter().filter(|b| b.ssid == ssid.as_bytes()).collect();
            same.iter()
                .filter(|b| !b.band24 && b.rssi >= -80)
                .max_by_key(|b| b.rssi)
                .or_else(|| same.iter().max_by_key(|b| b.rssi))
                .map(|b| (*b).clone())
        };
        let mut bss = if now_ms().saturating_sub(self.scan_ms) < 60_000 { pick(self) } else { None };
        if bss.is_none() {
            step(self.scan_umac(), "scan")?;
            bss = pick(self);
        }
        let bss = bss.ok_or_else(|| format!("\"{ssid}\" was not heard in the scan"))?;
        let sec = wlan::security(&bss);
        let rsn = match sec {
            0 => None,
            1 => return Err(String::from("WEP networks are not supported")),
            2 => return Err(String::from("WPA1 (TKIP) networks are not supported; use WPA2-AES")),
            _ => Some(wlan::rsn_ie_for(&bss).map_err(String::from)?),
        };
        if wlan::requires_ht(&bss) {
            return Err(String::from("the access point requires 802.11n/ac clients (this driver joins with 11a/b/g rates)"));
        }
        let pmk = match &rsn {
            None => None,
            Some(_) => {
                let p = psk.as_bytes();
                if p.len() == 64 && p.iter().all(|c| c.is_ascii_hexdigit()) {
                    let mut k = [0u8; 32];
                    for i in 0..32 {
                        k[i] = u8::from_str_radix(&psk[2 * i..2 * i + 2], 16).unwrap_or(0);
                    }
                    Some(k)
                } else if (8..=63).contains(&p.len()) {
                    Some(wpa::pmk_from_passphrase(p, ssid.as_bytes()))
                } else {
                    return Err(String::from("a WPA2 passphrase is 8-63 characters"));
                }
            },
        };
        let ch = bss.channel;
        let desc = format!("{} ch {ch} {} dBm", wlan::mac_str(&bss.bssid), bss.rssi);
        set_stage(5, St::Todo, format!("joining {ssid} ({desc})"));
        self.link = Some(Link {
            bss: bss.clone(),
            aid: 0,
            seq: 0,
            assoc: false,
            ptk: false,
            authorized: false,
            tlc: false,
            sup: None,
            rx: VecDeque::new(),
            mgmt: Vec::new(),
            eapol: VecDeque::new(),
            beacon: None,
            lost: None,
            rssi: bss.rssi,
            tx_fail_run: 0,
            tx_ok: 0,
            tx_err: 0,
            rx_data: 0,
        });
        // channel, BSSID, binding, station, queues
        let r = (|| -> Result<(), String> {
            step(self.phy_ctxt(PHY_ID, FW_CTXT_ACTION_MODIFY, ch), "PHY_CONTEXT_CMD (channel)")?;
            step(self.mac_ctxt(FW_CTXT_ACTION_MODIFY), "MAC_CONTEXT_CMD (bssid)")?;
            Ok(())
        })();
        if let Err(e) = r {
            self.teardown(false, false);
            return Err(e);
        }
        if let Err(e) = step(self.binding(FW_CTXT_ACTION_ADD), "BINDING_CONTEXT_CMD") {
            self.teardown(false, false);
            return Err(e);
        }
        if let Err(e) = step(self.add_sta(false, 0), "ADD_STA") {
            self.teardown(true, false);
            return Err(e);
        }
        let r = (|| -> Result<(), String> {
            step(self.enable_txq(0, IWL_MGMT_TID), "TX queue (management)")?;
            step(self.enable_txq(1, IWL_TID_NON_QOS), "TX queue (data)")?;
            Ok(())
        })();
        if let Err(e) = r {
            self.teardown(true, true);
            return Err(e);
        }
        let r = self.join_air(ssid, rsn, pmk);
        match r {
            Ok(s) => Ok(format!("{s} ({desc})")),
            Err(e) => {
                self.teardown(true, true);
                Err(e)
            },
        }
    }

    /// Authentication, association, the 4-way handshake, keys.
    fn join_air(&mut self, ssid: &str, rsn: Option<Vec<u8>>, pmk: Option<[u8; 32]>) -> Result<String, String> {
        let protected = step(self.session_protect(1000), "SESSION_PROTECTION_CMD")?;
        let mac = self.mac;
        // Open System authentication (3 tries, 300 ms each)
        let mut auth = None;
        for _ in 0..3 {
            let f = {
                let l = self.link.as_mut().unwrap();
                l.seq = (l.seq + 1) & 0x0FFF;
                wlan::build_auth(&l.bss.bssid, &mac, l.seq)
            };
            let (q, i) = step(self.tx_mgmt(&f), "auth TX")?;
            let st = self.wait_tx(q, i, 300_000);
            auth = self.wait_mgmt(300_000, wlan::auth_result);
            if auth.is_some() {
                break;
            }
            if st.is_none() && self.fw_errored() {
                return Err(format!("firmware error during auth: {}", self.fw_error_line()));
            }
        }
        match auth {
            None => {
                return Err(format!(
                    "no authentication response{} (tx ok {} err {})",
                    if protected { "" } else { " (no session protection)" },
                    self.link.as_ref().map(|l| l.tx_ok).unwrap_or(0),
                    self.link.as_ref().map(|l| l.tx_err).unwrap_or(0)
                ))
            },
            Some(Err(code)) => return Err(format!("authentication refused: status {code} {}", wlan::status_text(code))),
            Some(Ok(())) => {},
        }
        // association
        let mut assoc = None;
        for _ in 0..3 {
            let f = {
                let l = self.link.as_mut().unwrap();
                l.seq = (l.seq + 1) & 0x0FFF;
                wlan::build_assoc_req(&l.bss, &mac, l.seq, rsn.as_deref(), 10)
            };
            let (q, i) = step(self.tx_mgmt(&f), "assoc TX")?;
            let _ = self.wait_tx(q, i, 300_000);
            assoc = self.wait_mgmt(500_000, wlan::assoc_result);
            if assoc.is_some() {
                break;
            }
            if let Some(l) = self.link.as_ref() {
                if let Some(why) = &l.lost {
                    return Err(why.clone());
                }
            }
        }
        let aid = match assoc {
            None => return Err(String::from("no association response")),
            Some(Err(code)) => return Err(format!("association refused: status {code} {}", wlan::status_text(code))),
            Some(Ok(aid)) => aid,
        };
        {
            let l = self.link.as_mut().unwrap();
            l.aid = aid;
            l.assoc = true;
        }
        set_stage(5, St::Todo, format!("associated with {ssid}, aid {aid}"));
        // a fresh beacon for the DTIM timing (MAC context "associated")
        let end = now_us() + 400_000;
        while now_us() < end && self.link.as_ref().map(|l| l.beacon.is_none()).unwrap_or(false) {
            self.rx_poll();
            delay_us(200);
        }
        if self.link.as_ref().map(|l| l.beacon.is_none()).unwrap_or(false) {
            let b = self.link.as_ref().unwrap().bss.clone();
            self.link.as_mut().unwrap().beacon = Some((b.tsf, b.gp2, b.dtim_count));
        }
        step(self.add_sta(true, aid), "ADD_STA (associated)")?;
        step(self.mac_ctxt(FW_CTXT_ACTION_MODIFY), "MAC_CONTEXT_CMD (associated)")?;
        let tlc = self.tlc_config().is_ok();
        self.link.as_mut().unwrap().tlc = tlc;

        let Some(pmk) = pmk else {
            let l = self.link.as_mut().unwrap();
            l.authorized = true;
            return Ok(format!("joined {ssid} (open), aid {aid}"));
        };
        // the 4-way handshake (arch/aarch64/wpa.rs does the crypto)
        let mut snonce = [0u8; 32];
        if !crate::crypto::random_bytes(&mut snonce) {
            for (i, b) in snonce.iter_mut().enumerate() {
                *b = (now_us() >> (i % 8)) as u8 ^ (i as u8).wrapping_mul(97);
            }
        }
        let mut hs = wpa::Handshake::new(pmk, mac, snonce);
        if let Some(ie) = &rsn {
            hs.set_rsn_ie(ie);
        }
        self.link.as_mut().unwrap().sup = Some(hs);
        let end = now_us() + 8_000_000;
        let mut got_msg1 = false;
        loop {
            self.rx_poll();
            if let Some(why) = self.link.as_ref().and_then(|l| l.lost.clone()) {
                return Err(format!(
                    "{why} (during the WPA2 handshake{})",
                    if got_msg1 { "; wrong passphrase?" } else { ", before message 1" }
                ));
            }
            let frame = self.link.as_mut().and_then(|l| l.eapol.pop_front());
            if let Some(fr) = frame {
                got_msg1 = true;
                let acts = {
                    let l = self.link.as_mut().unwrap();
                    l.sup.as_mut().unwrap().rx(&fr)
                };
                let acts = match acts {
                    Ok(a) => a,
                    Err(e) => {
                        if e.starts_with("message integrity") {
                            return Err(String::from("WPA2 handshake: wrong passphrase (message 3 MIC check failed)"));
                        }
                        continue;
                    },
                };
                for a in acts {
                    match a {
                        wpa::Action::Send(eth) => {
                            let (q, i) = step(self.tx_eth(&eth, true), "EAPOL TX")?;
                            let _ = self.wait_tx(q, i, 300_000);
                        },
                        wpa::Action::Pairwise(tk) => {
                            step(self.add_key(0, 0, &tk, false), "ADD_STA_KEY (pairwise)")?;
                            self.link.as_mut().unwrap().ptk = true;
                        },
                        wpa::Action::Group { index, key, .. } => {
                            step(self.add_key(1, index, &key, true), "ADD_STA_KEY (group)")?;
                        },
                        wpa::Action::Done => {
                            self.link.as_mut().unwrap().authorized = true;
                        },
                    }
                }
                if self.link.as_ref().map(|l| l.authorized).unwrap_or(false) {
                    return Ok(format!("joined {ssid} (WPA2-PSK/CCMP), aid {aid}"));
                }
            }
            if now_us() >= end {
                return Err(String::from(if got_msg1 {
                    "WPA2 handshake did not finish: no message 3 (wrong passphrase?)"
                } else {
                    "no WPA2 handshake from the access point (no EAPOL message 1)"
                }));
            }
            if self.fw_errored() {
                return Err(format!("firmware error: {}", self.fw_error_line()));
            }
            delay_us(200);
        }
    }

    /// A group-key rekey (or a PTK rekey) while joined: feed EAPOL to the
    /// supplicant and carry out what it says.
    fn service_eapol(&mut self) {
        loop {
            let Some(fr) = self.link.as_mut().and_then(|l| l.eapol.pop_front()) else { return };
            let acts = match self.link.as_mut().and_then(|l| l.sup.as_mut()) {
                Some(s) => s.rx(&fr),
                None => return,
            };
            let Ok(acts) = acts else { continue };
            for a in acts {
                match a {
                    wpa::Action::Send(eth) => {
                        let _ = self.tx_eth(&eth, false);
                    },
                    wpa::Action::Pairwise(tk) => {
                        let _ = self.add_key(0, 0, &tk, false);
                    },
                    wpa::Action::Group { index, key, .. } => {
                        let _ = self.add_key(1, index, &key, true);
                    },
                    wpa::Action::Done => {},
                }
            }
        }
    }
}

// ======================================================================
// Driving the stages

/// Stages 2-5 from a stopped device: wake, firmware, init, configure. True
/// when the radio is ready to scan. Each stage's result (or the reason it
/// failed) is kept for `wifi-intel`.
fn run_from_scratch() -> bool {
    unsafe { PROBED = true };
    for i in 0..6 {
        set_stage(i, St::NotRun, String::new());
    }
    if dev().is_none() {
        if let Err(e) = stage_detect() {
            set_stage(0, St::Fail, e);
            return false;
        }
    } else {
        let d = dev().unwrap();
        d.stop_device();
        let kind = d.hw_rev;
        set_stage(0, St::Ok, format!("rev 0x{kind:x}, bar0 0x{:x}, fw {}", d.bar0_phys, d.fw_name));
    }
    let Some(d) = dev() else { return false };
    d.configured = false;
    d.wedged = false;
    d.link = None;
    d.bss.clear();
    d.tx_done.clear();
    for q in d.txq.iter_mut() {
        q.active = false;
    }
    d.last_up_try_ms = now_ms();
    // Linux's order first; when the firmware never answers, once more without
    // the integrated-22000 forced power gating (and remember what worked).
    let first = unsafe { WAKE_VARIANT };
    let mut earlier = String::new();
    let mut order = alloc::vec![first];
    order.extend((0..3u8).filter(|&v| v != first));
    let last = *order.last().unwrap();
    for variant in order {
        unsafe { WAKE_VARIANT = variant };
        crate::drivers::serial::write_str(&format!("wifi-intel: bring-up variant {variant}\n"));
        if !earlier.is_empty() {
            d.stop_device();
        }
        let r = stage_wake(d).map_err(|e| (1, e)).and_then(|w| {
            set_stage(1, St::Ok, w);
            stage_firmware(d).map_err(|e| (2, e))
        });
        match r {
            Ok(s) => {
                set_stage(2, St::Ok, s);
                earlier.clear();
                break;
            },
            Err((i, e)) => {
                crate::drivers::serial::write_str(&format!("wifi-intel: variant {variant} stage {} failed: {e}\n", i + 1));
                if variant == last {
                    set_stage(i, St::Fail, format!("{e} || all 3 start variants failed; first (variant {first}): {earlier}"));
                    unsafe { WAKE_VARIANT = first };
                    return false;
                }
                if earlier.is_empty() {
                    earlier = e;
                }
            },
        }
    }
    match stage_init(d) {
        Ok(s) => set_stage(3, St::Ok, s),
        Err(e) => {
            let e = format!("{e} | {}", d.regs_line());
            set_stage(3, St::Fail, e);
            return false;
        },
    }
    match d.mvm_config() {
        Ok(s) => set_stage(4, St::Ok, s),
        Err(e) => {
            let e = if d.fw_errored() { format!("{e} | {}", d.fw_error_line()) } else { e };
            set_stage(4, St::Fail, e);
            return false;
        },
    }
    set_stage(5, St::NotRun, String::from("not joined"));
    true
}

/// Detection only (what boot does); never starts the firmware.
fn ensure_probed() {
    if unsafe { PROBED } {
        return;
    }
    boot();
}

/// The radio ready to scan: the first scan or join after boot starts the
/// firmware here (a few seconds); a firmware that asserted or a command that
/// timed out is restarted. A failed bring-up is retried at most every 10 s.
fn ensure_up() -> Result<&'static mut Dev, String> {
    ensure_probed();
    let Some(d) = dev() else {
        return Err(String::from("no Intel Wi-Fi on this machine"));
    };
    if d.configured && !d.wedged && !d.fw_errored() {
        return Ok(d);
    }
    if !d.configured && d.last_up_try_ms != 0 && now_ms().saturating_sub(d.last_up_try_ms) < 10_000 {
        return Err(failure_text());
    }
    if !run_from_scratch() {
        return Err(failure_text());
    }
    dev().ok_or_else(|| String::from("device vanished"))
}

/// "Intel AX201: <stage> failed: <why>" (the start of it; `wifi-intel` has all).
fn failure_text() -> String {
    let s = stages();
    match s.iter().position(|x| x.st == St::Fail) {
        Some(i) => {
            let t: String = s[i].text.chars().take(140).collect();
            format!("Intel AX201: {} failed: {t}", STAGE_NAMES[i])
        },
        None => String::from("Intel AX201 is not ready (wifi-intel up)"),
    }
}

/// One scanned network for wifi::scan().
pub struct ScanRow {
    pub ssid: String,
    pub rssi: i32,
    pub secure: u8,
    pub channel: u16,
}

fn joined_link(d: &Dev) -> Option<&Link> {
    if d.wedged || d.fw_errored() {
        return None;
    }
    d.link.as_ref().filter(|l| l.authorized && l.lost.is_none() && l.tx_fail_run < 20)
}

/// Scan (starting the firmware on first use). While joined the last results
/// are returned: scanning off-channel during an association is not written.
pub fn scan() -> Result<Vec<ScanRow>, String> {
    let d = ensure_up()?;
    if joined_link(d).is_none() {
        if d.link.is_some() {
            d.teardown(true, true);
        }
        match d.scan_umac() {
            Ok(s) => set_stage(4, St::Ok, s),
            Err(e) => {
                set_stage(4, St::Fail, e.clone());
                return Err(format!("Intel AX201 scan failed: {e}"));
            },
        }
    }
    let mut rows: Vec<ScanRow> = Vec::new();
    for b in &d.bss {
        if b.ssid.is_empty() || b.ssid.iter().all(|&c| c == 0) {
            continue; // hidden network
        }
        let ssid = String::from_utf8_lossy(&b.ssid).into_owned();
        match rows.iter_mut().find(|r| r.ssid == ssid) {
            Some(r) if r.rssi >= b.rssi => {},
            Some(r) => {
                r.rssi = b.rssi;
                r.secure = wlan::security(b);
                r.channel = b.channel as u16;
            },
            None => rows.push(ScanRow { ssid, rssi: b.rssi, secure: wlan::security(b), channel: b.channel as u16 }),
        }
    }
    rows.sort_by(|a, b| b.rssi.cmp(&a.rssi));
    Ok(rows)
}

/// Join a network (open or WPA2-PSK). Ok carries a one-line summary.
pub fn join(ssid: &str, psk: &str) -> Result<String, String> {
    let d = ensure_up()?;
    if d.link.is_some() {
        d.teardown(true, true);
    }
    match d.join(ssid, psk) {
        Ok(s) => {
            set_stage(5, St::Ok, s.clone());
            Ok(s)
        },
        Err(e) => {
            let e = if d.fw_errored() { format!("{e} | {}", d.fw_error_line()) } else { e };
            set_stage(5, St::Fail, e.clone());
            Err(e)
        },
    }
}

/// Leave the network (deauthenticate, drop the station and keys).
pub fn leave() {
    if let Some(d) = dev() {
        if d.link.is_some() && d.configured && !d.wedged {
            d.teardown(true, true);
        } else {
            d.link = None;
        }
        set_stage(5, St::NotRun, String::from("left the network"));
    }
}

/// Joined and the port authorized (data flows).
pub fn is_joined() -> bool {
    dev().map(|d| joined_link(d).is_some()).unwrap_or(false)
}

/// Joined once, and the link has since gone (deauth, missed beacons, the
/// firmware died): why.
pub fn dropped() -> Option<String> {
    let d = dev()?;
    let l = d.link.as_ref()?;
    if d.fw_errored() || d.wedged {
        return Some(String::from("the Wi-Fi firmware stopped answering"));
    }
    if l.tx_fail_run >= 20 {
        return Some(String::from("the access point stopped acknowledging"));
    }
    l.lost.clone()
}

pub fn rssi() -> i32 {
    dev().and_then(|d| joined_link(d).map(|l| l.rssi)).unwrap_or(0)
}

/// Process what the radio sent meanwhile (rekeys, deauth); the desktop's tick.
pub fn poll() {
    if let Some(d) = dev() {
        if d.link.is_some() && d.configured {
            d.rx_poll();
            d.service_eapol();
        }
    }
}

/// netdev: an Ethernet frame out over Wi-Fi.
pub fn transmit(eth: &[u8]) -> bool {
    let Some(d) = dev() else { return false };
    if joined_link(d).is_none() {
        return false;
    }
    d.tx_eth(eth, false).is_ok()
}

/// netdev: the next received Ethernet frame.
pub fn receive(out: &mut [u8]) -> Option<usize> {
    let d = dev()?;
    joined_link(d)?;
    if d.link.as_ref().map(|l| l.rx.is_empty()).unwrap_or(true) {
        d.rx_poll();
        d.service_eapol();
    }
    let f = d.link.as_mut()?.rx.pop_front()?;
    let n = f.len().min(out.len());
    out[..n].copy_from_slice(&f[..n]);
    Some(n)
}

/// One line about the link for wifi::link_note() / `wifi-intel`.
pub fn link_line() -> String {
    let Some(d) = dev() else { return String::new() };
    let Some(l) = d.link.as_ref() else { return String::new() };
    format!(
        "{} {} ch {} {} dBm aid {}{}{} tx ok {} err {} rx {}{}",
        String::from_utf8_lossy(&l.bss.ssid),
        wlan::mac_str(&l.bss.bssid),
        l.bss.channel,
        l.rssi,
        l.aid,
        if l.ptk { " keys" } else { "" },
        if l.tlc { " tlc" } else { "" },
        l.tx_ok,
        l.tx_err,
        l.rx_data,
        match &l.lost {
            Some(w) => format!(" LOST: {w}"),
            None => String::new(),
        }
    )
}

/// Boot hook (x86 `ling_kernel_net_init`): bring the radio up as far as the
/// driver goes and print every stage to the console / boot log.
///
/// Boot only DETECTS the card. Starting the firmware (stages 2-4) waits on the
/// radio for seconds when it does not answer, and on a real Z590 that held the
/// boot screen on "firmware"; with scan/join not written yet it bought nothing.
/// `wifi-intel up` runs the full bring-up on demand.
pub fn boot() {
    if unsafe { PROBED } {
        return;
    }
    unsafe { PROBED = true };
    for i in 0..6 {
        set_stage(i, St::NotRun, String::new());
    }
    if let Err(e) = stage_detect() {
        set_stage(0, St::Fail, e);
    }
    if unsafe { ABSENT } || dev().is_none() {
        crate::console_write(b"wifi-intel: no Intel Wi-Fi (CNVi AX201) on the PCI bus\n");
        return;
    }
    for i in 1..5 {
        set_stage(i, St::Todo, String::from("starts on the first scan (or `wifi-intel up`)"));
    }
    set_stage(5, St::NotRun, String::from("not joined"));
    crate::console_write(b"wifi-intel: Intel AX201 found; firmware starts on the first Wi-Fi scan (`wifi-intel up` starts it now)\n");
}

/// The firmware is up and answered NVM_GET_INFO (stage 4 ok).
pub fn up() -> bool {
    (unsafe { PROBED }) && stages()[3].st == St::Ok
}

/// A supported device was found on the PCI bus.
pub fn present() -> bool {
    dev().is_some()
}

pub fn mac() -> [u8; 6] {
    dev().map(|d| d.mac).unwrap_or([0; 6])
}

/// One line for wifi::status() / the picker header. Never starts the firmware.
pub fn short_status() -> String {
    ensure_probed();
    let Some(d) = dev() else {
        return String::from("no Intel Wi-Fi on this machine");
    };
    if let Some(l) = joined_link(d) {
        return format!("Intel AX201: joined {} ({} dBm)", String::from_utf8_lossy(&l.bss.ssid), l.rssi);
    }
    if let Some(why) = dropped() {
        return format!("Intel AX201: link lost: {why}");
    }
    let s = stages();
    if let Some(i) = s.iter().position(|x| x.st == St::Fail) {
        return format!("Intel AX201: stage {} {} failed (wifi-intel shows why)", i + 1, STAGE_NAMES[i]);
    }
    if d.configured {
        return format!("Intel AX201 ready (fw {}, {}), not joined", d.fw_version, mac_str(&d.mac));
    }
    String::from("Intel AX201 found; the firmware starts on the first scan")
}

/// The status report: one line per stage, short enough for an HDMI photo.
pub fn report() -> String {
    let mut out = String::new();
    let Some(d) = dev() else {
        let s = stages();
        if s[0].st == St::Fail {
            return format!("wifi-intel: {}\n", s[0].text);
        }
        return String::from("wifi-intel: not probed\n");
    };
    if up() {
        d.rx_poll(); // drain notifications the firmware sent since
    }
    out.push_str(&format!(
        "wifi-intel: Intel AX201 8086:{:04x} sub {:04x}:{:04x} at {:02x}:{:02x}.{}\n",
        d.device_id,
        d.subsys & 0xFFFF,
        d.subsys >> 16,
        d.loc.bus,
        d.loc.device,
        d.loc.function
    ));
    for (i, s) in stages().iter().enumerate() {
        let tag = match s.st {
            St::Ok => "ok  ",
            St::Fail => "FAIL",
            St::Todo => "todo",
            St::NotRun => "-   ",
        };
        out.push_str(&format!(" {} {:<8} {} {}\n", i + 1, STAGE_NAMES[i], tag, s.text));
    }
    if d.configured {
        out.push_str(&format!(
            " radio: {} channels, {} networks heard, {} frames rx{}\n",
            d.channels.len(),
            d.bss.len(),
            d.mpdus,
            if d.wedged { ", WEDGED (next scan restarts the firmware)" } else { "" }
        ));
    }
    if d.link.is_some() {
        out.push_str(&format!(" link: {}\n", link_line()));
    }
    if !d.log.is_empty() {
        out.push_str(" rx:");
        for (g, c) in &d.log {
            out.push_str(&format!(" {g:02x}.{c:02x}"));
        }
        out.push('\n');
    }
    out
}

/// `wifi-intel [up | regs | stop]` (console and SSH).
pub fn command(arg: &str) -> String {
    match arg.trim() {
        "" | "status" => {
            ensure_probed();
            report()
        },
        "up" | "retry" => {
            run_from_scratch();
            report()
        },
        "scan" => match scan() {
            Ok(rows) => {
                let mut s = report();
                s.push_str(&format!("wifi-intel: {} networks\n", rows.len()));
                for r in rows.iter().take(20) {
                    let sec = ["open", "WEP", "WPA", "WPA2", "WPA3"][r.secure.min(4) as usize];
                    s.push_str(&format!("  {:<32} {:>4} dBm ch {:<3} {sec}\n", r.ssid, r.rssi, r.channel));
                }
                s
            },
            Err(e) => {
                let mut s = report();
                s.push_str(&format!("wifi-intel: scan failed: {e}\n"));
                s
            },
        },
        "regs" => match dev() {
            Some(d) => {
                let mut s = format!("wifi-intel: {}\n", d.regs_line());
                s.push_str(&format!(
                    "  HW_REV={:08x} RF_ID={:08x} CNVI={:08x} {}\n",
                    d.r32(CSR_HW_REV),
                    d.r32(CSR_HW_RF_ID),
                    d.cnvi_mbox,
                    d.boot_diag()
                ));
                if up() {
                    s.push_str(&format!("  {}\n", d.fw_error_line()));
                }
                s
            },
            None => String::from("wifi-intel: no device\n"),
        },
        "stop" => match dev() {
            Some(d) => {
                d.link = None;
                d.configured = false;
                d.stop_device();
                for i in 1..6 {
                    set_stage(i, St::NotRun, String::from("stopped"));
                }
                String::from("wifi-intel: device stopped (wifi-intel up starts it again)\n")
            },
            None => String::from("wifi-intel: no device\n"),
        },
        _ => String::from("usage: wifi-intel [status | up | scan | regs | stop]\n"),
    }
}
