//! SQLite 저장소(같은 프로세스 안, 파일 하나). 계정·세션·메일 코드·패스키·회의·사용량을 둔다.
//! 연결은 하나를 잠금으로 공유하고, 호출은 `spawn_blocking`에서 돌려 런타임 스레드를 막지 않는다.
//! 잠금은 블로킹 스레드 안에서만 잡으므로 await 중에 잠금을 쥐는 일이 없다.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

pub use rusqlite::{OptionalExtension, params};

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

/// 스키마 판. 바꿀 때는 아래 MIGRATIONS 끝에 추가만 한다(이미 배포된 판은 고치지 않는다).
const MIGRATIONS: &[&str] = &[
    // 1: 계정·세션·메일 코드·패스키
    "
    CREATE TABLE users (
        id INTEGER PRIMARY KEY,
        email TEXT NOT NULL UNIQUE,
        name TEXT NOT NULL DEFAULT '',
        password_hash TEXT,
        email_verified INTEGER NOT NULL DEFAULT 0,
        google_sub TEXT UNIQUE,
        locale TEXT NOT NULL DEFAULT 'en',
        totp_secret TEXT,
        totp_enabled INTEGER NOT NULL DEFAULT 0,
        totp_last_step INTEGER NOT NULL DEFAULT 0,
        plan TEXT NOT NULL DEFAULT 'none',
        plan_renews_at INTEGER,
        plan_canceled INTEGER NOT NULL DEFAULT 0,
        paddle_customer_id TEXT,
        paddle_subscription_id TEXT,
        extra_seconds INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE sessions (
        token_hash TEXT PRIMARY KEY,
        user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        created_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL,
        user_agent TEXT NOT NULL DEFAULT ''
    );
    CREATE INDEX sessions_user ON sessions(user_id);
    CREATE TABLE email_codes (
        id INTEGER PRIMARY KEY,
        user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        kind TEXT NOT NULL,
        code_hash TEXT NOT NULL,
        expires_at INTEGER NOT NULL,
        attempts INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL
    );
    CREATE INDEX email_codes_user ON email_codes(user_id, kind);
    CREATE TABLE passkeys (
        id TEXT PRIMARY KEY,
        user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        public_key BLOB NOT NULL,
        sign_count INTEGER NOT NULL DEFAULT 0,
        name TEXT NOT NULL DEFAULT '',
        created_at INTEGER NOT NULL,
        last_used_at INTEGER
    );
    CREATE INDEX passkeys_user ON passkeys(user_id);
    ",
    // 2: 회의(예약·지난 회의)와 번역 사용량
    "
    CREATE TABLE meetings (
        id INTEGER PRIMARY KEY,
        code TEXT NOT NULL UNIQUE,
        owner_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        title TEXT NOT NULL DEFAULT '',
        kind TEXT NOT NULL DEFAULT 'meeting',
        lang_a TEXT NOT NULL DEFAULT '',
        lang_b TEXT NOT NULL DEFAULT '',
        starts_at INTEGER,
        duration_min INTEGER NOT NULL DEFAULT 60,
        guests TEXT NOT NULL DEFAULT '[]',
        created_at INTEGER NOT NULL,
        started_at INTEGER,
        ended_at INTEGER,
        translated_seconds INTEGER NOT NULL DEFAULT 0,
        sentences INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX meetings_owner ON meetings(owner_id, starts_at);
    CREATE TABLE usage (
        id INTEGER PRIMARY KEY,
        user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        meeting_code TEXT NOT NULL,
        period TEXT NOT NULL,
        seconds INTEGER NOT NULL,
        recorded_at INTEGER NOT NULL
    );
    CREATE INDEX usage_user_period ON usage(user_id, period);
    CREATE TABLE paddle_events (
        id TEXT PRIMARY KEY,
        received_at INTEGER NOT NULL
    );
    ",
    // 3: 운영자 계정 표시(HOST_ACCOUNTS·개발용). 목록에서 빠지면 시작할 때 권한을 거둔다.
    "ALTER TABLE users ADD COLUMN operator INTEGER NOT NULL DEFAULT 0;",
];

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// 테스트용 메모리 DB.
    pub fn memory() -> Self {
        Self::init(Connection::open_in_memory().expect("in-memory sqlite")).expect("migrate")
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        // WAL: 읽기가 쓰기를 기다리지 않는다. busy_timeout: Litestream 체크포인트와 겹쳐도 잠깐 기다린다.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute_batch(&format!("PRAGMA user_version = {}", i + 1))?;
            tx.commit()?;
        }
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// 블로킹 스레드에서 연결을 빌려 일을 한다.
    pub async fn call<T, F>(&self, f: F) -> rusqlite::Result<T>
    where
        F: FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut guard)
        })
        .await
        .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()))
    }

    /// 시작할 때처럼 런타임 밖에서 바로 쓸 때.
    pub fn blocking<T>(&self, f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrates_and_reopens_without_rerunning() {
        let dir = std::env::temp_dir().join(format!("tolkmeet-db-{}", std::process::id()));
        let _ = std::fs::remove_file(&dir);
        let db = Db::open(&dir).unwrap();
        db.call(|c| {
            c.execute(
                "INSERT INTO users (email, created_at) VALUES ('a@b.c', 1)",
                [],
            )
        })
        .await
        .unwrap();
        drop(db);
        let db = Db::open(&dir).unwrap();
        let n: i64 = db
            .call(|c| c.query_row("SELECT count(*) FROM users", [], |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(n, 1);
        let v: i64 = db
            .call(|c| c.query_row("PRAGMA user_version", [], |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", dir.display()));
        }
    }
}
