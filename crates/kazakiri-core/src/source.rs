//! ファイルの中身へのアクセス。
//!
//! 大きなファイルはメモリマップで開くので、全体を読み込まない。
//! 小さなファイル（上限以下）はメモリに読み込んで、ファイルをすぐ閉じる。
//! Windows は、マップされているファイルの切り詰めを拒否する（書く側がエラー 1224 になる）ため、
//! マップしたままだと、開いているログを `copytruncate` 型のローテーションや `>` で作り直すアプリを妨げてしまう。

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use memmap2::Mmap;

/// この大きさ（バイト）以下のファイルは、マップせずにメモリに読み込む（既定値）
pub const DEFAULT_MEMORY_LIMIT: u64 = 64 << 20;

/// 追記かどうかを確かめるときに比べる、先頭と元の末尾の大きさ
const PROBE: usize = 4096;

pub enum Source {
    Mapped(Mmap),
    Memory(Vec<u8>),
}

impl Source {
    /// `memory_limit` バイト以下ならメモリに読み込み、超えるならメモリマップで開く（0 なら常にマップ）
    pub fn open(path: &Path, memory_limit: u64) -> io::Result<Source> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        if len == 0 {
            // 0 バイトのファイルは Windows ではマップできない
            return Ok(Source::Memory(Vec::new()));
        }
        if len <= memory_limit {
            let mut buf = Vec::new();
            // 確保できなければ、マップで開く
            if buf.try_reserve_exact(len as usize).is_ok() {
                // 読んでいる間に追記された分は読まない（次の確認で「追記」として読み込む）。
                // 読んでいる間に切り詰められたら、短いまま返る（次の確認で「置き換え」になる）
                (&file).take(len).read_to_end(&mut buf)?;
                return Ok(Source::Memory(buf));
            }
        }
        // SAFETY: マップ中に他プロセスがファイルを切り詰めると未定義動作になり得る。
        // Windows ではマップ中のファイルの切り詰めは OS が拒否する。
        let map = unsafe { Mmap::map(&file)? };
        Ok(Source::Mapped(map))
    }

    /// `self` を読んだファイルが末尾に追記されただけなら、新しい内容を返す。
    /// 先頭か、元の末尾付近が違う（または短くなった）なら `None`。
    ///
    /// メモリに読み込んでいて、追記後も上限以下なら、増えた分だけをディスクから読む
    /// （追従中に、毎回ファイル全体を読み直さないため）。
    pub fn reopen_appended(&self, path: &Path, new_len: u64, memory_limit: u64) -> io::Result<Option<Source>> {
        let old = self.bytes();
        let head = old.len().min(PROBE);
        let tail = old.len().saturating_sub(PROBE);
        if new_len < old.len() as u64 {
            return Ok(None);
        }
        if matches!(self, Source::Memory(_)) && new_len <= memory_limit {
            let mut file = File::open(path)?;
            if !same_at(&mut file, 0, &old[..head])? || !same_at(&mut file, tail as u64, &old[tail..])? {
                return Ok(None);
            }
            let mut buf = Vec::new();
            if buf.try_reserve_exact(new_len as usize).is_ok() {
                buf.extend_from_slice(old);
                file.seek(SeekFrom::Start(old.len() as u64))?;
                file.take(new_len - old.len() as u64).read_to_end(&mut buf)?;
                // 確認している間に短くなった
                return Ok((buf.len() as u64 == new_len).then_some(Source::Memory(buf)));
            }
        }
        let new = Source::open(path, memory_limit)?;
        let data = new.bytes();
        let appended = data.len() >= old.len() && old[..head] == data[..head] && old[tail..] == data[tail..old.len()];
        Ok(appended.then_some(new))
    }

    pub fn from_vec(v: Vec<u8>) -> Source {
        Source::Memory(v)
    }

    /// メモリマップで開いているか（開いている間、ほかのアプリはこのファイルを切り詰められない）
    pub fn is_mapped(&self) -> bool {
        matches!(self, Source::Mapped(_))
    }

    #[inline]
    pub fn bytes(&self) -> &[u8] {
        match self {
            Source::Mapped(m) => m,
            Source::Memory(v) => v,
        }
    }

    #[inline]
    pub fn len(&self) -> u64 {
        self.bytes().len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// ファイルの `at` からの内容が `expected` と同じか（途中で終わっていれば違う）
fn same_at(file: &mut File, at: u64, expected: &[u8]) -> io::Result<bool> {
    file.seek(SeekFrom::Start(at))?;
    let mut buf = Vec::with_capacity(expected.len());
    file.take(expected.len() as u64).read_to_end(&mut buf)?;
    Ok(buf == expected)
}
