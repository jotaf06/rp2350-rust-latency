/* Memory layout for the RP2040 (Raspberry Pi Pico). Selected by build.rs.
 * Taken from embassy/examples/rp/memory.x. The first 256 bytes of flash hold
 * the second-stage bootloader (boot2, provided by embassy-rp for the W25Q080
 * flash of the Pico); embassy-rp's link-rp.x places it in BOOT2. */
MEMORY {
    BOOT2 : ORIGIN = 0x10000000, LENGTH = 0x100
    FLASH : ORIGIN = 0x10000100, LENGTH = 2048K - 0x100
    /* SRAM0-SRAM3, striped mapping (SRAM4/5 unused here). */
    RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}
