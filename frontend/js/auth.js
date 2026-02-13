// Den - 認証モジュール
const Auth = (() => {
  const TOKEN_KEY = 'den_token';

  function getToken() {
    return sessionStorage.getItem(TOKEN_KEY);
  }

  function setToken(token) {
    sessionStorage.setItem(TOKEN_KEY, token);
  }

  function clearToken() {
    sessionStorage.removeItem(TOKEN_KEY);
  }

  async function login(password) {
    const res = await fetch('/api/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ password }),
    });
    if (!res.ok) throw new Error('Unauthorized');
    const data = await res.json();
    setToken(data.token);
    return data.token;
  }

  function isLoggedIn() {
    return !!getToken();
  }

  return { getToken, setToken, clearToken, login, isLoggedIn };
})();
