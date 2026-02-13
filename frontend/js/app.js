// Den - アプリケーションエントリポイント
document.addEventListener('DOMContentLoaded', () => {
  const loginScreen = document.getElementById('login-screen');
  const mainScreen = document.getElementById('main-screen');
  const loginForm = document.getElementById('login-form');
  const passwordInput = document.getElementById('password-input');
  const loginError = document.getElementById('login-error');

  let claudeInitialized = false;

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
      tab.addEventListener('click', () => switchTab(tab.dataset.tab));
    });
  }

  function switchTab(tabName) {
    // タブボタン更新
    document.querySelectorAll('.tab').forEach((t) => t.classList.remove('active'));
    document.querySelector(`.tab[data-tab="${tabName}"]`).classList.add('active');

    // ペイン表示切り替え
    document.getElementById('terminal-pane').hidden = tabName !== 'terminal';
    document.getElementById('claude-pane').hidden = tabName !== 'claude';

    // キーバーはターミナル時のみ
    const keybar = document.getElementById('keybar');
    if (tabName === 'terminal') {
      if (Keybar.isTouchDevice()) keybar.classList.add('visible');
      DenTerminal.focus();
    } else {
      keybar.classList.remove('visible');
    }

    // Claude 初期化（初回のみ）
    if (tabName === 'claude' && !claudeInitialized) {
      claudeInitialized = true;
      DenClaude.init(Auth.getToken());
    }
  }
});
