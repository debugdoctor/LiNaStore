use std::{error::Error, path::Path, result::Result, sync::Arc};
use rusqlite::Connection;
use uuid::Uuid;

const SQL_INIT: &str = r#"
CREATE TABLE IF NOT EXISTS link (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    ext TEXT NOT NULL,
    source_id TEXT NOT NULL,
    FOREIGN KEY (source_id) REFERENCES source (id)
);

CREATE INDEX IF NOT EXISTS link_name_idx ON link (name);
CREATE INDEX IF NOT EXISTS link_ext_idx ON link (ext);

CREATE TABLE IF NOT EXISTS source (
    id TEXT PRIMARY KEY,
    hash256 TEXT NOT NULL,
    compressed BOOLEAN NOT NULL DEFAULT(0),
    size INT NOT NULL DEFAULT(0),
    count INT NOT NULL DEFAULT(0),
    create_at TEXT NOT NULL,
    update_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS source_size_idx ON source (size);
"#;

// Core data models
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Link {
    pub id: String,
    pub name: String,
    pub ext: String,
    pub source_id: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Source {
    pub id: String,
    pub hash256: String,
    pub compressed: bool,
    pub size: u64,
    pub count: u64,
    pub create_at: String,
    pub update_at: String,
}

// DAO trait for database operations
#[derive(Debug, Clone)]
pub struct Dao {
    conn: Arc<Connection>,
}

impl Dao {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn Error>> {
        // Directory creation
        if let Some(parent_dir) = path.as_ref().parent() {
            std::fs::create_dir_all(parent_dir)
                .map_err(|e| Box::new(e) as Box<dyn Error>)?;
        }
        
        // Database connection
        let conn = Connection::open_with_flags(
            path, 
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | 
            rusqlite::OpenFlags::SQLITE_OPEN_CREATE
        )?;
        
        let dao = Self {
            conn: Arc::new(conn),
        };
        
        // Initialize schema from SQL file
        dao.init_from_sql_file()?;
        
        Ok(dao)
    }

    fn init_from_sql_file(&self) -> Result<(), Box<dyn Error>> {
        // Execute SQL statements
        self.conn.execute_batch(SQL_INIT)
            .map_err(|e| Box::new(e) as Box<dyn Error>)?;
        
        Ok(())
    }
    // Link operations
    pub fn insert_link(&self, name: &str, ext: &str, source_id: &str) -> Result<(), Box<dyn Error>> {
        self.conn.execute(
            "INSERT INTO link (id, name, ext, source_id) VALUES (?1, ?2, ?3, ?4)",
            [
                Uuid::new_v4().to_string(), 
                name.to_string(), 
                ext.to_string(), 
                source_id.to_string()
            ],
        ).map_err(|e| Box::new(e) as Box<dyn Error>)?;
        Ok(())
    }

    pub fn get_links_by_name(&self, name: &str, fuzzy: bool) -> Result<Vec<Link>, Box<dyn Error>> {
        let mut stmt = if fuzzy {
            self.conn.prepare(
                "SELECT id, name, ext, source_id FROM link WHERE name LIKE ?1"
            )?
            } else {
                self.conn.prepare(
                    "SELECT id, name, ext, source_id FROM link WHERE name = ?1"
                )?
            };
        
        let links = stmt.query_map([name], |row| {
            Ok(Link {
                id: row.get(0)?,
                name: row.get(1)?,
                ext: row.get(2)?,
                source_id: row.get(3)?,
            })
        })?.collect::<Result<_, _>>()?;
        
        Ok(links)
    }

    pub fn get_links_by_ext(&self, ext: &str) -> Result<Vec<Link>, Box<dyn Error>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, ext, source_id FROM link WHERE ext = ?1"
        )?;
        
        let links = stmt.query_map([ext], |row| {
            Ok(Link {
                id: row.get(0)?,
                name: row.get(1)?,
                ext: row.get(2)?,
                source_id: row.get(3)?,
            })
        })?.collect::<Result<_, _>>()?;
        
        Ok(links)
    }

    pub fn get_n_links(&self, n: u32) -> Result<Vec<Link>, Box<dyn Error>> {
        let mut stmt;
        
        if n == 0 {
            stmt = self.conn.prepare(
            "SELECT id, name, ext, source_id FROM link"
            )?;
        } else {
            stmt = self.conn.prepare(
                "SELECT id, name, ext, source_id FROM link LIMIT ?1"
            )?;
        }
        
        let links = stmt.query_map([n], |row| {
            Ok(Link {
                id: row.get(0)?,
                name: row.get(1)?,
                ext: row.get(2)?,
                source_id: row.get(3)?,
            })
        })?.collect::<Result<_, _>>()?;

        Ok(links)
    }

    pub fn delete_link_by_id(&self, id: &str) -> Result<(), Box<dyn Error>> {
        self.conn.execute(
            "DELETE FROM link WHERE id = ?1",
            [id]
        ).map_err(|e| Box::new(e) as Box<dyn Error>)?;
        Ok(())
    }

    // Source operations
    pub fn insert_source(&self, id: &str, hash256: &str, compressed: bool, size: u64) -> Result<(), Box<dyn Error>> {
        self.conn.execute(
            "INSERT INTO source (id, hash256, compressed, size, count, create_at, update_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            [
                id.to_string(),
                hash256.to_string(),
                (compressed as u8).to_string(),
                size.to_string(),
                "1".to_string(),
                chrono::Utc::now().naive_local().format("%Y-%m-%d %H:%M:%S").to_string(),
                chrono::Utc::now().naive_local().format("%Y-%m-%d %H:%M:%S").to_string()
            ],
        ).map_err(|e| Box::new(e) as Box<dyn Error>)?;
        Ok(())
    }

    pub fn get_source_by_id(&self, id: &str) -> Result<Option<Source>, Box<dyn Error>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, hash256, compressed, size, count, create_at, update_at FROM source WHERE id = ?1"
        )?;
        
        let source = stmt.query_map([id], |row| {
            Ok(Source {
                id: row.get(0)?,
                hash256: row.get(1)?,
                compressed: row.get(2)?,
                size: row.get(3)?,
                count: row.get(4)?,
                create_at: row.get(5)?,
                update_at: row.get(6)?,
            })
        })?.next().transpose()?;
        
        Ok(source)
    }

    pub fn get_source_by_hash256(&self, hash256: &str) -> Result<Option<Source>, Box<dyn Error>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, hash256, compressed, size, count, create_at, update_at FROM source WHERE hash256 = ?1"
        )?;
        
        let source = stmt.query_map([hash256], |row| {
            Ok(Source {
                id: row.get(0)?,
                hash256: row.get(1)?,
                compressed: row.get(2)?,
                size: row.get(3)?,
                count: row.get(4)?,
                create_at: row.get(5)?,
                update_at: row.get(6)?,
            })
        })?.next().transpose()?;
        
        Ok(source)
    }

    pub fn update_link_source_id(&self, link_id: &str, new_source_id: &str) -> Result<(), Box<dyn Error>> {
        self.conn.execute(
            "UPDATE link SET source_id = ?1 WHERE id = ?2",
            [new_source_id, link_id]
        ).map_err(|e| Box::new(e) as Box<dyn Error>)?;
        Ok(())
   }

    pub fn update_source(&self, id: &str, new_hash256: &str, new_compressed: bool, new_size: u64, new_count: u64) -> Result<(), Box<dyn Error>> {
        self.conn.execute(
            "UPDATE source SET hash256 = ?2, compressed = ?3, size = ?4, count = ?5, update_at = datetime('now') WHERE id = ?1",
            [id, new_hash256, &(new_compressed as u8).to_string() , &new_size.to_string(), &new_count.to_string()]
        ).map_err(|e| Box::new(e) as Box<dyn Error>)?;
        Ok(())
    }

    pub fn delete_source_by_id(&self, id: &str) -> Result<(), Box<dyn Error>> {
        self.conn.execute("DELETE FROM source WHERE id = ?1", [id])?;
        
        Ok(())
    }
}