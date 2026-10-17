/**
 * 가입 직후 인증 화면으로 넘어갈 때만 메모리에 잠깐 들고 있는 이메일·비밀번호.
 * 서버는 코드와 가입 비밀번호가 함께 맞아야 계정을 열기 때문이다. 저장소(localStorage)에는 두지 않는다.
 */
let pending: { email: string; password: string } | null = null

export function setPendingSignup(email: string, password: string) {
  pending = { email, password }
}

export function pendingPassword(email: string): string | null {
  return pending && pending.email === email.trim().toLowerCase() ? pending.password : null
}

export function clearPendingSignup() {
  pending = null
}
