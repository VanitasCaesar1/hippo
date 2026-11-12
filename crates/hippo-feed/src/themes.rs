//! Theme packs the feed serves.
//!
//! A pack is served while it is listed, its post is visible, and an accepted
//! stylesheet exists. Until the registry sync fills the tables, nothing is.

use rusqlite::{Connection, OptionalExtension};

/// The pack hash of the stylesheet the feed serves for theme pack `name`: the
/// newest accepted one by `theme_files.at`, if the pack is listed and its post
/// visible.
pub fn serves(conn: &Connection, name: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT f.pack_hash FROM theme_files f
         JOIN packs p ON p.name = f.pack
         JOIN posts o ON o.pack = p.name
         WHERE f.pack = ?1 AND f.css IS NOT NULL AND p.kind = 'theme'
               AND p.listed = 1 AND o.state = 'visible'
         ORDER BY f.at DESC LIMIT 1",
        [name],
        |r| r.get(0),
    )
    .optional()
}

/// The names of the theme packs the feed serves, in name order.
pub fn served(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT name FROM packs WHERE kind = 'theme' ORDER BY name")?;
    let names = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for name in names {
        if serves(conn, &name)?.is_some() {
            out.push(name);
        }
    }
    Ok(out)
}

/// The stylesheet's address: `/themes/<name>/<pack hash>.css`.
pub fn href(name: &str, pack_hash: &str) -> String {
    format!("/themes/{name}/{pack_hash}.css")
}
