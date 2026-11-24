//! Generic read-only row access for the UI's fixed aggregate queries.

use crate::{Reader, StorageError};
use rusqlite::types::{Value, ValueRef};

/// One SQLite value.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
}

impl Cell {
    pub fn int(&self) -> i64 {
        match self {
            Self::Int(n) => *n,
            Self::Real(f) => *f as i64,
            _ => 0,
        }
    }
    pub fn opt_int(&self) -> Option<i64> {
        matches!(self, Self::Int(_) | Self::Real(_)).then(|| self.int())
    }
    pub fn text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Int(n) => n.to_string(),
            Self::Real(f) => f.to_string(),
            Self::Null => String::new(),
        }
    }
    pub fn opt_text(&self) -> Option<String> {
        matches!(self, Self::Text(_)).then(|| self.text())
    }
}

impl Reader {
    /// Run a fixed, caller-written SELECT on the read-only connection and return at most
    /// `max_rows` rows. The connection is `query_only`: nothing can be modified through it.
    pub fn rows(
        &self,
        sql: &str,
        params: &[Cell],
        max_rows: usize,
    ) -> Result<Vec<Vec<Cell>>, StorageError> {
        let vals: Vec<Value> = params
            .iter()
            .map(|p| match p {
                Cell::Null => Value::Null,
                Cell::Int(n) => Value::Integer(*n),
                Cell::Real(f) => Value::Real(*f),
                Cell::Text(s) => Value::Text(s.clone()),
            })
            .collect();
        let mut stmt = self.conn().prepare(sql)?;
        let cols = stmt.column_count();
        let mut rows = stmt.query(rusqlite::params_from_iter(vals))?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            if out.len() >= max_rows {
                break;
            }
            let mut row = Vec::with_capacity(cols);
            for i in 0..cols {
                row.push(match r.get_ref(i)? {
                    ValueRef::Integer(n) => Cell::Int(n),
                    ValueRef::Real(f) => Cell::Real(f),
                    ValueRef::Text(t) => Cell::Text(String::from_utf8_lossy(t).into_owned()),
                    ValueRef::Null | ValueRef::Blob(_) => Cell::Null,
                });
            }
            out.push(row);
        }
        Ok(out)
    }

    /// `PRAGMA user_version` of the opened database.
    pub fn schema_version(&self) -> Result<i64, StorageError> {
        Ok(self
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }
}
