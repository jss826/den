// Den - アプリケーションエントリポイント
document.addEventListener('DOMContentLoaded', () => {
  const loginScreen = document.getElementById('login-screen');
  const mainScreen = document.getElementById('main-screen');
  const loginForm = document.getElementById('login-form');
  const passwordInput = document.getElementById('password-input');
  const loginError = document.getElementById('login-error');

  // ログイン処理
  loginForm.addEventListener('submit', async (e) => {
    e.preventDefault();
    loginError.hidden = true;
    try {
      await Auth.login(passwordInput.value);
      showMain();
    } catch {
      loginError.hidden = false;
      passwordInput.value = '';
      passwordInput.focus();
    }
  });

  // 既にトークンがあればメイン画面へ
  if (Auth.isLoggedIn()) {
    showMain();
  } else {
    passwordInput.focus();
  }

  function showMain() {
    loginScreen.hidden = true;
    mainScreen.hidden = false;

    // ターミナル初期化
    const container = document.getElementById('terminal-container');
    DenTerminal.init(container);
    DenTerminal.connect(Auth.getToken());

    // キーバー初期化
    Keybar.init(document.getElementById('keybar'));

    // タブ切り替え
    document.querySelectorAll('.tab').forEach((tab) => {
      tab.addEventListener('click', () => {
        document.querySelectorAll('.tab').forEach((t) => t.classList.remove('active'));
        tab.classList.add('active');
        // v0.2, v0.3 でペイン切り替え実装
      });
    });
  }
});
