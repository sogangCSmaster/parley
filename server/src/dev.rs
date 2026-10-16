//! `tolkmeet-server dev`: 로컬·클라우드 개발용 실행 모드.
//!
//! 주최자 계정이 설정돼 있지 않으면 테스트 계정 `dev`를 만들고 비밀번호를 git 제외 파일에 저장한다.
//! 사람이든 AI든 그 파일을 읽어 바로 로그인해 화면을 볼 수 있다. 파일이 이미 있으면 같은 비밀번호를
//! 재사용해 재시작해도 바뀌지 않는다.
//!
//! 안전장치: loopback(127.0.0.1 / ::1)에만 바인딩할 때만 허용한다. 실수로 공개 서버에서 켜도
//! 알려진 테스트 계정이 외부에 열리지 않게 하기 위해서다.

use std::fs;
use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;

pub const USERNAME: &str = "dev";

pub struct Credentials {
    pub username: String,
    pub password: String,
}

/// dev 모드를 켤 수 있는 바인딩 주소인지.
pub fn allowed_bind(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

/// 파일에 저장된 테스트 계정을 읽고, 없으면 새로 만들어 저장한다.
pub fn ensure_credentials(path: &Path) -> std::io::Result<Credentials> {
    if let Ok(text) = fs::read_to_string(path)
        && let Some(creds) = parse(&text)
    {
        return Ok(creds);
    }
    let creds = Credentials {
        username: USERNAME.to_owned(),
        password: random_password(),
    };
    write_private(
        path,
        &format!(
            "username: {}\npassword: {}\n",
            creds.username, creds.password
        ),
    )?;
    Ok(creds)
}

fn parse(text: &str) -> Option<Credentials> {
    let field = |name: &str| {
        text.lines()
            .find_map(|l| {
                l.strip_prefix(name)?
                    .strip_prefix(':')
                    .map(|v| v.trim().to_owned())
            })
            .filter(|v| !v.is_empty())
    };
    Some(Credentials {
        username: field("username")?,
        password: field("password")?,
    })
}

/// 20자, 영숫자(혼동 문자 제외). 약 116bit.
fn random_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; 20];
    getrandom::fill(&mut bytes).expect("OS RNG unavailable");
    bytes
        .iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect()
}

/// 소유자만 읽고 쓸 수 있게(0600) 저장한다.
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(contents.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_mode_only_on_loopback() {
        assert!(allowed_bind(&"127.0.0.1:8080".parse().unwrap()));
        assert!(allowed_bind(&"[::1]:8080".parse().unwrap()));
        assert!(!allowed_bind(&"0.0.0.0:8080".parse().unwrap()));
        assert!(!allowed_bind(&"192.168.0.10:8080".parse().unwrap()));
    }

    #[test]
    fn creates_once_then_reuses_the_same_password() {
        let path = std::env::temp_dir().join(format!("tolkmeet-dev-{}", std::process::id()));
        let _ = fs::remove_file(&path);

        let first = ensure_credentials(&path).unwrap();
        assert_eq!(first.username, USERNAME);
        assert_eq!(first.password.len(), 20);
        let second = ensure_credentials(&path).unwrap();
        assert_eq!(second.password, first.password);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn reads_the_existing_file_format() {
        let creds = parse("username: dev\npassword: abc123\n").unwrap();
        assert_eq!(
            (creds.username.as_str(), creds.password.as_str()),
            ("dev", "abc123")
        );
        assert!(parse("username: dev\n").is_none());
    }
}
