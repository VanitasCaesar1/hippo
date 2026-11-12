//! One page of a list: the rows, and whether a next page exists.

/// A page of rows. `more` is true when at least one row follows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub rows: Vec<T>,
    pub more: bool,
}

impl<T> Page<T> {
    pub fn empty() -> Self {
        Self {
            rows: Vec::new(),
            more: false,
        }
    }

    /// Splits `rows`, fetched with one row more than `size`, into the page
    /// and the `more` flag.
    pub fn from_overfetched(mut rows: Vec<T>, size: usize) -> Self {
        let more = rows.len() > size;
        rows.truncate(size);
        Self { rows, more }
    }
}

/// The 1-based page number as an offset in rows of `size`; `0` counts as 1.
pub fn offset(page: usize, size: usize) -> usize {
    page.max(1).saturating_sub(1).saturating_mul(size)
}
