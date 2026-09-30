//! data 区画 (P2、236 kB) の先頭セクタに置く小さな記録 (0.4.2〜、`boot_policy::Record`)
//!
//! - **Wi-Fi の資格情報の写し**: 通常モードが SD の `wifi.txt` を読めたとき、内容が変わっていれば書く。
//!   回復モードはこれを使い、SD には触れない (SD の読み込みで落ちる版でも回復モードは Wi-Fi につながる)。
//!   写しが無いときだけ、回復モードは期限付き (`sdcard::set_deadline`) で `wifi.txt` だけを読む。
//! - **入れない版** (`Record::blocked`): 他方区画へ戻してきた版 (回復モードでも落ち続けた版)。その版以下は
//!   OTA で入れない (同じ壊れた版を入れ直して行き来するのを防ぐ)。
//!
//! 書き込みは内容が変わったときだけ (通常は初回の 1 回、wifi.txt を書き換えたときにもう 1 回)。
//! 4 kB セクタを 1 つ消して 256 B を 1 ページ書く (割り込み禁止 ≈ 50〜400 ms、LCD の DMA は SRAM だけを読む
//! ので走査は乱れない。OTA の書き込みと同じ)。
//!
//! **注意**: パスワードは平文でフラッシュに残る (SD の `wifi.txt` と同じく平文。picotool で読み出せる)。
//! 消したいときは `picotool erase` で data 区画を消すか、`wifi.txt` を書き換えて一度通常起動する (上書きされる)。

use embassy_rp::flash::PAGE_SIZE;

use crate::ab_boot::{FLAGS_ACCEPTS_FAMILY_DATA, PartitionTable};
use crate::boot_policy::{RECORD_LEN, Record};
use crate::ota::slot::{self, OtaFlash, SECTOR_SIZE};

/// data 区画の先頭 (ストレージオフセット)。パーティションテーブルの `data` (family data を受ける区画) を探す
pub fn data_offset() -> Option<u32> {
    let table = PartitionTable::read().ok()?;
    if !table.present {
        return None;
    }
    let p = table
        .partitions()
        .find(|p| p.name() == "data" || p.has_flag(FLAGS_ACCEPTS_FAMILY_DATA))?;
    (p.size() >= SECTOR_SIZE as u32).then_some(p.start_offset())
}

/// 記録を読む (無い / 壊れていれば None)
pub fn read(offset: u32) -> Option<Record> {
    let mut buf = [0u8; RECORD_LEN];
    slot::read_storage(offset, &mut buf);
    Record::decode(&buf)
}

/// 記録が `record` と違えば書く。書いたら Ok(true)
pub fn write_if_changed(flash: &mut OtaFlash, offset: u32, record: &Record) -> Result<bool, &'static str> {
    if read(offset).as_ref() == Some(record) {
        return Ok(false);
    }
    let mut page = [0xffu8; PAGE_SIZE];
    page[..RECORD_LEN].copy_from_slice(&record.encode());
    flash
        .blocking_erase(offset, offset + SECTOR_SIZE as u32)
        .map_err(|_| "erase failed")?;
    flash.blocking_write(offset, &page).map_err(|_| "write failed")?;
    // 読み戻して確かめる (0x1C00_0000 の窓はキャッシュを通らない)
    if read(offset).as_ref() == Some(record) {
        Ok(true)
    } else {
        Err("readback mismatch")
    }
}
