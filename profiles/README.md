# Board facts

The hardware facts for every board in the implementation plan's [Bench](../docs/implementation-plan.md#bench), recorded in step 0.1. Later steps read them from here: 0.4 links against the app start, 0.5 finds boards by USB serial number, 1.9 builds its hard-coded board table from the wiring, and the board presets are transcribed from it.

How to read it:

- Wiring is written as `[board]` keys from the design plan's [Board semantics](../docs/mesh_virtual_machine_5ae0e741.plan.md#board-semantics). Keys the design plan does not name yet (`radio.fem.*`, `gps.*`, `display.*`) are notes for later steps, not new names.
- **Measured** means read from the board on the bench on 2026-11-01. Everything else comes from documents, with a source on each line.
- When sources disagree, the schematic wins over a datasheet, and Meshtastic's variant file breaks a remaining tie. Each conflict is listed in its board's section.
- **Inferred** marks a fact no source states directly. **Unconfirmed** marks a planned board that is not on the bench yet.
- `radio.max_dbm` is the SX1262 power setting, chosen so the output stays within US FCC 15.247 at 915 MHz (30 dBm conducted, assuming an antenna of 6 dBi or less).
- An app-mode USB VID:PID belongs to whatever firmware is running, not to the board. MVM sets its own. The values below are what each board ran when measured.

## Summary

| Id | MCU | USB path | Measured VID:PID | USB serial | Bootloader | SoftDevice → app start | Flash | PSRAM | SWD |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `XN` | nRF52840 | native | `239a:810b` (app), `2886:0045` (UF2) | `22EF6FE969F6B622` | UF2 Bootloader 0.6.1 | **S140 7.3.0** → `0x27000` | 1 MB + 2 MB QSPI (P25Q16H) | none | pads on the back |
| `RAK` | nRF52840 | native | `239a:0029` (UF2) | `C596982503BC463A` | UF2 Bootloader 0.4.3 | S140 6.1.1 → `0x26000` | 1 MB | none | J2 pads on the core |
| `RSD` | nRF52840 | native | `239a:0029` (UF2) | `DFA755CFFD79B76A` | UF2 Bootloader 0.4.3 | S140 6.1.1 → `0x26000` | 1 MB | none | J2 pads on the core |
| `TAG` | nRF52840 | native (magnetic pogo) | `239a:0029` (UF2) | `B2A0A043890EBCBA` | **EnvyBoot 0.9.2-OTAFIX2.2** | S140 6.1.1 → `0x26000` | 1 MB | none | unknown (sealed) |
| `T96` | nRF52840 | native | `239a:8029` (app), `239a:0071` (UF2) | `BE65C5A3A134268E` | **EnvyBoot 0.9.2-ev1** | S140 6.1.1 → `0x26000` | 1 MB (QSPI footprint not fitted) | none | header P2 pins 9, 10 |
| `TDK` | ESP32-S3 rev v0.2 | USB-Serial-JTAG | `303a:1001` | `D0:CF:13:02:09:34` (MAC) | ESP32-S3 ROM | n/a | 16 MB | 8 MB octal, in package | USB-JTAG |
| `HV3` | ESP32-S3 rev v0.2 | CP2102 bridge to UART0 | `10c4:ea60` | **`0001` (not unique)** | ESP32-S3 ROM | n/a | 8 MB in package | none | USB-JTAG not wired |
| `HV4` | ESP32-S3 (S3R2) | USB-Serial-JTAG (unconfirmed) | `303a:1001` (unconfirmed) | MAC (unconfirmed) | ESP32-S3 ROM | n/a | 16 MB (W25Q128) | 2 MB quad, in package | USB-JTAG |
| `XS3` | ESP32-S3 (S3R8) | USB-Serial-JTAG (unconfirmed) | `303a:1001` (unconfirmed) | MAC (unconfirmed) | ESP32-S3 ROM | n/a | 8 MB | 8 MB octal, in package | USB-JTAG |

The app start follows from the SoftDevice version: S140 6.1.1 puts the app at `0x26000`, S140 7.3.0 at `0x27000`. The Adafruit layout puts the bootloader at `0xF4000`, the MBR parameters at `0xFE000`, and the bootloader settings at `0xFF000`. The RAM start is measured in 0.6.

| Id | Radio | Power rails to drive | Battery ADC | GNSS UART | I2C | SD | Display |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `XN` | SX1262 (Wio-SX1262), 22 dBm | none | yes | no (optional L76K) | pads only | no | no |
| `RAK` | SX1262 (RAK4630), 22 dBm | 3V3_S, RF-switch power | yes | no | header J6 | no | no |
| `RSD` | SX1262 (RAK4630), 22 dBm | 3V3_S, RF-switch power | yes | no | header J12 | RAK15002, own SPI bus | no |
| `TAG` | SX1262 (RAK4630), 22 dBm | 3V3_EN, RF-switch power | yes | AT6558R | internal (accelerometer) | no | no |
| `T96` | SX1262 + KCT8103L PA, 28 dBm out | Vext, VGNSS, VFEM, FEM enable | yes | UC6580 | no dedicated bus | no | ST7735S TFT |
| `TDK` | SX1262 (HPD16A), 22 dBm | peripheral power | yes | MIA-M10Q or L76K | internal (keyboard, touch) | yes, **shares the radio's SPI bus** | ST7789 TFT |
| `HV3` | SX1262, 22 dBm | Vext | yes | no | OLED bus | no | SSD1306 OLED (I2C) |
| `HV4` | SX1262 + FEM (by revision), 28 dBm out | Vext, VFEM, FEM enable | yes | connector, no module | OLED bus | no | SSD1315 OLED (I2C) |
| `XS3` | SX1262 (Wio-SX1262), 22 dBm | none | no | no | pads only | no | no |

## Sources

