// Den - ターミナルモジュール
const DenTerminal = (() => {
  let term = null;
  let fitAddon = null;
  let ws = null;
  let currentSession = null;
  let currentRemote = null; // null for local, connectionId for remote Den (direct or relay)
  let connectGeneration = 0; // doConnect の世代カウンタ（高速切り替え時の race 防止）
  let pingTimer = null; // WS keepalive ping interval
  const WS_PING_INTERVAL_MS = 30000;
  const WS_PING_MSG = JSON.stringify({ type: 'ping' });
  const textEncoder = new TextEncoder(); // 再利用で毎回の alloc を回避

  /** Strip port from host:port string, returning just hostname */
  function stripPort(hp) {
    if (!hp) return null;
    const i = hp.lastIndexOf(':');
    return i > 0 && !hp.endsWith(']') ? hp.substring(0, i) : hp;
  }

  /** Look up display label for a remote identifier. Accepts optional pre-fetched connections map. */
  function getRemoteLabel(remote, cachedConns) {
    const conns = cachedConns || (typeof FilerRemote !== 'undefined' ? FilerRemote.getDenConnections() : {});
    const conn = conns[remote];
    return conn?.displayName || stripPort(conn?.hostPort) || remote;
  }

  function getWsPath() {
    if (!currentRemote) return '/api/ws';
    const conns = typeof FilerRemote !== 'undefined' ? FilerRemote.getDenConnections() : {};
    const conn = conns[currentRemote];
    if (conn?.type === 'relay') return `/api/relay/${currentRemote}/ws`;
    return `/api/remote/${currentRemote}/ws`;
  }

  /** Merge multiple Uint8Array chunks into one to reduce xterm.js parser invocations. */
  function mergeChunks(chunks) {
    let total = 0;
    for (const c of chunks) total += c.length;
    const merged = new Uint8Array(total);
    let offset = 0;
    for (const c of chunks) { merged.set(c, offset); offset += c.length; }
    return merged;
  }
  let lastSentCols = 0;
  let lastSentRows = 0;
  let lastKnownPorts = {}; // session key → Set of port numbers (for toast dedup)

  // Mouse sequence filters — strip SGR/URXVT/X10 mouse reports before sending to PTY
  // eslint-disable-next-line no-control-regex
  const MOUSE_SEQ_RE = /\x1b\[<?\d+;\d+;\d+[Mm]/g;
  function filterMouseSeqs(s) { return s.replace(MOUSE_SEQ_RE, ''); }
  function isX10Mouse(d) {
    return d.length >= 6 && d.charCodeAt(0) === 0x1b && d.charCodeAt(1) === 0x5b && d.charCodeAt(2) === 0x4d;
  }

  /** fit + refresh + resize 通知を 1 フレームに集約する */
  let fitRetryCount = 0;
  let fitRafId = null;
  let pendingFitOptions = { force: false, refresh: false };
  let lastFitContainerWidth = 0;
  let lastFitContainerHeight = 0;

  function flushFit({ force = false, refresh = false } = {}) {
    fitRafId = null;
    if (!term || !fitAddon) return;
    const container = term.element?.parentElement;
    const width = container?.clientWidth ?? 0;
    const height = container?.clientHeight ?? 0;
    if (container && (width === 0 || height === 0)) {
      if (fitRetryCount < 10) {
        fitRetryCount++;
        scheduleFit({ force, refresh });
      }
      return;
    }
    fitRetryCount = 0;

    const shouldFit = force || width !== lastFitContainerWidth || height !== lastFitContainerHeight;
    if (!shouldFit) {
      if (refresh && term.rows > 0) {
        term.refresh(0, term.rows - 1);
      }
      sendResize();
      return;
    }

    const prevCols = term.cols;
    const prevRows = term.rows;
    fitAddon.fit();
    lastFitContainerWidth = width;
    lastFitContainerHeight = height;

    if (term.rows > 0 && (refresh || term.cols !== prevCols || term.rows !== prevRows)) {
      term.refresh(0, term.rows - 1);
    }
    sendResize();
  }

  function scheduleFit(options = {}) {
    pendingFitOptions.force = pendingFitOptions.force || !!options.force;
    pendingFitOptions.refresh = pendingFitOptions.refresh || !!options.refresh;
    if (fitRafId != null) return;
    fitRafId = requestAnimationFrame(() => {
      const currentOptions = pendingFitOptions;
      pendingFitOptions = { force: false, refresh: false };
      flushFit(currentOptions);
    });
  }

  function fitAndRefresh() {
    // Synchronous fit — no rAF delay.  Needed for tab-switch where the
    // previous tab's content must disappear in the same frame.
    if (fitRafId != null) { cancelAnimationFrame(fitRafId); fitRafId = null; }
    pendingFitOptions = { force: false, refresh: false };
    flushFit({ force: true, refresh: true });
  }

  // xterm.js theme definitions per Den theme
  const XTERM_THEMES = {
    dark: {
      background: '#1a1b26', foreground: '#c0caf5', cursor: '#c0caf5', selectionBackground: '#33467c',
      black: '#15161e', red: '#f7768e', green: '#9ece6a', yellow: '#e0af68',
      blue: '#7aa2f7', magenta: '#bb9af7', cyan: '#7dcfff', white: '#a9b1d6',
      brightBlack: '#414868', brightRed: '#f7768e', brightGreen: '#9ece6a', brightYellow: '#e0af68',
      brightBlue: '#7aa2f7', brightMagenta: '#bb9af7', brightCyan: '#7dcfff', brightWhite: '#c0caf5',
    },
    light: {
      background: '#f8f9fa', foreground: '#1a1b26', cursor: '#1a1b26', selectionBackground: '#b4d5fe',
      black: '#1a1b26', red: '#dc2626', green: '#16a34a', yellow: '#d97706',
      blue: '#2563eb', magenta: '#9333ea', cyan: '#0891b2', white: '#d1d5db',
      brightBlack: '#6b7280', brightRed: '#ef4444', brightGreen: '#22c55e', brightYellow: '#f59e0b',
      brightBlue: '#3b82f6', brightMagenta: '#a855f7', brightCyan: '#06b6d4', brightWhite: '#f8f9fa',
    },
    'github-light': {
      background: '#ffffff', foreground: '#1f2328', cursor: '#1f2328', selectionBackground: '#b6d7ff',
      black: '#1f2328', red: '#cf222e', green: '#1a7f37', yellow: '#9a6700',
      blue: '#0969da', magenta: '#8250df', cyan: '#1b7c83', white: '#d0d7de',
      brightBlack: '#656d76', brightRed: '#cf222e', brightGreen: '#1a7f37', brightYellow: '#9a6700',
      brightBlue: '#0969da', brightMagenta: '#8250df', brightCyan: '#1b7c83', brightWhite: '#ffffff',
    },
    'one-light': {
      background: '#fafafa', foreground: '#383a42', cursor: '#383a42', selectionBackground: '#bfceff',
      black: '#383a42', red: '#e45649', green: '#50a14f', yellow: '#c18401',
      blue: '#4078f2', magenta: '#a626a4', cyan: '#0184bc', white: '#d3d3d3',
      brightBlack: '#a0a1a7', brightRed: '#e45649', brightGreen: '#50a14f', brightYellow: '#c18401',
      brightBlue: '#4078f2', brightMagenta: '#a626a4', brightCyan: '#0184bc', brightWhite: '#fafafa',
    },
    'solarized-dark': {
      background: '#002b36', foreground: '#839496', cursor: '#839496', selectionBackground: '#073642',
      black: '#073642', red: '#dc322f', green: '#859900', yellow: '#b58900',
      blue: '#268bd2', magenta: '#d33682', cyan: '#2aa198', white: '#eee8d5',
      brightBlack: '#586e75', brightRed: '#cb4b16', brightGreen: '#859900', brightYellow: '#b58900',
      brightBlue: '#268bd2', brightMagenta: '#6c71c4', brightCyan: '#2aa198', brightWhite: '#fdf6e3',
    },
    'solarized-light': {
      background: '#fdf6e3', foreground: '#657b83', cursor: '#657b83', selectionBackground: '#eee8d5',
      black: '#073642', red: '#dc322f', green: '#859900', yellow: '#b58900',
      blue: '#268bd2', magenta: '#d33682', cyan: '#2aa198', white: '#eee8d5',
      brightBlack: '#586e75', brightRed: '#cb4b16', brightGreen: '#859900', brightYellow: '#b58900',
      brightBlue: '#268bd2', brightMagenta: '#6c71c4', brightCyan: '#2aa198', brightWhite: '#fdf6e3',
    },
    monokai: {
      background: '#272822', foreground: '#f8f8f2', cursor: '#f8f8f0', selectionBackground: '#49483e',
      black: '#272822', red: '#f92672', green: '#a6e22e', yellow: '#f4bf75',
      blue: '#66d9ef', magenta: '#ae81ff', cyan: '#a1efe4', white: '#f8f8f2',
      brightBlack: '#75715e', brightRed: '#f92672', brightGreen: '#a6e22e', brightYellow: '#f4bf75',
      brightBlue: '#66d9ef', brightMagenta: '#ae81ff', brightCyan: '#a1efe4', brightWhite: '#f9f8f5',
    },
    nord: {
      background: '#2e3440', foreground: '#d8dee9', cursor: '#d8dee9', selectionBackground: '#434c5e',
      black: '#3b4252', red: '#bf616a', green: '#a3be8c', yellow: '#ebcb8b',
      blue: '#81a1c1', magenta: '#b48ead', cyan: '#88c0d0', white: '#e5e9f0',
      brightBlack: '#4c566a', brightRed: '#bf616a', brightGreen: '#a3be8c', brightYellow: '#ebcb8b',
      brightBlue: '#81a1c1', brightMagenta: '#b48ead', brightCyan: '#8fbcbb', brightWhite: '#eceff4',
    },
    dracula: {
      background: '#282a36', foreground: '#f8f8f2', cursor: '#f8f8f2', selectionBackground: '#44475a',
      black: '#21222c', red: '#ff5555', green: '#50fa7b', yellow: '#f1fa8c',
      blue: '#bd93f9', magenta: '#ff79c6', cyan: '#8be9fd', white: '#f8f8f2',
      brightBlack: '#6272a4', brightRed: '#ff6e6e', brightGreen: '#69ff94', brightYellow: '#ffffa5',
      brightBlue: '#d6acff', brightMagenta: '#ff92df', brightCyan: '#a4ffff', brightWhite: '#ffffff',
    },
    'gruvbox-dark': {
      background: '#282828', foreground: '#ebdbb2', cursor: '#ebdbb2', selectionBackground: '#504945',
      black: '#282828', red: '#cc241d', green: '#98971a', yellow: '#d79921',
      blue: '#458588', magenta: '#b16286', cyan: '#689d6a', white: '#a89984',
      brightBlack: '#928374', brightRed: '#fb4934', brightGreen: '#b8bb26', brightYellow: '#fabd2f',
      brightBlue: '#83a598', brightMagenta: '#d3869b', brightCyan: '#8ec07c', brightWhite: '#ebdbb2',
    },
    'gruvbox-light': {
      background: '#fbf1c7', foreground: '#3c3836', cursor: '#3c3836', selectionBackground: '#ebdbb2',
      black: '#fbf1c7', red: '#cc241d', green: '#98971a', yellow: '#d79921',
      blue: '#458588', magenta: '#b16286', cyan: '#689d6a', white: '#7c6f64',
      brightBlack: '#928374', brightRed: '#9d0006', brightGreen: '#79740e', brightYellow: '#b57614',
      brightBlue: '#076678', brightMagenta: '#8f3f71', brightCyan: '#427b58', brightWhite: '#3c3836',
    },
    catppuccin: {
      background: '#1e1e2e', foreground: '#cdd6f4', cursor: '#f5e0dc', selectionBackground: '#45475a',
      black: '#45475a', red: '#f38ba8', green: '#a6e3a1', yellow: '#f9e2af',
      blue: '#89b4fa', magenta: '#f5c2e7', cyan: '#94e2d5', white: '#bac2de',
      brightBlack: '#585b70', brightRed: '#f38ba8', brightGreen: '#a6e3a1', brightYellow: '#f9e2af',
      brightBlue: '#89b4fa', brightMagenta: '#f5c2e7', brightCyan: '#94e2d5', brightWhite: '#a6adc8',
    },
    'one-dark': {
      background: '#282c34', foreground: '#abb2bf', cursor: '#528bff', selectionBackground: '#3e4451',
      black: '#282c34', red: '#e06c75', green: '#98c379', yellow: '#e5c07b',
      blue: '#61afef', magenta: '#c678dd', cyan: '#56b6c2', white: '#abb2bf',
      brightBlack: '#5c6370', brightRed: '#e06c75', brightGreen: '#98c379', brightYellow: '#e5c07b',
      brightBlue: '#61afef', brightMagenta: '#c678dd', brightCyan: '#56b6c2', brightWhite: '#ffffff',
    },
  };

  /** Get xterm.js theme for the given Den theme name (defaults to dark) */
  function getXtermThemeFor(themeName) {
    return XTERM_THEMES[themeName] || XTERM_THEMES.dark;
  }

  const FONT_FAMILY = '"Cascadia Code", "Fira Code", "Source Code Pro", "Menlo", "Symbols Nerd Font Mono", monospace';

  /** レンダラー選択: WebGL → DOM フォールバック（CanvasAddon は 6.0 で廃止） */
  function selectRenderer(t) {
    try {
      const webglAddon = new WebglAddon.WebglAddon();
      webglAddon.onContextLost?.(() => {
        console.warn('WebGL context lost, falling back to DOM renderer');
        webglAddon.dispose();
      });
      t.loadAddon(webglAddon);
    } catch (e) {
      console.warn('WebGL not available, using DOM renderer', e);
    }
  }

  async function init(container) {
    const { TerminalClass, FitAddonClass, needsWebgl, isRestty } = await TerminalAdapter.ready();
    const scrollback = DenSettings.get('terminal_scrollback') ?? 1000;
    const fontSize = DenSettings.get('font_size') ?? 15;
    term = new TerminalClass({
      cursorBlink: true,
      fontSize,
      fontFamily: FONT_FAMILY,
      scrollback,
      theme: getXtermThemeFor(DenSettings.getPaneTheme('terminal-pane')),
    });

    fitAddon = new FitAddonClass();
    term.loadAddon(fitAddon);

    if (needsWebgl) selectRenderer(term);

    term.open(container);

    // OSC 52: clipboard write from terminal programs
    term.parser.registerOscHandler(52, (data) => {
      // Format: "c;base64data" or just "base64data"
      const parts = data.split(';');
      const b64 = parts.length > 1 ? parts[parts.length - 1] : parts[0];
      if (b64 === '?') return true; // query — ignore
      try {
        const text = atob(b64);
        DenClipboard.write(text, { source: 'osc52' }).catch(() => {});
      } catch (_) { /* invalid base64 — ignore */ }
      return true;
    });

    term.onTitleChange((title) => { DenSettings.setOscTitle(title); });

    fitAndRefresh();

    // フォント読み込み完了後に再 fit
    if (document.fonts?.ready) {
      document.fonts.ready.then(() => fitAndRefresh());
    }
    window.addEventListener('pageshow', () => fitAndRefresh());
    const resizeObserver = new ResizeObserver(() => scheduleFit());
    resizeObserver.observe(container);

    // restty auto-resize: onGridSize fires onResize — sync PTY server
    term.onResize(() => sendResize());

    // キーバー修飾キーが ON のとき、物理キーと組み合わせて修飾付きシーケンスを送信
    const PHYSICAL_KEY_MAP = {
      Enter: '\r', Tab: '\t', Escape: '\x1b', Backspace: '\x7f',
      Delete: '\x1b[3~', Insert: '\x1b[2~',
      ArrowUp: '\x1b[A', ArrowDown: '\x1b[B', ArrowRight: '\x1b[C', ArrowLeft: '\x1b[D',
      Home: '\x1b[H', End: '\x1b[F', PageUp: '\x1b[5~', PageDown: '\x1b[6~',
    };
    // iPad soft keyboard workaround: after keybar modifier combo,
    // the character may leak through input event despite preventDefault.
    let _suppressLeakedChar = null;
    let _suppressTimer = null;

    term.attachCustomKeyEventHandler((ev) => {
      if (ev.type !== 'keydown') return true;
      // ハードウェア修飾キー自体や単独の Meta は無視
      if (ev.key === 'Control' || ev.key === 'Alt' || ev.key === 'Shift' || ev.key === 'Meta') return true;

      const keybarMods = Keybar.getModifiers();
      // キーバー修飾 + ハードウェア修飾をマージ
      const mergedMods = {
        ctrl: keybarMods.ctrl || ev.ctrlKey,
        alt: keybarMods.alt || ev.altKey,
        shift: keybarMods.shift || ev.shiftKey,
      };

      // 修飾キーなし → xterm に委譲
      if (!mergedMods.ctrl && !mergedMods.alt && !mergedMods.shift) return true;

      // キーバー修飾が未使用 + 印字文字 → xterm のネイティブ処理に任せる
      // （Ctrl+C, Alt+D 等は xterm が正しく処理する）
      if (!keybarMods.ctrl && !keybarMods.alt && !keybarMods.shift && ev.key.length === 1) {
        return true;
      }

      // キーバー修飾が未使用 + Meta キー → ブラウザ/xterm に委譲（Cmd+C=コピー等）
      if (!keybarMods.ctrl && !keybarMods.alt && !keybarMods.shift && ev.metaKey) {
        return true;
      }

      // キーバー修飾 or ハードウェア修飾 + 特殊キーの組み合わせを送信
      const send = ev.key.length === 1 ? ev.key : PHYSICAL_KEY_MAP[ev.key];
      if (send) {
        ev.preventDefault(); // Prevent character insertion into xterm's textarea
        Keybar.executeKey({ send }, mergedMods);
        // iPad fallback: soft keyboard may still insert the character via input event
        if (ev.key.length === 1) {
          _suppressLeakedChar = ev.key;
          if (_suppressTimer) clearTimeout(_suppressTimer);
          _suppressTimer = setTimeout(() => { _suppressLeakedChar = null; }, 100);
        }
        return false;
      }
      // 未マップキー（F1〜F12等）はキーバー修飾をリセットして xterm に委譲
      Keybar.resetModifiers();
      return true;
    });

    // キー入力 → WebSocket
    // restty dedup: restty fires onData multiple times per keystroke
    // (keydown + beforeinput events, each triggering emitData + ptyTransport).
    // Allow only the first send per browser task; clear with setTimeout(0)
    // which fires after all events in the current task are processed.
    let _resttyDedupActive = false;

    term.onData((data) => {
      // Suppress leaked character from keybar modifier combo (iPad soft keyboard workaround)
      if (_suppressLeakedChar !== null && data === _suppressLeakedChar) {
        _suppressLeakedChar = null;
        if (_suppressTimer) { clearTimeout(_suppressTimer); _suppressTimer = null; }
        return;
      }
      // Do not send input when terminal pane is hidden (e.g. Chat/Files tab active)
      if (document.getElementById('terminal-pane').hidden) return;
      if (isRestty && _resttyDedupActive) return;
      if (isRestty) {
        _resttyDedupActive = true;
        setTimeout(() => { _resttyDedupActive = false; }, 0);
      }
      if (ws && ws.readyState === WebSocket.OPEN) {
        const filtered = filterMouseSeqs(data);
        if (filtered) ws.send(textEncoder.encode(filtered));
      }
    });

    term.onBinary((data) => {
      if (ws && ws.readyState === WebSocket.OPEN) {
        if (isX10Mouse(data)) return;
        const filtered = filterMouseSeqs(data);
        if (!filtered) return;
        const bytes = new Uint8Array(filtered.length);
        for (let i = 0; i < filtered.length; i++) {
          bytes[i] = filtered.charCodeAt(i) & 0xff;
        }
        ws.send(bytes);
      }
    });

    // Context menu: "Send to Chat" when text is selected
    container.addEventListener('contextmenu', (e) => {
      const sel = term.getSelection();
      if (!sel) return; // no selection — let default menu through
      e.preventDefault();
      showTerminalContextMenu(e.clientX, e.clientY, sel);
    });

    return term;
  }

  // ── Terminal context menu ──
  let ctxMenu = null;

  function showTerminalContextMenu(x, y, selectedText) {
    hideTerminalContextMenu();
    ctxMenu = document.createElement('div');
    ctxMenu.className = 'context-menu';
    ctxMenu.style.left = x + 'px';
    ctxMenu.style.top = y + 'px';

    const sendItem = document.createElement('div');
    sendItem.className = 'context-menu-item';
    sendItem.textContent = 'Send to Chat';
    sendItem.addEventListener('click', () => {
      document.dispatchEvent(new CustomEvent('den:send-to-chat', {
        detail: { text: selectedText, source: 'terminal' },
      }));
      hideTerminalContextMenu();
    });
    ctxMenu.appendChild(sendItem);

    const copyItem = document.createElement('div');
    copyItem.className = 'context-menu-item';
    copyItem.textContent = 'Copy';
    copyItem.addEventListener('click', () => {
      DenClipboard.write(selectedText).catch(() => {});
      hideTerminalContextMenu();
    });
    ctxMenu.appendChild(copyItem);

    document.body.appendChild(ctxMenu);

    // Clamp to viewport
    const rect = ctxMenu.getBoundingClientRect();
    if (rect.right > window.innerWidth) ctxMenu.style.left = (window.innerWidth - rect.width - 4) + 'px';
    if (rect.bottom > window.innerHeight) ctxMenu.style.top = (window.innerHeight - rect.height - 4) + 'px';

    // Close on click outside or Escape
    setTimeout(() => {
      document.addEventListener('click', hideTerminalContextMenu, { once: true });
      document.addEventListener('keydown', ctxMenuEscHandler);
    }, 0);
  }

  function ctxMenuEscHandler(e) {
    if (e.key === 'Escape') hideTerminalContextMenu();
  }

  function hideTerminalContextMenu() {
    if (ctxMenu) {
      ctxMenu.remove();
      ctxMenu = null;
      document.removeEventListener('click', hideTerminalContextMenu);
      document.removeEventListener('keydown', ctxMenuEscHandler);
    }
  }

  function connect(sessionName, remoteName) {
    currentSession = sessionName || null;
    currentRemote = remoteName || null;
    if (!currentSession) {
      disconnect();
      showEmptyState();
      DenSettings.setTitleTab('terminal', null);
      return;
    }
    hideEmptyState();
    const displayName = currentRemote ? `${getRemoteLabel(currentRemote)}:${currentSession}` : currentSession;
    DenSettings.setTitleTab('terminal', displayName);
    doConnect();
  }

  let emptyStateEl = null;

  function showEmptyState() {
    if (emptyStateEl) return;
    const container = document.getElementById('terminal-container');
    if (!container) return;
    emptyStateEl = document.createElement('div');
    emptyStateEl.className = 'terminal-empty-state';
    emptyStateEl.setAttribute('role', 'status');
    emptyStateEl.textContent = 'No sessions. Press + to create one.';
    container.appendChild(emptyStateEl);
  }

  function hideEmptyState() {
    if (emptyStateEl) { emptyStateEl.remove(); emptyStateEl = null; }
  }

  /** Transition to sessionless (null) state */
  function enterNullState() {
    currentSession = null;
    currentRemote = null;
    DenSettings.setOscTitle('');
    DenSettings.setTitleTab('terminal', null);
    disconnect();
    if (term) term.reset();
    showEmptyState();
    window.DenApp?.updateSessionHash(null);
  }

  function disconnect() {
    connectGeneration++;
    if (pingTimer) { clearInterval(pingTimer); pingTimer = null; }
    if (manualReconnectDisposable) { manualReconnectDisposable.dispose(); manualReconnectDisposable = null; }
    if (ws) {
      ws.onopen = ws.onclose = ws.onerror = ws.onmessage = null;
      ws.close();
      ws = null;
    }
  }

  function doConnect() {
    const generation = ++connectGeneration;
    reconnectAttempts = 0;
    lastSentCols = 0;
    lastSentRows = 0;
    if (manualReconnectDisposable) { manualReconnectDisposable.dispose(); manualReconnectDisposable = null; }
    const cols = term.cols;
    const rows = term.rows;
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    // Route WS through remote/relay proxy if connected to another Den
    const wsPath = getWsPath();
    const url = `${proto}//${location.host}${wsPath}?cols=${cols}&rows=${rows}&session=${encodeURIComponent(currentSession)}`;

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

      // rAF batching: buffer incoming WS binary data and flush once per frame.
      // null sentinel is used instead of 0 because requestAnimationFrame() is
      // specified to return a positive integer, but null unambiguously means
      // "no pending rAF".
      let writeBuf = [];
      let writeRaf = null;

      ws.onopen = () => {
        retries = 0;
        if (pingTimer) clearInterval(pingTimer);
        pingTimer = setInterval(() => {
          if (ws && ws.readyState === WebSocket.OPEN) {
            ws.send(WS_PING_MSG);
          }
        }, WS_PING_INTERVAL_MS);
        term.focus();
        fitAndRefresh();
      };

      ws.onmessage = (event) => {
        if (typeof event.data === 'string') {
          // Text branch carries only JSON control messages (e.g. session_ended);
          // written immediately since batching is not needed here.
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
          writeBuf.push(new Uint8Array(event.data));
          if (writeRaf === null) {
            // When the tab is hidden, rAF callbacks are suspended by the browser,
            // which would cause writeBuf to grow without bound. Fall back to
            // direct write so data is consumed immediately.
            if (document.hidden) {
              const chunks = writeBuf;
              writeBuf = [];
              if (chunks.length === 1) {
                term.write(chunks[0]);
              } else {
                const merged = mergeChunks(chunks);
                term.write(merged);
              }
            } else {
              writeRaf = requestAnimationFrame(() => {
                const chunks = writeBuf;
                writeBuf = [];
                writeRaf = null;
                if (chunks.length === 1) {
                  term.write(chunks[0]);
                } else {
                  const merged = mergeChunks(chunks);
                  term.write(merged);
                }
              });
            }
          }
        }
      };

      ws.onclose = () => {
        if (pingTimer) { clearInterval(pingTimer); pingTimer = null; }
        // Cancel any pending rAF to prevent stale data from a closed connection
        // being written to the terminal after reconnect.
        if (writeRaf !== null) { cancelAnimationFrame(writeRaf); writeRaf = null; }
        writeBuf = [];
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

    let countdown = 1;
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
  function switchSession(name, remote) {
    remote = remote || null;
    if (!name || (name === currentSession && remote === currentRemote)) return;
    currentSession = name;
    currentRemote = remote;
    hideEmptyState();
    DenSettings.setOscTitle('');
    const displayName = remote ? `${getRemoteLabel(remote)}:${name}` : name;
    DenSettings.setTitleTab('terminal', displayName);
    scheduleSessionTabsLayout({ scrollActive: true });
    term.reset();
    doConnect();
    window.DenApp?.updateSessionHash(remote ? `${remote}:${name}` : name);
  }

  function sendResize() {
    if (ws && ws.readyState === WebSocket.OPEN && term) {
      const { cols, rows } = term;
      if (cols === 0 || rows === 0) return;
      if (cols === lastSentCols && rows === lastSentRows) return;
      lastSentCols = cols;
      lastSentRows = rows;
      ws.send(JSON.stringify({ type: 'resize', cols, rows }));
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
  let selectModeStart = null; // { col, row } (buffer coordinates)
  let selectModeOnExit = null;
  let selectModeScreen = null; // F016: cached .xterm-screen element

  function enterSelectMode(onExit) {
    if (selectModeActive) return;
    const container = document.getElementById('terminal-container');
    if (!container) return;
    selectModeActive = true;
    selectModeStart = null;
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
    selectModeStart = null;
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

  /** Convert tap coordinates to buffer (col, row) */
  function tapToBufferPos(e) {
    const screen = selectModeScreen
      || term.element?.querySelector('.xterm-screen')
      || term.element?.querySelector('.wterm');
    if (!screen) return null;
    const rect = screen.getBoundingClientRect();
    if (rect.height === 0 || rect.width === 0 || term.rows === 0 || term.cols === 0) return null;
    const cellHeight = rect.height / term.rows;
    const cellWidth = rect.width / term.cols;
    const viewportRow = Math.max(0, Math.min(term.rows - 1, Math.floor((e.clientY - rect.top) / cellHeight)));
    const col = Math.max(0, Math.min(term.cols - 1, Math.floor((e.clientX - rect.left) / cellWidth)));
    const bufferRow = viewportRow + term.buffer.active.viewportY;
    return { col, row: bufferRow };
  }

  async function onSelectModeTap(e) {
    if (!term) return;

    const pos = tapToBufferPos(e);
    if (!pos) return;

    if (selectModeStart === null) {
      // First tap — mark start position and show a single-character selection as feedback
      selectModeStart = pos;
      term.select(pos.col, pos.row, 1);
    } else {
      // Second tap — select range from start to end position and copy
      const start = selectModeStart;
      const end = pos;

      // Normalize: ensure start is before end
      let sCol, sRow, eCol, eRow;
      if (start.row < end.row || (start.row === end.row && start.col <= end.col)) {
        sCol = start.col; sRow = start.row;
        eCol = end.col; eRow = end.row;
      } else {
        sCol = end.col; sRow = end.row;
        eCol = start.col; eRow = start.row;
      }

      // Calculate selection length in characters
      let length;
      if (sRow === eRow) {
        length = eCol - sCol + 1;
      } else {
        length = (term.cols - sCol) // rest of first line
               + (eRow - sRow - 1) * term.cols // middle lines
               + (eCol + 1); // beginning of last line
      }

      term.select(sCol, sRow, length);
      const sel = term.getSelection();
      if (sel) {
        try {
          await DenClipboard.write(sel);
          if (typeof Toast !== 'undefined') Toast.success('Copied');
        } catch (_) {
          if (typeof Toast !== 'undefined') Toast.error('Copy failed');
        }
      }
      exitSelectMode();
    }
  }

  function focus() {
    if (term) term.focus();
  }

  function blur() {
    if (term) term.blur();
  }

  function getTerminal() {
    return term;
  }

  function getCurrentSession() {
    return currentSession;
  }

  // --- Session management ---

  // F022: Cache static DOM elements (set in initSessionBar)
  let sessionTabsEl = null;
  let sessionClientsEl = null;
  // F004: Skip DOM rebuild when sessions unchanged
  let lastSessionsKey = '';
  let sessionTabsLayoutRafId = null;
  let shouldScrollActiveSessionTab = false;

  function syncSessionTabSelection() {
    if (!sessionTabsEl) return;
    for (const tab of sessionTabsEl.querySelectorAll('.session-tab')) {
      const isActive = tab.dataset.session === currentSession
        && (tab.dataset.remote || null) === currentRemote;
      tab.classList.toggle('active', isActive);
      tab.setAttribute('tabindex', isActive ? '0' : '-1');
      tab.setAttribute('aria-selected', isActive ? 'true' : 'false');
    }
  }

  function updateSessionTabsOverflow() {
    if (!sessionTabsEl) return;
    const maxScrollLeft = Math.max(0, sessionTabsEl.scrollWidth - sessionTabsEl.clientWidth);
    const scrollLeft = sessionTabsEl.scrollLeft;
    sessionTabsEl.classList.toggle('overflow-left', scrollLeft > 4);
    sessionTabsEl.classList.toggle('overflow-right', maxScrollLeft - scrollLeft > 4);
  }

  function onWindowResize() { scheduleSessionTabsLayout(); }

  function scheduleSessionTabsLayout(options = {}) {
    if (!sessionTabsEl) return;
    shouldScrollActiveSessionTab = shouldScrollActiveSessionTab || !!options.scrollActive;
    if (sessionTabsLayoutRafId != null) return;
    sessionTabsLayoutRafId = requestAnimationFrame(() => {
      sessionTabsLayoutRafId = null;
      syncSessionTabSelection();
      const activeTab = sessionTabsEl.querySelector('.session-tab.active');
      if (shouldScrollActiveSessionTab && activeTab) {
        activeTab.scrollIntoView({ block: 'nearest', inline: 'nearest' });
      }
      shouldScrollActiveSessionTab = false;
      updateSessionTabsOverflow();
    });
  }

  async function fetchSessions() {
    try {
      const resp = await fetch('/api/terminal/sessions', {
        credentials: 'same-origin',
      });
      if (resp.ok) return await resp.json();
    } catch (_) { /* ignore */ }
    return [];
  }

  async function saveSessionOrder(order) {
    try {
      await fetch('/api/terminal/sessions/order', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'same-origin',
        body: JSON.stringify(order),
      });
    } catch (_) { /* best-effort */ }
  }

  /** Fetch sessions from local + all remote Den connections + relay */
  async function fetchAllSessions() {
    const local = await fetchSessions();

    // Mark local sessions
    const all = local.map(s => ({ ...s, remote: null, remoteDisplayName: null }));

    // Fetch sessions for all Den connections (direct + relay) in parallel
    const denConns = typeof FilerRemote !== 'undefined' ? FilerRemote.getDenConnections() : {};
    const denEntries = Object.entries(denConns);
    if (denEntries.length > 0) {
      const results = await Promise.all(denEntries.map(async ([connId, info]) => {
        try {
          const apiPrefix = info.type === 'relay'
            ? `/api/relay/${connId}`
            : `/api/remote/${connId}`;
          const [sessResp, portsResp] = await Promise.all([
            fetch(`${apiPrefix}/terminal/sessions`, { credentials: 'same-origin' }),
            fetch(`${apiPrefix}/ports`, { credentials: 'same-origin' }),
          ]);
          if (sessResp.ok) {
            const remotePorts = portsResp.ok ? await portsResp.json() : [];
            return (await sessResp.json()).map(s => ({
              ...s, remote: connId,
              remoteDisplayName: info.displayName || null,
              detected_ports: remotePorts.filter(p => !p.session || p.session === s.name),
            }));
          }
        } catch { /* ignore */ }
        return [];
      }));
      for (const sessions of results) all.push(...sessions);
    }

    return all;
  }

  /** Get API base path for session operations */
  function sessionApiBase(remote) {
    if (!remote) return '/api';
    const conns = typeof FilerRemote !== 'undefined' ? FilerRemote.getDenConnections() : {};
    const conn = conns[remote];
    if (conn?.type === 'relay') return `/api/relay/${remote}`;
    return `/api/remote/${remote}`;
  }

  async function createSession(name, sshConfig, remote) {
    try {
      const body = { name };
      if (sshConfig) body.ssh = sshConfig;
      const base = sessionApiBase(remote);
      const resp = await fetch(`${base}/terminal/sessions`, {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });
      return resp.ok || resp.status === 201;
    } catch (_) {
      return false;
    }
  }

  async function renameSession(oldName, newName, remote) {
    try {
      const base = sessionApiBase(remote);
      const resp = await fetch(`${base}/terminal/sessions/${encodeURIComponent(oldName)}`, {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: newName }),
      });
      return resp.ok || resp.status === 204;
    } catch (_) {
      return false;
    }
  }

  async function destroySession(name, remote) {
    try {
      const base = sessionApiBase(remote);
      const resp = await fetch(`${base}/terminal/sessions/${encodeURIComponent(name)}`, {
        method: 'DELETE',
        credentials: 'same-origin',
      });
      return resp.ok || resp.status === 204;
    } catch (_) {
      return false;
    }
  }

  /** Check if a session matches the current selection */
  function isCurrentSession(s) {
    return s.name === currentSession && (s.remote || null) === currentRemote;
  }

  async function refreshSessionList() {
    if (!sessionTabsEl) return;

    const sessions = await fetchAllSessions();

    // F004: Skip DOM rebuild when sessions haven't changed
    const grouping = typeof DenSettings !== 'undefined'
      ? DenSettings.get('group_remote_sessions') !== false : true;
    const sessionsKey = JSON.stringify(sessions) + '|' + currentSession + '|' + currentRemote + '|' + grouping;
    if (sessionsKey === lastSessionsKey) return;
    lastSessionsKey = sessionsKey;

    sessionTabsEl.innerHTML = '';
    // Cache connections map for the render loop to avoid repeated shallow copies
    const cachedDenConns = typeof FilerRemote !== 'undefined' ? FilerRemote.getDenConnections() : {};

    // No sessions: show empty state and disconnect
    if (sessions.length === 0) {
      if (currentSession !== null) enterNullState();
    } else if (!currentSession) {
      // Recovery: sessions appeared while in empty state
      const alive = sessions.filter(s => s.alive);
      const target = alive.length > 0 ? alive[0] : sessions[0];
      switchSession(target.name, target.remote);
    } else if (currentSession && !sessions.find(s => isCurrentSession(s))) {
      // Current session no longer exists: switch to first alive session
      const alive = sessions.filter(s => s.alive);
      const target = alive.length > 0 ? alive[0] : sessions[0];
      switchSession(target.name, target.remote);
    }

    for (const s of sessions) {
      const tab = document.createElement('div');
      tab.className = 'session-tab';
      tab.dataset.session = s.name;
      tab.dataset.remote = s.remote || '';
      tab.setAttribute('role', 'tab');
      tab.draggable = !s.remote; // only local sessions are reorderable
      const isActive = isCurrentSession(s);
      tab.setAttribute('tabindex', isActive ? '0' : '-1');
      tab.setAttribute('aria-selected', isActive ? 'true' : 'false');
      if (isActive) tab.classList.add('active');
      if (!s.alive) tab.classList.add('dead');

      const label = document.createElement('span');
      label.className = 'session-tab-label';
      let displayLabel;
      if (s.remote && grouping) {
        const remoteLabel = s.remoteDisplayName || getRemoteLabel(s.remote, cachedDenConns);
        displayLabel = `${remoteLabel}:${s.name}`;
      } else {
        displayLabel = s.name;
      }
      label.textContent = displayLabel;
      label.title = s.remote
        ? `${s.remoteDisplayName ? s.remoteDisplayName + ' — ' : ''}${getRemoteLabel(s.remote, cachedDenConns)} — session: ${s.name}`
        : s.name;
      tab.appendChild(label);

      const closeBtn = document.createElement('button');
      closeBtn.className = 'session-tab-close';
      closeBtn.type = 'button';
      closeBtn.setAttribute('tabindex', '-1');
      closeBtn.textContent = '\u00d7';
      closeBtn.setAttribute('aria-label', `Kill session ${displayLabel}`);
      tab.appendChild(closeBtn);

      sessionTabsEl.appendChild(tab);
    }

    // F015: Null-safe clientsSpan access
    if (sessionClientsEl) {
      const current = sessions.find(s => isCurrentSession(s));
      if (current) {
        sessionClientsEl.textContent = `${current.client_count} client${current.client_count !== 1 ? 's' : ''}`;
      } else {
        sessionClientsEl.textContent = '';
      }
    }

    scheduleSessionTabsLayout();

    // Update port bar with detected ports from sessions
    checkRemotePorts(sessions);

    // Notify other modules (e.g. FloatTerminal) via event — avoids circular dependency
    document.dispatchEvent(new CustomEvent('den:sessions-changed', { detail: { sessions } }));
  }

  async function checkRemotePorts(sessions) {
    // Collect ports from SSH and remote sessions
    const allPorts = [];
    const seenPorts = new Set();
    for (const s of sessions) {
      if (!s.detected_ports || s.detected_ports.length === 0) continue;
      // Show ports from SSH sessions and remote Den sessions
      if (!s.ssh_host && !s.remote) continue;
      const sessionKey = s.remote ? `${s.remote}:${s.name}` : s.name;
      for (const p of s.detected_ports) {
        const key = `${s.remote || ''}:${p.port}`;
        if (!seenPorts.has(key)) {
          seenPorts.add(key);
          allPorts.push({ ...p, session: s.name, remote: s.remote, sshHost: s.ssh_host, sessionKey });
        }
      }
    }

    // Show clickable toast for newly detected ports
    for (const p of allPorts) {
      if (!lastKnownPorts[p.sessionKey]) lastKnownPorts[p.sessionKey] = new Set();
      if (!lastKnownPorts[p.sessionKey].has(p.port)) {
        lastKnownPorts[p.sessionKey].add(p.port);
        const label = p.remote ? `${p.remote}:${p.port}` : `Port ${p.port}`;
        Toast.show(`${label} detected — click to open`, 'info', 5000, {
          onClick: () => openPort(p),
        });
      }
    }

    // Update ports button visibility
    updatePortsButton(allPorts);
  }

  // Track current ports for the dialog
  let _currentPorts = [];

  function updatePortsButton(ports) {
    _currentPorts = ports;
    const btn = document.getElementById('ports-btn');
    if (!btn) return;
    btn.hidden = ports.length === 0;
    btn.classList.toggle('active', ports.length > 0);
  }

  function initPortsButton() {
    const btn = document.getElementById('ports-btn');
    if (!btn) return;
    btn.addEventListener('click', () => showPortsDialog());
  }

  function showPortsDialog() {
    const allModals = document.querySelectorAll('.modal');
    allModals.forEach(m => { m.hidden = true; });

    let modal = document.getElementById('ports-modal');
    if (!modal) {
      modal = document.createElement('div');
      modal.id = 'ports-modal';
      modal.className = 'modal';
      modal.innerHTML = `
        <div class="modal-content" style="max-width:420px">
          <h3>Detected Ports</h3>
          <div id="ports-modal-body"></div>
          <div class="modal-actions">
            <button class="modal-btn" id="ports-modal-close">Close</button>
          </div>
        </div>`;
      document.body.appendChild(modal);
      modal.addEventListener('click', (e) => {
        if (e.target === modal) modal.hidden = true;
      });
      modal.querySelector('#ports-modal-close').addEventListener('click', () => {
        modal.hidden = true;
      });
    }

    const body = modal.querySelector('#ports-modal-body');
    body.textContent = '';

    if (_currentPorts.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'connections-empty';
      empty.textContent = 'No ports detected';
      body.appendChild(empty);
    } else {
      for (const p of _currentPorts) {
        const entry = document.createElement('div');
        entry.className = 'connection-entry';
        entry.style.cursor = 'pointer';

        const header = document.createElement('div');
        header.className = 'connection-header';

        const name = document.createElement('span');
        name.className = 'connection-name';
        const host = p.sshHost || (p.remote ? p.remote : '');
        name.textContent = host ? `${host}:${p.port}` : `Port ${p.port}`;
        header.appendChild(name);

        if (p.remote) {
          const badge = document.createElement('span');
          badge.className = 'connection-type-badge direct';
          badge.textContent = p.sshHost ? 'SSH' : p.remote;
          header.appendChild(badge);
        }

        entry.appendChild(header);

        if (p.source) {
          const details = document.createElement('div');
          details.className = 'connection-details';
          details.textContent = p.source;
          entry.appendChild(details);
        }

        entry.addEventListener('click', () => {
          modal.hidden = true;
          openPort(p);
        });
        body.appendChild(entry);
      }
    }

    modal.hidden = false;
  }

  function getFwdUrl(portInfo) {
    if (portInfo.remote) {
      const conns = typeof FilerRemote !== 'undefined' ? FilerRemote.getDenConnections() : {};
      const conn = conns[portInfo.remote];
      if (conn?.type === 'relay') {
        return `/api/relay/${portInfo.remote}/fwd/${portInfo.port}/`;
      }
      return `/api/remote/${portInfo.remote}/fwd/${portInfo.port}/`;
    }
    return `/fwd/${portInfo.port}/`;
  }

  async function openPort(portInfo) {
    const url = getFwdUrl(portInfo);
    // Open tab first to avoid popup blocker after await (F004)
    const tab = window.open('about:blank', '_blank', 'noopener,noreferrer');

    // For local SSH sessions, start tunnel first
    if (!portInfo.forwarded && !portInfo.remote && portInfo.session) {
      try {
        const resp = await fetch(
          `/api/terminal/sessions/${encodeURIComponent(portInfo.session)}/ports/${portInfo.port}/forward`,
          { method: 'POST', credentials: 'same-origin' }
        );
        if (!resp.ok && resp.status !== 201) {
          const msg = await resp.text();
          if (msg && !msg.includes('Password auth')) {
            Toast.error(`Forward failed: ${msg}`);
            if (tab) tab.close();
            return;
          }
        }
      } catch { /* ignore — will open directly */ }
    }

    if (tab) {
      tab.location.href = url;
    }
  }

  function initSessionBar() {
    // F022: Cache static DOM elements
    sessionTabsEl = document.getElementById('session-tabs');
    sessionClientsEl = document.getElementById('session-clients');
    const newBtn = document.getElementById('session-new-btn');

    if (sessionTabsEl) {
      sessionTabsEl.addEventListener('scroll', updateSessionTabsOverflow, { passive: true });
      window.addEventListener('resize', onWindowResize);

      // Event delegation for session tabs
      sessionTabsEl.addEventListener('click', async (e) => {
        const closeBtn = e.target.closest('.session-tab-close');
        if (closeBtn) {
          const tab = closeBtn.closest('.session-tab');
          if (!tab) return; // F008: null guard
          const name = tab.dataset.session;
          const remote = tab.dataset.remote || null;
          const displayName = remote ? `${getRemoteLabel(remote)}:${name}` : name;
          if (!(await Toast.confirm(`Kill session "${displayName}"?`))) return;
          const ok = await destroySession(name, remote);
          if (!ok) {
            Toast.error('Failed to kill session');
            return;
          }
          if (name === currentSession && remote === currentRemote) {
            enterNullState();
          }
          lastSessionsKey = ''; // Force refresh
          await refreshSessionList();
          return;
        }
        const tab = e.target.closest('.session-tab');
        if (tab) switchSession(tab.dataset.session, tab.dataset.remote || null);
      });

      // Rename session on double-click
      sessionTabsEl.addEventListener('dblclick', async (e) => {
        const tab = e.target.closest('.session-tab');
        if (!tab || e.target.closest('.session-tab-close')) return;
        const oldName = tab.dataset.session;
        const remote = tab.dataset.remote || null;
        const newName = await Toast.prompt('Rename session:', oldName);
        if (!newName || !newName.trim() || newName.trim() === oldName) return;
        const trimmed = newName.trim();
        const validationError = validateSessionName(trimmed);
        if (validationError) {
          Toast.error(validationError);
          return;
        }
        const ok = await renameSession(oldName, trimmed, remote);
        if (!ok) {
          Toast.error('Failed to rename session');
          return;
        }
        if (oldName === currentSession && remote === currentRemote) {
          currentSession = trimmed;
          const hash = remote ? `${remote}:${trimmed}` : trimmed;
          window.DenApp?.updateSessionHash(hash);
        }
        lastSessionsKey = '';
        await refreshSessionList();
      });

      // F001: Keyboard navigation (roving tabindex)
      sessionTabsEl.addEventListener('keydown', (e) => {
        const tab = e.target.closest('.session-tab');
        if (!tab) return;
        const tabs = [...sessionTabsEl.querySelectorAll('.session-tab')];
        const idx = tabs.indexOf(tab);
        if (idx === -1) return;

        let target;
        if (e.key === 'ArrowRight') {
          target = tabs[(idx + 1) % tabs.length];
        } else if (e.key === 'ArrowLeft') {
          target = tabs[(idx - 1 + tabs.length) % tabs.length];
        } else if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          switchSession(tab.dataset.session, tab.dataset.remote || null);
          return;
        } else {
          return;
        }
        e.preventDefault();
        if (target) {
          tab.setAttribute('tabindex', '-1');
          target.setAttribute('tabindex', '0');
          target.focus();
        }
      });

      // Drag & drop for session tab reordering
      let draggedTab = null;

      sessionTabsEl.addEventListener('dragstart', (e) => {
        const tab = e.target.closest('.session-tab');
        if (!tab || tab.dataset.remote) { e.preventDefault(); return; }
        draggedTab = tab;
        tab.classList.add('dragging');
        e.dataTransfer.effectAllowed = 'move';
        e.dataTransfer.setData('text/plain', tab.dataset.session);
      });

      sessionTabsEl.addEventListener('dragover', (e) => {
        if (!draggedTab) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = 'move';
        const tab = e.target.closest('.session-tab');
        if (!tab || tab === draggedTab || tab.dataset.remote) return;
        // Clear previous indicators
        for (const t of sessionTabsEl.querySelectorAll('.drag-over-left,.drag-over-right')) {
          t.classList.remove('drag-over-left', 'drag-over-right');
        }
        const rect = tab.getBoundingClientRect();
        const mid = rect.left + rect.width / 2;
        tab.classList.add(e.clientX < mid ? 'drag-over-left' : 'drag-over-right');
      });

      sessionTabsEl.addEventListener('dragleave', (e) => {
        const tab = e.target.closest('.session-tab');
        if (tab) tab.classList.remove('drag-over-left', 'drag-over-right');
      });

      sessionTabsEl.addEventListener('drop', async (e) => {
        e.preventDefault();
        if (!draggedTab) return;
        const tab = e.target.closest('.session-tab');
        if (!tab || tab === draggedTab || tab.dataset.remote) return;
        tab.classList.remove('drag-over-left', 'drag-over-right');
        const rect = tab.getBoundingClientRect();
        const mid = rect.left + rect.width / 2;
        if (e.clientX < mid) {
          sessionTabsEl.insertBefore(draggedTab, tab);
        } else {
          sessionTabsEl.insertBefore(draggedTab, tab.nextSibling);
        }
        // Save new order to server
        const order = [...sessionTabsEl.querySelectorAll('.session-tab')]
          .filter(t => !t.dataset.remote)
          .map(t => t.dataset.session);
        await saveSessionOrder(order);
      });

      sessionTabsEl.addEventListener('dragend', () => {
        if (draggedTab) draggedTab.classList.remove('dragging');
        draggedTab = null;
        for (const t of sessionTabsEl.querySelectorAll('.drag-over-left,.drag-over-right')) {
          t.classList.remove('drag-over-left', 'drag-over-right');
        }
      });
    }

    // Swipe gesture for session switching on touch devices
    const termContainer = document.getElementById('terminal-container');
    if (termContainer) {
      let touchStartX = 0;
      let touchStartY = 0;
      let swiping = false;

      termContainer.addEventListener('touchstart', (e) => {
        if (e.touches.length !== 1) return;
        touchStartX = e.touches[0].clientX;
        touchStartY = e.touches[0].clientY;
        swiping = true;
      }, { passive: true });

      termContainer.addEventListener('touchmove', (e) => {
        if (!swiping || !e.touches.length) return;
        const dy = Math.abs(e.touches[0].clientY - touchStartY);
        if (dy > 30) swiping = false;
      }, { passive: true });

      termContainer.addEventListener('touchend', (e) => {
        if (!swiping || !e.changedTouches.length) return;
        swiping = false;
        const dx = e.changedTouches[0].clientX - touchStartX;
        if (Math.abs(dx) < 50) return;

        const tabs = [...sessionTabsEl.querySelectorAll('.session-tab')];
        if (tabs.length < 2) return;
        const activeIdx = tabs.findIndex(t => t.classList.contains('active'));
        if (activeIdx === -1) return;

        let nextIdx;
        if (dx < 0) {
          // swipe left = next
          nextIdx = (activeIdx + 1) % tabs.length;
        } else {
          // swipe right = prev
          nextIdx = (activeIdx - 1 + tabs.length) % tabs.length;
        }
        const target = tabs[nextIdx];
        switchSession(target.dataset.session, target.dataset.remote || null);
      }, { passive: true });
    }

    if (newBtn) {
      newBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        showNewSessionMenu(newBtn);
      });
    }

    const redrawBtn = document.getElementById('session-redraw-btn');
    if (redrawBtn) {
      redrawBtn.addEventListener('click', () => {
        if (ws && ws.readyState === WebSocket.OPEN) {
          ws.send(JSON.stringify({ type: 'nudge' }));
        }
      });
    }

    // F016: Guard against timer double-start on visibilitychange
    let sessionRefreshTimer = setInterval(refreshSessionList, 5000);
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) {
        if (sessionRefreshTimer) {
          clearInterval(sessionRefreshTimer);
          sessionRefreshTimer = null;
        }
      } else if (!sessionRefreshTimer) {
        refreshSessionList();
        sessionRefreshTimer = setInterval(refreshSessionList, 5000);
      }
    });

    document.addEventListener('den:remote-changed', () => {
      lastSessionsKey = '';
      refreshSessionList();
    });

    initPortsButton();
  }

  /** Generate a unique session name from a base, appending -2, -3, etc. if needed. */
  async function uniqueSessionName(base) {
    const sessions = await fetchSessions();
    const names = new Set(sessions.map(s => s.name));
    if (!names.has(base)) return base;
    for (let i = 2; i <= 100; i++) {
      const candidate = `${base}-${i}`;
      if (!names.has(candidate)) return candidate;
    }
    return `${base}-${Date.now()}`;
  }

  /** Show dropdown menu for new session creation (local + SSH bookmarks). */
  async function showNewSessionMenu(anchorEl) {
    // Remove existing menu if any
    const existing = document.getElementById('new-session-menu');
    if (existing) { existing.remove(); return; }

    // Refresh connections to remove stale/disconnected entries
    if (typeof FilerRemote !== 'undefined' && FilerRemote.refreshDenConnections) {
      await FilerRemote.refreshDenConnections();
    }

    const menu = document.createElement('div');
    menu.id = 'new-session-menu';
    menu.className = 'new-session-menu';

    // Centralized cleanup: remove menu + all document listeners
    let closeMenu;

    // Local terminal option
    const localItem = document.createElement('div');
    localItem.className = 'new-session-menu-item';
    localItem.textContent = 'Local Terminal';
    localItem.addEventListener('click', async () => {
      closeMenu();
      const name = await Toast.prompt('Session name:');
      if (!name || !name.trim()) return;
      const trimmed = name.trim();
      const validationError = validateSessionName(trimmed);
      if (validationError) { Toast.error(validationError); return; }
      const ok = await createSession(trimmed);
      if (!ok) { Toast.error('Failed to create session'); return; }
      lastSessionsKey = '';
      await refreshSessionList();
      switchSession(trimmed);
    });
    menu.appendChild(localItem);

    // Den connections (one section per connection, direct + relay unified)
    const denConns = typeof FilerRemote !== 'undefined' ? FilerRemote.getDenConnections() : {};
    for (const [connId, info] of Object.entries(denConns)) {
      const prefix = info.type === 'relay' ? 'Relay' : 'Remote';
      const sep = document.createElement('div');
      sep.className = 'new-session-menu-separator';
      sep.textContent = `${prefix} ${info.displayName || stripPort(info.hostPort) || connId}`;
      menu.appendChild(sep);

      const newItem = document.createElement('div');
      newItem.className = 'new-session-menu-item';
      newItem.textContent = 'New Terminal';
      newItem.addEventListener('click', async () => {
        closeMenu();
        const name = await Toast.prompt('Session name:');
        if (!name || !name.trim()) return;
        const trimmed = name.trim();
        const validationError = validateSessionName(trimmed);
        if (validationError) { Toast.error(validationError); return; }
        const ok = await createSession(trimmed, null, connId);
        if (!ok) { Toast.error('Failed to create remote session'); return; }
        lastSessionsKey = '';
        await refreshSessionList();
        switchSession(trimmed, connId);
      });
      menu.appendChild(newItem);
    }

    // Quick Connect option
    const quickItem = document.createElement('div');
    quickItem.className = 'new-session-menu-item';
    quickItem.textContent = 'Quick Connect Den\u2026';
    quickItem.addEventListener('click', () => {
      closeMenu();
      DenFiler.showDenModal();
    });
    menu.appendChild(quickItem);

    // SSH bookmarks
    const bookmarks = DenSettings.get('ssh_bookmarks') || [];
    if (bookmarks.length > 0) {
      const sep = document.createElement('div');
      sep.className = 'new-session-menu-separator';
      sep.textContent = 'SSH';
      menu.appendChild(sep);

      for (const b of bookmarks) {
        const item = document.createElement('div');
        item.className = 'new-session-menu-item';
        item.textContent = b.label;
        item.title = `${b.username}@${b.host}:${b.port || 22}`;
        item.addEventListener('click', async () => {
          closeMenu();
          const base = `ssh-${b.label}`.replace(/[^a-zA-Z0-9-]/g, '-').replace(/-+/g, '-').substring(0, 60);
          const sessionName = await uniqueSessionName(base);
          const sshConfig = {
            host: b.host,
            port: b.port || 22,
            username: b.username,
            auth_type: b.auth_type || 'password',
            key_path: b.key_path || null,
            initial_dir: b.initial_dir || null,
          };
          const ok = await createSession(sessionName, sshConfig);
          if (!ok) { Toast.error('Failed to create SSH session'); return; }
          lastSessionsKey = '';
          await refreshSessionList();
          switchSession(sessionName);
        });
        menu.appendChild(item);
      }
    }

    // Position menu relative to the button, anchored to right edge
    const rect = anchorEl.getBoundingClientRect();
    menu.style.position = 'fixed';
    menu.style.right = (window.innerWidth - rect.right) + 'px';
    document.body.appendChild(menu);

    // Prefer opening above, fall back to below if not enough space
    const menuHeight = menu.offsetHeight;
    if (rect.top >= menuHeight + 4) {
      menu.style.bottom = (window.innerHeight - rect.top + 4) + 'px';
    } else {
      menu.style.top = (rect.bottom + 4) + 'px';
    }

    const closeHandler = (e) => {
      if (!menu.contains(e.target) && e.target !== anchorEl) closeMenu();
    };
    const escHandler = (e) => {
      if (e.key === 'Escape') { e.stopPropagation(); closeMenu(); }
    };
    closeMenu = () => {
      menu.remove();
      document.removeEventListener('click', closeHandler, true);
      document.removeEventListener('keydown', escHandler, true);
    };

    // Delay to avoid immediate close from the same click
    requestAnimationFrame(() => {
      document.addEventListener('click', closeHandler, true);
      document.addEventListener('keydown', escHandler, true);
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

  function getCurrentRemote() {
    return currentRemote;
  }

  // Update xterm theme when Den theme changes
  document.addEventListener('den:theme-changed', () => {
    if (!term) return;
    term.options.theme = getXtermThemeFor(DenSettings.getPaneTheme('terminal-pane'));
  });

  // Listen for "Run in Terminal" requests from Chat (module-scope, registered once)
  document.addEventListener('den:run-in-terminal', (e) => {
    const cmd = e.detail?.command;
    if (!cmd) return;
    if (!ws || ws.readyState !== WebSocket.OPEN) {
      if (typeof Toast !== 'undefined') Toast.error('Terminal not connected');
      return;
    }
    // Paste command without executing — user presses Enter to confirm
    sendInput(cmd);
  });

  return {
    init, connect, disconnect, sendInput, sendResize, focus, blur, fitAndRefresh, scheduleFit, getTerminal,
    getCurrentSession, getCurrentRemote, switchSession, refreshSessionList, initSessionBar,
    fetchSessions, fetchAllSessions, createSession, destroySession,
    enterSelectMode, exitSelectMode, isSelectMode,
    validateSessionName,
    getXtermTheme(themeName) { return getXtermThemeFor(themeName || DenSettings.getPaneTheme('terminal-pane')); },
    getFontFamily() { return FONT_FAMILY; },
    selectRenderer,
  };
})();
