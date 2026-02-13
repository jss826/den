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

  // 既にトークンがあればサーバーに有効性を確認してからメイン画面へ
  if (Auth.isLoggedIn()) {
    validateAndShow();
  } else {
    passwordInput.focus();
  }

  async function validateAndShow() {
    try {
      const resp = await fetch('/api/settings', {
        headers: { 'Authorization': `Bearer ${Auth.getToken()}` },
      });
      if (resp.ok) {
        showMain();
      } else {
        Auth.clearToken();
        loginScreen.hidden = false;
        mainScreen.hidden = true;
        passwordInput.focus();
      }
    } catch {
      Auth.clearToken();
      loginScreen.hidden = false;
      mainScreen.hidden = true;
      passwordInput.focus();
    }
  }

  async function showMain() {
    loginScreen.hidden = true;
    mainScreen.hidden = false;

    // 設定ロード＆適用
    await DenSettings.load();
    DenSettings.apply();
    DenSettings.bindUI();

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
