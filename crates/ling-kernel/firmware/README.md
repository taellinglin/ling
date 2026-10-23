# Radio firmware baked into the Raspberry Pi kernel

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
