//! kazakiri のコア。UI に依存しない、ファイルの読み込み・文字コード判定・行インデックス・検索。

pub mod convert;
pub mod csv;
pub mod document;
pub mod encoding;
pub mod grep;
pub mod editor;
pub mod line_index;
mod pieces;
pub mod search;
pub mod source;
pub mod split;
pub mod syntax;
pub mod textops;

pub use convert::Convert;
pub use document::{commit_save, Document, EditError, Line, Refresh};
pub use editor::{EditKind, Editor};
pub use search::{Direction, FindAll, Query, SearchControl, SearchOptions, SearchOutcome};
pub use line_index::{Newline, Progress};
