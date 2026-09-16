//! Id generation. UUID v7 is time-ordered, which keeps message primary keys
//! in index order (design §3.6 persistence + `ORDER BY created_at, rowid`).

pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
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
}