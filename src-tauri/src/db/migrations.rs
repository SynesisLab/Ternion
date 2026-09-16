//! Forward-only migrations keyed by `PRAGMA user_version`. Each migration runs
//! atomically (BEGIN/COMMIT), so a crash mid-migration leaves the DB intact
//! and the same migration re-applies on next open.

use rusqlite::Connection;

const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_init", include_str!("migrations/0001_init.sql")),
    ("0002_add_reasoning", include_str!("migrations/0002_add_reasoning.sql")),
    ("0003_triad_settings", include_str!("migrations/0003_triad_settings.sql")),
];

pub fn run(conn: &Connection) -> rusqlite::Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (idx, (name, sql)) in MIGRATIONS.iter().enumerate() {
        let version = (idx + 1) as i64;
        if current >= version {
            continue;
        }
        log::info!("applying migration {name}");
        conn.execute_batch(&format!("BEGIN;\n{sql}\nCOMMIT;"))?;
        conn.pragma_update(None, "user_version", version)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_run_is_a_no_op() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, MIGRATIONS.len() as i64);
        // Re-running on an already-migrated connection must not re-apply.
        run(&conn).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, MIGRATIONS.len() as i64);
    }
}