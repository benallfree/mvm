/* nRF52840 with S140 6.1.1: the app starts at 0x26000 and ends where the
   Adafruit layout's internal filesystem begins (0xED000). The RAM start is a
   placeholder until 0.6 measures the SoftDevice's. */
MEMORY
{
  FLASH : ORIGIN = 0x00026000, LENGTH = 0xC7000
  RAM   : ORIGIN = 0x20006000, LENGTH = 0x3A000
}
