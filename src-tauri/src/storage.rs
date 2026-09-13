use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection, OpenFlags, Row};
#[cfg(test)]
use serde::Serialize;
use thiserror::Error;

use crate::{
    crypto::{CryptoError, KeyMaterial},
    model::{ClipItem, Group, ListQuery, ListResult, Stats, SyncItem, SyncTombstone},
};

const SCHEMA_VERSION: i64 = 7;
const TAG_SEPARATOR: char = '\u{1f}';
const SELECT_BASE: &str = r#"
SELECT i.*, (
  SELECT group_concat(t.name, char(31)) FROM item_tags it
  JOIN tags t ON t.id = it.tag_id WHERE it.item_id = i.id
) AS tags
FROM items i
"#;
const SCHEMA: &str = r#"
CREATE TABLE items (
  id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, text TEXT, html TEXT,
  preview TEXT NOT NULL DEFAULT '', auto_kind TEXT NOT NULL DEFAULT 'plain',
  hash TEXT NOT NULL UNIQUE, blob_name TEXT, thumb BLOB, width INTEGER, height INTEGER,
  bytes INTEGER NOT NULL DEFAULT 0, source_app TEXT, pinned INTEGER NOT NULL DEFAULT 0,
  use_count INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, last_used_at INTEGER NOT NULL,
  note TEXT, hotkey TEXT, group_id INTEGER REFERENCES groups(id) ON DELETE SET NULL
);
CREATE INDEX idx_items_order ON items(pinned DESC, last_used_at DESC);
CREATE INDEX idx_items_kind ON items(kind);
CREATE INDEX idx_items_auto_kind ON items(auto_kind);
CREATE INDEX idx_items_group ON items(group_id);
CREATE UNIQUE INDEX idx_items_hotkey ON items(hotkey) WHERE hotkey IS NOT NULL;
CREATE TABLE groups (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  parent_id INTEGER REFERENCES groups(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  sort INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE tags (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE);
CREATE TABLE item_tags (
  item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
  PRIMARY KEY (item_id, tag_id)
);
CREATE INDEX idx_item_tags_tag ON item_tags(tag_id);
CREATE TABLE sync_tombstones (
  hash TEXT PRIMARY KEY,
  deleted_at INTEGER NOT NULL
);
CREATE VIRTUAL TABLE items_fts USING fts5(
  text, preview, note, content='items', content_rowid='id', tokenize='trigram'
);
CREATE TRIGGER items_ai AFTER INSERT ON items BEGIN
  INSERT INTO items_fts(rowid, text, preview, note) VALUES (new.id, new.text, new.preview, new.note);
END;
CREATE TRIGGER items_ad AFTER DELETE ON items BEGIN
  INSERT INTO items_fts(items_fts, rowid, text, preview, note) VALUES ('delete', old.id, old.text, old.preview, old.note);
END;
CREATE TRIGGER items_au AFTER UPDATE OF text, preview, note ON items BEGIN
  INSERT INTO items_fts(items_fts, rowid, text, preview, note) VALUES ('delete', old.id, old.text, old.preview, old.note);
  INSERT INTO items_fts(rowid, text, preview, note) VALUES (new.id, new.text, new.preview, new.note);
END;
"#;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("storage lock poisoned")]
    Poisoned,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg(test)]
pub struct CompatibilityProbe {
    pub schema_version: i64,
    pub item_count: i64,
    pub sqlite_version: String,
    pub cipher: String,
    pub os_protected: bool,
}

#[derive(Clone)]
pub struct NewItem {
    pub kind: String,
    pub text: Option<String>,
    pub html: Option<String>,
    pub preview: String,
    pub auto_kind: String,
    pub hash: String,
    pub blob_name: Option<String>,
    pub thumb: Option<Vec<u8>>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bytes: usize,
    pub source_app: Option<String>,
}

pub struct SqliteStore {
    connection: Mutex<Connection>,
    data_dir: PathBuf,
    keys: KeyMaterial,
}

fn apply_key(connection: &Connection, keys: &KeyMaterial) -> Result<(), rusqlite::Error> {
    connection.execute_batch(&format!("PRAGMA key=\"x'{}'\";", keys.database_key_hex()))?;
    connection.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))?;
    Ok(())
}