- **Meshtastic** variant files are at `meshtastic/firmware` `develop`, commit `4c89b69` (2026-11-01), under `variants/`: [nrf52840/seeed_xiao_nrf52840_kit](https://github.com/meshtastic/firmware/blob/develop/variants/nrf52840/seeed_xiao_nrf52840_kit/variant.h), [nrf52840/rak4631](https://github.com/meshtastic/firmware/blob/develop/variants/nrf52840/rak4631/variant.h), [nrf52840/rak_wismeshtag](https://github.com/meshtastic/firmware/blob/develop/variants/nrf52840/rak_wismeshtag/variant.h), [nrf52840/heltec_mesh_node_t096](https://github.com/meshtastic/firmware/blob/develop/variants/nrf52840/heltec_mesh_node_t096/variant.h), [esp32s3/t-deck](https://github.com/meshtastic/firmware/blob/develop/variants/esp32s3/t-deck/variant.h), [esp32s3/heltec_v3](https://github.com/meshtastic/firmware/blob/develop/variants/esp32s3/heltec_v3/variant.h), [esp32s3/heltec_v4](https://github.com/meshtastic/firmware/blob/develop/variants/esp32s3/heltec_v4/variant.h), [esp32s3/seeed_xiao_s3](https://github.com/meshtastic/firmware/blob/develop/variants/esp32s3/seeed_xiao_s3/variant.h). The FEM logic is in [src/mesh/LoRaFEMInterface.cpp](https://github.com/meshtastic/firmware/blob/develop/src/mesh/LoRaFEMInterface.cpp).
- Each board section names its vendor schematics and datasheets.

---

## `XN`: Seeed XIAO nRF52840 with Wio-SX1262

The Seeed "XIAO nRF52840 & Wio-SX1262" kit. **The bench board is the XIAO nRF52840 Sense** (measured). The plain, Sense, and Plus XIAO boards map the same nRF pins for everything here. They differ in their USB product id, the Plus's extra pads, and the Sense's IMU and microphone.

Sources:
- [XIAO nRF52840 schematic V1.2](https://files.seeedstudio.com/wiki/XIAO-BLE/Res/260828_XIAO_nRF52840.pdf) (XSCH)
- [Wio-SX1262 for XIAO schematic V1.0](https://files.seeedstudio.com/products/SenseCAP/Wio_SX1262/Wio-SX1262%20for%20XIAO%20V1.0_SCH.pdf) (WSCH)
- [Wio-SX1262 module datasheet](https://files.seeedstudio.com/products/SenseCAP/Wio_SX1262/Wio-SX1262-N_Module_Datasheet.pdf) (WDS)
- [Seeed wiki](https://wiki.seeedstudio.com/XIAO_BLE/)
- [Adafruit bootloader board files](https://github.com/adafruit/Adafruit_nRF52_Bootloader/tree/master/src/boards/xiao_nrf52840_ble) (ABL)
- Meshtastic (MT)

USB and boot:
- Measured: `239a:810b`, product string `XIAO_BOOT`, serial `22EF6FE969F6B622`, running its existing app firmware (Meshtastic's id, per its board file). In the UF2 bootloader: `2886:0045`, product string `XIAO nRF52840 Sense`, the same serial.
- Measured `INFO_UF2.TXT`: `UF2 Bootloader 0.6.1`, Model `Seeed XIAO nRF52840`, Board-ID `Seeed_XIAO_nRF52840_Sense`, dated Nov 12 2021, `SoftDevice: S140 version 7.3.0`. So the app starts at `0x27000`.
- **This is the only bench board on S140 7.3.0.** Every other nRF board runs 6.1.1. That is 0.6's decision.
- The bootloader samples P0.03 (SX1262 DIO1) as its OTA pin. That is harmless, because OTA also needs the DFU button, which is the reset line (ABL).

```ini
power_on = (none)               ; no switched rails on the XIAO or the adapter — XSCH, WSCH
clock.lfxo = true               ; X1 32.768 kHz on P0.00/P0.01 — XSCH
power.dcdc = true               ; L3 10 µH on DCC — XSCH
radio.chip = sx1262             ; WDS
radio.sck = P1.13               ; D8 — WSCH J2-6, XSCH; MT agrees
radio.mosi = P1.15              ; D10 — WSCH J2-4, XSCH; MT agrees
radio.miso = P1.14              ; D9 — WSCH J2-5, XSCH; MT agrees
radio.nss = P0.04               ; D4 — WSCH J1-5, XSCH; MT agrees
radio.reset = P0.28             ; D2 — WSCH J1-3, XSCH; MT agrees
radio.busy = P0.29              ; D3 — WSCH J1-4, XSCH; MT agrees
radio.dio1 = P0.03              ; D1 — WSCH J1-2, XSCH; MT agrees
radio.rfswitch = dio2           ; DIO2 high = TX — WDS; MT
radio.rxen = P0.05              ; D5, high = RX, 10 kΩ pull-up on the adapter — WSCH, WDS; MT SX126X_RXEN
radio.tcxo_mv = 1800            ; TCXO on DIO3, 1.7–3.3 V allowed — WDS; value from MT
radio.regulator = dcdc          ; WDS "utilizes the DC-DC design"
radio.max_dbm = 22              ; WDS, no PA
led.red = P0.26                 ; active low, common-anode RGB LED — XSCH
led.green = P0.30               ; active low — XSCH
led.blue = P0.06                ; active low — XSCH
button.user = P0.02             ; D0, active low, 10 kΩ pull-up on the adapter (K1) — WSCH
button.reset = P0.18            ; nRESET — XSCH
qspi.flash.sck = P0.21          ; P25Q16H, 2 MB — XSCH, Seeed wiki
qspi.flash.csn = P0.25          ; XSCH
qspi.flash.io0 = P0.20          ; XSCH
qspi.flash.io1 = P0.24          ; XSCH
qspi.flash.io2 = P0.22          ; XSCH
qspi.flash.io3 = P0.23          ; XSCH
adc.battery = P0.31             ; AIN7; read with P0.14 driven low, never high — XSCH, Seeed wiki
adc.battery.divider = 2.961     ; 1 MΩ over 510 kΩ — XSCH; MT rounds to 3
```

Also:
- The charge LED is red, driven by the charger's `~CHG` on P0.17, not by the MCU (XSCH).
- P0.13 sets the charge current: low gives about 100 mA, high-Z about 50 mA (XSCH).
- I2C: there is no header. The Sense's LSM6DS3TR IMU is on SDA P0.07, SCL P0.27, powered by P1.08 high (Sense v1.1 schematic, MT), so 1.18 has a device to talk to. Meshtastic's default build puts another bus on the NFC pads (SDA P0.09, SCL P0.10).
- GNSS: none in the kit. The optional L76K uses TX P1.11, RX P1.12, 9600 baud, and standby on P0.02, which is the same pin as the user button (MT).
- SWD: pads on the back. TP1 GND, TP2 RESET, TP3 SWDCLK, TP5 SWDIO (XSCH).

Conflicts:
- **LED polarity.** XSCH (common anode) and MT say active low. The Seeed Arduino BSP says active high. The schematic wins: active low.
- **QSPI flash.** This board is a Sense, and the Sense v1.1 schematic marks the flash chip "DNP". The plain V1.2 schematic, the wiki (which lists 2 MB flash for the Sense too), and the BSP show it fitted. Recorded as fitted; 1.22 confirms it, and if it is absent, the XIAO's QSPI steps (1.22, 5.15, 5.16) need another board.

## `RAK`: RAK4631 on RAK19003

The RAK19003 Mini Base has slots C and D only, with no IO slot. Its headers J6 and J7 carry BOOT, I2C1, and UART1. The core facts below do not depend on the base. The facts marked *[base]* come from the RAK19003 datasheet, whose LED, reset, and 3V3_S tables match the RAK19007's. The RAK19003 schematic was not read, so the battery divider is taken from the RAK19007 and is unconfirmed.

Sources:
- [RAK4631 datasheet](https://docs.rakwireless.com/product-categories/wisblock/rak4631/datasheet/)
- RAK4630 module schematic, [page 1 (SX1262)](https://github.com/RAKWireless/rakwireless-docs/blob/master/docs/.vuepress/public/assets/images/wisduo/rak4630-module/datasheet/schematic-1.png) and page 2 (nRF52840) (MSCH)
- [RAK19003 datasheet](https://github.com/RAKWireless/rakwireless-docs/blob/master/docs/Product-Categories/WisBlock/RAK19003/Datasheet/README.md) (D19003)
- [RAK19007 schematics](https://github.com/RAKWireless/rakwireless-docs/tree/master/docs/.vuepress/public/assets/images/wisblock/rak19007/datasheet) (BSCH), for the divider and LED circuits
- [RAK bootloader](https://github.com/RAKWireless/WisBlock/tree/master/bootloader/RAK4630)
- [RAK Arduino BSP](https://github.com/RAKWireless/RAK-nRF52-Arduino)
- Meshtastic (MT)

USB and boot:
- Measured: `239a:0029` in the UF2 bootloader, serial `C596982503BC463A`.
- Measured `INFO_UF2.TXT`: `UF2 Bootloader 0.4.3`, Board-ID `WisBlock-RAK4631-Board`, dated May 20 2023, `SoftDevice: S140 6.1.1`. So the app starts at `0x26000`.
- The bootloader's CDC-only DFU id is `239a:002a` (RAK bootloader `board.h`).
- The bootloader leaves the nRF DC/DC off, so the kernel enables it.

```ini
power_on = P1.02:high, P1.05:high   ; 3V3_S enable (WB_IO2) [base] — D19003, MT PIN_3V3_EN; RF-switch power — RAK4631 datasheet, MT SX126X_POWER_EN
clock.lfxo = true               ; X2 32.768 kHz on P0.00/P0.01 — MSCH p2; MT USE_LFXO
power.dcdc = true               ; inductors on DCC and DCCH — MSCH p2 (fitting inferred: the module is shielded)
radio.chip = sx1262             ; RAK4631 datasheet
radio.sck = P1.11               ; RAK4631 datasheet; MT agrees
radio.mosi = P1.12              ; RAK4631 datasheet; MT agrees
radio.miso = P1.13              ; RAK4631 datasheet; MT agrees
radio.nss = P1.10               ; RAK4631 datasheet; MT agrees
radio.reset = P1.06             ; RAK4631 datasheet; MT agrees
radio.busy = P1.14              ; RAK4631 datasheet; MT agrees
radio.dio1 = P1.15              ; RAK4631 datasheet; MT agrees
radio.rfswitch = dio2           ; DIO2 → PE4259 switch — MSCH p1
radio.tcxo_mv = 1800            ; MT; MSCH shows the TCXO on DIO3 with no voltage (see Conflicts)
radio.regulator = dcdc          ; RAK4631 datasheet; L5 15 µH on DCC_SW — MSCH p1
radio.max_dbm = 22              ; RAK4631 datasheet, no PA in MSCH
led.green = P1.03               ; active high [base] — D19003, BSCH; MT
led.blue = P1.04                ; active high [base] — D19003, BSCH; MT
button.reset = nRESET           ; [base] — D19003; no user button
i2c.sensors.sda = P0.13         ; I2C1, also on header J6 [base] — D19003; BSP
i2c.sensors.scl = P0.14         ; [base] — D19003; BSP
adc.battery = P0.05             ; AIN3 [base] — MT BATTERY_PIN; divider circuit as on the RAK19007 (unconfirmed)
adc.battery.divider = 1.667     ; 1 MΩ over 1.5 MΩ on the RAK19007 — BSCH (unconfirmed on the RAK19003; 1.10 measures it); MT uses 1.73
power.sensors = P1.02           ; 3V3_S feeds the sensor slots [base] — D19003
```

Also:
- P1.07 sits on the SX1262's DIO2 net. Leave it an input, never drive it (MSCH p1).
- The second I2C bus (SDA P0.24, SCL P0.25) goes to the IO slot, which the RAK19003 does not have (BSP).
- UART1 (TX P0.16, RX P0.15) is free on header J7 (D19003, BSP).
- The red LED is the charger's, not the MCU's.
- The QSPI flash footprint U2 is on the schematic but not fitted ([RAK forum](https://forum.rakwireless.com/t/spi-flash-chip-u2-rak4630/6044/2)).
- SWD: the J2 1×5 pads on the core (3V3, SWDIO, SWCLK, GND, RST). They are not routed to the base's headers.

Conflicts:
- **RF switch pin.** The RAK4631 datasheet table says P1.05 is "SX1262 RF switch power", while its text says P1.07 drives the antenna switch. MSCH settles it: P1.05 powers the switch, and P1.07 is on the DIO2 net.
- **TCXO voltage.** MT says 1.8 V. RAK's SX126x-Arduino library keeps its 3.3 V default. The schematic is silent, so Meshtastic breaks the tie: 1800. 1.12's probe confirms it.
- **External flash.** The MT and BSP variants declare an IS25LP080D. RAK says U2 is not fitted. Recorded as none.
- **UART1 direction.** One RAK pin-map file swaps TX and RX. The datasheet, the BSP, and MT agree on TX P0.16.
- **LED1.** The RAK19003 and RAK19007 pin tables call core pin 14 (LED1) a charge LED. The RAK19007 schematic shows it driving the green user LED, and the RAK19003 datasheet's LED section says green and blue are MCU-controlled while red is the charger's. The schematic wins.

## `RSD`: RAK4631 on RAK19007 with RAK15002 microSD

Everything in `RAK` applies, with the RAK19007 as the base, plus the SD card. The SD card is on the core's general SPI bus, apart from the radio's.

Sources:
- [RAK15002 schematic](https://github.com/RAKWireless/rakwireless-docs/blob/master/docs/.vuepress/public/assets/images/wisblock/rak15002/datasheet/image-20210315105240003.png) (SDSCH), with its pinout drawing
- RAK19007 slot schematic (BSCH)
- [RAK4631 core pin map](https://github.com/RAKWireless/rakwireless-docs/blob/master/docs/.vuepress/includes/pin-mapper/modules/wiscore/rak4631.js)

USB and boot:
- Measured: `239a:0029` in the UF2 bootloader, serial `DFA755CFFD79B76A`.
- Measured `INFO_UF2.TXT`: `UF2 Bootloader 0.4.3`, the same file as `RAK`, with `SoftDevice: S140 6.1.1`. So the app starts at `0x26000`.

```ini
spi.sd.sck = P0.03              ; IO slot pin 26 SPI_CLK — SDSCH, BSCH, pin map
spi.sd.mosi = P0.30             ; IO slot pin 28 SPI_MOSI — SDSCH, BSCH, pin map
spi.sd.miso = P0.29             ; IO slot pin 27 SPI_MISO — SDSCH, BSCH, pin map
spi.sd.cs = P0.26               ; IO slot pin 25 SPI_CS, 100 kΩ pull-up on the module — SDSCH
spi.sd.cd = P0.10               ; IO6, IO slot pin 38, active low, 100 kΩ pull-up — SDSCH; the RAK15002 datasheet names IO6 as card detect
```

Also:
- The SD card is powered from the always-on 3V3 (IO slot pin 5), not 3V3_S (SDSCH).
- Card detect is on P0.10, an NFC pin. It works as a GPIO only once UICR `NFCPINS` is cleared, which 0.5's bench script does.
- The SD pins are the ones the unfitted QSPI footprint would use, so they do not clash.

Conflicts:
- **The RAK15002 datasheet contradicts itself.** Its note says "SPI related pins, IO6", its schematic caption says "Only I2C1 related pins, IO6", and its pin table calls pin 38 "CD". The schematic and the pinout drawing settle it: SPI on IO slot pins 25–28, and card detect on IO6 (pin 38).

## `TAG`: RAK WisMesh Tag

There is no published schematic or pin table, so the pins are from Meshtastic only. The radio pins match a RAK4630 module, so the RAK4630 facts in `RAK` (the RF-switch power, DC/DC, crystal) are inferred to apply.

Sources:
- [WisMesh Tag quick start](https://docs.rakwireless.com/product-categories/meshtastic/wismesh-tag/quickstart)
- Meshtastic `rak_wismeshtag` (MT)

USB and boot:
- Measured: `239a:0029` in the UF2 bootloader, product string `WisMesh Tag`, serial `B2A0A043890EBCBA`.
- Measured `INFO_UF2.TXT`: **`EnvyBoot 0.9.2-OTAFIX2.2-BP1.3-16-g73daa43`**, Model `WisMesh Tag`, Board-ID `WisMesh-Tag`, dated Aug 23 2026, `SoftDevice: S140 6.1.1`. So the app starts at `0x26000`.
- This is not the stock Adafruit bootloader.

```ini
power_on = P1.02:high, P1.05:high   ; 3V3_EN, also powers the GNSS — MT PIN_3V3_EN; RF-switch power — MT SX126X_POWER_EN
clock.lfxo = true               ; MT USE_LFXO
power.dcdc = true               ; inferred from the RAK4630 module
radio.chip = sx1262             ; vendor quick start
radio.sck = P1.11               ; MT
radio.mosi = P1.12              ; MT
radio.miso = P1.13              ; MT
radio.nss = P1.10               ; MT
radio.reset = P1.06             ; MT
radio.busy = P1.14              ; MT
radio.dio1 = P1.15              ; MT
radio.rfswitch = dio2           ; MT
radio.tcxo_mv = 1800            ; MT
radio.regulator = dcdc          ; inferred from the RAK4630 module
radio.max_dbm = 22              ; inferred: no PA anywhere in the documents
led.green = P1.03               ; active high, status — MT; vendor quick start
led.blue = P1.04                ; active high, messages — MT; vendor quick start
button.user = P0.09             ; front button, active low, needs a pull-up — MT only
button.reset = nRESET           ; back button, double press enters DFU — vendor quick start
i2c.sensors.sda = P0.25         ; MT (the accelerometer's bus, inferred)
i2c.sensors.scl = P0.24         ; MT
uart.gps.tx = P0.16             ; MCU → GNSS — MT
uart.gps.rx = P0.15             ; GNSS → MCU — MT
uart.gps.baud = 9600            ; inferred: the AT6558 default
adc.battery = P0.05             ; AIN3 — MT
adc.battery.divider = 1.667     ; 1 MΩ over 1.5 MΩ, per MT's comment citing R22/R24; MT uses 1.73
power.gps = P1.02               ; shared with 3V3_EN — MT
```

Also:
- GNSS chip: AT6558R, PPS on P0.17. No standby pin is defined (MT).
- Accelerometer: LIS2DH. Its address and INT pin are unknown.
- Buzzer: P0.21 (MT only).
- `button.user` on P0.09 is an NFC pin. It works as a GPIO only once UICR `NFCPINS` is cleared (0.5).
- SWD: unknown, because the case is sealed and the pogo pins carry only USB.

Conflicts:
- **I2C pins.** On a RAK4631 core, I2C2 is SDA P0.24, SCL P0.25. MT's Tag variant has them the other way round. There is no Tag schematic, so MT stands. 1.18 confirms it.

## `T96`: Heltec Mesh Node T096

Sources:
- [T096 schematic V0.2](https://resource.heltec.cn/download/Mesh_Node_T096/Schematic/Mesh_Node_T096_V0.2.pdf) (HSCH)
- [T096 datasheet 1.0](https://resource.heltec.cn/download/Mesh_Node_T096/Datasheet/Mesh%20Node%20T096.pdf) (HDS)
- [Heltec nRF52 BSP and bootloader](https://github.com/HelTecAutomation/Heltec_nRF52) (HBSP)
- Meshtastic `heltec_mesh_node_t096` (MT)

USB and boot:
- Measured: `239a:8029`, product string `HT_n5262G`, serial `BE65C5A3A134268E`, running its existing app firmware.
- Measured in the UF2 bootloader: `239a:0071`, the same serial.
- Measured `INFO_UF2.TXT`: **`EnvyBoot 0.9.2-ev1`**, Model and Board-ID `HT-n5262G`, dated Aug 30 2026, `SoftDevice: S140 6.1.1`. So the app starts at `0x26000`.
- This is not Heltec's published bootloader (`HT-n5262_bootloader-0.9.0_s140_6.1.1`), though the version number matches.
- The Heltec BSP keeps a LoRaWAN licence at `0xF3000`–`0xF3FFF`. Erasing that page loses it.

```ini
power_on = P0.26:high, P0.06:low, P0.30:high, P0.12:high
                                ; P0.26 Vext: LCD and backlight, the GNSS rail's source, header 3V3 — HSCH, HDS, MT
                                ; P0.06 VGNSS, active low, needs Vext on — HSCH, MT
                                ; P0.30 VFEM, the FEM's LDO, pulled up so it is on by default — HSCH; MT LORA_PA_POWER
                                ; P0.12 PA_CSD, the KCT8103L enable — HSCH, MT
clock.lfxo = true               ; X2 32.768 kHz — HSCH; MT USE_LFXO
power.dcdc = true               ; L2 10 µH + L1 15 nH on DCC — HSCH
radio.chip = sx1262             ; HSCH
radio.sck = P1.08               ; HSCH; MT agrees
radio.mosi = P0.11              ; HSCH; MT agrees
radio.miso = P0.14              ; HSCH; MT agrees
radio.nss = P0.05               ; 10 kΩ pull-up — HSCH; MT agrees
radio.reset = P0.16             ; HSCH; MT agrees
radio.busy = P0.19              ; HSCH; MT agrees
radio.dio1 = P0.21              ; HSCH; MT agrees
radio.rfswitch = dio2           ; DIO2 drives the KCT8103L CPS pin — HSCH net PA_CPS; MT
radio.txen = P1.09              ; KCT8103L CTX: high in TX, low in RX (LNA on) — MT, LoRaFEMInterface.cpp
radio.tcxo_mv = 1800            ; TCXO on DIO3 — HSCH; value from MT
radio.regulator = dcdc          ; L28 15 µH on DCC_SW — HSCH
radio.max_dbm = 22              ; reasoning below
led.white = P0.28               ; active high (not a recommended name yet) — HSCH, HDS; white LED seen on the bench board
button.user = P1.10             ; active low, 10 kΩ pull-up — HSCH, HDS, MT
button.reset = P0.18            ; S1 — HSCH
spi.display.sck = P0.20         ; HSCH, MT
spi.display.mosi = P0.17        ; HSCH, MT
spi.display.cs = P0.22          ; HSCH, MT
spi.display.dc = P0.15          ; HSCH (RS), MT
spi.display.reset = P0.13       ; HSCH, MT
spi.display.backlight = P1.12   ; active low (PMOS Q1) — HSCH, MT
uart.gps.tx = P0.25             ; MCU → UC6580 — HSCH, MT
uart.gps.rx = P0.23             ; UC6580 → MCU — HSCH, MT
uart.gps.baud = 115200          ; MT, HBSP
adc.battery = P0.03             ; AIN1; read with P1.15 high — HSCH, HDS, MT
adc.battery.divider = 4.9       ; 390 kΩ over 100 kΩ — HSCH; MT uses 4.916 (calibrated)
power.display = P0.26           ; Vext — HSCH
power.gps = P0.06               ; active low, with P0.26 high — HSCH, MT
power.sensors = P0.26           ; Vext feeds header P2 pins 4, 6 — HSCH
```

`radio.max_dbm`:
- The KCT8103L saturates at about 28 dBm out. Meshtastic's T096 gain table reaches that plateau from an SX1262 setting of 15 upward.
- In the US, Meshtastic computes 23 for this board and clamps it to the SX1262's 22.
- Heltec rates the output at 28 ± 1 dBm at 902–928 MHz (HDS). So a setting of 22 gives at most about 29 dBm conducted, which is under 30.
- A setting of 15 reaches the same output with less PA overdrive and current. That is a later tuning choice, not a limit.
- A 21 dB pi attenuator (R37–R39) sits between the SX1262 and the PA (inferred from its values).
- The KCT8103L datasheet was not found, so the gain figures come from Meshtastic's table only.

Also:
- No QSPI flash is fitted: footprint U8 is marked NC. Its pads (CS P1.01, CLK P1.05, IO0 P1.03, IO1 P1.00, IO2 P0.24, IO3 P1.07) also go to headers P2 and P3.
- GNSS: reset on P1.14 (active low, more than 100 ms), PPS on P1.11 (HSCH, MT).
- Display: ST7735S, 80×160, x offset 24 (HDS, MT).
- I2C: the schematic has no dedicated bus. MT and HBSP use SDA P0.07, SCL P0.08 (header P2 pins 7, 8).
- SWD: header P2, pin 9 SWDCLK and pin 10 SWDIO, with GND on pins 1, 3, 5 (HSCH, HDS).

Conflicts:
- **LED colour.** HSCH V0.2 and HDS say white, and the bench board has a white LED (seen 2026-11-01). MT says green "confirmed on 1.0 board". Whether the board also has a green LED is unknown; 1.9 blinks P0.28 and settles which LED it drives.
- **Schematic revision.** The schematic is V0.2, and MT mentions a 1.0 board. There may be changes in between.
- **I2C.** The schematic labels none. MT and HBSP use P0.07/P0.08 anyway.
- **USB ids.** MT's board file has ids copied from the T114. HBSP and the bootloader image have `239a:8071` (app) and `239a:0071` (bootloader). The measured `239a:8029` is the running firmware's.

## `TDK`: LilyGO T-Deck Plus

**The SD card, the radio, and the display share one SPI bus** (SCK GPIO40, MOSI GPIO41, MISO GPIO38), with their own chip selects. This is the 1.23 case.

Sources:
- [LilyGO T-Deck examples](https://github.com/Xinyuan-LilyGO/T-Deck/blob/master/examples/UnitTest/utilities.h) (LG)
- [T-Deck schematic](https://github.com/Xinyuan-LilyGO/T-Deck/blob/master/schematic/schematic.pdf), dated 2023-05-31: the original T-Deck, because there is no Plus mainboard schematic (LGSCH)
- [Zephyr t_deck board](https://github.com/zephyrproject-rtos/zephyr/tree/main/boards/lilygo/t_deck) (Z)
- Meshtastic `t-deck` (MT)

LG, MT, and Z agree on every pin below.

USB and chip:
- Measured: `303a:1001` USB-Serial-JTAG, serial `D0:CF:13:02:09:34`, which is the MAC.
- Measured with `espflash board-info` and `esptool chip-id`: ESP32-S3 (QFN56) revision v0.2, 40 MHz crystal, 16 MB flash, "Embedded PSRAM 8MB (AP_3v3)".
- Module: ESP32-S3-WROOM-1 N16R8 (LGSCH, Z). The R8 part's in-package PSRAM is octal, on GPIO33–37. Flash is on GPIO26–32.

```ini
power_on = GPIO10:high          ; peripheral power — LG BOARD_POWERON; MT; Z
radio.chip = sx1262             ; HPD16A module — LGSCH; MT
radio.sck = GPIO40              ; shared bus — LG; MT; Z
radio.mosi = GPIO41             ; LG; MT
radio.miso = GPIO38             ; LG; MT
radio.nss = GPIO9               ; LG; MT; Z
radio.reset = GPIO17            ; LG; LGSCH; Z
radio.busy = GPIO13             ; LG; LGSCH; Z
radio.dio1 = GPIO45             ; LG; LGSCH; Z
radio.rfswitch = dio2           ; MT; Z
radio.tcxo_mv = 1800            ; MT; Z
radio.regulator = dcdc          ; inferred: no source states it (the RadioLib and MT default)
radio.max_dbm = 22              ; LG RADIO_TX_POWER, no PA
spi.sd.sck = GPIO40             ; shared with the radio — LG; MT
spi.sd.mosi = GPIO41            ; LG; MT
spi.sd.miso = GPIO38            ; LG; MT
spi.sd.cs = GPIO39              ; LG; Z
spi.display.sck = GPIO40        ; shared with the radio — LG; MT
spi.display.mosi = GPIO41       ; LG; MT
spi.display.cs = GPIO12         ; LG; Z
spi.display.dc = GPIO11         ; LG; Z
spi.display.backlight = GPIO42  ; AW9364 driver — LG; LGSCH; Z
i2c.sensors.sda = GPIO18        ; keyboard 0x55, touch GT911 0x5D — LG; Z
i2c.sensors.scl = GPIO8         ; LG; Z
button.user = GPIO0             ; trackball press, also BOOT, active low — LG; Z
uart.gps.tx = GPIO43            ; MCU → GNSS — LG; Z
uart.gps.rx = GPIO44            ; LG; Z
uart.gps.baud = 38400           ; MIA-M10Q; an L76K is 9600 (see Conflicts) — LG GPS example; Z
adc.battery = GPIO4             ; ADC1 channel 3 — LG; MT; Z
adc.battery.divider = 2         ; 100 kΩ over 100 kΩ — Z; MT uses 2.11 (empirical)
power.display = GPIO10          ; LG, MT
power.sensors = GPIO10          ; keyboard controller, trackball, microphone — MT
```

Also:
- Drive all three chip selects high before the first transfer (MT).
- Turn GPIO10 on early in boot; a late power-on can brown out the ESP32 (MT).
- Trackball (plain GPIOs, active low with pull-ups): up GPIO3, down GPIO15, left GPIO1, right GPIO2, press GPIO0 (MT, Z).
- Keyboard INT is GPIO46. Touch INT is GPIO16.
- There is no user LED. The keyboard backlight is set over I2C.
- Audio: I2S amp MAX98357A (WS GPIO5, BCK GPIO7, DOUT GPIO6), and ES7210 microphone (MCLK GPIO48, LRCK GPIO21, SCK GPIO47, DIN GPIO14) (LG).
- GNSS sits on what the original T-Deck used as its Grove port, so the Plus has no free I2C connector.
- Strapping pins in use: GPIO0 (BOOT), GPIO3 (trackball), GPIO45 (radio DIO1, must read low at reset; DIO1 idles low), and GPIO46 (keyboard INT).
- Whether GPIO10 also powers the radio and the GNSS is inferred, not stated.

Conflicts:
- **GNSS chip.** The 2024-03 GPS schematic and Z say MIA-M10Q at 38400. A 2024-08 schematic shows an L76K, and LilyGO's example probes an L76K at 9600 first. Units differ, so firmware should probe both. 1.19 records which this board has.
- **GPIO46.** LG calls it the keyboard interrupt. MT calls it an unused keyboard backlight pin. LG stands.

## `HV3`: Heltec WiFi LoRa 32 V3

Sources:
- [V3.2 schematic](https://resource.heltec.cn/download/WiFi_LoRa_32_V3/WiFi_LoRa_32_V3.2_Schematic_Diagram.pdf) (S32)
- [V3.2 datasheet](https://resource.heltec.cn/download/WiFi_LoRa_32_V3/HTIT-WB32LA_V3.2.pdf) (D32)
- Meshtastic `heltec_v3` (MT)

USB and chip:
- Measured: `10c4:ea60` CP2102 bridge to UART0 (GPIO43/44), serial **`0001`**. That is the CP2102's generic default, not unique: a second V3 would report the same. Flagged for 0.5.
- Measured with `espflash board-info` and `esptool chip-id`: ESP32-S3 (QFN56) revision v0.2, 40 MHz crystal, "Embedded Flash 8MB (GD)", no PSRAM, MAC `3c:0f:02:ed:63:70`. That matches the ESP32-S3FN8 (D32).
- Native USB (GPIO19/20) reaches only the header. The Type-C connects to it only if R29 and R3 are fitted (D32).
- The 32.768 kHz crystal is on GPIO15/16 (S32).

```ini
power_on = GPIO36:low           ; Vext, PMOS — powers the OLED and the "Ve" header pins — D32, S32, MT
radio.chip = sx1262             ; S32, D32
radio.sck = GPIO9               ; MT
radio.mosi = GPIO10             ; MT
radio.miso = GPIO11             ; MT
radio.nss = GPIO8               ; S32, MT
radio.reset = GPIO12            ; S32, MT
radio.busy = GPIO13             ; S32, MT
radio.dio1 = GPIO14             ; S32, MT
radio.rfswitch = dio2           ; S32 (DIO2 → ANT_SW_CTRL), MT
radio.tcxo_mv = 1800            ; MT; S32 shows the TCXO on DIO3
radio.regulator = dcdc          ; 15 µH on DCC_SW — S32 (inferred from the part)
radio.max_dbm = 22              ; no PA; Heltec rates 21 ± 1 dBm — D32
led.status = GPIO35             ; active high (inferred) — D32, MT
button.user = GPIO0             ; PRG, also BOOT, active low — D32, MT
i2c.sensors.sda = GPIO41        ; header pins by convention, no connector — MT, Arduino variant
i2c.sensors.scl = GPIO42        ; same
adc.battery = GPIO1             ; ADC1 channel 0 — D32, MT
adc.battery.divider = 4.9       ; 390 kΩ over 100 kΩ — S32; MT uses 4.9 × 1.045
power.display = GPIO36          ; Vext, active low — D32, MT
```

Also:
- OLED: SSD1306, 128×64, on I2C. SDA GPIO17, SCL GPIO18, reset GPIO21 (S32, D32). The 0x3C address is inferred.
- Battery read enable: GPIO37. Its polarity differs between board revisions. Meshtastic reads the idle level and drives the opposite (see Conflicts).
- GPIO45 picks the flash voltage and must stay low at reset. GPIO3 and GPIO46 are free straps.

Conflicts:
- **Battery formula.** D32 prints the divider upside down. The schematic gives VBAT = VADC × 4.9.
- **ADC_CTRL polarity.** MT's constant says active low, but its V3 code ignores it and drives the inverse of the idle level. The V3.2 schematic text did not settle it, so 1.10 measures it.
- **USB id.** The Arduino and PlatformIO board files list `303a:1001`. The real path is the CP2102, as measured.

## `HV4`: Heltec WiFi LoRa 32 V4 (planned, unconfirmed)

Recorded for the HTIT-WB32LAF (ESP32-S3R2, 28 dBm). **Which SKU and board revision arrives is unknown.** A "V4 R8" (8 MB octal PSRAM) moves Vext to GPIO40, the LED to GPIO46, and the GNSS enable to GPIO42, and the -N SKUs have no PA.

Sources:
- [V4.2 schematic](https://resource.heltec.cn/download/WiFi_LoRa_32_V4/Schematic/WiFi_LoRa_32_V4.2.pdf) (S42)
- [V4.3 schematic](https://resource.heltec.cn/download/WiFi_LoRa_32_V4/Schematic/HTIT-WB32LAF_V4.3.pdf) (S43)
- [V4.3.1 datasheet](https://resource.heltec.cn/download/WiFi_LoRa_32_V4/datasheet/WiFi_LoRa_32_V4.3.1_Datasheet.pdf) (D431)
- Meshtastic `heltec_v4` (MT)

USB and chip:
- Native USB-Serial-JTAG on GPIO19/20, with the CP2102 removed. The id would be `303a:1001`, with the MAC as its serial (Heltec docs, MT).
- ESP32-S3R2 with 2 MB quad PSRAM in the package, and a 16 MB W25Q128 external flash (D431, S43). GPIO26 is taken by the PSRAM (inferred from the part).

```ini
power_on = GPIO36:low, GPIO7:high, GPIO2:high
                                ; GPIO36 Vext, PMOS, powers the OLED and header — S43, MT
                                ; GPIO7 VFEM, the FEM's LDO — D431, S42, S43, MT
                                ; GPIO2 FEM enable (CSD) — D431, S43, MT
radio.chip = sx1262             ; D431, S43
radio.sck = GPIO9               ; S43, MT
radio.mosi = GPIO10             ; MT
radio.miso = GPIO11             ; MT
radio.nss = GPIO8               ; S43, MT
radio.reset = GPIO12            ; MT
radio.busy = GPIO13             ; MT
radio.dio1 = GPIO14             ; MT
radio.rfswitch = dio2           ; MT
radio.txen = GPIO46 (V4.2) or GPIO5 (V4.3)   ; depends on the FEM; see below — S42, S43, MT
radio.tcxo_mv = 1800            ; MT
radio.regulator = dcdc          ; 15 µH on DCC_SW — S42, S43 (inferred from the part)
radio.max_dbm = 22              ; about 28 ± 1 dBm out, under 30 — D431; same reasoning as T96
led.status = GPIO35             ; active high (inferred) — D431, S43, MT
button.user = GPIO0             ; PRG, also BOOT — D431, MT
i2c.sensors.sda = GPIO4         ; by convention, no connector — MT
i2c.sensors.scl = GPIO3         ; MT
uart.gps.tx = GPIO38            ; MCU → GNSS — D431, S43, MT
uart.gps.rx = GPIO39            ; MT
uart.gps.baud = 9600            ; CM121 default — MT
adc.battery = GPIO1             ; read with GPIO37 high — D431, MT
adc.battery.divider = 4.9       ; 390 kΩ over 100 kΩ — S42; MT uses 4.9 × 1.045
power.display = GPIO36          ; D431, MT
power.gps = GPIO34:low          ; VGNSS — D431, S43, MT
```

The FEM depends on the board revision:
- **V4.2 (GC1109):** GPIO46 is high in TX and low in RX. The FEM's CTX comes from DIO2. GPIO46 is also a strap, so drive it only after boot.
- **V4.3 (KCT8103L):** GPIO5 is high in TX and low in RX. The FEM's CPS comes from DIO2.
- **Telling them apart:** read GPIO2 as an input at boot. High means a KCT8103L, low a GC1109 (LoRaFEMInterface.cpp).
- One fixed `radio.txen` cannot cover both revisions. Record which revision arrives.

Also:
- GNSS: the connector has no module on the bare board; the CM121 comes in the expansion kit. Reset on GPIO42 (active low), wake-up on GPIO40, PPS on GPIO41.
- OLED: SSD1315, 128×64, on I2C. SDA GPIO17, SCL GPIO18, reset GPIO21.
- The 32.768 kHz crystal is on GPIO15/16.

Conflicts:
- **Vext polarity.** D431 says pulled high, while the V4.2 datasheet, S43 (PMOS gate), and MT say active low. Recorded as active low.

## `XS3`: Seeed XIAO ESP32S3 with Wio-SX1262 (planned, unconfirmed)

The radio GPIO numbers are from Meshtastic only. The Seeed schematic names the nets but not the XIAO GPIOs they land on.

Sources:
- [Wio-SX1262 for XIAO schematic](https://files.seeedstudio.com/products/SenseCAP/Wio_SX1262/Schematic_Diagram_Wio-SX1262_for_XIAO.pdf) (WSCH)
- [Wio-SX1262 datasheet](https://files.seeedstudio.com/products/SenseCAP/Wio_SX1262/Wio-SX1262_Module_Datasheet.pdf) (WDS)
- [Seeed wiki](https://wiki.seeedstudio.com/xiao_esp32s3_getting_started/)
- Meshtastic `seeed_xiao_s3` (MT)

USB and chip:
- Native USB. In ROM and USB-Serial-JTAG mode the id would be `303a:1001` with the MAC as its serial (inferred).
- ESP32-S3R8: 8 MB flash, and 8 MB octal PSRAM in the package on GPIO33–37 (Seeed wiki).

```ini
power_on = (none)               ; the module takes 3V3 straight from the B2B connector — WSCH (inferred)
radio.chip = sx1262             ; WDS
radio.sck = GPIO7               ; MT; Arduino variant
radio.mosi = GPIO9              ; MT
radio.miso = GPIO8              ; MT
radio.nss = GPIO41              ; MT
radio.reset = GPIO42            ; MT
radio.busy = GPIO40             ; MT
radio.dio1 = GPIO39             ; MT
radio.rfswitch = dio2           ; WDS; MT
radio.rxen = GPIO38             ; module RF_SW pin — WSCH net LORA_RF_SW1; MT SX126X_RXEN
radio.tcxo_mv = 1800            ; MT; WDS gives only 1.7–3.3 V
radio.regulator = dcdc          ; WDS
radio.max_dbm = 22              ; WDS
led.status = GPIO21             ; XIAO orange LED, active low — Seeed wiki
led.green = GPIO48              ; Wio LED, active high — WSCH; MT
button.user = GPIO21            ; Wio K1, active low, shared with the XIAO LED — WSCH; MT
```

Also:
- No battery ADC, GNSS, SD card, or I2C connector. The I2C pads are D4 SDA GPIO5 and D5 SCL GPIO6.
- GPIO39–42 are the chip's JTAG pins, which is fine with USB-JTAG.

Conflicts:
- **RF switch.** WDS says DIO2 "solely" picks TX or RX, but also lists an RF_SW pin that MT drives as RXEN. MT stands.
- **GPIO21** is both the XIAO's LED and the Wio's button. Pressing the button lights the LED (inferred).
- **I2C.** One MT file says SDA 47 / SCL 48, but GPIO48 is the Wio LED. MT's variant, the Arduino variant, and the wiki say 5/6.
