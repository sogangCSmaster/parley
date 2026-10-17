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
    // 4: 회의의 마지막으로 번역이 돌아간 시각(회의 길이 계산), 번역 켜짐 여부
    "ALTER TABLE meetings ADD COLUMN last_active_at INTEGER;
     ALTER TABLE meetings ADD COLUMN translation INTEGER NOT NULL DEFAULT 1;",
    // 5: 회의에서 보이는 이름·기본 설정·비밀번호 바꾼 때, 세션 마지막 사용, 회의 최대 인원,
    //    2차 인증 복구 코드·믿는 기기, 메일 코드의 덧붙임 값(바꿀 새 이메일)
    "ALTER TABLE users ADD COLUMN display_name TEXT NOT NULL DEFAULT '';
     ALTER TABLE users ADD COLUMN prefs TEXT NOT NULL DEFAULT '{}';
     ALTER TABLE users ADD COLUMN password_changed_at INTEGER;
     ALTER TABLE sessions ADD COLUMN last_seen_at INTEGER;
     ALTER TABLE meetings ADD COLUMN peak_participants INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE email_codes ADD COLUMN payload TEXT;
     CREATE TABLE recovery_codes (
         id INTEGER PRIMARY KEY,
         user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
         code_hash TEXT NOT NULL,
         used_at INTEGER
     );
     CREATE INDEX recovery_codes_user ON recovery_codes(user_id);
     CREATE TABLE trusted_devices (
         token_hash TEXT PRIMARY KEY,
         user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
         expires_at INTEGER NOT NULL
     );",
    // 지금 구독에 마지막으로 반영한 Paddle 이벤트 시각(늦게 도착한 옛 이벤트를 버리려고)
    "ALTER TABLE users ADD COLUMN paddle_event_at INTEGER NOT NULL DEFAULT 0;",
    // Paddle(판매 대행사)에 제공·국외 이전하는 데 동의한 마지막 시각(개인정보 보호법 제17조·제28조의8 동의의 기록)
    "ALTER TABLE users ADD COLUMN paddle_consent_at INTEGER;",
    // Paddle 동의를 철회한 시각(운영자가 철회 요청을 받아 기록). 다시 동의하기 전까지 결제·요금제 변경을 막는다.
    "ALTER TABLE users ADD COLUMN paddle_consent_withdrawn_at INTEGER;",
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
        // secure_delete: 지운 행을 빈 페이지에 남기지 않고 0으로 덮는다(백업 사본에 지운 개인정보가 옮겨지지 않게).
        // FAST는 한 페이지를 넘는 값(초대 손님 목록이 길 때)의 넘침 페이지를 덮지 않으므로 ON을 쓴다.
        // 큰 행을 지울 때만 쓰기가 조금 늘어 속도 영향은 거의 없다.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA secure_delete = ON;
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
    pub fn blocking<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
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

    #[test]
    fn deleted_account_leaves_no_bytes_in_the_file() {
        // 처리방침: 계정을 지우면 바로 지우고 백업(파일 스냅숏)에도 남지 않는다.
        // 한 페이지를 넘는 값(넘침 페이지)도 확인한다.
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                for ext in ["", "-wal", "-shm"] {
                    let _ = std::fs::remove_file(format!("{}{ext}", self.0.display()));
                }
            }
        }
        let path = std::env::temp_dir().join(format!("tolkmeet-secure-delete-{}.db", std::process::id()));
        let cleanup = Cleanup(path.clone());
        drop(Cleanup(path.clone()));
        let marker = "forget-me-7f3a@example.com";
        // 주소를 값 끝에 두어 넘침 페이지에 들어가게 한다.
        let big = format!("[\"{}\",\"{marker}\"]", "x".repeat(6000));
        let db = Db::open(&path).unwrap();
        db.blocking(|c| {
            c.execute("INSERT INTO users (email, created_at) VALUES (?1, 1)", params![marker])?;
            let uid = c.last_insert_rowid();
            c.execute(
                "INSERT INTO meetings (code, owner_id, guests, created_at) VALUES ('aaaa-aaaa-aaaa', ?1, ?2, 1)",
                params![uid, big],
            )?;
            c.execute("DELETE FROM users WHERE id = ?1", params![uid])?;
            c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        })
        .unwrap();
        drop(db);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend(std::fs::read(format!("{}-wal", path.display())).unwrap_or_default());
        assert!(!bytes.windows(marker.len()).any(|w| w == marker.as_bytes()), "deleted email still in the file");
        drop(cleanup);
    }

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
