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
    undo: VecDeque<Vec<u8>>,
    redo: VecDeque<Vec<u8>>,
}

impl History {
    pub(crate) fn record(&mut self, changeset: Option<Vec<u8>>) {
        let Some(changeset) = changeset else {
            return;
        };
        self.redo.clear();
        if changeset.len() > HISTORY_BYTE_LIMIT {
            self.undo.clear();
            return;
        }
        while self
            .undo
            .iter()
            .map(Vec::len)
            .sum::<usize>()
            .saturating_add(changeset.len())
            > HISTORY_BYTE_LIMIT
        {
            self.undo.pop_front();
        }
        self.undo.push_back(changeset);
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
        let Some(changeset) = self.history.undo.pop_back() else {
            return Ok(false);
        };
        let mut inverse = Vec::new();
        if let Err(error) = invert_strm(&mut changeset.as_slice(), &mut inverse) {
            self.history.undo.push_back(changeset);
            return Err(error.into());
        }
        if let Err(error) = self.apply_history(&inverse) {
            self.history.undo.push_back(changeset);
            return Err(error);
        }
        self.history.redo.push_back(changeset);
        Ok(true)
    }

    /// Redoes the latest mutation undone during this engine session.
    ///
    /// Returns `false` when there is nothing to redo. A new product mutation
    /// clears the redo history.
    pub fn redo(&mut self) -> Result<bool> {
        let Some(changeset) = self.history.redo.pop_back() else {
            return Ok(false);
        };
        if let Err(error) = self.apply_history(&changeset) {
            self.history.redo.push_back(changeset);
            return Err(error);
        }
        self.history.undo.push_back(changeset);
        Ok(true)
    }

    fn apply_history(&mut self, changeset: &[u8]) -> Result<()> {
        let captured = capture_session(&self.connection)?;
        let transaction = self.connection.unchecked_transaction()?;
        let conflicted = Arc::new(AtomicBool::new(false));
        let conflict_seen = Arc::clone(&conflicted);
        let mut input = changeset;
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
        commit_mutation(transaction, captured, self.sync_device_id)?;
        Ok(())
    }
}
