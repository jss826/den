// Den - ターミナルモジュール
const DenTerminal = (() => {
  let term = null;
  let fitAddon = null;
  let ws = null;
  let currentSession = null;
  let currentPeer = null; // peer name (null for local sessions)
  let connectGeneration = 0; // doConnect の世代カウンタ（高速切り替え時の race 防止）

  // Peer color palette for remote session tab indicators
  const PEER_COLORS = ['#7aa2f7','#9ece6a','#e0af68','#bb9af7','#7dcfff','#f7768e','#ff9e64','#73daca'];
  let peerColorMap = {}; // peer name → color
  let cachedPeers = []; // last fetched peer list
  const textEncoder = new TextEncoder(); // 再利用で毎回の alloc を回避

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
    scheduleFit({ force: true, refresh: true });
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
    term.onData((data) => {
      // Suppress leaked character from keybar modifier combo (iPad soft keyboard workaround)
      if (_suppressLeakedChar !== null && data === _suppressLeakedChar) {
        _suppressLeakedChar = null;
        if (_suppressTimer) { clearTimeout(_suppressTimer); _suppressTimer = null; }
        return;
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

    return term;
  }

  function connect(sessionName, peerName) {
    currentSession = sessionName || null;
    currentPeer = peerName || null;
    if (!currentSession) {
      disconnect();
      showEmptyState();
      DenSettings.setTitleTab('terminal', null);
      return;
    }
    hideEmptyState();
    const displayName = currentPeer ? `${currentPeer}:${currentSession}` : currentSession;
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
    currentPeer = null;
    DenSettings.setOscTitle('');
    DenSettings.setTitleTab('terminal', null);
    disconnect();
    if (term) term.reset();
    showEmptyState();
    window.DenApp?.updateSessionHash(null);
  }

  function disconnect() {
    connectGeneration++;
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
    // Route WS through peer proxy if remote session
    const wsPath = currentPeer
      ? `/api/peers/${encodeURIComponent(currentPeer)}/ws`
      : '/api/ws';
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
  function switchSession(name, peer) {
    peer = peer || null;
    if (!name || (name === currentSession && peer === currentPeer)) return;
    currentSession = name;
    currentPeer = peer;
    hideEmptyState();
    DenSettings.setOscTitle('');
    const displayName = peer ? `${peer}:${name}` : name;
    DenSettings.setTitleTab('terminal', displayName);
    scheduleSessionTabsLayout({ scrollActive: true });
    term.reset();
    doConnect();
    window.DenApp?.updateSessionHash(peer ? `${peer}:${name}` : name);
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
    const screen = selectModeScreen || term.element?.querySelector('.xterm-screen');
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
        && (tab.dataset.peer || null) === currentPeer;
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

  async function fetchPeers() {
    return PeerCache.get();
  }

  /** Fetch sessions from local + all connected peers */
  async function fetchAllSessions() {
    const [local, peers] = await Promise.all([fetchSessions(), fetchPeers()]);
    cachedPeers = peers;

    // Update peer color map
    const connected = peers.filter(p => p.status === 'connected');
    peerColorMap = {};
    connected.forEach((p, i) => { peerColorMap[p.name] = PEER_COLORS[i % PEER_COLORS.length]; });

    // Mark local sessions
    const all = local.map(s => ({ ...s, peer: null }));

    // Fetch remote sessions in parallel
    const remotes = await Promise.all(connected.map(async (p) => {
      try {
        const r = await fetch(`/api/peers/${encodeURIComponent(p.name)}/terminal/sessions`, {
          credentials: 'same-origin',
        });
        if (!r.ok) return [];
        return (await r.json()).map(s => ({ ...s, peer: p.name }));
      } catch { return []; }
    }));
    for (const rs of remotes) all.push(...rs);
    return all;
  }

  /** Get API base path for session operations */
  function sessionApiBase(peer) {
    if (!peer) return '';
    return `/api/peers/${encodeURIComponent(peer)}`;
  }

  async function createSession(name, sshConfig, peer) {
    try {
      const body = { name };
      if (sshConfig) body.ssh = sshConfig;
      const base = sessionApiBase(peer);
      const resp = await fetch(`${base}/api/terminal/sessions`, {
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

  async function renameSession(oldName, newName, peer) {
    try {
      const base = sessionApiBase(peer);
      const resp = await fetch(`${base}/api/terminal/sessions/${encodeURIComponent(oldName)}`, {
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

  async function destroySession(name, peer) {
    try {
      const base = sessionApiBase(peer);
      const resp = await fetch(`${base}/api/terminal/sessions/${encodeURIComponent(name)}`, {
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
    return s.name === currentSession && (s.peer || null) === currentPeer;
  }

  async function refreshSessionList() {
    if (!sessionTabsEl) return;

    const sessions = await fetchAllSessions();

    // F004: Skip DOM rebuild when sessions haven't changed
    const sessionsKey = JSON.stringify(sessions) + '|' + currentSession + '|' + currentPeer;
    if (sessionsKey === lastSessionsKey) return;
    lastSessionsKey = sessionsKey;

    sessionTabsEl.innerHTML = '';

    // No sessions: show empty state and disconnect
    if (sessions.length === 0) {
      if (currentSession !== null) enterNullState();
    } else if (!currentSession) {
      // Recovery: sessions appeared while in empty state
      const alive = sessions.filter(s => s.alive);
      const target = alive.length > 0 ? alive[0] : sessions[0];
      switchSession(target.name, target.peer);
    } else if (currentSession && !sessions.find(s => isCurrentSession(s))) {
      // Current session no longer exists: switch to first alive session
      const alive = sessions.filter(s => s.alive);
      const target = alive.length > 0 ? alive[0] : sessions[0];
      switchSession(target.name, target.peer);
    }

    for (const s of sessions) {
      const tab = document.createElement('div');
      tab.className = 'session-tab';
      tab.dataset.session = s.name;
      tab.dataset.peer = s.peer || '';
      tab.setAttribute('role', 'tab');
      const isActive = isCurrentSession(s);
      tab.setAttribute('tabindex', isActive ? '0' : '-1');
      tab.setAttribute('aria-selected', isActive ? 'true' : 'false');
      if (isActive) tab.classList.add('active');
      if (!s.alive) tab.classList.add('dead');

      // Peer color indicator
      if (s.peer && peerColorMap[s.peer]) {
        tab.classList.add('peer-session');
        tab.style.setProperty('--peer-color', peerColorMap[s.peer]);
      }

      const label = document.createElement('span');
      label.className = 'session-tab-label';
      const displayName = s.peer ? `${s.peer}:${s.name}` : s.name;
      label.textContent = displayName;
      label.title = s.peer
        ? `${s.peer} — session: ${s.name}`
        : s.name;
      tab.appendChild(label);

      const closeBtn = document.createElement('button');
      closeBtn.className = 'session-tab-close';
      closeBtn.type = 'button';
      closeBtn.setAttribute('tabindex', '-1');
      closeBtn.textContent = '\u00d7';
      closeBtn.setAttribute('aria-label', `Kill session ${displayName}`);
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
    // Collect SSH session ports (local non-SSH ports are accessible directly)
    const allPorts = [];
    const seenPorts = new Set();
    for (const s of sessions) {
      if (!s.detected_ports || s.detected_ports.length === 0) continue;
      // Only show ports from SSH sessions (local ports are directly accessible)
      if (!s.ssh_host && !s.peer) continue;
      const sessionKey = s.peer ? `${s.peer}:${s.name}` : s.name;
      for (const p of s.detected_ports) {
        const key = `${s.peer || ''}:${p.port}`;
        if (!seenPorts.has(key)) {
          seenPorts.add(key);
          allPorts.push({ ...p, session: s.name, peer: s.peer, sessionKey });
        }
      }
    }

    // Fetch remote peer ports in parallel (F001)
    const connectedPeers = (cachedPeers || []).filter(p => p.status === 'connected');
    const peerResults = await Promise.all(connectedPeers.map(async (peer) => {
      try {
        const resp = await fetch(`/api/peers/${encodeURIComponent(peer.name)}/ports`, { credentials: 'same-origin' });
        if (resp.ok) return { peer: peer.name, ports: await resp.json() };
      } catch { /* ignore */ }
      return null;
    }));
    for (const result of peerResults) {
      if (!result) continue;
      for (const rp of result.ports) {
        const key = `${result.peer}:${rp.port}`;
        if (!seenPorts.has(key)) {
          seenPorts.add(key);
          allPorts.push({ port: rp.port, forwarded: false, session: rp.session || null, peer: result.peer, sessionKey: `${result.peer}:${rp.session || 'system'}`, source: rp.source || 'system' });
        }
      }
    }

    // Show clickable toast for newly detected ports
    for (const p of allPorts) {
      if (!lastKnownPorts[p.sessionKey]) lastKnownPorts[p.sessionKey] = new Set();
      if (!lastKnownPorts[p.sessionKey].has(p.port)) {
        lastKnownPorts[p.sessionKey].add(p.port);
        const label = p.peer ? `${p.peer}:${p.port}` : `Port ${p.port}`;
        Toast.show(`${label} detected — click to open`, 'info', 5000, {
          onClick: () => openPort(p),
        });
      }
    }
  }

  async function openPort(portInfo) {
    // Open tab first to avoid popup blocker after await (F004)
    const url = portInfo.peer
      ? `/fwd/peer/${encodeURIComponent(portInfo.peer)}/${portInfo.port}/`
      : `/fwd/${portInfo.port}/`;
    const tab = window.open('about:blank', '_blank', 'noopener,noreferrer');

    // For SSH sessions, start tunnel first
    if (!portInfo.forwarded && portInfo.peer === null && portInfo.session) {
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
          const peer = tab.dataset.peer || null;
          const displayName = peer ? `${peer}:${name}` : name;
          if (!(await Toast.confirm(`Kill session "${displayName}"?`))) return;
          const ok = await destroySession(name, peer);
          if (!ok) {
            Toast.error('Failed to kill session');
            return;
          }
          if (name === currentSession && peer === currentPeer) {
            enterNullState();
          }
          lastSessionsKey = ''; // Force refresh
          await refreshSessionList();
          return;
        }
        const tab = e.target.closest('.session-tab');
        if (tab) switchSession(tab.dataset.session, tab.dataset.peer || null);
      });

      // Rename session on double-click
      sessionTabsEl.addEventListener('dblclick', async (e) => {
        const tab = e.target.closest('.session-tab');
        if (!tab || e.target.closest('.session-tab-close')) return;
        const oldName = tab.dataset.session;
        const peer = tab.dataset.peer || null;
        const newName = await Toast.prompt('Rename session:', oldName);
        if (!newName || !newName.trim() || newName.trim() === oldName) return;
        const trimmed = newName.trim();
        const validationError = validateSessionName(trimmed);
        if (validationError) {
          Toast.error(validationError);
          return;
        }
        const ok = await renameSession(oldName, trimmed, peer);
        if (!ok) {
          Toast.error('Failed to rename session');
          return;
        }
        if (oldName === currentSession && peer === currentPeer) {
          currentSession = trimmed;
          const hash = peer ? `${peer}:${trimmed}` : trimmed;
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

        let target = null;
        if (e.key === 'ArrowRight') {
          target = tabs[(idx + 1) % tabs.length];
        } else if (e.key === 'ArrowLeft') {
          target = tabs[(idx - 1 + tabs.length) % tabs.length];
        } else if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          switchSession(tab.dataset.session, tab.dataset.peer || null);
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
        switchSession(target.dataset.session, target.dataset.peer || null);
      }, { passive: true });
    }

    if (newBtn) {
      newBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        showNewSessionMenu(newBtn);
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
  function showNewSessionMenu(anchorEl) {
    // Remove existing menu if any
    const existing = document.getElementById('new-session-menu');
    if (existing) { existing.remove(); return; }

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

    // Connected peers
    const connectedPeers = cachedPeers.filter(p => p.status === 'connected');
    if (connectedPeers.length > 0) {
      for (const p of connectedPeers) {
        const sep = document.createElement('div');
        sep.className = 'new-session-menu-separator';
        const dot = document.createElement('span');
        dot.className = 'peer-color-dot';
        dot.style.background = peerColorMap[p.name] || PEER_COLORS[0];
        sep.appendChild(dot);
        sep.appendChild(document.createTextNode(` ${p.name}`));
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
          const ok = await createSession(trimmed, null, p.name);
          if (!ok) { Toast.error('Failed to create remote session'); return; }
          lastSessionsKey = '';
          await refreshSessionList();
          switchSession(trimmed, p.name);
        });
        menu.appendChild(newItem);
      }
    }

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

    // Position menu above the button
    const rect = anchorEl.getBoundingClientRect();
    menu.style.position = 'fixed';
    menu.style.left = rect.left + 'px';
    menu.style.bottom = (window.innerHeight - rect.top + 4) + 'px';
    document.body.appendChild(menu);

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

  function getCurrentPeer() {
    return currentPeer;
  }

  return {
    init, connect, disconnect, sendInput, sendResize, focus, fitAndRefresh, getTerminal,
    getCurrentSession, getCurrentPeer, switchSession, refreshSessionList, initSessionBar,
    fetchSessions, fetchAllSessions, createSession, destroySession,
    enterSelectMode, exitSelectMode, isSelectMode,
    validateSessionName,
    getXtermTheme() { return XTERM_THEME; },
    getFontFamily() { return FONT_FAMILY; },
    selectRenderer,
  };
})();
