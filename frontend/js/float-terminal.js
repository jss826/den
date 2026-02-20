/* global Terminal, FitAddon, WebglAddon, CanvasAddon, Auth, DenSettings, DenTerminal, Toast */
// Den - Floating Terminal module
const FloatTerminal = (() => {
  // --- State ---
  let initialized = false;
  let visible = false;
  let minimized = false;
  let term = null;
  let fitAddon = null;
  let ws = null;
  let currentSession = 'float';
  let connectGeneration = 0;
  const textEncoder = new TextEncoder();

  // DOM refs (set on init)
  let panel = null;
  let body = null;
  let restoreBtn = null;
  let sessionSelect = null;
  let clientsSpan = null;

  // Drag state
  let dragState = null;
  // Resize state
  let resizeState = null;

  const STORAGE_KEY = 'den-float-pos';
  const MIN_W = 320;
  const MIN_H = 200;
  const HEADER_H = 32;

  // --- Init (called once at app startup, does NOT create xterm) ---
  function init() {
    panel = document.getElementById('float-terminal');
    body = panel.querySelector('.float-terminal-body');
    restoreBtn = document.getElementById('float-terminal-restore');
    sessionSelect = panel.querySelector('.float-session-select');
    clientsSpan = panel.querySelector('.float-session-clients');

    // Header buttons
    panel.querySelector('.float-close').addEventListener('click', hide);
    panel.querySelector('.float-minimize').addEventListener('click', minimize);
    panel.querySelector('.float-session-new').addEventListener('click', onNewSession);
    panel.querySelector('.float-session-kill').addEventListener('click', onKillSession);
    sessionSelect.addEventListener('change', () => switchSession(sessionSelect.value));
    restoreBtn.addEventListener('click', restore);

    // Drag (header bar)
    const header = panel.querySelector('.float-terminal-header');
    header.addEventListener('pointerdown', onDragStart);

    // Resize handles
    panel.querySelectorAll('.float-resize').forEach(h => {
      h.addEventListener('pointerdown', onResizeStart);
    });
  }

  // --- Lazy create xterm ---
  function ensureTerminal() {
    if (initialized) return;
    initialized = true;

    const scrollback = DenSettings.get('terminal_scrollback') ?? 1000;
    term = new Terminal({
      cursorBlink: true,
      fontSize: 15,
      fontFamily: '"Cascadia Code", "Fira Code", "Source Code Pro", "Menlo", monospace',
      scrollback,
      theme: {
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
      },
      allowProposedApi: true,
    });

    fitAddon = new FitAddon.FitAddon();
    term.loadAddon(fitAddon);

    // Renderer selection (same logic as DenTerminal)
    const isIOS = /iPad|iPhone|iPod/.test(navigator.userAgent)
      || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
    const isSafari = !isIOS && /^((?!chrome|android).)*safari/i.test(navigator.userAgent);
    if (!isIOS && !isSafari) {
      try {
        const webglAddon = new WebglAddon.WebglAddon();
        webglAddon.onContextLost(() => webglAddon.dispose());
        term.loadAddon(webglAddon);
      } catch (_e) {
        try { term.loadAddon(new CanvasAddon.CanvasAddon()); } catch (_e2) { /* DOM */ }
      }
    } else {
      try { term.loadAddon(new CanvasAddon.CanvasAddon()); } catch (_e) { /* DOM */ }
    }

    term.open(body);

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
  }

  // --- Fit ---
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

  function sendResize() {
    if (ws && ws.readyState === WebSocket.OPEN && term) {
      ws.send(JSON.stringify({ type: 'resize', cols: term.cols, rows: term.rows }));
    }
  }

  // --- WS connection ---
  let reconnectAttempts = 0;
  const MAX_RECONNECT = 3;
  let manualReconnectDisposable = null;

  function doConnect() {
    const generation = ++connectGeneration;
    reconnectAttempts = 0;
    if (manualReconnectDisposable) { manualReconnectDisposable.dispose(); manualReconnectDisposable = null; }
    const token = Auth.getToken();
    const cols = term.cols;
    const rows = term.rows;
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${proto}//${location.host}/api/ws?token=${encodeURIComponent(token)}&cols=${cols}&rows=${rows}&session=${encodeURIComponent(currentSession)}`;

    let retries = 0;

    const attemptConnect = () => {
      if (generation !== connectGeneration) return;
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
          try {
            const msg = JSON.parse(event.data);
            if (msg.type === 'session_ended') {
              sessionEnded = true;
              term.writeln('\r\n\x1b[33mSession ended.\x1b[0m');
              refreshSessionList();
              return;
            }
          } catch (_) { /* text data */ }
          term.write(event.data);
        } else if (event.data instanceof ArrayBuffer) {
          term.write(new Uint8Array(event.data));
        }
      };

      ws.onclose = () => {
        if (generation !== connectGeneration) return;
        if (sessionEnded) return;
        // Only reconnect if still visible
        if (!visible) return;
        startReconnect(generation);
      };

      ws.onerror = () => {};

      setTimeout(() => {
        if (generation !== connectGeneration) return;
        if (ws && ws.readyState === WebSocket.CONNECTING && retries < 3) {
          retries++;
          attemptConnect();
        }
      }, 3000);
    };

    setTimeout(attemptConnect, 200);
  }

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

  function disconnect() {
    connectGeneration++;
    if (manualReconnectDisposable) { manualReconnectDisposable.dispose(); manualReconnectDisposable = null; }
    if (ws) {
      ws.onopen = ws.onclose = ws.onerror = ws.onmessage = null;
      ws.close();
      ws = null;
    }
  }

  // --- Show / Hide / Toggle ---
  function show() {
    if (visible) { term?.focus(); return; }
    ensureTerminal();
    visible = true;
    minimized = false;

    // Restore position/size from sessionStorage
    restorePosition();

    panel.hidden = false;
    restoreBtn.hidden = true;

    // Connect WS
    doConnect();

    requestAnimationFrame(() => {
      fitAndRefresh();
      term.focus();
    });

    // Extra fits for late layout
    setTimeout(() => fitAndRefresh(), 100);
    setTimeout(() => fitAndRefresh(), 500);

    refreshSessionList();
  }

  function hide() {
    if (!visible) return;
    visible = false;
    minimized = false;
    panel.hidden = true;
    restoreBtn.hidden = true;
    disconnect();
  }

  function toggle() {
    if (visible && !minimized) {
      hide();
    } else if (minimized) {
      restore();
    } else {
      show();
    }
  }

  function minimize() {
    if (!visible) return;
    minimized = true;
    panel.hidden = true;
    restoreBtn.hidden = false;
    // Keep WS alive while minimized — disconnect would lose context
  }

  function restore() {
    if (!minimized) { show(); return; }
    minimized = false;
    panel.hidden = false;
    restoreBtn.hidden = true;
    requestAnimationFrame(() => {
      fitAndRefresh();
      term.focus();
    });
  }

  // --- Session management ---
  function switchSession(name) {
    if (name === currentSession) return;
    currentSession = name;
    if (term) term.clear();
    if (visible && !minimized) doConnect();
  }

  async function onNewSession() {
    const name = await Toast.prompt('Session name:');
    if (!name || !name.trim()) return;
    const trimmed = name.trim();
    if (!/^[a-zA-Z0-9-]+$/.test(trimmed)) {
      Toast.error('Session name must be alphanumeric + hyphens only');
      return;
    }
    if (trimmed.length > 64) {
      Toast.error('Session name too long (max 64 characters)');
      return;
    }
    const ok = await DenTerminal.createSession(trimmed);
    if (!ok) { Toast.error('Failed to create session'); return; }
    await DenTerminal.refreshSessionList();
    switchSession(trimmed);
  }

  async function onKillSession() {
    if (!(await Toast.confirm(`Kill session "${currentSession}"?`))) return;
    await DenTerminal.destroySession(currentSession);
    currentSession = 'float';
    if (term) term.clear();
    if (visible && !minimized) doConnect();
    await DenTerminal.refreshSessionList();
  }

  async function refreshSessionList() {
    if (!sessionSelect) return;
    const sessions = await DenTerminal.fetchSessions();

    sessionSelect.innerHTML = '';
    if (sessions.length === 0) {
      const opt = document.createElement('option');
      opt.value = 'float';
      opt.textContent = 'float';
      sessionSelect.appendChild(opt);
    } else {
      for (const s of sessions) {
        const opt = document.createElement('option');
        opt.value = s.name;
        const status = s.alive ? '' : ' (dead)';
        opt.textContent = `${s.name}${status}`;
        if (s.name === currentSession) opt.selected = true;
        sessionSelect.appendChild(opt);
      }
    }

    // Update client count
    if (clientsSpan) {
      const current = sessions.find(s => s.name === currentSession);
      if (current) {
        clientsSpan.textContent = `${current.client_count} client${current.client_count !== 1 ? 's' : ''}`;
      } else {
        clientsSpan.textContent = '';
      }
    }
  }

  // --- Position / Size persistence ---
  function savePosition() {
    const r = panel.getBoundingClientRect();
    const data = { left: r.left, top: r.top, width: r.width, height: r.height };
    try { sessionStorage.setItem(STORAGE_KEY, JSON.stringify(data)); } catch (_) { /* ignore */ }
  }

  function restorePosition() {
    let data = null;
    try {
      const raw = sessionStorage.getItem(STORAGE_KEY);
      if (raw) data = JSON.parse(raw);
    } catch (_) { /* ignore */ }

    if (data && data.width >= MIN_W && data.height >= MIN_H) {
      // Clamp to viewport
      const vw = window.innerWidth;
      const vh = window.innerHeight;
      const w = Math.min(data.width, vw - 20);
      const h = Math.min(data.height, vh - 20);
      const left = Math.max(0, Math.min(data.left, vw - 100));
      const top = Math.max(0, Math.min(data.top, vh - HEADER_H));
      panel.style.left = left + 'px';
      panel.style.top = top + 'px';
      panel.style.width = w + 'px';
      panel.style.height = h + 'px';
      // Clear centering transform
      panel.style.transform = 'none';
    } else {
      // Default: centered, 60% of viewport
      const w = Math.max(MIN_W, Math.min(800, window.innerWidth * 0.6));
      const h = Math.max(MIN_H, Math.min(500, window.innerHeight * 0.6));
      panel.style.width = w + 'px';
      panel.style.height = h + 'px';
      panel.style.left = ((window.innerWidth - w) / 2) + 'px';
      panel.style.top = ((window.innerHeight - h) / 2) + 'px';
      panel.style.transform = 'none';
    }
  }

  // --- Drag ---
  function onDragStart(e) {
    // Don't drag on buttons/select
    if (e.target.closest('button') || e.target.closest('select')) return;
    e.preventDefault();
    const rect = panel.getBoundingClientRect();
    dragState = {
      startX: e.clientX,
      startY: e.clientY,
      origLeft: rect.left,
      origTop: rect.top,
    };
    panel.querySelector('.float-terminal-header').setPointerCapture(e.pointerId);
    document.addEventListener('pointermove', onDragMove);
    document.addEventListener('pointerup', onDragEnd);
  }

  function onDragMove(e) {
    if (!dragState) return;
    const dx = e.clientX - dragState.startX;
    const dy = e.clientY - dragState.startY;
    let newLeft = dragState.origLeft + dx;
    let newTop = dragState.origTop + dy;

    // Clamp: keep at least HEADER_H pixels visible vertically, 100px horizontally
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const w = panel.offsetWidth;
    newLeft = Math.max(-w + 100, Math.min(newLeft, vw - 100));
    newTop = Math.max(0, Math.min(newTop, vh - HEADER_H));

    panel.style.left = newLeft + 'px';
    panel.style.top = newTop + 'px';
  }

  function onDragEnd() {
    dragState = null;
    document.removeEventListener('pointermove', onDragMove);
    document.removeEventListener('pointerup', onDragEnd);
    savePosition();
  }

  // --- Resize ---
  function onResizeStart(e) {
    e.preventDefault();
    e.stopPropagation();
    const dir = e.currentTarget.dataset.dir;
    const rect = panel.getBoundingClientRect();
    resizeState = {
      dir,
      startX: e.clientX,
      startY: e.clientY,
      origLeft: rect.left,
      origTop: rect.top,
      origW: rect.width,
      origH: rect.height,
    };
    e.currentTarget.setPointerCapture(e.pointerId);
    document.addEventListener('pointermove', onResizeMove);
    document.addEventListener('pointerup', onResizeEnd);
  }

  function onResizeMove(e) {
    if (!resizeState) return;
    const { dir, startX, startY, origLeft, origTop, origW, origH } = resizeState;
    const dx = e.clientX - startX;
    const dy = e.clientY - startY;

    let newLeft = origLeft;
    let newTop = origTop;
    let newW = origW;
    let newH = origH;

    if (dir.includes('e')) newW = origW + dx;
    if (dir.includes('w')) { newW = origW - dx; newLeft = origLeft + dx; }
    if (dir.includes('s')) newH = origH + dy;
    if (dir.includes('n')) { newH = origH - dy; newTop = origTop + dy; }

    // Enforce minimums
    if (newW < MIN_W) {
      if (dir.includes('w')) newLeft = origLeft + origW - MIN_W;
      newW = MIN_W;
    }
    if (newH < MIN_H) {
      if (dir.includes('n')) newTop = origTop + origH - MIN_H;
      newH = MIN_H;
    }

    panel.style.left = newLeft + 'px';
    panel.style.top = newTop + 'px';
    panel.style.width = newW + 'px';
    panel.style.height = newH + 'px';

    fitAndRefresh();
  }

  function onResizeEnd() {
    resizeState = null;
    document.removeEventListener('pointermove', onResizeMove);
    document.removeEventListener('pointerup', onResizeEnd);
    savePosition();
    fitAndRefresh();
  }

  // --- Theme / Settings sync ---
  function applySettings() {
    if (!term) return;
    const scrollback = DenSettings.get('terminal_scrollback') ?? 1000;
    term.options.scrollback = Math.max(100, Math.min(50000, scrollback));
  }

  function getTerminal() {
    return term;
  }

  function isVisible() {
    return visible && !minimized;
  }

  return {
    init,
    show, hide, toggle,
    minimize, restore,
    refreshSessionList,
    applySettings,
    getTerminal,
    isVisible,
  };
})();
