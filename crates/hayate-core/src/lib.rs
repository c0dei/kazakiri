//! hayate のコア。UI に依存しない、ファイルの読み込み・文字コード判定・行インデックス・検索。

pub mod document;
pub mod encoding;
pub mod line_index;
pub mod source;

pub use document::{Document, Line, Searcher};
pub use line_index::{Newline, Progress};
