/**
 * 가입 직후 인증 화면으로 넘어갈 때만 메모리에 잠깐 들고 있는 이메일·비밀번호.
 * 서버는 코드와 가입 비밀번호가 함께 맞아야 계정을 열기 때문이다. 저장소(localStorage)에는 두지 않는다.
 * `sent`: 방금 코드 메일을 보냈다(가입 직후). 로그인에서 넘어왔으면 예전 코드라 바로 다시 보낼 수 있게 한다.
 */
let pending: { email: string; password: string; sent: boolean } | null = null

export function setPendingSignup(email: string, password: string, sent = true) {
  pending = { email, password, sent }
}

export function pendingPassword(email: string): string | null {
  return pending && pending.email === email.trim().toLowerCase() ? pending.password : null
}

export function justSent(email: string): boolean {
  return !!pending && pending.sent && pending.email === email.trim().toLowerCase()
}

export function clearPendingSignup() {
  pending = null
}
