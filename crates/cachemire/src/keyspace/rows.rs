//! The tree's row model: rows held like a `VecModel`'s, but taken in or out many at a
//! time, so a page of the walk costs one pass over the rows, not one per row added.

use std::any::Any;
use std::cell::RefCell;
use std::ops::Range;

use slint::{Model, ModelNotify, ModelTracker};

use crate::KeyspaceRowData;

#[derive(Default)]
pub(super) struct TreeRows {
    rows: RefCell<Vec<KeyspaceRowData>>,
    notify: ModelNotify,
}

impl Model for TreeRows {
    type Data = KeyspaceRowData;

    fn row_count(&self) -> usize {
        self.rows.borrow().len()
    }

    fn row_data(&self, row: usize) -> Option<KeyspaceRowData> {
        self.rows.borrow().get(row).cloned()
    }

    fn set_row_data(&self, row: usize, data: KeyspaceRowData) {
        if let Some(slot) = self.rows.borrow_mut().get_mut(row) {
            *slot = data;
        } else {
            return;
        }
        self.notify.row_changed(row);
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl TreeRows {
    pub(super) fn set_vec(&self, rows: Vec<KeyspaceRowData>) {
        *self.rows.borrow_mut() = rows;
        self.notify.reset();
    }

    /// Adds `rows`, each at its position in `positions` (ascending, and counted
    /// once every row is in): the rows already there keep their order around them.
    pub(super) fn insert_at(&self, positions: &[usize], rows: Vec<KeyspaceRowData>) {
        if rows.is_empty() {
            return;
        }
        {
            let mut held = self.rows.borrow_mut();
            let mut old = std::mem::take(&mut *held).into_iter();
            let mut merged = Vec::with_capacity(old.len() + rows.len());
            for (&position, row) in positions.iter().zip(rows) {
                while merged.len() < position
                    && let Some(before) = old.next()
                {
                    merged.push(before);
                }
                merged.push(row);
            }
            merged.extend(old);
            *held = merged;
        }
        // one notice per run of adjacent rows, in order: each is right once those before it are
        let mut run: Option<(usize, usize)> = None;
        for &position in positions {
            run = match run {
                Some((start, len)) if start + len == position => Some((start, len + 1)),
                Some((start, len)) => {
                    self.notify.row_added(start, len);
                    Some((position, 1))
                }
                None => Some((position, 1)),
            };
        }
        if let Some((start, len)) = run {
            self.notify.row_added(start, len);
        }
    }

    /// Takes out the rows in `range`.
    pub(super) fn remove_range(&self, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        let start = range.start;
        let removed = self.rows.borrow_mut().drain(range).count();
        self.notify.row_removed(start, removed);
    }
}
