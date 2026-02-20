// Den - 認証モジュール
// トークンは HttpOnly Cookie で管理（XSS でのトークン窃取を防止）
const Auth = (() => {
  const LOGGED_IN_COOKIE = 'den_logged_in';

  /** JS 可読な den_logged_in フラグ Cookie の存在を確認 */
  function isLoggedIn() {
    return document.cookie.split(';').some(c => c.trim().startsWith(LOGGED_IN_COOKIE + '='));
  }

  async function login(password) {
    const res = await fetch('/api/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      credentials: 'same-origin',
      body: JSON.stringify({ password }),
    });
    if (!res.ok) throw new Error('Unauthorized');
    // トークンは HttpOnly Cookie としてサーバーが Set-Cookie で設定済み
  }

  /** サーバー側で HttpOnly Cookie を無効化し、フラグ Cookie も削除 */
  async function logout() {
    try {
      await fetch('/api/logout', { method: 'POST', credentials: 'same-origin' });
    } catch (_) { /* ignore network errors */ }
    clearToken();
  }

  /** フラグ Cookie を削除（HttpOnly Cookie はサーバー側で期限切れ） */
  function clearToken() {
    document.cookie = LOGGED_IN_COOKIE + '=; Path=/; Max-Age=0';
  }

  return { login, logout, isLoggedIn, clearToken };
})();
