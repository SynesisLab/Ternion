//! Id + timestamp generation. UUID v7 is time-ordered, which keeps message
//! primary keys in index order (design §8.2 persistence + `ORDER BY created_at`).

pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Unix epoch milliseconds (SQLite storage format across the schema).
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    #[test]
    fn ids_are_unique_and_sorted() {
        let a = super::new_id();
        let b = super::new_id();
        assert_ne!(a, b);
        assert!(a < b, "v7 ids must sort ascending by creation time");
    }

    #[test]
    fn now_ms_is_plausible() {
        let now = super::now_ms();
        // After 2026-01-01, before 2100.
        assert!(now > 1_760_000_000_000 && now < 4_102_444_800_000);
    }
}