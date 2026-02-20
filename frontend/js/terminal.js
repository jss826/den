// Den - ターミナルモジュール
const DenTerminal = (() => {
  let term = null;
  let fitAddon = null;
  let ws = null;
  let currentSession = 'default';
  let connectGeneration = 0; // doConnect の世代カウンタ（高速切り替え時の race 防止）
  const textEncoder = new TextEncoder(); // 再利用で毎回の alloc を回避

  /** fit + refresh + resize 通知をまとめて実行 */
  let fitRetryCount = 0;
  function fitAndRefresh() {
    if (!term || !fitAddon) return;
    const container = term.element?.parentElement;
    if (container && container.clientWidth === 0) {
      if (fitRetryCount < 10) {
        fitRetryCount++;
        requestAnimationFrame(() => fitAndRefresh());
      }
      return;
    }
    fitRetryCount = 0;
    fitAddon.fit();
    term.refresh(0, term.rows - 1);
    sendResize();
  }

  // Shared xterm.js theme (Tokyo Night)
  const XTERM_THEME = {
    background: '#1a1b26',
    foreground: '#c0caf5',
    cursor: '#c0caf5',
    selectionBackground: '#33467c',
    black: '#15161e',
    red: '#f7768e',
    green: '#9ece6a',
    yellow: '#e0af68',
    blue: '#7aa2f7',
    magenta: '#bb9af7',
    cyan: '#7dcfff',
    white: '#a9b1d6',
    brightBlack: '#414868',
    brightRed: '#f7768e',
    brightGreen: '#9ece6a',
    brightYellow: '#e0af68',
    brightBlue: '#7aa2f7',
    brightMagenta: '#bb9af7',
    brightCyan: '#7dcfff',
    brightWhite: '#c0caf5',
  };

  const FONT_FAMILY = '"Cascadia Code", "Fira Code", "Source Code Pro", "Menlo", "Symbols Nerd Font Mono", monospace';

  /** レンダラー選択: デスクトップ → WebGL、iOS/Safari → Canvas、フォールバック → DOM */
  function selectRenderer(t) {
    const isIOS = /iPad|iPhone|iPod/.test(navigator.userAgent)
      || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
    const isSafari = !isIOS && /^((?!chrome|android).)*safari/i.test(navigator.userAgent);
    if (!isIOS && !isSafari) {
      try {
        const webglAddon = new WebglAddon.WebglAddon();
        webglAddon.onContextLost(() => webglAddon.dispose());
        t.loadAddon(webglAddon);
      } catch (_e) {
        console.warn('WebGL not available, falling back to canvas renderer');
        try {
          t.loadAddon(new CanvasAddon.CanvasAddon());
        } catch (_e2) { /* DOM fallback */ }
      }
    } else {
      // iOS/Safari: Canvas レンダラーを明示的にロード
      try {
        t.loadAddon(new CanvasAddon.CanvasAddon());
      } catch (_e) {
        console.warn('Canvas addon not available, using DOM renderer');
      }
    }
  }

  function init(container) {
    const scrollback = DenSettings.get('terminal_scrollback') ?? 1000;
    const fontSize = DenSettings.get('font_size') ?? 15;
    term = new Terminal({
      cursorBlink: true,
      fontSize,
      fontFamily: FONT_FAMILY,
      scrollback,
      theme: XTERM_THEME,
    });

    fitAddon = new FitAddon.FitAddon();
    term.loadAddon(fitAddon);

    selectRenderer(term);

    term.open(container);
    fitAndRefresh();
    requestAnimationFrame(() => fitAndRefresh());
    setTimeout(() => fitAndRefresh(), 300);
    setTimeout(() => fitAndRefresh(), 1000);

    // フォント読み込み完了後に再 fit
    if (document.fonts?.ready) {
      document.fonts.ready.then(() => fitAndRefresh());
    }
    window.addEventListener('pageshow', () => fitAndRefresh());
    const resizeObserver = new ResizeObserver(() => fitAndRefresh());
    resizeObserver.observe(container);

    // キーバー修飾キー + OS キーボード連携
    term.attachCustomKeyEventHandler((ev) => {
      if (ev.type !== 'keydown') return true;
      const mods = Keybar.getModifiers();
      if (!mods.ctrl && !mods.alt) return true;
      // ハードウェア修飾キー自体や単独の Shift/Meta は無視
      if (ev.key === 'Control' || ev.key === 'Alt' || ev.key === 'Shift' || ev.key === 'Meta') return true;
      // OS 側の修飾が既に押されている場合はキーバー状態を使わない
      if (ev.ctrlKey || ev.altKey || ev.metaKey) return true;

      // 非印字キー（Enter, Tab, 矢印等）: 修飾リセットだけ行い xterm に通常処理させる
      if (ev.key.length !== 1) {
        Keybar.resetModifiers();
        return true;
      }

      let data = ev.key;
      if (mods.ctrl) {
        const code = data.toUpperCase().charCodeAt(0);
        if (code >= 0x40 && code <= 0x5f) {
          data = String.fromCharCode(code - 0x40);
        }
      }
      if (mods.alt) {
        data = '\x1b' + data;
      }

      sendInput(data);
      Keybar.resetModifiers();
      return false; // xterm のデフォルト処理を抑止
    });

    // キー入力 → WebSocket
    term.onData((data) => {
      if (ws && ws.readyState === WebSocket.OPEN) {
        ws.send(textEncoder.encode(data));
      }
    });

    term.onBinary((data) => {
      if (ws && ws.readyState === WebSocket.OPEN) {
        const bytes = new Uint8Array(data.length);
        for (let i = 0; i < data.length; i++) {
          bytes[i] = data.charCodeAt(i) & 0xff;
        }
        ws.send(bytes);
      }
    });

    return term;
  }

  function connect(sessionName) {
    currentSession = sessionName || 'default';
    doConnect();
  }

  function doConnect() {
    const generation = ++connectGeneration;
    reconnectAttempts = 0;
    if (manualReconnectDisposable) { manualReconnectDisposable.dispose(); manualReconnectDisposable = null; }
    const cols = term.cols;
    const rows = term.rows;
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    // 認証は Cookie（HttpOnly）で自動送信 — URL にトークンを含めない
    const url = `${proto}//${location.host}/api/ws?cols=${cols}&rows=${rows}&session=${encodeURIComponent(currentSession)}`;

    let retries = 0;

    const attemptConnect = () => {
      // 世代チェック: 新しい doConnect() が呼ばれていたら中断
      if (generation !== connectGeneration) return;
      // 古い接続を破棄
      if (ws) {
        ws.onopen = ws.onclose = ws.onerror = ws.onmessage = null;
        ws.close();
        ws = null;
      }

      ws = new WebSocket(url);
      ws.binaryType = 'arraybuffer';
      let sessionEnded = false;

      ws.onopen = () => {
        retries = 0;
        term.focus();
        fitAndRefresh();
      };

      ws.onmessage = (event) => {
        if (typeof event.data === 'string') {
          // JSON メッセージ
          try {
            const msg = JSON.parse(event.data);
            if (msg.type === 'session_ended') {
              sessionEnded = true;
              term.writeln('\r\n\x1b[33mSession ended.\x1b[0m');
              refreshSessionList();
              return;
            }
          } catch (_) {
            // テキストデータとして扱う
          }
          term.write(event.data);
        } else if (event.data instanceof ArrayBuffer) {
          term.write(new Uint8Array(event.data));
        }
      };

      ws.onclose = () => {
        if (generation !== connectGeneration) return;
        // session_ended 後の切断は再接続不要
        if (sessionEnded) return;
        startReconnect(generation);
      };

      ws.onerror = (event) => {
        console.error('[DenTerminal] WebSocket error', event);
      };

      // Safari: WebSocket が CONNECTING のまま stall する問題のリトライ
      setTimeout(() => {
        if (generation !== connectGeneration) return;
        if (ws && ws.readyState === WebSocket.CONNECTING && retries < 3) {
          retries++;
          attemptConnect();
        }
      }, 3000);
    };

    // 少し遅延させてから接続（Safari の初回 WS stall 軽減）
    setTimeout(attemptConnect, 200);
  }

  let reconnectAttempts = 0;
  const MAX_RECONNECT = 3;
  let manualReconnectDisposable = null;

  function startReconnect(generation) {
    reconnectAttempts++;
    if (reconnectAttempts > MAX_RECONNECT) {
      term.writeln('\r\n\x1b[31mConnection lost. Press Enter to reconnect.\x1b[0m');
      manualReconnectDisposable = term.onData((data) => {
        if (data === '\r' || data === '\n') {
          if (manualReconnectDisposable) { manualReconnectDisposable.dispose(); manualReconnectDisposable = null; }
          reconnectAttempts = 0;
          term.writeln('\r\n\x1b[33mReconnecting...\x1b[0m');
          doConnect();
        }
      });
      return;
    }

    let countdown = 3;
    term.write(`\r\n\x1b[31mDisconnected.\x1b[0m Reconnecting in \x1b[33m${countdown}\x1b[0m...`);
    const timer = setInterval(() => {
      if (generation !== connectGeneration) { clearInterval(timer); return; }
      countdown--;
      if (countdown > 0) {
        term.write(`\x1b[33m${countdown}\x1b[0m...`);
      } else {
        clearInterval(timer);
        term.writeln('');
        if (generation === connectGeneration) doConnect();
      }
    }, 1000);
  }

  /** セッションを切り替え */
  function switchSession(name) {
    if (name === currentSession) return;
    currentSession = name;
    term.clear();
    doConnect();
  }

  function sendResize() {
    if (ws && ws.readyState === WebSocket.OPEN && term) {
      ws.send(JSON.stringify({
        type: 'resize',
        cols: term.cols,
        rows: term.rows,
      }));
    }
  }

  function sendInput(data) {
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(textEncoder.encode(data));
    }
  }

  // --- Select Mode ---
  let selectModeActive = false;
  let selectModeOverlay = null;
  let selectModeStartRow = null;
  let selectModeOnExit = null;
  let selectModeScreen = null; // F016: cached .xterm-screen element

  function enterSelectMode(onExit) {
    if (selectModeActive) return;
    const container = document.getElementById('terminal-container');
    if (!container) return;
    selectModeActive = true;
    selectModeStartRow = null;
    selectModeOnExit = onExit || null;
    selectModeScreen = term?.element?.querySelector('.xterm-screen') ?? null; // F016

    container.classList.add('select-mode');

    selectModeOverlay = document.createElement('div');
    selectModeOverlay.className = 'select-mode-overlay';
    container.appendChild(selectModeOverlay);

    selectModeOverlay.addEventListener('click', onSelectModeTap);
    document.addEventListener('keydown', onSelectModeKeydown); // F011
    document.addEventListener('visibilitychange', onSelectModeVisChange); // F008
  }

  function exitSelectMode() {
    if (!selectModeActive) return;
    selectModeActive = false;
    selectModeStartRow = null;
    selectModeScreen = null; // F016

    document.removeEventListener('keydown', onSelectModeKeydown); // F011
    document.removeEventListener('visibilitychange', onSelectModeVisChange); // F008

    const container = document.getElementById('terminal-container');
    if (container) {
      container.classList.remove('select-mode');
    }

    if (selectModeOverlay) {
      selectModeOverlay.removeEventListener('click', onSelectModeTap);
      selectModeOverlay.remove();
      selectModeOverlay = null;
    }

    if (term) term.clearSelection();

    const cb = selectModeOnExit;
    selectModeOnExit = null;
    if (cb) cb();
  }

  function isSelectMode() {
    return selectModeActive;
  }

  function onSelectModeKeydown(e) { // F011: Escape to exit
    if (e.key === 'Escape') exitSelectMode();
  }

  function onSelectModeVisChange() { // F008: tab switch cleanup
    if (document.hidden && selectModeActive) exitSelectMode();
  }

  async function onSelectModeTap(e) { // F009: async/await consistency
    if (!term) return;

    const screen = selectModeScreen || term.element?.querySelector('.xterm-screen'); // F016
    if (!screen) return;
    const rect = screen.getBoundingClientRect();
    if (rect.height === 0 || term.rows === 0) return; // F004: zero guard
    const cellHeight = rect.height / term.rows;
    const viewportRow = Math.max(0, Math.min(term.rows - 1, Math.floor((e.clientY - rect.top) / cellHeight))); // F004: clamp
    const bufferRow = viewportRow + term.buffer.active.viewportY;

    if (selectModeStartRow === null) {
      // First tap — highlight single line
      // selectLines() is stable in xterm.js v6 (no allowProposedApi needed)
      selectModeStartRow = bufferRow;
      term.selectLines(bufferRow, bufferRow);
    } else {
      // Second tap — select range and copy
      const startRow = Math.min(selectModeStartRow, bufferRow);
      const endRow = Math.max(selectModeStartRow, bufferRow);
      term.selectLines(startRow, endRow);
      const sel = term.getSelection();
      if (sel) {
        try {
          await navigator.clipboard.writeText(sel);
          if (typeof Toast !== 'undefined') Toast.success('Copied');
        } catch (_) {
          if (typeof Toast !== 'undefined') Toast.error('Clipboard access denied');
        }
      }
      exitSelectMode();
    }
  }

  function focus() {
    if (term) term.focus();
  }

  function getTerminal() {
    return term;
  }

  function getCurrentSession() {
    return currentSession;
  }

  // --- Session management ---

  async function fetchSessions() {
    try {
      const resp = await fetch('/api/terminal/sessions', {
        credentials: 'same-origin',
      });
      if (resp.ok) return await resp.json();
    } catch (_) { /* ignore */ }
    return [];
  }

  async function createSession(name) {
    try {
      const resp = await fetch('/api/terminal/sessions', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name }),
      });
      return resp.ok || resp.status === 201;
    } catch (_) {
      return false;
    }
  }

  async function destroySession(name) {
    try {
      const resp = await fetch(`/api/terminal/sessions/${encodeURIComponent(name)}`, {
        method: 'DELETE',
        credentials: 'same-origin',
      });
      return resp.ok || resp.status === 204;
    } catch (_) {
      return false;
    }
  }

  async function refreshSessionList() {
    const select = document.getElementById('session-select');
    const clientsSpan = document.getElementById('session-clients');
    if (!select) return;

    const sessions = await fetchSessions();

    select.innerHTML = '';
    if (sessions.length === 0) {
      // No sessions — just show "default" as placeholder
      const opt = document.createElement('option');
      opt.value = 'default';
      opt.textContent = 'default';
      select.appendChild(opt);
    } else {
      for (const s of sessions) {
        const opt = document.createElement('option');
        opt.value = s.name;
        const status = s.alive ? '' : ' (dead)';
        opt.textContent = `${s.name}${status}`;
        if (s.name === currentSession) opt.selected = true;
        select.appendChild(opt);
      }
    }

    // Update client count display
    const current = sessions.find(s => s.name === currentSession);
    if (current) {
      clientsSpan.textContent = `${current.client_count} client${current.client_count !== 1 ? 's' : ''}`;
    } else {
      clientsSpan.textContent = '';
    }

    // Notify other modules (e.g. FloatTerminal) via event — avoids circular dependency
    document.dispatchEvent(new CustomEvent('den:sessions-changed', { detail: { sessions } }));
  }

  function initSessionBar() {
    const select = document.getElementById('session-select');
    const newBtn = document.getElementById('session-new-btn');
    const killBtn = document.getElementById('session-kill-btn');

    if (select) {
      select.addEventListener('change', () => {
        switchSession(select.value);
      });
    }

    if (newBtn) {
      newBtn.addEventListener('click', async () => {
        const name = await Toast.prompt('Session name:');
        if (!name || !name.trim()) return;
        const trimmed = name.trim();
        const validationError = validateSessionName(trimmed);
        if (validationError) {
          Toast.error(validationError);
          return;
        }
        const ok = await createSession(trimmed);
        if (!ok) {
          Toast.error('Failed to create session');
          return;
        }
        await refreshSessionList();
        switchSession(trimmed);
      });
    }

    if (killBtn) {
      killBtn.addEventListener('click', async () => {
        if (!(await Toast.confirm(`Kill session "${currentSession}"?`))) return;
        await destroySession(currentSession);
        currentSession = 'default';
        term.clear();
        doConnect();
        await refreshSessionList();
      });
    }

    // 定期更新（ページ非表示時は停止）
    let sessionRefreshTimer = setInterval(refreshSessionList, 5000);
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) {
        clearInterval(sessionRefreshTimer);
        sessionRefreshTimer = null;
      } else {
        refreshSessionList();
        sessionRefreshTimer = setInterval(refreshSessionList, 5000);
      }
    });
  }

  /** Validate session name. Returns error message string or null if valid. */
  function validateSessionName(name) {
    if (!/^[a-zA-Z0-9-]+$/.test(name)) {
      return 'Session name must be alphanumeric + hyphens only';
    }
    if (name.length > 64) {
      return 'Session name too long (max 64 characters)';
    }
    return null;
  }

  return {
    init, connect, sendInput, sendResize, focus, fitAndRefresh, getTerminal,
    getCurrentSession, switchSession, refreshSessionList, initSessionBar,
    fetchSessions, createSession, destroySession,
    enterSelectMode, exitSelectMode, isSelectMode,
    validateSessionName,
    getXtermTheme() { return XTERM_THEME; },
    getFontFamily() { return FONT_FAMILY; },
    selectRenderer,
  };
})();
