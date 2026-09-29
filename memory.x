/*
 * FLASH の LENGTH は A/B パーティション 1 スロット分 (partition/pico2w-ab.json の
 * app-a / app-b = 1920K) に合わせている。イメージは常に 0x10000000 でリンクし、
 * どちらのスロットに置かれても bootrom の QMI アドレス変換 (RP2350 データシート
 * 5.1.19) で 0x10000000 に見える。スロットを超えるサイズはリンク時に検出する。
 * (パーティションテーブル無しで先頭に置く従来の使い方でもそのまま動く)
 */
MEMORY {
    FLASH : ORIGIN = 0x10000000, LENGTH = 1920K
    RAM   : ORIGIN = 0x20000000, LENGTH = 512K
    SRAM4 : ORIGIN = 0x20080000, LENGTH = 4K
    SRAM5 : ORIGIN = 0x20081000, LENGTH = 4K
}

SECTIONS {
    .start_block : ALIGN(4)
    {
        __start_block_addr = .;
        KEEP(*(.start_block));
        KEEP(*(.boot_info));
    } > FLASH
} INSERT AFTER .vector_table;

_stext = ADDR(.start_block) + SIZEOF(.start_block);

SECTIONS {
    .bi_entries : ALIGN(4)
    {
        __bi_entries_start = .;
        KEEP(*(.bi_entries));
        . = ALIGN(4);
        __bi_entries_end = .;
    } > FLASH
} INSERT AFTER .text;

SECTIONS {
    .end_block : ALIGN(4)
    {
        __end_block_addr = .;
        KEEP(*(.end_block));
    } > FLASH
} INSERT AFTER .uninit;

PROVIDE(start_to_end = __end_block_addr - __start_block_addr);
PROVIDE(end_to_start = __start_block_addr - __end_block_addr);
