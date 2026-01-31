//! Full-text search using SQLite FTS5

use std::collections::HashSet;
use std::time::{Duration, Instant};

use color_eyre::Result;
use rust_apt::cache::{Cache, PackageSort};
use rusqlite::{Connection, params};

/// SQLite FTS5 search index for packages
pub struct SearchIndex {
    conn: Connection,
}

impl SearchIndex {
    pub fn new() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS packages USING fts5(name, description)",
            [],
        )?;
        Ok(Self { conn })
    }

    /// Build the search index from the APT cache
    pub fn build(&mut self, cache: &Cache) -> Result<(usize, Duration)> {
        let start = Instant::now();
        let mut count = 0;

        // Use transaction for much faster bulk inserts
        self.conn.execute("BEGIN TRANSACTION", [])?;

        // Clear existing data
        self.conn.execute("DELETE FROM packages", [])?;

        // Insert all packages
        let mut stmt = self.conn.prepare("INSERT INTO packages (name, description) VALUES (?, ?)")?;

        for pkg in cache.packages(&PackageSort::default()) {
            let name = pkg.name();
            let desc = pkg.candidate()
                .and_then(|c| c.summary())
                .unwrap_or_default();
            stmt.execute(params![name, desc])?;
            count += 1;
        }

        self.conn.execute("COMMIT", [])?;

        Ok((count, start.elapsed()))
    }

    /// Search for packages matching the query
    pub fn search(&self, query: &str) -> Result<HashSet<String>> {
        let mut results = HashSet::new();

        if query.is_empty() {
            return Ok(results);
        }

        // Escape special FTS5 characters and add prefix matching
        let fts_query = query
            .split_whitespace()
            .map(|word| format!("{}*", word.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" ");

        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT name FROM packages WHERE packages MATCH ?"
        )?;

        let rows = stmt.query_map([&fts_query], |row| row.get::<_, String>(0))?;

        for name in rows.flatten() {
            results.insert(name);
        }

        Ok(results)
    }
}