impl SqliteStore {
    pub fn open(data_dir: &Path) -> Result<Self, StorageError> {
        fs::create_dir_all(data_dir)?;
        let keys = KeyMaterial::load_or_create(data_dir)?;
        let connection = Connection::open_with_flags(
            data_dir.join("clipboard.db"),
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
        )?;
        apply_key(&connection, &keys)?;
        connection.busy_timeout(std::time::Duration::from_secs(3))?;
        let existing_version: i64 =
            connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if existing_version > 0 && existing_version < SCHEMA_VERSION {
            backup_before_migration(&connection, data_dir, &keys, existing_version)?;
        }
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;",
        )?;
        migrate(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
            data_dir: data_dir.to_path_buf(),
            keys,
        })
    }

    pub fn is_os_protected(&self) -> bool {
        self.keys.is_os_protected()
    }
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
    pub(crate) fn keys(&self) -> KeyMaterial {
        self.keys.clone()
    }

    pub fn add(&self, item: NewItem) -> Result<(i64, bool), StorageError> {
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        if let Some(id) = connection
            .query_row("SELECT id FROM items WHERE hash=?1", [&item.hash], |row| {
                row.get(0)
            })
            .optional()?
        {
            connection.execute(
                "UPDATE items SET last_used_at=?1, use_count=use_count+1 WHERE id=?2",
                params![now_ms(), id],
            )?;
            connection.execute("DELETE FROM sync_tombstones WHERE hash=?1", [&item.hash])?;
            return Ok((id, false));
        }
        connection.execute(
            r#"INSERT INTO items
            (kind,text,html,preview,auto_kind,hash,blob_name,thumb,width,height,bytes,source_app,created_at,last_used_at)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?13)"#,
            params![item.kind,item.text,item.html,item.preview,item.auto_kind,item.hash,item.blob_name,
                item.thumb,item.width,item.height,item.bytes as i64,item.source_app,now_ms()],
        )?;
        connection.execute("DELETE FROM sync_tombstones WHERE hash=?1", [&item.hash])?;
        Ok((connection.last_insert_rowid(), true))
    }

    pub fn list(&self, query: &ListQuery) -> Result<ListResult, StorageError> {
        // 子树解析要访问连接，必须在 list 自己加锁前完成，否则重入死锁。
        let group_subtree = match query.group_id {
            Some(0) | None => None,
            Some(group_id) => Some(self.group_subtree_ids(group_id)?),
        };
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let mut clauses = Vec::<String>::new();
        let mut values = Vec::<SqlValue>::new();
        if let Some(kind) = query.kind.as_ref().filter(|value| !value.is_empty()) {
            clauses.push("i.kind=?".to_string());
            values.push(kind.clone().into());
        }
        if let Some(kind) = query.auto_kind.as_ref().filter(|value| !value.is_empty()) {
            clauses.push("i.auto_kind=?".to_string());
            values.push(kind.clone().into());
        }
        if query.pinned_only.unwrap_or(false) {
            clauses.push("i.pinned=1".to_string());
        }
        if let Some(group_id) = query.group_id {
            if group_id == 0 {
                // 0 是保留值：未分组。
                clauses.push("i.group_id IS NULL".to_string());
            } else if let Some(ids) = group_subtree {
                if ids.is_empty() {
                    clauses.push("1=0".to_string());
                } else {
                    clauses.push(format!(
                        "i.group_id IN ({})",
                        ids.iter()
                            .map(|id| id.to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    ));
                }
            }
        }
        if let Some(tag) = query.tag.as_ref().filter(|value| !value.is_empty()) {
            clauses.push("EXISTS (SELECT 1 FROM item_tags it JOIN tags t ON t.id=it.tag_id WHERE it.item_id=i.id AND t.name=?)".to_string());
            values.push(tag.clone().into());
        }
        let needle = query.q.as_deref().unwrap_or_default().trim();
        if !needle.is_empty() {
            let escaped = needle
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            let like = format!("%{escaped}%");
            if needle.chars().count() >= 3 {
                clauses.push("(i.id IN (SELECT rowid FROM items_fts WHERE items_fts MATCH ?) OR i.source_app LIKE ? ESCAPE '\\')".to_string());
                values.push(format!("\"{}\"", needle.replace('"', "\"\"")).into());
                values.push(like.into());
            } else {
                clauses.push("(i.preview LIKE ? ESCAPE '\\' OR i.text LIKE ? ESCAPE '\\' OR i.source_app LIKE ? ESCAPE '\\')".to_string());
                values.extend([
                    SqlValue::from(like.clone()),
                    SqlValue::from(like.clone()),
                    SqlValue::from(like),
                ]);
            }
        }
        let clause = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        let total = connection.query_row(
            &format!("SELECT count(*) FROM items i {clause}"),
            params_from_iter(values.iter()),
            |row| row.get::<_, i64>(0),
        )? as usize;
        let mut list_values = values;
        list_values.push((query.limit.unwrap_or(300).clamp(1, 1_000) as i64).into());
        list_values.push((query.offset.unwrap_or(0) as i64).into());
        let sql = format!(
            "{SELECT_BASE} {clause} ORDER BY i.pinned DESC,i.last_used_at DESC LIMIT ? OFFSET ?"
        );
        let mut statement = connection.prepare(&sql)?;
        let items = statement
            .query_map(params_from_iter(list_values.iter()), row_to_item)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ListResult { items, total })
    }

    pub fn get(&self, id: i64) -> Result<Option<ClipItem>, StorageError> {
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        Ok(connection
            .query_row(&format!("{SELECT_BASE} WHERE i.id=?1"), [id], row_to_item)
            .optional()?)
    }

    pub fn related(&self, id: i64, limit: usize) -> Result<Vec<ClipItem>, StorageError> {
        let Some(base) = self.get(id)? else {
            return Ok(Vec::new());
        };
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let sql = format!("{SELECT_BASE} WHERE i.id<>?1 AND abs(i.last_used_at-?2)<=5000 ORDER BY abs(i.last_used_at-?2),i.last_used_at DESC LIMIT ?3");
        let mut statement = connection.prepare(&sql)?;
        let items = statement
            .query_map(
                params![id, base.last_used_at, limit.clamp(1, 5) as i64],
                row_to_item,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(items)
    }

    pub fn tags(&self) -> Result<Vec<String>, StorageError> {
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let mut statement = connection.prepare("SELECT t.name FROM tags t JOIN item_tags it ON it.tag_id=t.id GROUP BY t.id ORDER BY count(*) DESC,t.name")?;
        let tags = statement
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(tags)
    }

    // ---- 分组 / 备注 / 条目热键 ----

    pub fn groups(&self) -> Result<Vec<Group>, StorageError> {
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let mut statement = connection.prepare(
            "SELECT g.id, g.parent_id, g.name, (SELECT count(*) FROM items i WHERE i.group_id=g.id)
             FROM groups g ORDER BY g.sort, g.id",
        )?;
        let groups = statement
            .query_map([], |row| {
                Ok(Group {
                    id: row.get(0)?,
                    parent_id: row.get(1)?,
                    name: row.get(2)?,
                    count: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(groups)
    }

    pub fn group_create(&self, name: &str, parent_id: Option<i64>) -> Result<i64, StorageError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StorageError::Io(std::io::Error::other("group name is empty")));
        }
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        // 不允许把分组挂到不存在的父级或自己后代下面，避免成环。
        if let Some(parent) = parent_id {
            let exists: bool = connection
                .query_row("SELECT 1 FROM groups WHERE id=?1", [parent], |_| Ok(true))
                .unwrap_or(false);
            if !exists {
                return Err(StorageError::Io(std::io::Error::other("parent group not found")));
            }
        }
        connection.execute(
            "INSERT INTO groups(parent_id, name) VALUES (?1, ?2)",
            params![parent_id, name],
        )?;
        Ok(connection.last_insert_rowid())
    }

    pub fn group_rename(&self, id: i64, name: &str) -> Result<(), StorageError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StorageError::Io(std::io::Error::other("group name is empty")));
        }
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        connection.execute("UPDATE groups SET name=?2 WHERE id=?1", params![id, name])?;
        Ok(())
    }

    pub fn group_delete(&self, id: i64) -> Result<(), StorageError> {
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        // 外键 ON DELETE SET NULL/CASCADE 负责条目与子分组的归属。
        connection.execute("DELETE FROM groups WHERE id=?1", [id])?;
        Ok(())
    }

    fn group_subtree_ids(&self, root: i64) -> Result<Vec<i64>, StorageError> {
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let mut frontier = vec![root];
        let mut visited = Vec::new();
        while let Some(current) = frontier.pop() {
            if visited.contains(&current) {
                continue;
            }
            visited.push(current);
            let mut statement = connection.prepare("SELECT id FROM groups WHERE parent_id=?1")?;
            let children = statement
                .query_map([current], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            frontier.extend(children);
        }
        Ok(visited)
    }

    pub fn item_set_group(&self, id: i64, group_id: Option<i64>) -> Result<(), StorageError> {
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let changed = connection.execute(
            "UPDATE items SET group_id=?2 WHERE id=?1",
            params![id, group_id],
        )?;
        if changed == 0 {
            return Err(StorageError::Io(std::io::Error::other("item not found")));
        }
        Ok(())
    }

    pub fn set_item_note(&self, id: i64, note: Option<&str>) -> Result<(), StorageError> {
        let clean = note.map(str::trim).filter(|value| !value.is_empty());
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let changed = connection.execute(
            "UPDATE items SET note=?2 WHERE id=?1",
            params![id, clean],
        )?;
        if changed == 0 {
            return Err(StorageError::Io(std::io::Error::other("item not found")));
        }
        Ok(())
    }

    /// 设置条目热键；热键字符串全局唯一（部分唯一索引保证），重复分配返回错误。
    pub fn set_item_hotkey(&self, id: i64, hotkey: Option<&str>) -> Result<(), StorageError> {
        let clean = hotkey.map(str::trim).filter(|value| !value.is_empty());
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let changed = connection.execute(
            "UPDATE items SET hotkey=?2 WHERE id=?1",
            params![id, clean],
        )?;
        if changed == 0 {
            return Err(StorageError::Io(std::io::Error::other("item not found")));
        }
        Ok(())
    }

    pub fn item_hotkeys(&self) -> Result<Vec<(i64, String)>, StorageError> {
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let mut statement =
            connection.prepare("SELECT id, hotkey FROM items WHERE hotkey IS NOT NULL")?;
        let pairs = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(pairs)
    }

    pub fn set_tags(&self, id: i64, names: &[String]) -> Result<(), StorageError> {
        let mut connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM item_tags WHERE item_id=?1", [id])?;
        let mut clean = names
            .iter()
            .map(|name| name.trim())
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>();
        clean.sort();
        clean.dedup();
        clean.truncate(12);
        for name in clean {
            transaction.execute("INSERT OR IGNORE INTO tags(name) VALUES (?1)", [name])?;
            let tag_id: i64 =
                transaction.query_row("SELECT id FROM tags WHERE name=?1", [name], |row| {
                    row.get(0)
                })?;
            transaction.execute(
                "INSERT OR IGNORE INTO item_tags(item_id,tag_id) VALUES (?1,?2)",
                params![id, tag_id],
            )?;
        }
        transaction.execute(
            "DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM item_tags)",
            [],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn toggle_pin(&self, id: i64) -> Result<(), StorageError> {
        self.execute("UPDATE items SET pinned=1-pinned WHERE id=?1", id)
    }
    pub fn touch(&self, id: i64) -> Result<(), StorageError> {
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        connection.execute(
            "UPDATE items SET last_used_at=?1,use_count=use_count+1 WHERE id=?2",
            params![now_ms(), id],
        )?;
        Ok(())
    }
    fn execute(&self, sql: &str, id: i64) -> Result<(), StorageError> {
        self.connection
            .lock()
            .map_err(|_| StorageError::Poisoned)?
            .execute(sql, [id])?;
        Ok(())
    }

    pub fn remove(&self, id: i64) -> Result<(), StorageError> {
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let item = connection
            .query_row(
                "SELECT hash,blob_name FROM items WHERE id=?1",
                [id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()?;
        let Some((hash, blob)) = item else {
            return Ok(());
        };
        connection.execute(
            "INSERT INTO sync_tombstones(hash,deleted_at) VALUES (?1,?2) ON CONFLICT(hash) DO UPDATE SET deleted_at=max(deleted_at,excluded.deleted_at)",
            params![hash, now_ms()],
        )?;
        connection.execute("DELETE FROM items WHERE id=?1", [id])?;
        drop(connection);
        if let Some(hash) = blob {
            self.gc_blob(&hash)?;
        }
        Ok(())
    }

    pub fn clear_all(&self) -> Result<(), StorageError> {
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let deleted_at = now_ms();
        connection.execute(
            "INSERT INTO sync_tombstones(hash,deleted_at) SELECT hash,?1 FROM items WHERE pinned=0 AND hotkey IS NULL ON CONFLICT(hash) DO UPDATE SET deleted_at=max(deleted_at,excluded.deleted_at)",
            [deleted_at],
        )?;
        connection.execute_batch("DELETE FROM items WHERE pinned=0 AND hotkey IS NULL; DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM item_tags);")?;
        drop(connection);
        self.gc_orphan_blobs()?;
        Ok(())
    }

    pub fn stats(&self) -> Result<Stats, StorageError> {
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        Ok(connection.query_row("SELECT count(*),coalesce(sum(pinned),0),coalesce(sum(kind='image'),0),coalesce(sum(bytes),0) FROM items", [], |row| Ok(Stats {
            total: row.get::<_,i64>(0)? as usize, pinned: row.get::<_,i64>(1)? as usize,
            images: row.get::<_,i64>(2)? as usize, bytes: row.get::<_,i64>(3)? as usize,
        }))?)
    }

    pub fn prune(&self, max_items: usize, max_days: u32) -> Result<usize, StorageError> {
        let mut connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let transaction = connection.transaction()?;
        let mut removed = 0;
        if max_days > 0 {
            let cutoff = now_ms() - max_days as i64 * 86_400_000;
            transaction.execute(
                "INSERT INTO sync_tombstones(hash,deleted_at) SELECT hash,?1 FROM items WHERE pinned=0 AND hotkey IS NULL AND last_used_at<?2 ON CONFLICT(hash) DO UPDATE SET deleted_at=max(deleted_at,excluded.deleted_at)",
                params![now_ms(), cutoff],
            )?;
            removed += transaction.execute(
                "DELETE FROM items WHERE pinned=0 AND hotkey IS NULL AND last_used_at<?1",
                [cutoff],
            )?;
        }
        if max_items > 0 {
            transaction.execute(
                "INSERT INTO sync_tombstones(hash,deleted_at) SELECT hash,?1 FROM items WHERE pinned=0 AND hotkey IS NULL AND id NOT IN (SELECT id FROM items WHERE pinned=0 AND hotkey IS NULL ORDER BY last_used_at DESC LIMIT ?2) ON CONFLICT(hash) DO UPDATE SET deleted_at=max(deleted_at,excluded.deleted_at)",
                params![now_ms(), max_items as i64],
            )?;
            removed+=transaction.execute("DELETE FROM items WHERE pinned=0 AND hotkey IS NULL AND id NOT IN (SELECT id FROM items WHERE pinned=0 AND hotkey IS NULL ORDER BY last_used_at DESC LIMIT ?1)",[max_items as i64])?;
        }
        transaction.execute(
            "DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM item_tags)",
            [],
        )?;
        transaction.commit()?;
        drop(connection);
        if removed > 0 {
            self.gc_orphan_blobs()?;
        }
        Ok(removed)
    }

    pub fn put_blob(&self, hash: &str, png: &[u8]) -> Result<String, StorageError> {
        let path = self.blob_path(hash);
        if !path.exists() {
            fs::create_dir_all(path.parent().expect("blob shard"))?;
            fs::write(path, self.keys.seal_blob(png)?)?;
        }
        Ok(hash.to_string())
    }
    pub fn image_png(&self, id: i64) -> Result<Option<Vec<u8>>, StorageError> {
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let hash = connection
            .query_row("SELECT blob_name FROM items WHERE id=?1", [id], |row| {
                row.get::<_, Option<String>>(0)
            })
            .optional()?
            .flatten();
        drop(connection);
        let Some(hash) = hash else { return Ok(None) };
        let path = self.blob_path(&hash);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(self.keys.open_blob(&fs::read(path)?)?))
    }

    pub fn image_png_by_hash(&self, hash: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let path = self.blob_path(hash);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(self.keys.open_blob(&fs::read(path)?)?))
    }

    pub fn all_for_sync(&self) -> Result<Vec<SyncItem>, StorageError> {
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let mut statement =
            connection.prepare(&format!("{SELECT_BASE} ORDER BY i.last_used_at DESC"))?;
        let items = statement
            .query_map([], row_to_item)?
            .map(|result| result.map(sync_item_from_clip))
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)?;
        Ok(items)
    }

    pub fn sync_tombstones(&self) -> Result<Vec<SyncTombstone>, StorageError> {
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let mut statement = connection
            .prepare("SELECT hash,deleted_at FROM sync_tombstones ORDER BY deleted_at")?;
        let tombstones = statement
            .query_map([], |row| {
                Ok(SyncTombstone {
                    hash: row.get(0)?,
                    deleted_at: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)?;
        Ok(tombstones)
    }

    pub fn apply_sync_item(
        &self,
        item: &SyncItem,
        image_png: Option<&[u8]>,
    ) -> Result<bool, StorageError> {
        if let (Some(blob_hash), Some(png)) = (item.blob_name.as_deref(), image_png) {
            self.put_blob(blob_hash, png)?;
        }
        let thumb = item.thumb.as_deref().and_then(|value| {
            value
                .split_once(',')
                .and_then(|(_, payload)| BASE64.decode(payload).ok())
        });
        let mut connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let transaction = connection.transaction()?;
        let tombstone = transaction
            .query_row(
                "SELECT deleted_at FROM sync_tombstones WHERE hash=?1",
                [&item.hash],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if tombstone.is_some_and(|deleted_at| deleted_at >= item.last_used_at) {
            transaction.commit()?;
            return Ok(false);
        }
        let existing = transaction
            .query_row(
                "SELECT id,last_used_at FROM items WHERE hash=?1",
                [&item.hash],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        if existing.is_some_and(|(_, last_used_at)| last_used_at > item.last_used_at) {
            transaction.execute(
                "DELETE FROM sync_tombstones WHERE hash=?1 AND deleted_at<?2",
                params![item.hash, existing.expect("checked above").1],
            )?;
            transaction.commit()?;
            return Ok(false);
        }
        transaction.execute(
            r#"INSERT INTO items
            (kind,text,html,preview,auto_kind,hash,blob_name,thumb,width,height,bytes,source_app,pinned,use_count,created_at,last_used_at)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
            ON CONFLICT(hash) DO UPDATE SET
              kind=excluded.kind,text=excluded.text,html=excluded.html,preview=excluded.preview,
              auto_kind=excluded.auto_kind,blob_name=excluded.blob_name,thumb=excluded.thumb,
              width=excluded.width,height=excluded.height,bytes=excluded.bytes,source_app=excluded.source_app,
              pinned=excluded.pinned,use_count=excluded.use_count,
              created_at=min(items.created_at,excluded.created_at),last_used_at=excluded.last_used_at"#,
            params![
                item.kind,item.text,item.html,item.preview,item.auto_kind,item.hash,item.blob_name,thumb,
                item.width,item.height,item.bytes as i64,item.source_app,item.pinned as i64,
                item.use_count as i64,item.created_at,item.last_used_at
            ],
        )?;
        let id: i64 =
            transaction.query_row("SELECT id FROM items WHERE hash=?1", [&item.hash], |row| {
                row.get(0)
            })?;
        transaction.execute("DELETE FROM item_tags WHERE item_id=?1", [id])?;
        for name in item
            .tags
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .take(12)
        {
            transaction.execute("INSERT OR IGNORE INTO tags(name) VALUES (?1)", [name])?;
            let tag_id: i64 =
                transaction.query_row("SELECT id FROM tags WHERE name=?1", [name], |row| {
                    row.get(0)
                })?;
            transaction.execute(
                "INSERT OR IGNORE INTO item_tags(item_id,tag_id) VALUES (?1,?2)",
                params![id, tag_id],
            )?;
        }
        transaction.execute("DELETE FROM sync_tombstones WHERE hash=?1", [&item.hash])?;
        transaction.commit()?;
        Ok(true)
    }

    pub fn apply_sync_tombstone(&self, tombstone: &SyncTombstone) -> Result<bool, StorageError> {
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let item = connection
            .query_row(
                "SELECT id,last_used_at,blob_name FROM items WHERE hash=?1",
                [&tombstone.hash],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;
        if item
            .as_ref()
            .is_some_and(|(_, last_used_at, _)| *last_used_at > tombstone.deleted_at)
        {
            return Ok(false);
        }
        connection.execute(
            "INSERT INTO sync_tombstones(hash,deleted_at) VALUES (?1,?2) ON CONFLICT(hash) DO UPDATE SET deleted_at=max(deleted_at,excluded.deleted_at)",
            params![tombstone.hash,tombstone.deleted_at],
        )?;
        let removed = if let Some((id, last_used_at, _)) = &item {
            if *last_used_at <= tombstone.deleted_at {
                connection.execute("DELETE FROM items WHERE id=?1", [id])? > 0
            } else {
                false
            }
        } else {
            false
        };
        let blob = item.and_then(|(_, _, blob)| blob);
        drop(connection);
        if removed {
            if let Some(hash) = blob {
                self.gc_blob(&hash)?;
            }
        }
        Ok(removed)
    }
    fn blob_path(&self, hash: &str) -> PathBuf {
        self.data_dir
            .join("blobs")
            .join(&hash[..hash.len().min(2)])
            .join(format!("{hash}.bin"))
    }
    fn gc_blob(&self, hash: &str) -> Result<(), StorageError> {
        let referenced = self
            .connection
            .lock()
            .map_err(|_| StorageError::Poisoned)?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM items WHERE blob_name=?1)",
                [hash],
                |row| row.get::<_, bool>(0),
            )?;
        if !referenced {
            let _ = fs::remove_file(self.blob_path(hash));
        }
        Ok(())
    }
    pub fn gc_orphan_blobs(&self) -> Result<usize, StorageError> {
        let connection = self.connection.lock().map_err(|_| StorageError::Poisoned)?;
        let refs = connection
            .prepare("SELECT DISTINCT blob_name FROM items WHERE blob_name IS NOT NULL")?
            .query_map([], |row| row.get(0))?
            .collect::<Result<HashSet<String>, _>>()?;
        drop(connection);
        let root = self.data_dir.join("blobs");
        let mut removed = 0;
        if root.exists() {
            for shard in fs::read_dir(root)? {
                for file in fs::read_dir(shard?.path())? {
                    let file = file?;
                    if file.path().extension().is_some_and(|e| e == "bin") {
                        if let Some(hash) = file
                            .path()
                            .file_stem()
                            .map(|v| v.to_string_lossy().into_owned())
                        {
                            if !refs.contains(&hash) {
                                fs::remove_file(file.path())?;
                                removed += 1;
                            }
                        }
                    }
                }
            }
        }
        Ok(removed)
    }
}

fn backup_before_migration(
    source: &Connection,
    data_dir: &Path,
    keys: &KeyMaterial,
    version: i64,
) -> Result<(), StorageError> {
    let backup_dir = data_dir.join("migration-backups");
    fs::create_dir_all(&backup_dir)?;
    let backup_path = backup_dir.join(format!("clipboard-v{version}-{}.db", now_ms()));
    let mut destination = Connection::open(&backup_path)?;
    apply_key(&destination, keys)?;
    let result = rusqlite::backup::Backup::new(source, &mut destination)?.run_to_completion(
        64,
        std::time::Duration::from_millis(10),
        None,
    );
    drop(destination);
    if let Err(error) = result {
        let _ = fs::remove_file(&backup_path);
        return Err(error.into());
    }
    let verification = Connection::open_with_flags(&backup_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    apply_key(&verification, keys)?;
    let backed_up_version: i64 =
        verification.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if backed_up_version != version {
        let _ = fs::remove_file(&backup_path);
        return Err(StorageError::Io(std::io::Error::other(
            "migration backup verification failed",
        )));
    }
    Ok(())
}

fn row_to_item(row: &Row<'_>) -> rusqlite::Result<ClipItem> {
    let thumb: Option<Vec<u8>> = row.get("thumb")?;
    let tags: Option<String> = row.get("tags")?;
    Ok(ClipItem {
        id: row.get("id")?,
        kind: row.get("kind")?,
        text: row.get("text")?,
        html: row.get("html")?,
        preview: row.get("preview")?,
        hash: row.get("hash")?,
        thumb: thumb.map(|data| format!("data:image/png;base64,{}", BASE64.encode(data))),
        width: row.get("width")?,
        height: row.get("height")?,
        bytes: row.get::<_, i64>("bytes")? as usize,
        source_app: row.get("source_app")?,
        auto_kind: row.get("auto_kind")?,
        tags: tags
            .map(|value| value.split(TAG_SEPARATOR).map(str::to_string).collect())
            .unwrap_or_default(),
        pinned: row.get::<_, i64>("pinned")? != 0,
        use_count: row.get::<_, i64>("use_count")? as u32,
        created_at: row.get("created_at")?,
        last_used_at: row.get("last_used_at")?,
        note: row.get("note")?,
        hotkey: row.get("hotkey")?,
        group_id: row.get("group_id")?,
    })
}

fn migrate(connection: &Connection) -> Result<(), rusqlite::Error> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version == 0 {
        connection.execute_batch("BEGIN;")?;
        if let Err(error) = connection.execute_batch(SCHEMA) {
            let _ = connection.execute_batch("ROLLBACK;");
            return Err(error);
        }
        connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        connection.execute_batch("COMMIT;")?;
    } else if version < SCHEMA_VERSION {
        connection.execute_batch("BEGIN IMMEDIATE;")?;
        let result = (|| {
            if version < 2 {
                connection.execute_batch(
                    "CREATE INDEX IF NOT EXISTS idx_items_auto_kind ON items(auto_kind);",
                )?;
            }
            if version < 3 {
                backfill_auto_kind(connection, "key")?;
            }
            if version < 4 {
                backfill_auto_kind(connection, "model")?;
            }
            if version < 5 {
                connection.execute_batch("ALTER TABLE items ADD COLUMN html TEXT;")?;
            }
            if version < 6 {
                connection.execute_batch(
                    "CREATE TABLE IF NOT EXISTS sync_tombstones (hash TEXT PRIMARY KEY, deleted_at INTEGER NOT NULL);",
                )?;
            }
            if version < 7 {
                connection.execute_batch(
                    "ALTER TABLE items ADD COLUMN note TEXT;
                     ALTER TABLE items ADD COLUMN hotkey TEXT;
                     CREATE TABLE groups (
                       id INTEGER PRIMARY KEY AUTOINCREMENT,
                       parent_id INTEGER REFERENCES groups(id) ON DELETE CASCADE,
                       name TEXT NOT NULL,
                       sort INTEGER NOT NULL DEFAULT 0
                     );
                     ALTER TABLE items ADD COLUMN group_id INTEGER REFERENCES groups(id) ON DELETE SET NULL;
                     CREATE INDEX idx_items_group ON items(group_id);
                     CREATE UNIQUE INDEX idx_items_hotkey ON items(hotkey) WHERE hotkey IS NOT NULL;",
                )?;
                // 备注纳入全文索引需要重建 FTS 表与触发器；trigram 对中文友好。
                connection.execute_batch(
                    "DROP TRIGGER items_ai; DROP TRIGGER items_ad; DROP TRIGGER items_au;
                     DROP TABLE items_fts;
                     CREATE VIRTUAL TABLE items_fts USING fts5(
                       text, preview, note, content='items', content_rowid='id', tokenize='trigram'
                     );
                     INSERT INTO items_fts(rowid, text, preview, note) SELECT id, text, preview, note FROM items;
                     CREATE TRIGGER items_ai AFTER INSERT ON items BEGIN
                       INSERT INTO items_fts(rowid, text, preview, note) VALUES (new.id, new.text, new.preview, new.note);
                     END;
                     CREATE TRIGGER items_ad AFTER DELETE ON items BEGIN
                       INSERT INTO items_fts(items_fts, rowid, text, preview, note) VALUES ('delete', old.id, old.text, old.preview, old.note);
                     END;
                     CREATE TRIGGER items_au AFTER UPDATE OF text, preview, note ON items BEGIN
                       INSERT INTO items_fts(items_fts, rowid, text, preview, note) VALUES ('delete', old.id, old.text, old.preview, old.note);
                       INSERT INTO items_fts(rowid, text, preview, note) VALUES (new.id, new.text, new.preview, new.note);
                     END;",
                )?;
            }
            connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            connection.execute_batch("COMMIT;")
        })();
        if result.is_err() {
            let _ = connection.execute_batch("ROLLBACK;");
        }
        result?;
    }
    Ok(())
}

fn sync_item_from_clip(item: ClipItem) -> SyncItem {
    let blob_name = (item.kind == "image").then(|| item.hash.clone());
    SyncItem {
        kind: item.kind,
        text: item.text,
        html: item.html,
        preview: item.preview,
        hash: item.hash,
        blob_name,
        thumb: item.thumb,
        width: item.width,
        height: item.height,
        bytes: item.bytes,
        source_app: item.source_app,
        auto_kind: item.auto_kind,
        tags: item.tags,
        pinned: item.pinned,
        use_count: item.use_count,
        created_at: item.created_at,
        last_used_at: item.last_used_at,
    }
}

fn backfill_auto_kind(connection: &Connection, target: &str) -> Result<(), rusqlite::Error> {
    let rows = connection
        .prepare("SELECT id,text FROM items WHERE kind='text' AND auto_kind='plain'")?
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (id, text) in rows {
        if text
            .as_deref()
            .is_some_and(|value| crate::classify::classify(value) == target)
        {
            connection.execute(
                "UPDATE items SET auto_kind=?1 WHERE id=?2",
                params![target, id],
            )?;
        }
    }
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
pub fn probe_existing(data_dir: &Path) -> Result<CompatibilityProbe, StorageError> {
    let keys = KeyMaterial::load_existing(data_dir)?;
    let connection = Connection::open_with_flags(
        data_dir.join("clipboard.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    apply_key(&connection, &keys)?;
    Ok(CompatibilityProbe {
        schema_version: connection.pragma_query_value(None, "user_version", |r| r.get(0))?,
        item_count: connection.query_row("SELECT count(*) FROM items", [], |r| r.get(0))?,
        sqlite_version: connection.query_row("SELECT sqlite_version()", [], |r| r.get(0))?,
        cipher: connection
            .pragma_query_value(None, "cipher", |r| r.get(0))
            .unwrap_or_else(|_| "sqleet(default)".into()),
        os_protected: keys.is_os_protected(),
    })
}

trait OptionalRow<T> {
    fn optional(self) -> rusqlite::Result<Option<T>>;
}
impl<T> OptionalRow<T> for rusqlite::Result<T> {
    fn optional(self) -> rusqlite::Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_item(hash: &str, text: &str) -> NewItem {
        NewItem {
            kind: "text".into(),
            text: Some(text.into()),
            html: None,
            preview: text.into(),
            auto_kind: "plain".into(),
            hash: hash.into(),
            blob_name: None,
            thumb: None,
            width: None,
            height: None,
            bytes: text.len(),
            source_app: Some("test.exe".into()),
        }
    }

    #[test]
    fn groups_crud_subtree_filter_and_note_search() {
        let directory = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(directory.path()).unwrap();
        let (parent_item, _) = store.add(text_item("hash-parent", "in parent")).unwrap();
        let (child_item, _) = store.add(text_item("hash-child", "in child")).unwrap();
        let (loose_item, _) = store.add(text_item("hash-loose", "no group")).unwrap();

        let parent = store.group_create("工作", None).unwrap();
        let child = store.group_create("代码", Some(parent)).unwrap();
        store.item_set_group(parent_item, Some(parent)).unwrap();
        store.item_set_group(child_item, Some(child)).unwrap();
        store.set_item_note(loose_item, Some("营业执照 复印件")).unwrap();

        // 直接计数与层级计数
        let groups = store.groups().unwrap();
        assert_eq!(groups.iter().find(|g| g.id == parent).unwrap().count, 1);
        assert_eq!(groups.iter().find(|g| g.id == child).unwrap().count, 1);

        // 选中父分组应包含子分组条目
        let in_tree = store
            .list(&ListQuery {
                group_id: Some(parent),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(in_tree.total, 2);

        // 未分组过滤
        let ungrouped = store
            .list(&ListQuery {
                group_id: Some(0),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(ungrouped.total, 1);
        assert_eq!(ungrouped.items[0].id, loose_item);

        // 备注纳入全文搜索（trigram：中文 >=3 字可命中）
        let by_note = store
            .list(&ListQuery {
                q: Some("营业执照".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_note.total, 1);
        assert_eq!(by_note.items[0].id, loose_item);

        // 删除父分组：条目回到未分组，子分组级联消失
        store.group_delete(parent).unwrap();
        assert!(store.groups().unwrap().iter().all(|g| g.id != child));
        let after = store
            .list(&ListQuery {
                group_id: Some(0),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(after.total, 3);
    }

    #[test]
    fn item_hotkeys_are_unique_and_survive_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(directory.path()).unwrap();
        let (first, _) = store.add(text_item("hk-a", "a")).unwrap();
        let (second, _) = store.add(text_item("hk-b", "b")).unwrap();

        store.set_item_hotkey(first, Some("Ctrl+Alt+0")).unwrap();
        // 同一热键分配给另一条目必须失败
        assert!(store.set_item_hotkey(second, Some("Ctrl+Alt+0")).is_err());
        store.set_item_hotkey(second, Some("Ctrl+Alt+9")).unwrap();
        assert_eq!(store.item_hotkeys().unwrap().len(), 2);

        // 清空与按天数清理都跳过绑了热键的条目
        store.clear_all().unwrap();
        assert_eq!(store.item_hotkeys().unwrap().len(), 2);
        store
            .prune(0, 1)
            .unwrap();
        assert_eq!(store.item_hotkeys().unwrap().len(), 2);
        // 解绑后可以重新分配
        store.set_item_hotkey(first, None).unwrap();
        store
            .set_item_hotkey(second, Some("Ctrl+Alt+0"))
            .unwrap();
    }

    /// v6 时代的最小库：没有 note/hotkey/group_id、没有 groups 表、FTS 只索引两列。
    const V6_LEGACY_SCHEMA: &str = r#"
CREATE TABLE items (
  id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, text TEXT, html TEXT,
  preview TEXT NOT NULL DEFAULT '', auto_kind TEXT NOT NULL DEFAULT 'plain',
  hash TEXT NOT NULL UNIQUE, blob_name TEXT, thumb BLOB, width INTEGER, height INTEGER,
  bytes INTEGER NOT NULL DEFAULT 0, source_app TEXT, pinned INTEGER NOT NULL DEFAULT 0,
  use_count INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, last_used_at INTEGER NOT NULL
);
CREATE INDEX idx_items_order ON items(pinned DESC, last_used_at DESC);
CREATE INDEX idx_items_kind ON items(kind);
CREATE INDEX idx_items_auto_kind ON items(auto_kind);
CREATE TABLE tags (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE);
CREATE TABLE item_tags (
  item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
  PRIMARY KEY (item_id, tag_id)
);
CREATE INDEX idx_item_tags_tag ON item_tags(tag_id);
CREATE TABLE sync_tombstones (hash TEXT PRIMARY KEY, deleted_at INTEGER NOT NULL);
CREATE VIRTUAL TABLE items_fts USING fts5(text, preview, content='items', content_rowid='id', tokenize='trigram');
CREATE TRIGGER items_ai AFTER INSERT ON items BEGIN
  INSERT INTO items_fts(rowid, text, preview) VALUES (new.id, new.text, new.preview);
END;
CREATE TRIGGER items_ad AFTER DELETE ON items BEGIN
  INSERT INTO items_fts(items_fts, rowid, text, preview) VALUES ('delete', old.id, old.text, old.preview);
END;
CREATE TRIGGER items_au AFTER UPDATE OF text, preview ON items BEGIN
  INSERT INTO items_fts(items_fts, rowid, text, preview) VALUES ('delete', old.id, old.text, old.preview);
  INSERT INTO items_fts(rowid, text, preview) VALUES (new.id, new.text, new.preview);
END;
INSERT INTO items(kind,text,preview,auto_kind,hash,bytes,created_at,last_used_at)
  VALUES ('text','v6 存量内容','v6 存量内容','plain','v6-hash',0,1,1);
"#;

    #[test]
    fn migration_v7_rebuilds_fts_and_keeps_legacy_content_searchable() {
        let directory = tempfile::tempdir().unwrap();
        let keys = KeyMaterial::load_or_create(directory.path()).unwrap();
        {
            let connection = Connection::open(directory.path().join("clipboard.db")).unwrap();
            apply_key(&connection, &keys).unwrap();
            connection.execute_batch(V6_LEGACY_SCHEMA).unwrap();
            connection.pragma_update(None, "user_version", 6).unwrap();
        }

        let store = SqliteStore::open(directory.path()).unwrap();
        assert_eq!(
            store
                .connection
                .lock()
                .unwrap()
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        // 重建 FTS 后存量内容仍可搜索
        let legacy = store
            .list(&ListQuery {
                q: Some("存量内容".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(legacy.total, 1);
        // 新增的备注列也可搜索，说明触发器已换到三列版本
        let id = legacy.items[0].id;
        store.set_item_note(id, Some("迁移后新增的备注")).unwrap();
        let by_note = store
            .list(&ListQuery {
                q: Some("新增的备注".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_note.total, 1);
        // 新列与分组表就位
        let group = store.group_create("迁移分组", None).unwrap();
        store.item_set_group(id, Some(group)).unwrap();
    }

    #[test]
    fn persistent_store_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(directory.path()).unwrap();
        let (first, created) = store.add(text_item("hash-a", "alpha")).unwrap();
        assert!(created);
        assert!(!store.add(text_item("hash-a", "alpha")).unwrap().1);
        store
            .set_tags(first, &["配置".into(), "API".into()])
            .unwrap();
        store.toggle_pin(first).unwrap();
        let listed = store.list(&ListQuery::default()).unwrap();
        assert_eq!(listed.total, 1);
        assert_eq!(listed.items[0].use_count, 1);
        assert!(listed.items[0].pinned);
        assert_eq!(store.tags().unwrap().len(), 2);
        assert_eq!(store.stats().unwrap().total, 1);
    }

    #[test]
    fn encrypted_blob_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(directory.path()).unwrap();
        let png = b"\x89PNG\r\nfixture";
        let blob = store.put_blob("abcdef", png).unwrap();
        let mut item = text_item("image-hash", "");
        item.kind = "image".into();
        item.text = None;
        item.blob_name = Some(blob);
        let (id, _) = store.add(item).unwrap();
        assert_eq!(
            store.image_png(id).unwrap().as_deref(),
            Some(png.as_slice())
        );
    }

    #[test]
    fn v4_migration_creates_verified_encrypted_backup() {
        let directory = tempfile::tempdir().unwrap();
        let keys = KeyMaterial::load_or_create(directory.path()).unwrap();
        let connection = Connection::open(directory.path().join("clipboard.db")).unwrap();
        apply_key(&connection, &keys).unwrap();
        // v4 库从 v6 遗留结构派生：再摘掉 html 列与 tombstones 表（v5/v6 迁移负责补）
        connection
            .execute_batch(
                &V6_LEGACY_SCHEMA
                    .replace("text TEXT, html TEXT,", "text TEXT,")
                    .replace(
                        "CREATE TABLE sync_tombstones (hash TEXT PRIMARY KEY, deleted_at INTEGER NOT NULL);",
                        "",
                    ),
            )
            .unwrap();
        connection.pragma_update(None, "user_version", 4).unwrap();
        drop(connection);

        let store = SqliteStore::open(directory.path()).unwrap();
        assert_eq!(
            store
                .connection
                .lock()
                .unwrap()
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        let backup_path = fs::read_dir(directory.path().join("migration-backups"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let backup =
            Connection::open_with_flags(backup_path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        apply_key(&backup, &keys).unwrap();
        assert_eq!(
            backup
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            4
        );
    }

    #[test]
    fn sync_tombstones_block_old_items_but_allow_newer_recopy() {
        let directory = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(directory.path()).unwrap();
        let (id, _) = store.add(text_item("sync-hash", "hello")).unwrap();
        let old_item = store.all_for_sync().unwrap().pop().unwrap();

        store.remove(id).unwrap();
        let tombstone = store.sync_tombstones().unwrap().pop().unwrap();
        assert!(!store.apply_sync_item(&old_item, None).unwrap());
        assert_eq!(store.list(&ListQuery::default()).unwrap().total, 0);

        let mut newer_item = old_item;
        newer_item.last_used_at = tombstone.deleted_at + 1;
        assert!(store.apply_sync_item(&newer_item, None).unwrap());
        assert_eq!(store.list(&ListQuery::default()).unwrap().total, 1);
        assert!(store.sync_tombstones().unwrap().is_empty());

        assert!(!store.apply_sync_tombstone(&tombstone).unwrap());
        assert_eq!(store.list(&ListQuery::default()).unwrap().total, 1);
        assert!(store.sync_tombstones().unwrap().is_empty());
    }

    #[test]
    fn opens_real_electron_database_read_only_when_requested() {
        let Some(path) = std::env::var_os("WCC_COMPAT_DATA_DIR") else {
            return;
        };
        let probe = probe_existing(Path::new(&path)).unwrap();
        assert_eq!(probe.schema_version, 4);
        assert!(probe.sqlite_version.starts_with("3."));
        assert!(probe.os_protected);

        let data_dir = Path::new(&path);
        let keys = KeyMaterial::load_existing(data_dir).unwrap();
        let connection = Connection::open_with_flags(
            data_dir.join("clipboard.db"),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        apply_key(&connection, &keys).unwrap();
        let blob: Option<String> = connection
            .query_row(
                "SELECT blob_name FROM items WHERE blob_name IS NOT NULL LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        if let Some(hash) = blob {
            let sealed = fs::read(
                data_dir
                    .join("blobs")
                    .join(&hash[..2])
                    .join(format!("{hash}.bin")),
            )
            .unwrap();
            assert!(keys.open_blob(&sealed).unwrap().starts_with(b"\x89PNG"));
        }
    }
}
