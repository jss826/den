// Den - ターミナルモジュール
const DenTerminal = (() => {
  // #115 PoC: per-session term retention. Each session owns its own term + WS
  // (SessionTerm), kept live in a hidden host div so switching never resets the
  // terminal — client scrollback is preserved and #114-style interleaving is
  // structurally impossible. `term`/`fitAddon` MIRROR the active session so the
  // active-terminal helpers (fit, select mode, context menu, focus, theme)
  // keep operating on a single reference. Connection state lives per SessionTerm.
  let term = null;            // === active?.term
  let fitAddon = null;        // === active?.fitAddon
  let rootContainer = null;   // #terminal-container (set in init)
  let active = null;          // active SessionTerm or null
  const sessionTerms = new Map(); // id -> SessionTerm
  const lruOrder = [];        // session ids, least-recent first, most-recent last
  const MAX_RETAINED = 2;     // PoC: keep at most K=2 live terms (LRU)
  let activateSeq = 0;        // guards against stale async activation
  let currentSession = null;
  let currentRemote = null; // null for local, connectionId for remote Den
  const WS_PING_INTERVAL_MS = 30000;
  const WS_PING_MSG = JSON.stringify({ type: 'ping' });
  const textEncoder = new TextEncoder(); // 再利用で毎回の alloc を回避
  // Written (not term.reset()) when the server sends a full resync. A full
  // replay only happens when the server's window starts *past* our lastSeq, so
  // it is a history gap — never an overlap. Marking the gap keeps the existing
  // scrollback instead of wiping it (the old reset was the main cause of sparse
  // iPad scrollback after frequent reconnects).
  const GAP_MARKER = textEncoder.encode('\r\n\x1b[90m── reconnected ──\x1b[0m\r\n');

  /** Stable identity key for a (name, remote) session. */
  function sessionId(name, remote) {
    return (remote || '') + ' ' + name;
  }

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

  /** Merge multiple Uint8Array chunks into one to reduce xterm.js parser invocations. */
  function mergeChunks(chunks) {
    let total = 0;
    for (const c of chunks) total += c.length;
    const merged = new Uint8Array(total);
    let offset = 0;
    for (const c of chunks) { merged.set(c, offset); offset += c.length; }
    return merged;
  }

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

  function init(container) {
    rootContainer = container;
    // フォント読み込み完了後に active term を再 fit
    if (document.fonts?.ready) {
      document.fonts.ready.then(() => fitAndRefresh());
    }
    window.addEventListener('pageshow', () => fitAndRefresh());
    const resizeObserver = new ResizeObserver(() => scheduleFit());
    resizeObserver.observe(container);
  }

  /**
   * Build the xterm/restty/wterm instance for a SessionTerm and wire all its
   * per-term handlers (input → its OWN ws, OSC52, keybar, context menu).
   * The term renders into the session's hidden host div.
   */
  async function buildTerm(st) {
    const { TerminalClass, FitAddonClass, needsWebgl, isRestty } = await TerminalAdapter.ready();
    const scrollback = DenSettings.get('terminal_scrollback') ?? 5000;
    const fontSize = DenSettings.get('font_size') ?? 15;
    const t = new TerminalClass({
      cursorBlink: true,
      fontSize,
      fontFamily: FONT_FAMILY,
      scrollback,
      theme: getXtermThemeFor(DenSettings.getPaneTheme('terminal-pane')),
    });
    const fit = new FitAddonClass();
    t.loadAddon(fit);

    if (needsWebgl) selectRenderer(t);

    // The await above yields; the session may have been evicted/disposed in the
    // meantime (its host removed from the DOM). Don't open into a detached node.
    if (st.disposed) { try { t.dispose(); } catch (_) { /* ignore */ } return; }

    st.term = t;
    st.isRestty = isRestty;
    st.fitAddon = fit;

    t.open(st.host);

    // OSC 52: clipboard write from terminal programs
    t.parser.registerOscHandler(52, (data) => {
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

    // Only the active session may drive the window/tab title.
    t.onTitleChange((title) => { if (active === st) DenSettings.setOscTitle(title); });

    // restty auto-resize: onGridSize fires onResize — sync PTY server
    t.onResize(() => stSendResize(st));

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

    t.attachCustomKeyEventHandler((ev) => {
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

    // キー入力 → このセッションの WebSocket
    // restty dedup: restty fires onData multiple times per keystroke
    // (keydown + beforeinput events, each triggering emitData + ptyTransport).
    // Allow only the first send per browser task; clear with setTimeout(0)
    // which fires after all events in the current task are processed.
    let _resttyDedupActive = false;

    t.onData((data) => {
      // Background (non-active) terms never send input.
      if (active !== st) return;
      // Suppress leaked character from keybar modifier combo (iPad soft keyboard workaround)
      if (_suppressLeakedChar !== null && data === _suppressLeakedChar) {
        _suppressLeakedChar = null;
        if (_suppressTimer) { clearTimeout(_suppressTimer); _suppressTimer = null; }
        return;
      }
      // Do not send input when terminal pane is hidden (e.g. Files tab active)
      if (document.getElementById('terminal-pane').hidden) return;
      if (isRestty && _resttyDedupActive) return;
      if (isRestty) {
        _resttyDedupActive = true;
        setTimeout(() => { _resttyDedupActive = false; }, 0);
      }
      if (st.ws && st.ws.readyState === WebSocket.OPEN) {
        const filtered = filterMouseSeqs(data);
        if (filtered) st.ws.send(textEncoder.encode(filtered));
      }
    });

    t.onBinary((data) => {
      if (active !== st) return;
      if (st.ws && st.ws.readyState === WebSocket.OPEN) {
        if (isX10Mouse(data)) return;
        const filtered = filterMouseSeqs(data);
        if (!filtered) return;
        const bytes = new Uint8Array(filtered.length);
        for (let i = 0; i < filtered.length; i++) {
          bytes[i] = filtered.charCodeAt(i) & 0xff;
        }
        st.ws.send(bytes);
      }
    });

    // Context menu (Copy) when text is selected
    st.host.addEventListener('contextmenu', (e) => {
      const sel = t.getSelection();
      if (!sel) return; // no selection — let default menu through
      e.preventDefault();
      showTerminalContextMenu(e.clientX, e.clientY, sel);
    });
  }

  // ── Terminal context menu ──
  let ctxMenu = null;

  function showTerminalContextMenu(x, y, selectedText) {
    hideTerminalContextMenu();
    ctxMenu = document.createElement('div');
    ctxMenu.className = 'context-menu';
    ctxMenu.style.left = x + 'px';
    ctxMenu.style.top = y + 'px';

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

  const MAX_RECONNECT = 3;

  /** Reset module-level fit memo so the next fit re-measures the shown host. */
  function resetFitState() {
    lastFitContainerWidth = 0;
    lastFitContainerHeight = 0;
    fitRetryCount = 0;
  }

  // ── SessionTerm: one term + WS per session, retained across switches ──

  /** Create a SessionTerm and kick off its async term build. */
  function createSessionTerm(name, remote) {
    const host = document.createElement('div');
    host.className = 'term-session-host';
    host.hidden = true;
    rootContainer.appendChild(host);
    const st = {
      id: sessionId(name, remote), name, remote: remote || null,
      host, term: null, fitAddon: null, isRestty: false,
      ws: null, pingTimer: null, connectGeneration: 0,
      reconnectAttempts: 0, manualReconnectDisposable: null,
      lastSentCols: 0, lastSentRows: 0,
      // Absolute byte sequence of the last output the term has applied. Sent as
      // ?since=N on (re)connect so the server replays only the delta — preventing
      // the scrollback duplication that full re-replays caused on reconnect (#117).
      lastSeq: 0n,
      disposed: false, ready: null,
    };
    // Never reject: a failed build (adapter load error) leaves st.term null,
    // which activateSession/showSessionTerm guard against. This keeps callers
    // (which don't await activateSession) free of unhandled rejections.
    st.ready = buildTerm(st).catch((e) => {
      console.error('[DenTerminal] terminal build failed', e);
    });
    return st;
  }

  function stWsPath(st) {
    return st.remote ? `/api/remote/${st.remote}/ws` : '/api/ws';
  }

  async function stConnect(st) {
    await st.ready;
    if (st.disposed || !st.term) return; // build failed or evicted mid-flight
    const generation = ++st.connectGeneration;
    st.reconnectAttempts = 0;
    st.lastSentCols = 0;
    st.lastSentRows = 0;
    if (st.manualReconnectDisposable) { st.manualReconnectDisposable.dispose(); st.manualReconnectDisposable = null; }
    const cols = st.term.cols;
    const rows = st.term.rows;
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${proto}//${location.host}${stWsPath(st)}?cols=${cols}&rows=${rows}&session=${encodeURIComponent(st.name)}&since=${st.lastSeq}`;

    let retries = 0;

    const attemptConnect = () => {
      // 世代チェック: 新しい接続が始まっていたら中断
      if (generation !== st.connectGeneration) return;
      if (st.ws) {
        st.ws.onopen = st.ws.onclose = st.ws.onerror = st.ws.onmessage = null;
        st.ws.close();
        st.ws = null;
      }

      const ws = new WebSocket(url);
      st.ws = ws;
      ws.binaryType = 'arraybuffer';
      let sessionEnded = false;

      // rAF batching: buffer incoming WS binary data and flush once per frame.
      // null sentinel means "no pending rAF" (rAF returns a positive integer).
      let writeBuf = [];
      let writeRaf = null;
      // Seq of the most recently buffered (not-yet-written) frame. st.lastSeq is
      // advanced to this ONLY once the bytes are actually handed to the term in
      // flushWrite — never at receive time. Otherwise a frame received but then
      // discarded (writeBuf is cleared in ws.onclose, and a pending rAF is frozen
      // while the tab is backgrounded on iOS) would push lastSeq past data the
      // term never applied, leaving a gap in the delta replay after reconnect (#117).
      let pendingSeq = st.lastSeq;
      // Set when the server sends a {"type":"sync","mode":"full"} control frame,
      // meaning the client fell outside the server's replay window and the next
      // frame is a full window. We mark the gap and keep scrollback rather than
      // resetting (the window starts past lastSeq, so there is no overlap).
      let pendingReset = false;

      const flushWrite = () => {
        const chunks = writeBuf;
        if (chunks.length === 0) return;
        writeBuf = [];
        st.term.write(chunks.length === 1 ? chunks[0] : mergeChunks(chunks));
        // Commit the seq only now that the bytes live in the term's own buffer,
        // so a discarded writeBuf never advances lastSeq past unrendered output.
        st.lastSeq = pendingSeq;
      };

      ws.onopen = () => {
        retries = 0;
        if (st.pingTimer) clearInterval(st.pingTimer);
        st.pingTimer = setInterval(() => {
          if (ws.readyState === WebSocket.OPEN) ws.send(WS_PING_MSG);
        }, WS_PING_INTERVAL_MS);
        if (active === st) { st.term.focus(); fitAndRefresh(); }
      };

      ws.onmessage = (event) => {
        if (generation !== st.connectGeneration) return;
        if (typeof event.data === 'string') {
          // Text branch carries only JSON control messages (session_ended / sync).
          try {
            const msg = JSON.parse(event.data);
            if (msg.type === 'session_ended') {
              sessionEnded = true;
              st.term.writeln('\r\n\x1b[33mSession ended.\x1b[0m');
              refreshSessionList();
              return;
            }
            if (msg.type === 'sync') {
              // Full replay incoming: reset the term before applying it so the
              // authoritative window replaces (not appends to) stale scrollback.
              if (msg.mode === 'full') pendingReset = true;
              return;
            }
          } catch (_) {
            // テキストデータとして扱う
          }
          st.term.write(event.data);
        } else if (event.data instanceof ArrayBuffer) {
          // Every binary frame is [8-byte big-endian abs seq][terminal data].
          if (event.data.byteLength < 8) return;
          pendingSeq = new DataView(event.data).getBigUint64(0);
          if (pendingReset) {
            pendingReset = false;
            // Non-destructive resync: the full window starts past lastSeq (a
            // history gap, not an overlap), so keep the existing scrollback and
            // mark the gap. Queued before the full data so it flushes in order
            // after any already-buffered deltas.
            writeBuf.push(GAP_MARKER);
          }
          writeBuf.push(new Uint8Array(event.data, 8));
          if (writeRaf === null) {
            // rAF is throttled when the tab is hidden AND for background
            // (non-active) sessions whose host is display:none. Write directly
            // in those cases so the term keeps an accurate live scrollback.
            if (document.hidden || active !== st) {
              flushWrite();
            } else {
              writeRaf = requestAnimationFrame(() => {
                writeRaf = null;
                if (generation !== st.connectGeneration) return;
                flushWrite();
              });
            }
          }
        }
      };

      ws.onclose = () => {
        if (st.pingTimer) { clearInterval(st.pingTimer); st.pingTimer = null; }
        if (writeRaf !== null) { cancelAnimationFrame(writeRaf); writeRaf = null; }
        writeBuf = [];
        if (generation !== st.connectGeneration) return;
        if (sessionEnded) return;
        stStartReconnect(st, generation);
      };

      ws.onerror = (event) => {
        console.error('[DenTerminal] WebSocket error', event);
      };

      // Safari: WebSocket が CONNECTING のまま stall する問題のリトライ
      setTimeout(() => {
        if (generation !== st.connectGeneration) return;
        if (st.ws === ws && ws.readyState === WebSocket.CONNECTING && retries < 3) {
          retries++;
          attemptConnect();
        }
      }, 3000);
    };

    // 少し遅延させてから接続（Safari の初回 WS stall 軽減）
    setTimeout(attemptConnect, 200);
  }

  function stStartReconnect(st, generation) {
    st.reconnectAttempts++;
    if (st.reconnectAttempts > MAX_RECONNECT) {
      st.term.writeln('\r\n\x1b[31mConnection lost. Press Enter to reconnect.\x1b[0m');
      st.manualReconnectDisposable = st.term.onData((data) => {
        if (data === '\r' || data === '\n') {
          if (st.manualReconnectDisposable) { st.manualReconnectDisposable.dispose(); st.manualReconnectDisposable = null; }
          st.reconnectAttempts = 0;
          st.term.writeln('\r\n\x1b[33mReconnecting...\x1b[0m');
          stConnect(st);
        }
      });
      return;
    }

    let countdown = 1;
    st.term.write(`\r\n\x1b[31mDisconnected.\x1b[0m Reconnecting in \x1b[33m${countdown}\x1b[0m...`);
    const timer = setInterval(() => {
      if (generation !== st.connectGeneration) { clearInterval(timer); return; }
      countdown--;
      if (countdown > 0) {
        st.term.write(`\x1b[33m${countdown}\x1b[0m...`);
      } else {
        clearInterval(timer);
        st.term.writeln('');
        if (generation === st.connectGeneration) stConnect(st);
      }
    }, 1000);
  }

  function stDisconnect(st) {
    st.connectGeneration++;
    if (st.pingTimer) { clearInterval(st.pingTimer); st.pingTimer = null; }
    if (st.manualReconnectDisposable) { st.manualReconnectDisposable.dispose(); st.manualReconnectDisposable = null; }
    if (st.ws) {
      st.ws.onopen = st.ws.onclose = st.ws.onerror = st.ws.onmessage = null;
      st.ws.close();
      st.ws = null;
    }
  }

  function stSendResize(st) {
    if (st.ws && st.ws.readyState === WebSocket.OPEN && st.term) {
      const { cols, rows } = st.term;
      if (cols === 0 || rows === 0) return;
      if (cols === st.lastSentCols && rows === st.lastSentRows) return;
      st.lastSentCols = cols;
      st.lastSentRows = rows;
      st.ws.send(JSON.stringify({ type: 'resize', cols, rows }));
    }
  }

  function stSendInput(st, data) {
    if (st.ws && st.ws.readyState === WebSocket.OPEN) {
      st.ws.send(textEncoder.encode(data));
    }
  }

  function stDispose(st) {
    st.disposed = true; // signal an in-flight buildTerm to abort before t.open
    stDisconnect(st);
    try { st.term?.dispose(); } catch (_) { /* ignore */ }
    st.host?.remove();
  }

  // ── LRU retention (PoC: K=2) ──

  function touchLru(id) {
    const i = lruOrder.indexOf(id);
    if (i !== -1) lruOrder.splice(i, 1);
    lruOrder.push(id); // most-recent last
  }

  function forgetLru(id) {
    const i = lruOrder.indexOf(id);
    if (i !== -1) lruOrder.splice(i, 1);
  }

  /** Dispose and forget a retained session term. */
  function removeSessionTerm(id) {
    const st = sessionTerms.get(id);
    if (st) stDispose(st);
    sessionTerms.delete(id);
    forgetLru(id);
  }

  /** Evict least-recently-used terms beyond MAX_RETAINED (never the active). */
  function evictLru() {
    while (sessionTerms.size > MAX_RETAINED) {
      let victimId = null;
      for (const id of lruOrder) {
        if (!active || id !== active.id) { victimId = id; break; }
      }
      if (victimId == null) break;
      removeSessionTerm(victimId);
    }
  }

  /** Show a retained session term: unhide, re-apply settings, fit, focus. */
  async function showSessionTerm(st) {
    await st.ready;
    if (st.disposed || !st.term) return; // build failed or evicted mid-flight
    st.host.hidden = false;
    const t = st.term;
    // Re-apply settings that may have changed while this term was hidden.
    t.options.theme = getXtermThemeFor(DenSettings.getPaneTheme('terminal-pane'));
    t.options.fontSize = DenSettings.get('font_size') ?? 15;
    t.options.scrollback = DenSettings.get('terminal_scrollback') ?? 5000;
    resetFitState();
    fitAndRefresh();
    t.focus();
  }

  /**
   * Make (name, remote) the active session. Creates + connects the SessionTerm
   * if not retained, otherwise just shows it (no reset, scrollback preserved).
   */
  async function activateSession(name, remote) {
    remote = remote || null;
    const id = sessionId(name, remote);
    const seq = ++activateSeq;
    currentSession = name;
    currentRemote = remote;

    let st = sessionTerms.get(id);
    if (!st) {
      st = createSessionTerm(name, remote);
      sessionTerms.set(id, st);
      stConnect(st);
    }
    touchLru(id);

    await st.ready;
    if (seq !== activateSeq) return; // superseded by a newer activation
    if (st.disposed || !st.term) return; // build failed (e.g. adapter load error)

    if (active && active !== st) active.host.hidden = true;
    active = st;
    term = st.term;
    fitAddon = st.fitAddon;
    await showSessionTerm(st);
    if (seq !== activateSeq) return;
    evictLru();
  }

  function connect(sessionName, remoteName) {
    const name = sessionName || null;
    const remote = remoteName || null;
    if (!name) {
      enterNullState();
      return;
    }
    hideEmptyState();
    const displayName = remote ? `${getRemoteLabel(remote)}:${name}` : name;
    DenSettings.setTitleTab('terminal', displayName);
    activateSession(name, remote);
  }

  /** Transition to sessionless (null) state */
  function enterNullState() {
    currentSession = null;
    currentRemote = null;
    DenSettings.setOscTitle('');
    DenSettings.setTitleTab('terminal', null);
    // Abandon (dispose) the active session's term; other retained sessions
    // keep their live scrollback in case we switch back to them.
    if (active) {
      removeSessionTerm(active.id);
      active = null;
      term = null;
      fitAddon = null;
    }
    showEmptyState();
    window.DenApp?.updateSessionHash(null);
  }

  /** Disconnect the active session's WS (kept for API compatibility). */
  function disconnect() {
    if (active) stDisconnect(active);
  }

  /** セッションを切り替え */
  function switchSession(name, remote) {
    remote = remote || null;
    if (!name || (name === currentSession && remote === currentRemote)) return;
    hideEmptyState();
    DenSettings.setOscTitle('');
    const displayName = remote ? `${getRemoteLabel(remote)}:${name}` : name;
    DenSettings.setTitleTab('terminal', displayName);
    scheduleSessionTabsLayout({ scrollActive: true });
    // No reset / no full replay: the retained term keeps its scrollback (#115),
    // and there is no shared term for a stale connection to bleed into (#114).
    activateSession(name, remote);
    window.DenApp?.updateSessionHash(remote ? `${remote}:${name}` : name);
  }

  function sendResize() {
    if (active) stSendResize(active);
  }

  function sendInput(data) {
    if (active) stSendInput(active, data);
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
  // Guards against a second new-session menu being built while the first is
  // still awaiting (connection refresh + multiplexer status fetch).
  let newSessionMenuOpening = false;
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

  /** Fetch sessions from local + all remote Den connections */
  async function fetchAllSessions() {
    const local = await fetchSessions();

    // Mark local sessions
    const all = local.map(s => ({ ...s, remote: null, remoteDisplayName: null }));

    const denConns = typeof FilerRemote !== 'undefined' ? FilerRemote.getDenConnections() : {};
    const denEntries = Object.entries(denConns);
    if (denEntries.length > 0) {
      const results = await Promise.all(denEntries.map(async ([connId, info]) => {
        try {
          const apiPrefix = `/api/remote/${connId}`;
          const sessResp = await fetch(`${apiPrefix}/terminal/sessions`, { credentials: 'same-origin' });
          if (sessResp.ok) {
            return (await sessResp.json()).map(s => ({
              ...s, remote: connId,
              remoteDisplayName: info.displayName || null,
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
    return `/api/remote/${remote}`;
  }

  /**
   * Create or attach a session. Returns { ok, status, message }.
   * On a backend-name conflict the server replies 409 with a message that
   * callers can surface (e.g. "name already exists with a different backend").
   */
  async function createSession(name, sshConfig, remote, backend) {
    try {
      const body = { name };
      if (sshConfig) body.ssh = sshConfig;
      if (backend && backend !== 'shell') body.backend = backend;
      const base = sessionApiBase(remote);
      const resp = await fetch(`${base}/terminal/sessions`, {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });
      if (resp.ok || resp.status === 201) return { ok: true, status: resp.status, message: '' };
      let message = '';
      try { message = (await resp.text()).trim(); } catch (_) { /* ignore */ }
      return { ok: false, status: resp.status, message };
    } catch (_) {
      return { ok: false, status: 0, message: '' };
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

    // Notify filer connections dialog (and any future consumers) via event — avoids circular dependency
    document.dispatchEvent(new CustomEvent('den:sessions-changed', { detail: { sessions } }));
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
          } else {
            // Dispose the killed session's retained term if kept in the background.
            removeSessionTerm(sessionId(name, remote));
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
        if (active?.ws && active.ws.readyState === WebSocket.OPEN) {
          active.ws.send(JSON.stringify({ type: 'nudge' }));
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

  /**
   * Fetch multiplexer availability/sessions for local or a remote Den.
   * Bounded by a timeout so an unreachable remote Den can never stall the
   * new-session menu (returns null on timeout/error).
   */
  async function fetchMuxStatus(remoteConnId) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 2000);
    try {
      const base = remoteConnId ? `/api/remote/${remoteConnId}` : '/api';
      const resp = await fetch(`${base}/multiplexer/status`, {
        credentials: 'same-origin',
        signal: controller.signal,
      });
      if (!resp.ok) return null;
      return await resp.json();
    } catch (_) {
      return null;
    } finally {
      clearTimeout(timer);
    }
  }

  /**
   * Append backend (zellij/tmux) rows under the current machine group.
   * Each backend gets a single row: icon + label + session chips + New (+) chip.
   * status = { zellij:{available,sessions,aliases}, tmux:{available,sessions,aliases} }
   */
  function buildBackendSubmenu(menu, status, remoteConnId, closeMenu) {
    if (!status) return;
    for (const kind of ['zellij', 'tmux']) {
      const bs = status[kind];
      if (!bs || !bs.available) continue;

      const row = document.createElement('div');
      row.className = 'new-session-menu-backend';

      // Backend icon (SVG use reference)
      const icon = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
      icon.setAttribute('class', 'backend-icon');
      icon.dataset.backend = kind;
      const use = document.createElementNS('http://www.w3.org/2000/svg', 'use');
      use.setAttribute('href', `#ic-backend-${kind}`);
      icon.appendChild(use);
      row.appendChild(icon);

      // Backend label
      const label = document.createElement('span');
      label.className = 'new-session-menu-backend-label';
      label.textContent = kind;
      row.appendChild(label);

      // Session chips + New chip
      const chips = document.createElement('span');
      chips.className = 'new-session-menu-chips';
      const aliases = bs.aliases || {};
      for (const name of bs.sessions) {
        const chip = document.createElement('button');
        chip.type = 'button';
        chip.className = 'new-session-menu-chip';
        chip.textContent = aliases[name] || name;
        chip.title = aliases[name] ? `${aliases[name]} (${name})` : name;
        chip.addEventListener('click', async () => {
          closeMenu();
          const res = await createSession(name, null, remoteConnId, kind);
          if (!res.ok) { Toast.error(res.message || 'Failed to attach session'); return; }
          lastSessionsKey = '';
          await refreshSessionList();
          switchSession(name, remoteConnId || undefined);
        });
        chips.appendChild(chip);
      }
      // New (+) chip
      const plus = document.createElement('button');
      plus.type = 'button';
      plus.className = 'new-session-menu-chip new-session-menu-chip-new';
      plus.textContent = '+';
      plus.title = `New ${kind} session`;
      plus.addEventListener('click', async () => {
        closeMenu();
        const name = await Toast.prompt('Session name:');
        if (!name || !name.trim()) return;
        const trimmed = name.trim();
        const validationError = validateSessionName(trimmed);
        if (validationError) { Toast.error(validationError); return; }
        const res = await createSession(trimmed, null, remoteConnId, kind);
        if (!res.ok) { Toast.error(res.message || 'Failed to create session'); return; }
        lastSessionsKey = '';
        await refreshSessionList();
        switchSession(trimmed, remoteConnId || undefined);
      });
      chips.appendChild(plus);
      row.appendChild(chips);

      menu.appendChild(row);
    }
  }

  /**
   * Append a machine-group header row to the new-session menu.
   * iconId references an <symbol> in the SVG sprite in index.html.
   */
  function appendGroupHeader(menu, iconId, text) {
    const h = document.createElement('div');
    h.className = 'new-session-menu-group';
    const icon = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    icon.setAttribute('class', 'group-icon');
    const use = document.createElementNS('http://www.w3.org/2000/svg', 'use');
    use.setAttribute('href', `#${iconId}`);
    icon.appendChild(use);
    h.appendChild(icon);
    const span = document.createElement('span');
    span.textContent = text;
    h.appendChild(span);
    menu.appendChild(h);
    return h;
  }

  /** Show dropdown menu for new session creation (local + SSH bookmarks). */
  async function showNewSessionMenu(anchorEl) {
    // Remove existing menu if any (toggle behavior)
    const existing = document.getElementById('new-session-menu');
    if (existing) { existing.remove(); return; }
    // A build may already be in flight (awaits below run before the menu is
    // appended); ignore re-entry so we never create two menus.
    if (newSessionMenuOpening) return;
    newSessionMenuOpening = true;
    try {
      await buildNewSessionMenu(anchorEl);
    } finally {
      newSessionMenuOpening = false;
    }
  }

  async function buildNewSessionMenu(anchorEl) {
    // Refresh connections to remove stale/disconnected entries
    if (typeof FilerRemote !== 'undefined' && FilerRemote.refreshDenConnections) {
      await FilerRemote.refreshDenConnections();
    }

    const menu = document.createElement('div');
    menu.id = 'new-session-menu';
    menu.className = 'new-session-menu';

    // Centralized cleanup: remove menu + all document listeners
    let closeMenu;

    // Prefetch multiplexer status for local + every remote Den concurrently.
    // Serial awaits would make the menu open latency grow with each remote and
    // let a single unreachable remote stall the whole (local) menu.
    const denConns = typeof FilerRemote !== 'undefined' ? FilerRemote.getDenConnections() : {};
    const connIds = Object.keys(denConns);
    const [localStatus, ...remoteStatuses] = await Promise.all([
      fetchMuxStatus(null),
      ...connIds.map(id => fetchMuxStatus(id)),
    ]);
    const remoteStatusById = {};
    connIds.forEach((id, i) => { remoteStatusById[id] = remoteStatuses[i]; });

    // \u2500\u2500 Group: This Den (local) \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    appendGroupHeader(menu, 'ic-machine-local', 'This Den (local)');

    // Local Terminal item
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
      const backend = (typeof DenSettings !== 'undefined'
        ? DenSettings.get('default_backend') : 'shell') || 'shell';
      const res = await createSession(trimmed, null, null, backend);
      if (!res.ok) { Toast.error(res.message || 'Failed to create session'); return; }
      lastSessionsKey = '';
      await refreshSessionList();
      switchSession(trimmed);
    });
    menu.appendChild(localItem);

    // Local multiplexer backends (zellij/tmux) when available
    buildBackendSubmenu(menu, localStatus, null, () => closeMenu());

    // \u2500\u2500 Groups: Remote Den connections \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    for (const [connId, info] of Object.entries(denConns)) {
      const displayName = info.displayName || stripPort(info.hostPort) || connId;
      appendGroupHeader(menu, 'ic-machine-remote', `Remote: ${displayName}`);

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
        const res = await createSession(trimmed, null, connId);
        if (!res.ok) { Toast.error(res.message || 'Failed to create remote session'); return; }
        lastSessionsKey = '';
        await refreshSessionList();
        switchSession(trimmed, connId);
      });
      menu.appendChild(newItem);

      // Remote multiplexer backends (zellij/tmux) when available
      buildBackendSubmenu(menu, remoteStatusById[connId], connId, () => closeMenu());
    }

    // \u2500\u2500 Group: SSH bookmarks \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    const bookmarks = DenSettings.get('ssh_bookmarks') || [];
    if (bookmarks.length > 0) {
      appendGroupHeader(menu, 'ic-machine-ssh', 'SSH');

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
          const res = await createSession(sessionName, sshConfig);
          if (!res.ok) { Toast.error(res.message || 'Failed to create SSH session'); return; }
          lastSessionsKey = '';
          await refreshSessionList();
          switchSession(sessionName);
        });
        menu.appendChild(item);
      }
    }

    // \u2500\u2500 Manage sessions \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    const manageItem = document.createElement('div');
    manageItem.className = 'new-session-menu-item new-session-menu-manage';
    manageItem.textContent = 'Manage sessions\u2026';
    manageItem.addEventListener('click', () => {
      closeMenu();
      openSessionsModal();
    });
    menu.appendChild(manageItem);

    // \u2500\u2500 Quick Connect (always last) \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    const quickItem = document.createElement('div');
    quickItem.className = 'new-session-menu-item';
    quickItem.textContent = 'Quick Connect Den\u2026';
    quickItem.addEventListener('click', () => {
      closeMenu();
      DenFiler.showDenModal();
    });
    menu.appendChild(quickItem);

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

  // ── Sessions Management Modal ─────────────────────────────────────────────

  /** Return the API base prefix for local (null) or remote (connId) Den. */
  function muxApiBase(connId) {
    return connId ? `/api/remote/${connId}` : '/api';
  }

  /** POST to multiplexer/{op} endpoint, returns { ok, message? }. */
  async function muxOp(connId, op, payload) {
    try {
      const resp = await fetch(`${muxApiBase(connId)}/multiplexer/${op}`, {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload),
      });
      if (!resp.ok) return { ok: false, message: `HTTP ${resp.status}` };
      return await resp.json();
    } catch (e) {
      return { ok: false, message: String(e) };
    }
  }

  /** Build a single session row element. `exited` = zellij dead/resurrectable. */
  function buildSessionRow(kind, name, alias, connId, exited) {
    const row = document.createElement('div');
    row.className = 'sessions-row';
    row.dataset.backend = kind;
    row.dataset.name = name;
    if (exited) row.dataset.exited = 'true';

    const nameEl = document.createElement('span');
    nameEl.className = 'sessions-row-name';
    nameEl.textContent = alias ? `${alias} (${name})` : name;
    row.appendChild(nameEl);

    if (exited) {
      const tag = document.createElement('span');
      tag.className = 'sessions-row-tag';
      tag.textContent = 'exited';
      row.appendChild(tag);
    }

    const actions = document.createElement('span');
    actions.className = 'sessions-row-actions';

    const mk = (action, text) => {
      const b = document.createElement('button');
      b.type = 'button';
      b.className = 'sessions-action-btn';
      b.dataset.action = action;
      b.textContent = text;
      actions.appendChild(b);
      return b;
    };

    mk('rename', 'Rename').addEventListener('click', async () => {
      const next = await Toast.prompt('Alias (empty to clear):', alias || '');
      if (next === null) return;
      const res = await muxOp(connId, 'rename', { backend: kind, name, alias: next.trim() });
      if (!res.ok) { Toast.error(res.message || 'Rename failed'); return; }
      await renderSessionsModal(document.getElementById('sessions-modal-body'));
    });

    mk('copy', 'Copy attach').addEventListener('click', async () => {
      const cmd = kind === 'zellij' ? `zellij attach ${name}` : `tmux attach -t ${name}`;
      try {
        await navigator.clipboard.writeText(cmd);
        Toast.show('Copied', 'success');
      } catch (_) {
        Toast.error('Copy failed');
      }
    });

    // Kill targets *running* sessions. zellij refuses kill-session on an exited
    // session (surfaces a raw "Os NotFound" error), so for those we offer Delete.
    if (!exited) {
      mk('kill', 'Kill').addEventListener('click', async () => {
        const ok = await Toast.confirm(`Kill session "${name}"?`);
        if (!ok) return;
        const res = await muxOp(connId, 'kill', { backend: kind, name });
        if (!res.ok) { Toast.error(res.message || 'Kill failed'); return; }
        await renderSessionsModal(document.getElementById('sessions-modal-body'));
      });
    }

    // Delete (purge) only applies to exited zellij sessions — delete on a running
    // session is a no-op, and tmux has no separate delete concept.
    if (kind === 'zellij' && exited) {
      mk('delete', 'Delete').addEventListener('click', async () => {
        const ok = await Toast.confirm(`Delete (purge) session "${name}"?`);
        if (!ok) return;
        const res = await muxOp(connId, 'delete', { backend: kind, name });
        if (!res.ok) { Toast.error(res.message || 'Delete failed'); return; }
        await renderSessionsModal(document.getElementById('sessions-modal-body'));
      });
    }

    row.appendChild(actions);
    return row;
  }

  /** Render one Den's (local or remote) session groups into the modal body. */
  function renderSessionsGroup(body, title, status, connId) {
    if (!status) return;
    let any = false;

    const header = document.createElement('div');
    header.className = 'sessions-group-header';
    header.textContent = title;
    body.appendChild(header);

    for (const kind of ['zellij', 'tmux']) {
      const bs = status[kind];
      if (!bs || !bs.available || !bs.sessions || !bs.sessions.length) continue;
      any = true;

      const sub = document.createElement('div');
      sub.className = 'sessions-backend-header';
      sub.textContent = kind;
      body.appendChild(sub);

      const aliases = bs.aliases || {};
      const exitedSet = new Set(bs.exited || []);
      for (const name of bs.sessions) {
        body.appendChild(buildSessionRow(kind, name, aliases[name], connId, exitedSet.has(name)));
      }
    }

    if (!any) header.remove();
  }

  /** Fetch all sessions and render rows into the modal body. */
  async function renderSessionsModal(body) {
    body.innerHTML = '<div class="sessions-loading">Loading…</div>';

    const denConns = typeof FilerRemote !== 'undefined' ? FilerRemote.getDenConnections() : {};
    const connIds = Object.keys(denConns);

    const [localStatus, ...remoteStatuses] = await Promise.all([
      fetchMuxStatus(null),
      ...connIds.map(id => fetchMuxStatus(id)),
    ]);

    body.innerHTML = '';
    renderSessionsGroup(body, 'This Den (local)', localStatus, null);
    connIds.forEach((id, i) => {
      const info = denConns[id];
      const title = 'Remote: ' + (info.displayName || stripPort(info.hostPort) || id);
      renderSessionsGroup(body, title, remoteStatuses[i], id);
    });

    if (!body.children.length) {
      body.innerHTML = '<div class="sessions-empty">No multiplexer sessions found.</div>';
    }
  }

  /** Open the Sessions management modal. */
  async function openSessionsModal() {
    const modal = document.getElementById('sessions-modal');
    const body = document.getElementById('sessions-modal-body');
    if (!modal || !body) return;
    body.innerHTML = '';
    modal.hidden = false;
    try {
      await renderSessionsModal(body);
    } catch (_e) {
      modal.hidden = true;
      Toast.error('Failed to load sessions');
    }
  }

  // Wire up Sessions modal close/refresh buttons and backdrop click.
  // Scripts are loaded at the bottom of <body> so the DOM is already available here.
  (() => {
    const modal = document.getElementById('sessions-modal');
    const closeBtn = document.getElementById('sessions-modal-close');
    const refreshBtn = document.getElementById('sessions-modal-refresh');
    if (modal && closeBtn) {
      closeBtn.addEventListener('click', () => { modal.hidden = true; });
      modal.addEventListener('click', (e) => { if (e.target === modal) modal.hidden = true; });
    }
    const reRender = () => {
      const body = document.getElementById('sessions-modal-body');
      if (body) renderSessionsModal(body);
    };
    if (refreshBtn) refreshBtn.addEventListener('click', reRender);
    // The server lists sessions live, but the open modal is a snapshot — session
    // changes made elsewhere (in the terminal, another client, or directly on the
    // host) aren't reflected until a re-fetch. Re-fetch when returning to the tab.
    const refreshIfOpen = () => { if (modal && !modal.hidden && !document.hidden) reRender(); };
    window.addEventListener('focus', refreshIfOpen);
    document.addEventListener('visibilitychange', refreshIfOpen);
  })();

  // ── end Sessions Management Modal ─────────────────────────────────────────

  // Update xterm theme when Den theme changes — apply to ALL retained terms.
  document.addEventListener('den:theme-changed', () => {
    const theme = getXtermThemeFor(DenSettings.getPaneTheme('terminal-pane'));
    for (const st of sessionTerms.values()) {
      if (st.term) st.term.options.theme = theme;
    }
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
