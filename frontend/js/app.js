/* global Auth, DenSettings, DenTerminal, FloatTerminal, Keybar, DenFiler, FilerRemote, DenIcons, DenSnippet */
// Den - アプリケーションエントリポイント
document.addEventListener('DOMContentLoaded', () => {
  const loginScreen = document.getElementById('login-screen');
  const mainScreen = document.getElementById('main-screen');
  const loginForm = document.getElementById('login-form');
  const passwordInput = document.getElementById('password-input');
  const loginError = document.getElementById('login-error');

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
        credentials: 'same-origin',
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

  // モーダル ID 配列（keydown ハンドラで毎回再生成しないよう外に定義）
  // confirm-modal, prompt-modal は Toast 内で独自にハンドルするので Esc 対象外
  const escModals = ['settings-modal', 'filer-upload-modal', 'filer-search-modal', 'filer-quickopen-modal', 'sftp-connect-modal'];
  // ショートカット抑止にはすべてのモーダルを含める
  const allModals = ['confirm-modal', 'prompt-modal', ...escModals];

  // Esc キーで開いているモーダルを閉じる + Ctrl+1/2 タブ切替
  document.addEventListener('keydown', (e) => {
    const anyModalOpen = allModals.some((id) => {
      const m = document.getElementById(id);
      return m && !m.hidden;
    });

    if (e.key === 'Escape' && DenSnippet.isOpen()) {
      DenSnippet.close();
      return;
    }

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

    // Ctrl+K キーバー toggle（モーダル中はスキップ）
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key === 'k' && !anyModalOpen) {
      e.preventDefault();
      Keybar.toggleVisibility();
      return;
    }

    // Ctrl+` フローティングターミナル toggle（モーダル中はスキップ）
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.code === 'Backquote' && !anyModalOpen) {
      e.preventDefault();
      FloatTerminal.toggle();
      return;
    }

    // Ctrl+1/2 タブ切替（モーダル中はスキップ）
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && !anyModalOpen) {
      const tabs = { '1': 'terminal', '2': 'filer' };
      const tab = tabs[e.key];
      if (tab) {
        e.preventDefault();
        switchTab(tab);
      }
    }
  });

  // ツールチップ (body直下に配置して overflow:hidden を回避)
  function initTooltip() {
    const tip = document.createElement('div');
    tip.id = 'den-tooltip';
    document.body.appendChild(tip);

    let showTimer = null;

    document.addEventListener('pointerenter', (e) => {
      const target = e.target.closest('[data-tooltip]');
      if (!target) return;
      // タッチデバイスではツールチップ不要
      if (e.pointerType === 'touch') return;

      clearTimeout(showTimer);
      showTimer = setTimeout(() => {
        tip.textContent = target.getAttribute('data-tooltip');
        // 一旦表示して寸法を計測
        tip.style.opacity = '0';
        tip.style.display = 'block';
        const rect = target.getBoundingClientRect();
        const tipRect = tip.getBoundingClientRect();

        // デフォルト: 上部中央。はみ出す場合は下部
        let top = rect.top - tipRect.height - 6;
        if (top < 4) top = rect.bottom + 6;
        let left = rect.left + rect.width / 2 - tipRect.width / 2;
        // 画面端クランプ
        left = Math.max(4, Math.min(left, window.innerWidth - tipRect.width - 4));

        tip.style.top = top + 'px';
        tip.style.left = left + 'px';
        tip.style.opacity = '1';
      }, 400);
    }, true);

    document.addEventListener('pointerleave', (e) => {
      const target = e.target.closest('[data-tooltip]');
      if (!target) return;
      clearTimeout(showTimer);
      tip.style.opacity = '0';
    }, true);
  }

  // SVG アイコンを静的ボタンに注入
  function initIcons() {
    const map = {
      'filer-new-file': DenIcons.filePlus,
      'filer-new-folder': DenIcons.folderPlus,
      'filer-upload': DenIcons.upload,
      'filer-refresh': DenIcons.refresh,
      'snippet-btn': DenIcons.snippet,
      'float-terminal-btn': DenIcons.terminal,
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

    // ツールチップ初期化
    initTooltip();

    // SVG アイコン注入
    initIcons();

    // 設定ロード＆適用
    await DenSettings.load();
    DenSettings.apply();
    DenSettings.bindUI();

    // ターミナル初期化
    const container = document.getElementById('terminal-container');
    DenTerminal.init(container);
    const initialHash = parseHash();
    if (initialHash.session && DenTerminal.validateSessionName(initialHash.session)) {
      initialHash.session = null;
    }
    DenTerminal.connect(initialHash.session);
    DenTerminal.initSessionBar();
    DenTerminal.refreshSessionList();

    // フローティングターミナル初期化（DOM イベントのみ、xterm は lazy）
    FloatTerminal.init();
    document.getElementById('float-terminal-btn')?.addEventListener('click', () => FloatTerminal.toggle());

    // SFTP 接続状態チェック（ページリロード時の復元）
    FilerRemote.checkStatus();

    // スニペット初期化
    DenSnippet.init(document.getElementById('snippet-btn'));

    // キーバー初期化（カスタムキー設定があればそれを使用）
    Keybar.init(document.getElementById('keybar'), DenSettings.get('keybar_buttons'));

    // タブ切り替え
    document.querySelectorAll('.tab').forEach((tab) => {
      tab.addEventListener('click', () => switchTab(tab.dataset.tab));
    });

    // モバイルサイドバートグル
    initSidebarToggles();

    // ハッシュルーティング: 初期タブ適用 + hashchange リスナー
    if (initialHash.tab !== 'terminal') switchTab(initialHash.tab);
    window.addEventListener('hashchange', applyHash);

    // iPad Safari: visualViewport でキーボード表示時のビューポート高さを追従
    // Safari はキーボード表示時にページ自体をスクロールする（overflow:hidden でも）
    // → scrollTo(0,0) でリセットし、offsetTop を補正する
    if (window.visualViewport) {
      const update = () => {
        const vv = window.visualViewport;
        document.documentElement.style.setProperty('--viewport-height', vv.height + 'px');
        // Safari がページをスクロールした分をリセット
        if (vv.offsetTop > 0) {
          window.scrollTo(0, 0);
        }
      };
      window.visualViewport.addEventListener('resize', update);
      window.visualViewport.addEventListener('scroll', update);
      update();
    }
  }

  // 他モジュールからタブ切替できるようグローバル公開
  window.DenApp = {
    switchTab: (tab) => switchTab(tab),
    updateSessionHash: (name) => setHash(buildHash('terminal', name)),
  };

  function switchTab(tabName) {
    // タブボタン更新
    document.querySelectorAll('.tab').forEach((t) => {
      t.classList.remove('active');
      t.setAttribute('aria-selected', 'false');
    });
    const activeTab = document.querySelector(`.tab[data-tab="${tabName}"]`);
    if (!activeTab) return;
    activeTab.classList.add('active');
    activeTab.setAttribute('aria-selected', 'true');

    // ペイン表示切り替え
    document.getElementById('terminal-pane').hidden = tabName !== 'terminal';
    document.getElementById('filer-pane').hidden = tabName !== 'filer';

    if (tabName === 'terminal') {
      DenTerminal.fitAndRefresh();
      DenTerminal.focus();
    }

    // Filer 初期化（初回のみ）
    if (tabName === 'filer' && !filerInitialized) {
      filerInitialized = true;
      DenFiler.init();
    }

    // ハッシュ更新
    setHash(buildHash(tabName, tabName === 'terminal' ? DenTerminal.getCurrentSession() : null));
  }

  // --- Hash routing ---
  let lastSetHash = '';

  const TAB_MAP = { files: 'filer', terminal: 'terminal' };

  function parseHash() {
    const hash = location.hash.replace(/^#/, '');
    if (!hash) return { tab: 'terminal', session: null };
    const parts = hash.split('/');
    const tab = TAB_MAP[parts[0]] ?? 'terminal';
    let session = null;
    if (parts[1]) {
      try { session = decodeURIComponent(parts[1]); } catch { /* invalid encoding → null */ }
    }
    return { tab, session };
  }

  function buildHash(tabName, sessionName) {
    const urlTab = tabName === 'filer' ? 'files' : 'terminal';
    if (urlTab === 'terminal' && sessionName && sessionName !== 'default') {
      return '#' + urlTab + '/' + encodeURIComponent(sessionName);
    }
    return '#' + urlTab;
  }

  function setHash(hash) {
    if (location.hash === hash) return;
    lastSetHash = hash;
    location.hash = hash;
  }

  function applyHash() {
    if (location.hash === lastSetHash) return;
    const { tab, session } = parseHash();
    switchTab(tab);
    if (tab === 'terminal' && session && !DenTerminal.validateSessionName(session) && session !== DenTerminal.getCurrentSession()) {
      DenTerminal.switchSession(session);
    }
  }

  function initSidebarToggles() {
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
