//! Cursor-based pagination for remote directory listings.
//!
//! `list_remote_dirs` historically returned every subdirectory in one shot, which froze the
//! folder-picker dialog on paths like `/tmp` or `~/.cache` that hold thousands of entries.
//! This module slices a sorted listing into pages keyed by the last-seen entry name so the
//! frontend can stream them in as the user scrolls.

use crate::types::RemoteDirListing;

const DEFAULT_PAGE_SIZE: usize = 200;
const MAX_PAGE_SIZE: usize = 1000;

/// Sort `entries` case-insensitively (stable for ties) and return the page that begins
/// strictly after `cursor`. `limit == 0` falls back to `DEFAULT_PAGE_SIZE`; values above
/// `MAX_PAGE_SIZE` are clamped. The returned `next_cursor` is the last entry on the page
/// when more remain, otherwise `None`.
pub fn paginate_dir_listing(
    path: &str,
    parent: Option<String>,
    mut entries: Vec<String>,
    cursor: Option<String>,
    limit: usize,
    total_estimate: Option<u64>,
) -> RemoteDirListing {
    // Stable sort by case-insensitive key — Vec::sort_by_key preserves input order on ties,
    // so "alpha" stays before "ALPHA" if that's how the SFTP listing returned them.
    entries.sort_by_key(|e| e.to_ascii_lowercase());

    let start = match &cursor {
        None => 0,
        Some(c) => {
            let lc = c.to_ascii_lowercase();
            // Find the rightmost entry whose lowercase form is <= cursor's lowercase, *and*
            // — when lowercase ties — whose exact bytes match. The next page begins one slot
            // past that index. If the cursor isn't found exactly, partition_point lands at
            // the first entry that sorts strictly after it, which is also the right answer.
            entries
                .iter()
                .rposition(|e| e == c)
                .map(|i| i + 1)
                .unwrap_or_else(|| {
                    entries.partition_point(|e| e.to_ascii_lowercase() <= lc)
                })
        }
    };

    let lim = if limit == 0 {
        DEFAULT_PAGE_SIZE
    } else {
        limit.min(MAX_PAGE_SIZE)
    };

    let page: Vec<String> = if start >= entries.len() {
        Vec::new()
    } else {
        let end = (start + lim).min(entries.len());
        entries[start..end].to_vec()
    };
    let next_cursor = if start + page.len() < entries.len() {
        page.last().cloned()
    } else {
        None
    };

    RemoteDirListing {
        current: path.to_string(),
        parent,
        entries: page,
        next_cursor,
        total_estimate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(prefix: &str, n: usize) -> Vec<String> {
        (0..n).map(|i| format!("{prefix}-{i:04}")).collect()
    }

    #[test]
    fn paginate_three_pages_of_500() {
        let all = names("dir", 500);
        let page1 = paginate_dir_listing("/x", None, all.clone(), None, 200, None);
        assert_eq!(page1.entries.len(), 200);
        assert_eq!(page1.entries.first().unwrap(), "dir-0000");
        assert_eq!(page1.entries.last().unwrap(), "dir-0199");
        assert_eq!(page1.next_cursor.as_deref(), Some("dir-0199"));

        let page2 =
            paginate_dir_listing("/x", None, all.clone(), page1.next_cursor.clone(), 200, None);
        assert_eq!(page2.entries.len(), 200);
        assert_eq!(page2.entries.first().unwrap(), "dir-0200");
        assert_eq!(page2.entries.last().unwrap(), "dir-0399");
        assert_eq!(page2.next_cursor.as_deref(), Some("dir-0399"));

        let page3 = paginate_dir_listing("/x", None, all, page2.next_cursor.clone(), 200, None);
        assert_eq!(page3.entries.len(), 100);
        assert_eq!(page3.entries.first().unwrap(), "dir-0400");
        assert_eq!(page3.entries.last().unwrap(), "dir-0499");
        assert_eq!(page3.next_cursor, None);
    }

    #[test]
    fn paginate_cursor_past_end_returns_empty() {
        let entries = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let page = paginate_dir_listing(
            "/x",
            None,
            entries,
            Some("zzz".to_string()),
            200,
            None,
        );
        assert!(page.entries.is_empty());
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn paginate_sorts_case_insensitively() {
        let entries = vec![
            "Bravo".to_string(),
            "alpha".to_string(),
            "ALPHA".to_string(),
            "beta".to_string(),
        ];
        let page = paginate_dir_listing("/x", None, entries, None, 200, None);
        // "alpha" precedes "ALPHA" because the byte-order tiebreaker puts lowercase first.
        assert_eq!(page.entries, vec!["alpha", "ALPHA", "beta", "Bravo"]);
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn paginate_clamps_limit() {
        let all = names("dir", 1500);

        let zero = paginate_dir_listing("/x", None, all.clone(), None, 0, None);
        assert_eq!(zero.entries.len(), DEFAULT_PAGE_SIZE);

        let huge = paginate_dir_listing("/x", None, all, None, 9_999, None);
        assert_eq!(huge.entries.len(), MAX_PAGE_SIZE);
    }

    #[test]
    fn paginate_empty_dir() {
        let page = paginate_dir_listing("/x", None, Vec::new(), None, 200, None);
        assert!(page.entries.is_empty());
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn paginate_cursor_on_last_returns_empty() {
        let entries = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let page = paginate_dir_listing("/x", None, entries, Some("c".to_string()), 200, None);
        assert!(page.entries.is_empty());
        assert_eq!(page.next_cursor, None);
    }
}
