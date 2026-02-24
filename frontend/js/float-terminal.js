/* global Terminal, FitAddon, DenSettings, DenTerminal */
// Den - Floating Terminal module
const FloatTerminal = (() => {
  // --- State ---
  let initDone = false;
  let initialized = false;
  let visible = false;
  let minimized = false;
  let term = null;
  let fitAddon = null;
  let ws = null;
  let currentSession = 'float';
  let connectGeneration = 0;
  const textEncoder = new TextEncoder();

  // Mouse sequence filters — strip SGR/URXVT/X10 mouse reports before sending to PTY
  const MOUSE_SEQ_RE = /\x1b\[<?\d+;\d+;\d+[Mm]/g;
  function filterMouseSeqs(s) { return s.replace(MOUSE_SEQ_RE, ''); }
  function isX10Mouse(d) {
    return d.length >= 6 && d.charCodeAt(0) === 0x1b && d.charCodeAt(1) === 0x5b && d.charCodeAt(2) === 0x4d;
  }

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
  // Race condition guard for refreshSessionList
  let refreshGeneration = 0;

  const STORAGE_KEY = 'den-float-pos';
  const MIN_W = 320;
  const MIN_H = 200;
  const HEADER_H = 32;
  const SESSION_REFRESH_INTERVAL = 5000;
  const CONNECT_DELAY_MS = 200;
  const STALL_TIMEOUT_MS = 3000;
  const RECONNECT_COUNTDOWN_S = 3;
  const DRAG_VISIBLE_PX = 100;
  const VIEWPORT_MARGIN = 20;
  const DEFAULT_MAX_W = 800;
  const DEFAULT_MAX_H = 500;
  const DEFAULT_VIEWPORT_RATIO = 0.6;

  // Session refresh timer (independent from DenTerminal's polling)
  let sessionRefreshTimer = null;

  // --- Init (called once at app startup, does NOT create xterm) ---
  function init() {
    if (initDone) return;
    initDone = true;
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

    // Listen for session list updates from DenTerminal (event-driven, no circular dep)
    document.addEventListener('den:sessions-changed', (e) => {
      refreshSessionList(e.detail?.sessions);
    });

    // Pause/resume session polling when page visibility changes
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) {
        stopSessionRefresh();
      } else if (visible) {
        refreshSessionList();
        startSessionRefresh();
      }
    });

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
    const fontSize = DenSettings.get('font_size') ?? 15;
    term = new Terminal({
      cursorBlink: true,
      fontSize,
      fontFamily: DenTerminal.getFontFamily(),
      scrollback,
      theme: DenTerminal.getXtermTheme(),
    });

    fitAddon = new FitAddon.FitAddon();
    term.loadAddon(fitAddon);

    DenTerminal.selectRenderer(term);

    term.open(body);

    term.onData((data) => {
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

    // ResizeObserver: fit terminal when container size changes (replaces setTimeout polling)
    resizeObserver = new ResizeObserver(() => scheduleFit());
    resizeObserver.observe(body);
  }

  // --- Fit ---
  let fitRafId = null;
  let resizeObserver = null;

  function fitAndRefresh() {
    if (!term || !fitAddon || !visible) return;
    fitRafId = null;
    const container = term.element?.parentElement;
    if (container && container.clientWidth === 0) return;
    fitAddon.fit();
    term.refresh(0, term.rows - 1);
    sendResize();
  }

  function scheduleFit() {
    if (fitRafId != null) return;
    fitRafId = requestAnimationFrame(fitAndRefresh);
  }

  function cancelPendingFit() {
    if (fitRafId != null) {
      cancelAnimationFrame(fitRafId);
      fitRafId = null;
    }
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
  let connectDelayTimer = null;

  function doConnect(delay = CONNECT_DELAY_MS) {
    if (!term) return;
    const generation = ++connectGeneration;
    reconnectAttempts = 0;
    if (connectDelayTimer) { clearTimeout(connectDelayTimer); connectDelayTimer = null; }
    if (manualReconnectDisposable) { manualReconnectDisposable.dispose(); manualReconnectDisposable = null; }
    const cols = term.cols;
    const rows = term.rows;
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    // 認証は Cookie（HttpOnly）で自動送信 — URL にトークンを含めない
    const url = `${proto}//${location.host}/api/ws?cols=${cols}&rows=${rows}&session=${encodeURIComponent(currentSession)}`;

    let stallTimer = null;

    const attemptConnect = () => {
      if (generation !== connectGeneration) return;
      if (stallTimer) { clearTimeout(stallTimer); stallTimer = null; }
      if (ws) {
        ws.onopen = ws.onclose = ws.onerror = ws.onmessage = null;
        ws.close();
        ws = null;
      }

      ws = new WebSocket(url);
      ws.binaryType = 'arraybuffer';
      let sessionEnded = false;

      ws.onopen = () => {
        if (stallTimer) { clearTimeout(stallTimer); stallTimer = null; }
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
        if (stallTimer) { clearTimeout(stallTimer); stallTimer = null; }
        if (generation !== connectGeneration) return;
        if (sessionEnded) return;
        // Only reconnect if still visible
        if (!visible) return;
        startReconnect(generation);
      };

      ws.onerror = (event) => {
        console.error('[FloatTerminal] WebSocket error', event);
      };

      // Safari WS stall detection: if stuck in CONNECTING after 3s,
      // close stalled WS and delegate to startReconnect (unified retry budget)
      stallTimer = setTimeout(() => {
        stallTimer = null;
        if (generation !== connectGeneration) return;
        if (ws && ws.readyState === WebSocket.CONNECTING) {
          ws.onopen = ws.onclose = ws.onerror = ws.onmessage = null;
          ws.close();
          ws = null;
          startReconnect(generation);
        }
      }, STALL_TIMEOUT_MS);
    };

    if (delay > 0) {
      connectDelayTimer = setTimeout(() => {
        connectDelayTimer = null;
        attemptConnect();
      }, delay);
    } else {
      attemptConnect();
    }
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
          doConnect(0);
        }
      });
      return;
    }

    let countdown = RECONNECT_COUNTDOWN_S;
    term.write(`\r\n\x1b[31mDisconnected.\x1b[0m Reconnecting in \x1b[33m${countdown}\x1b[0m...`);
    const timer = setInterval(() => {
      if (generation !== connectGeneration) { clearInterval(timer); return; }
      countdown--;
      if (countdown > 0) {
        term.write(`\x1b[33m${countdown}\x1b[0m...`);
      } else {
        clearInterval(timer);
        term.writeln('');
        if (generation === connectGeneration) doConnect(0);
      }
    }, 1000);
  }

  function disconnect() {
    connectGeneration++;
    if (connectDelayTimer) { clearTimeout(connectDelayTimer); connectDelayTimer = null; }
    if (manualReconnectDisposable) { manualReconnectDisposable.dispose(); manualReconnectDisposable = null; }
    if (ws) {
      ws.onopen = ws.onclose = ws.onerror = ws.onmessage = null;
      ws.close();
      ws = null;
    }
  }

  // --- Session refresh timer ---
  function startSessionRefresh() {
    if (sessionRefreshTimer) return;
    sessionRefreshTimer = setInterval(() => refreshSessionList(), SESSION_REFRESH_INTERVAL);
  }

  function stopSessionRefresh() {
    if (sessionRefreshTimer) {
      clearInterval(sessionRefreshTimer);
      sessionRefreshTimer = null;
    }
  }

  // --- Show / Hide / Toggle ---
  function show() {
    if (visible) { term?.focus(); return; }
    ensureTerminal();
    visible = true;
    minimized = false;

    // Restore position/size from localStorage
    restorePosition();

    panel.hidden = false;
    restoreBtn.hidden = true;

    // Connect WS
    doConnect();

    // Single RAF for initial fit + focus; ResizeObserver handles late layout changes
    requestAnimationFrame(() => {
      fitAndRefresh();
      term.focus();
    });

    refreshSessionList();
    startSessionRefresh();
  }

  function hide() {
    if (!visible) return;
    visible = false;
    minimized = false;
    panel.hidden = true;
    restoreBtn.hidden = true;
    // Clean up any in-progress drag/resize operations
    if (dragState) onDragEnd();
    if (resizeState) onResizeEnd();
    cancelPendingFit();
    stopSessionRefresh();
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
    if (visible && !minimized) doConnect(0);
  }

  async function onNewSession() {
    const name = await Toast.prompt('Session name:');
    if (!name || !name.trim()) return;
    const trimmed = name.trim();
    const validationError = DenTerminal.validateSessionName(trimmed);
    if (validationError) {
      Toast.error(validationError);
      return;
    }
    const ok = await DenTerminal.createSession(trimmed);
    if (!ok) { Toast.error('Failed to create session'); return; }
    await DenTerminal.refreshSessionList();
    switchSession(trimmed);
  }

  async function onKillSession() {
    if (!(await Toast.confirm(`Kill session "${currentSession}"?`))) return;
    const ok = await DenTerminal.destroySession(currentSession);
    if (!ok) { Toast.error('Failed to kill session'); return; }
    await DenTerminal.refreshSessionList();
    const sessions = (await DenTerminal.fetchSessions()) ?? [];
    const alive = sessions.filter(s => s.status !== 'dead');
    currentSession = alive.length > 0 ? alive[0].name : 'float';
    if (term) term.clear();
    if (visible && !minimized) doConnect(0);
  }

  async function refreshSessionList(sessions) {
    if (!sessionSelect) return;
    const gen = ++refreshGeneration;
    if (!sessions) {
      try {
        sessions = (await DenTerminal.fetchSessions()) ?? [];
      } catch {
        console.error('[FloatTerminal] Failed to fetch sessions');
        return;
      }
    }
    if (gen !== refreshGeneration) return; // stale response

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
    try { localStorage.setItem(STORAGE_KEY, JSON.stringify(data)); } catch (_) { /* ignore */ }
  }

  function restorePosition() {
    let data = null;
    try {
      const raw = localStorage.getItem(STORAGE_KEY);
      if (raw) data = JSON.parse(raw);
    } catch (_) { /* ignore */ }

    const isValid = data
      && Number.isFinite(data.left) && Number.isFinite(data.top)
      && Number.isFinite(data.width) && Number.isFinite(data.height)
      && data.width >= MIN_W && data.height >= MIN_H;
    if (isValid) {
      // Clamp to viewport (ensure minimums even on very small screens)
      const vw = Math.max(MIN_W, window.innerWidth);
      const vh = Math.max(MIN_H, window.innerHeight);
      const w = Math.max(MIN_W, Math.min(data.width, vw - VIEWPORT_MARGIN));
      const h = Math.max(MIN_H, Math.min(data.height, vh - VIEWPORT_MARGIN));
      const left = Math.max(0, Math.min(data.left, vw - DRAG_VISIBLE_PX));
      const top = Math.max(0, Math.min(data.top, vh - HEADER_H));
      panel.style.left = left + 'px';
      panel.style.top = top + 'px';
      panel.style.width = w + 'px';
      panel.style.height = h + 'px';
      // Clear centering transform
      panel.style.transform = 'none';
    } else {
      // Default: centered, 60% of viewport
      const w = Math.max(MIN_W, Math.min(DEFAULT_MAX_W, window.innerWidth * DEFAULT_VIEWPORT_RATIO));
      const h = Math.max(MIN_H, Math.min(DEFAULT_MAX_H, window.innerHeight * DEFAULT_VIEWPORT_RATIO));
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
    document.addEventListener('pointercancel', onDragEnd);
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
    newLeft = Math.max(-w + DRAG_VISIBLE_PX, Math.min(newLeft, vw - DRAG_VISIBLE_PX));
    newTop = Math.max(0, Math.min(newTop, vh - HEADER_H));

    panel.style.left = newLeft + 'px';
    panel.style.top = newTop + 'px';
  }

  function onDragEnd() {
    dragState = null;
    document.removeEventListener('pointermove', onDragMove);
    document.removeEventListener('pointerup', onDragEnd);
    document.removeEventListener('pointercancel', onDragEnd);
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
    document.addEventListener('pointercancel', onResizeEnd);
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

    // Throttle fit to once per animation frame
    scheduleFit();
  }

  function onResizeEnd() {
    resizeState = null;
    cancelPendingFit();
    document.removeEventListener('pointermove', onResizeMove);
    document.removeEventListener('pointerup', onResizeEnd);
    document.removeEventListener('pointercancel', onResizeEnd);
    savePosition();
    fitAndRefresh();
  }

  // --- Theme / Settings sync ---
  function applySettings() {
    if (!term) return;
    const scrollback = DenSettings.get('terminal_scrollback') ?? 1000;
    const fontSize = DenSettings.get('font_size') ?? 15;
    term.options.scrollback = Math.max(100, Math.min(50000, scrollback));
    term.options.fontSize = Math.max(8, Math.min(32, fontSize));
    term.options.theme = DenTerminal.getXtermTheme();
    fitAndRefresh();
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
