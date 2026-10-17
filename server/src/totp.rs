//! 시간 기반 일회용 코드(TOTP, RFC 6238): 30초, 6자리, HMAC-SHA1. 인증 앱(Google Authenticator 등)과 맞는 기본값.

use hmac::{Hmac, Mac};
use sha1::Sha1;

const STEP: i64 = 30;
const DIGITS: u32 = 6;
const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// 새 비밀 키(160비트)를 Base32로.
pub fn new_secret() -> String {
    let mut bytes = [0u8; 20];
    getrandom::fill(&mut bytes).expect("OS RNG unavailable");
    base32(&bytes)
}

pub fn base32(data: &[u8]) -> String {
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &b in data {
        buffer = (buffer << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            out.push(ALPHABET[((buffer >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for c in s.bytes().filter(|c| *c != b'=' && *c != b' ') {
        let v = ALPHABET.iter().position(|a| *a == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | v;
        bits += 5;
        if bits >= 8 {
            out.push((buffer >> (bits - 8)) as u8);
            bits -= 8;
        }
    }
    Some(out)
}

fn code_at(secret: &[u8], step: i64) -> u32 {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("any key length");
    mac.update(&step.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let offset = (h[19] & 0x0f) as usize;
    let bin = (u32::from(h[offset] & 0x7f) << 24)
        | (u32::from(h[offset + 1]) << 16)
        | (u32::from(h[offset + 2]) << 8)
        | u32::from(h[offset + 3]);
    bin % 10u32.pow(DIGITS)
}

/// 맞으면 그 코드의 시간 칸 번호를 돌려준다(앞뒤 한 칸까지 허용, 시계 오차).
/// 같은 칸(이미 쓴 코드)이나 그 이전은 `last_step`으로 거부해 재사용을 막는다.
pub fn verify(secret_b32: &str, code: &str, now: i64, last_step: i64) -> Option<i64> {
    let code = code.trim().replace(' ', "");
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let want: u32 = code.parse().ok()?;
    let secret = base32_decode(secret_b32)?;
    let current = now.div_euclid(STEP);
    (current - 1..=current + 1)
        .filter(|s| *s > last_step)
        .find(|s| code_at(&secret, *s) == want)
}

/// 인증 앱에 넣는 주소.
pub fn otpauth_uri(secret_b32: &str, email: &str) -> String {
    fn enc(s: &str) -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    }
    format!(
        "otpauth://totp/Tolkmeet:{}?secret={secret_b32}&issuer=Tolkmeet&algorithm=SHA1&digits=6&period=30",
        enc(email)
    )
}

/// 주소를 QR 코드 SVG로(브라우저에 QR 라이브러리를 싣지 않으려고 서버에서 만든다).
pub fn qr_svg(data: &str) -> String {
    match qrcode::QrCode::new(data.as_bytes()) {
        Ok(code) => code
            .render::<qrcode::render::svg::Color>()
            .min_dimensions(200, 200)
            .quiet_zone(true)
            .build(),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6238_sha1_vectors() {
        // RFC 6238 부록 B의 키 "12345678901234567890", 8자리 값의 끝 6자리.
        let secret = b"12345678901234567890";
        assert_eq!(code_at(secret, 59 / 30), 287082);
        assert_eq!(code_at(secret, 1111111109 / 30), 81804);
        assert_eq!(code_at(secret, 1234567890 / 30), 5924);
    }

    #[test]
    fn base32_round_trip_and_verify_with_reuse_guard() {
        let secret = base32(b"12345678901234567890");
        assert_eq!(base32_decode(&secret).unwrap(), b"12345678901234567890");
        let step = verify(&secret, "287082", 59, 0).unwrap();
        assert_eq!(step, 1);
        // 이미 쓴 칸은 다시 받지 않는다.
        assert_eq!(verify(&secret, "287082", 59, step), None);
        assert_eq!(verify(&secret, "12345", 59, 0), None);
    }

    #[test]
    fn qr_is_svg() {
        assert!(qr_svg(&otpauth_uri(&new_secret(), "a@b.c")).starts_with("<?xml"));
    }
}
