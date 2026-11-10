# Radio firmware baked into the kernel

These binaries are loaded into the CYW43455 Wi-Fi/Bluetooth chip of the
Raspberry Pi 5 by `src/arch/aarch64/wifi.rs` (include_bytes!). They are the
same files Raspberry Pi OS ships, taken from the `bookworm` branch of
https://github.com/RPi-Distro/firmware-nonfree (debian/config/brcm80211/):

| file | origin | what |
| --- | --- | --- |
| `brcmfmac43455-sdio.bin` | `cypress/cyfmac43455-sdio-standard.bin` | firmware 7.45.265 (28bca26 CY), 2023-08-29 |
| `brcmfmac43455-sdio.clm_blob` | `cypress/cyfmac43455-sdio.clm_blob` | regulatory (CLM) table |
| `brcmfmac43455-sdio.txt` | `brcm/brcmfmac43455-sdio.txt` | NVRAM (board parameters) |

The firmware and CLM blob are Cypress/Infineon binaries redistributed under
the `LICENCE.cypress` terms of the linux-firmware tree (redistribution of
unmodified binaries allowed; no reverse engineering). The NVRAM text is from
Raspberry Pi. They are not part of the Ling sources and are not covered by
Ling's licence.

## Intel Wi-Fi 6 AX201 (x86-64)

`iwlwifi-QuZ-a0-hr-b0-72.ucode` (firmware 72.daa05125.0, 1 370 356 bytes,
SHA-256 3238e87e9d87b8ad5a9f6c8640c9578f4121c3895413d3a104add5d5267a0c07) is
loaded into the Intel Wi-Fi 6 AX201 -- the Tiger Lake PCH CNVi MAC (PCI
8086:43F0 / 8086:A0F0) whose CSR_HW_REV reads 0x351 ("QuZ"), with the HR RF
module -- by `src/drivers/iwlwifi.rs` (include_bytes!). It is the unmodified
file from linux-firmware (`intel/iwlwifi/`, fetched from the
gitlab.com/kernel-firmware/linux-firmware mirror), the one Linux 6.1's
iwlwifi loads on that machine. It is Intel's redistributable binary,
licensed under `LICENCE.iwlwifi_firmware` (copied here from linux-firmware's
`LICENSES/`): redistribution of the unmodified binary is allowed with that
notice; no reverse engineering. It is not part of the Ling sources.
