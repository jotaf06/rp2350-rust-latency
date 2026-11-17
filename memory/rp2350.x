/* Memory layout for the RP2350A (Raspberry Pi Pico 2). Selected by build.rs.
 * Taken from embassy/examples/rp235x/memory.x (run-from-RAM variant removed). */
MEMORY {
    /* 2 MiB is a safe default, although a Pico 2 has 4 MiB of flash. */
    FLASH : ORIGIN = 0x10000000, LENGTH = 2048K
    /* SRAM0-SRAM7, striped mapping. */
    RAM   : ORIGIN = 0x20000000, LENGTH = 512K
    /* SRAM8/9, direct mapping (unused here). */
    SRAM8 : ORIGIN = 0x20080000, LENGTH = 4K
    SRAM9 : ORIGIN = 0x20081000, LENGTH = 4K
}

SECTIONS {
    /* Boot ROM info (IMAGE_DEF block, provided by embassy-rp). Goes right after
     * .vector_table so it stays in the first 4K of flash, where the boot ROM
     * and picotool look for it. */
    .start_block : ALIGN(4)
    {
        __start_block_addr = .;
        KEEP(*(.start_block));
        KEEP(*(.boot_info));
    } > FLASH
} INSERT AFTER .vector_table;

/* move .text to start /after/ the boot info */
_stext = ALIGN(ADDR(.start_block) + SIZEOF(.start_block), ALIGNOF(.text));

SECTIONS {
    /* Picotool 'Binary Info' entries. */
    .bi_entries : ALIGN(4)
    {
        __bi_entries_start = .;
        KEEP(*(.bi_entries));
        . = ALIGN(4);
        __bi_entries_end = .;
    } > FLASH
} INSERT AFTER .text;

SECTIONS {
    /* Boot ROM extra info; goes after everything so it can hold a signature. */
    .end_block : ALIGN(4)
    {
        __end_block_addr = .;
        KEEP(*(.end_block));
    } > FLASH
} INSERT AFTER .uninit;

PROVIDE(start_to_end = __end_block_addr - __start_block_addr);
PROVIDE(end_to_start = __start_block_addr - __end_block_addr);
