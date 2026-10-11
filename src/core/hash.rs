//! Content hashing for the derived index.
//!
//! FNV-1a, not a cryptographic digest, and deliberately so: this is a cache
//! key that decides whether a committed file is re-read. Two properties it
//! must have — stable across machines and across archex versions, so a new
//! build does not re-import everyone's ledger — and one it must not have:
//! being load-bearing against someone who wants to change what they wrote.
//! The committed file is the record; this only answers "did it change?".

const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

pub fn content_hash(bytes: &[u8]) -> String {
    let mut hash = OFFSET_BASIS;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{:016x}", hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_bytes_hash_the_same_way_every_time() {
        assert_eq!(content_hash(b"decision"), content_hash(b"decision"));
        assert_ne!(content_hash(b"decision"), content_hash(b"decisions"));
        assert_ne!(content_hash(b""), content_hash(b" "));
    }

    /// A one-character edit must change the key, or sync would serve a
    /// stale record for a file that was edited.
    #[test]
    fn any_edit_changes_the_hash() {
        let base = content_hash(b"## Decision\nShip it.\n");
        assert_ne!(base, content_hash(b"## Decision\nShip it. \n"));
        assert_ne!(base, content_hash(b"## Decision\nShip IT.\n"));
    }
}