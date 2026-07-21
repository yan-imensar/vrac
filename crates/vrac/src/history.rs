use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::session::{ConflictAction, ConflictType, invert_strm};

use crate::db::Engine;
use crate::sync::{capture_session, commit_mutation};
use crate::{Error, Result};

const HISTORY_LIMIT: usize = 100;
const HISTORY_BYTE_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct History {
    undo: VecDeque<HistoryEntry>,
    redo: VecDeque<HistoryEntry>,
}

struct HistoryEntry {
    changesets: Vec<Vec<u8>>,
    bytes: usize,
}

impl History {
    pub(crate) fn record(&mut self, changeset: Option<Vec<u8>>) {
        let Some(changeset) = changeset else {
            return;
        };
        self.record_group(vec![changeset]);
    }

    pub(crate) fn record_group(&mut self, changesets: Vec<Vec<u8>>) {
        let bytes = changesets.iter().map(Vec::len).sum();
        if changesets.is_empty() {
            return;
        }
        self.redo.clear();
        if bytes > HISTORY_BYTE_LIMIT {
            self.undo.clear();
            return;
        }
        while self
            .undo
            .iter()
            .map(|entry| entry.bytes)
            .sum::<usize>()
            .saturating_add(bytes)
            > HISTORY_BYTE_LIMIT
        {
            self.undo.pop_front();
        }
        self.undo.push_back(HistoryEntry { changesets, bytes });
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.pop_front();
        }
    }

    pub(crate) fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

impl Engine {
    /// Undoes the latest product mutation from this engine session.
    ///
    /// Returns `false` when there is nothing to undo. History is bounded to
    /// the latest 100 mutations and 8 MiB of changesets. It is cleared by
    /// remote imports, checkpoint restoration, workspace changes, and process
    /// restarts.
    pub fn undo(&mut self) -> Result<bool> {
        let Some(entry) = self.history.undo.pop_back() else {
            return Ok(false);
        };
        let mut inverses = Vec::with_capacity(entry.changesets.len());
        for changeset in entry.changesets.iter().rev() {
            let mut inverse = Vec::new();
            if let Err(error) = invert_strm(&mut changeset.as_slice(), &mut inverse) {
                self.history.undo.push_back(entry);
                return Err(error.into());
            }
            inverses.push(inverse);
        }
        if let Err(error) = self.apply_history(&inverses) {
            self.history.undo.push_back(entry);
            return Err(error);
        }
        self.history.redo.push_back(entry);
        Ok(true)
    }

    /// Redoes the latest mutation undone during this engine session.
    ///
    /// Returns `false` when there is nothing to redo. A new product mutation
    /// clears the redo history.
    pub fn redo(&mut self) -> Result<bool> {
        let Some(entry) = self.history.redo.pop_back() else {
            return Ok(false);
        };
        if let Err(error) = self.apply_history(&entry.changesets) {
            self.history.redo.push_back(entry);
            return Err(error);
        }
        self.history.undo.push_back(entry);
        Ok(true)
    }

    fn apply_history(&mut self, changesets: &[Vec<u8>]) -> Result<()> {
        let captured = capture_session(&self.connection)?;
        let transaction = self.connection.unchecked_transaction()?;
        transaction.pragma_update(None, "defer_foreign_keys", true)?;
        let conflicted = Arc::new(AtomicBool::new(false));
        for changeset in changesets {
            let conflict_seen = Arc::clone(&conflicted);
            let mut input = changeset.as_slice();
            let applied =
                transaction.apply_strm(&mut input, None::<fn(&str) -> bool>, move |kind, item| {
                    if kind == ConflictType::SQLITE_CHANGESET_NOTFOUND
                        && item.op().is_ok_and(|operation| operation.indirect())
                    {
                        return ConflictAction::SQLITE_CHANGESET_OMIT;
                    }
                    conflict_seen.store(true, Ordering::Relaxed);
                    ConflictAction::SQLITE_CHANGESET_ABORT
                });
            if let Err(error) = applied {
                return if conflicted.load(Ordering::Relaxed) {
                    Err(Error::HistoryConflict)
                } else {
                    Err(error.into())
                };
            }
        }
        commit_mutation(transaction, captured, self.sync_device_id)?;
        Ok(())
    }
}
