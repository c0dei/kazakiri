//! ファイルの中身へのアクセス。メモリマップで開くので、ファイル全体を読み込まない。

use std::fs::File;
use std::io;
use std::path::Path;

use memmap2::Mmap;

pub enum Source {
    Mapped(Mmap),
    Memory(Vec<u8>),
}

impl Source {
    pub fn open(path: &Path) -> io::Result<Source> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        if len == 0 {
            // 0 バイトのファイルは Windows ではマップできない
            return Ok(Source::Memory(Vec::new()));
        }
        // SAFETY: マップ中に他プロセスがファイルを切り詰めると未定義動作になり得る。
        // Windows ではマップ中のファイルの切り詰めは OS が拒否する。
        let map = unsafe { Mmap::map(&file)? };
        Ok(Source::Mapped(map))
    }

    pub fn from_vec(v: Vec<u8>) -> Source {
        Source::Memory(v)
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
