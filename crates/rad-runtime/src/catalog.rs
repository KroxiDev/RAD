//! Bounded read-only catalogue pages; cursors bind the live catalogue and query.
use rad_launcher::CatalogEntry;
use serde_json::{Value, json};
use std::hash::{Hash, Hasher};
pub(crate) fn page(
    entries: &[CatalogEntry],
    connection: u64,
    query: &str,
    scope: &str,
    cursor: Option<&str>,
) -> Result<Value, &'static str> {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    connection.hash(&mut hash);
    query.hash(&mut hash);
    scope.hash(&mut hash);
    for entry in entries {
        entry.id.id().hash(&mut hash);
        entry.name.hash(&mut hash);
    }
    let stamp = hash.finish();
    let start = match cursor {
        None => 0,
        Some(s)
            if s.len() == 32
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
        {
            if u64::from_str_radix(&s[..16], 16).ok() != Some(stamp) {
                return Err("cursor_expired");
            }
            usize::from_str_radix(&s[16..], 16).map_err(|_| "cursor_expired")?
        }
        _ => return Err("cursor_expired"),
    };
    if start > entries.len() {
        return Err("cursor_expired");
    }
    let query = query.to_lowercase();
    let mut items = Vec::new();
    let mut position = start;
    while position < entries.len() {
        let e = &entries[position];
        if e.name.to_lowercase().contains(&query) || e.id.id().to_lowercase().contains(&query) {
            let item = json!({"desktop_entry":e.id.id(),"name":e.name});
            let next_size = serde_json::to_vec(&item)
                .map_err(|_| "resource_exhausted")?
                .len();
            // Reserve envelope, cursor and separators within IPC's 48KiB response cap.
            let size: usize = items
                .iter()
                .map(|v| serde_json::to_vec(v).expect("JSON value").len() + 1)
                .sum();
            if items.len() == 32 || size + next_size > 47 * 1024 {
                if items.is_empty() {
                    return Err("resource_exhausted");
                }
                break;
            }
            items.push(item);
        }
        position += 1;
    }
    Ok(
        json!({"items":items,"next_cursor":(position<entries.len()).then(||format!("{stamp:016x}{position:016x}"))}),
    )
}
