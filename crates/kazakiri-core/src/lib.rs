//! kazakiri のコア。UI に依存しない、ファイルの読み込み・文字コード判定・行インデックス・検索。

pub mod document;
pub mod encoding;
pub mod editor;
pub mod line_index;
mod pieces;
pub mod search;
pub mod source;

pub use document::{commit_save, Document, EditError, Line, Refresh};
pub use editor::{EditKind, Editor};
pub use search::{Direction, Query, SearchControl, SearchOptions, SearchOutcome};
pub use line_index::{Newline, Progress};
