/* global Auth, DenSettings, DenTerminal, Keybar, DenClaude, DenFiler, DenIcons */
// Den - アプリケーションエントリポイント
document.addEventListener('DOMContentLoaded', () => {
  const loginScreen = document.getElementById('login-screen');
  const mainScreen = document.getElementById('main-screen');
  const loginForm = document.getElementById('login-form');
  const passwordInput = document.getElementById('password-input');
  const loginError = document.getElementById('login-error');

  let claudeInitialized = false;
  let filerInitialized = false;

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

  // Esc キーで開いているモーダルを閉じる + Ctrl+1/2/3 タブ切替
  document.addEventListener('keydown', (e) => {
    // confirm-modal, prompt-modal は Toast 内で独自にハンドルするので Esc 対象外
    const escModals = ['settings-modal', 'filer-upload-modal', 'filer-search-modal', 'claude-modal', 'filer-quickopen-modal'];
    // ショートカット抑止にはすべてのモーダルを含める
    const allModals = ['confirm-modal', 'prompt-modal', ...escModals];

    const anyModalOpen = allModals.some((id) => {
      const m = document.getElementById(id);
      return m && !m.hidden;
    });

    if (e.key === 'Escape' && anyModalOpen) {
      for (const id of escModals) {
        const modal = document.getElementById(id);
        if (modal && !modal.hidden) {
          modal.hidden = true;
          return;
        }
      }
    }

    // Ctrl+Shift+F: ファイラ検索フォーカス（モーダル中はスキップ）
    if ((e.ctrlKey || e.metaKey) && e.shiftKey && !e.altKey && e.key === 'F' && !anyModalOpen) {
      e.preventDefault();
      switchTab('filer');
      DenFiler.focusSearch();
      return;
    }

    // Ctrl+P: クイックオープン（モーダル中はスキップ）
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key === 'p' && !anyModalOpen) {
      e.preventDefault();
      switchTab('filer');
      DenFiler.showQuickOpen();
      return;
    }

    // Ctrl+1/2/3 タブ切替（モーダル中はスキップ）
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && !anyModalOpen) {
      const tabs = { '1': 'terminal', '2': 'claude', '3': 'filer' };
      const tab = tabs[e.key];
      if (tab) {
        e.preventDefault();
        switchTab(tab);
      }
    }
  });

  // SVG アイコンを静的ボタンに注入
  function initIcons() {
    const map = {
      'filer-new-file': DenIcons.filePlus,
      'filer-new-folder': DenIcons.folderPlus,
      'filer-upload': DenIcons.upload,
      'filer-refresh': DenIcons.refresh,
      'settings-btn': DenIcons.gear,
    };
    for (const [id, fn] of Object.entries(map)) {
      const el = document.getElementById(id);
      if (el) el.innerHTML = fn();
    }
    // タブスクロールボタン
    const scrollL = document.querySelector('.filer-tabs-scroll.left');
    const scrollR = document.querySelector('.filer-tabs-scroll.right');
    if (scrollL) scrollL.innerHTML = DenIcons.chevronLeft();
    if (scrollR) scrollR.innerHTML = DenIcons.chevronRight();
  }

  async function showMain() {
    loginScreen.hidden = true;
    mainScreen.hidden = false;

    // SVG アイコン注入
    initIcons();

    // 設定ロード＆適用
    await DenSettings.load();
    DenSettings.apply();
    DenSettings.bindUI();

    // ターミナル初期化
    const container = document.getElementById('terminal-container');
    DenTerminal.init(container);
    DenTerminal.connect(Auth.getToken());
    DenTerminal.initSessionBar();
    DenTerminal.refreshSessionList();

    // キーバー初期化（カスタムキー設定があればそれを使用）
    Keybar.init(document.getElementById('keybar'), DenSettings.get('keybar_buttons'));

    // タブ切り替え
    document.querySelectorAll('.tab').forEach((tab) => {
      tab.addEventListener('click', () => switchTab(tab.dataset.tab));
    });

    // モバイルサイドバートグル
    initSidebarToggles();

    // iPad Safari: visualViewport でキーボード表示時のビューポート高さを追従
    if (window.visualViewport) {
      const update = () => {
        const vh = window.visualViewport.height;
        document.documentElement.style.setProperty('--viewport-height', vh + 'px');
      };
      window.visualViewport.addEventListener('resize', update);
      window.visualViewport.addEventListener('scroll', update);
      update();
    }
  }

  // 他モジュールからタブ切替できるようグローバル公開
  window.DenApp = { switchTab: (tab) => switchTab(tab) };

  function switchTab(tabName) {
    // タブボタン更新
    document.querySelectorAll('.tab').forEach((t) => {
      t.classList.remove('active');
      t.setAttribute('aria-selected', 'false');
    });
    const activeTab = document.querySelector(`.tab[data-tab="${tabName}"]`);
    activeTab.classList.add('active');
    activeTab.setAttribute('aria-selected', 'true');

    // ペイン表示切り替え
    document.getElementById('terminal-pane').hidden = tabName !== 'terminal';
    document.getElementById('claude-pane').hidden = tabName !== 'claude';
    document.getElementById('filer-pane').hidden = tabName !== 'filer';

    // キーバーはターミナル時のみ
    const keybar = document.getElementById('keybar');
    if (tabName === 'terminal') {
      if (Keybar.isTouchDevice()) keybar.classList.add('visible');
      DenTerminal.fitAndRefresh();
      DenTerminal.focus();
    } else {
      keybar.classList.remove('visible');
    }

    // Claude 初期化（初回のみ）
    if (tabName === 'claude' && !claudeInitialized) {
      claudeInitialized = true;
      DenClaude.init(Auth.getToken());
    }

    // Filer 初期化（初回のみ）
    if (tabName === 'filer' && !filerInitialized) {
      filerInitialized = true;
      DenFiler.init(Auth.getToken());
    }
  }

  function initSidebarToggles() {
    setupSidebarToggle('.claude-sidebar-toggle', '.claude-sidebar', '.claude-layout');
    setupSidebarToggle('.filer-sidebar-toggle', '.filer-sidebar', '.filer-layout');
  }

  function setupSidebarToggle(toggleSel, sidebarSel, layoutSel) {
    const toggle = document.querySelector(toggleSel);
    const sidebar = document.querySelector(sidebarSel);
    const layout = document.querySelector(layoutSel);
    if (!toggle || !sidebar || !layout) return;

    // オーバーレイ要素を作成
    const overlay = document.createElement('div');
    overlay.className = 'sidebar-overlay';
    layout.appendChild(overlay);

    function toggleSidebar() {
      const expanded = sidebar.classList.toggle('sidebar-expanded');
      overlay.classList.toggle('visible', expanded);
    }

    toggle.addEventListener('click', toggleSidebar);
    overlay.addEventListener('click', () => {
      sidebar.classList.remove('sidebar-expanded');
      overlay.classList.remove('visible');
    });
  }
});
