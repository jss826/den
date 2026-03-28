/* global Terminal, FitAddon, DenSettings, DenTerminal, TerminalAdapter */
// Den - Floating Terminal module
const FloatTerminal = (() => {
  // --- State ---
  let initDone = false;
  // initialized は _ensurePromise で管理（Promise ガードパターン）
  let visible = false;
  let minimized = false;
  let term = null;
  let fitAddon = null;
  let ws = null;
  let currentSession = null;
  let currentRemote = null; // null for local, connectionId for remote Den (direct or relay)
  let connectGeneration = 0;
  const textEncoder = new TextEncoder();

  function encodeSessionTarget(name, remote) {
    return JSON.stringify({ name, remote: remote || null });
  }

  function decodeSessionTarget(value) {
    try {
      const parsed = JSON.parse(value);
      if (parsed && typeof parsed.name === 'string') {
        // backward compat: renamed from 'peer' to 'remote'
        const remote = parsed.remote || parsed.peer || null;
        return { name: parsed.name, remote };
      }
    } catch (_) { /* ignore */ }
    return { name: value, remote: null };
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

  // Mouse sequence filters — strip SGR/URXVT/X10 mouse reports before sending to PTY
  // eslint-disable-next-line no-control-regex
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

  // WS keepalive ping
  let pingTimer = null;
  const WS_PING_INTERVAL_MS = 30000;
  const WS_PING_MSG = JSON.stringify({ type: 'ping' });
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
  const RECONNECT_COUNTDOWN_S = 1;
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
  let _ensurePromise = null;
  function ensureTerminal() {
    if (!_ensurePromise) _ensurePromise = _doEnsureTerminal();
    return _ensurePromise;
  }
  async function _doEnsureTerminal() {

    const { TerminalClass, FitAddonClass, needsWebgl } = await TerminalAdapter.ready();
    const scrollback = DenSettings.get('terminal_scrollback') ?? 1000;
    const fontSize = DenSettings.get('font_size') ?? 15;
    term = new TerminalClass({
      cursorBlink: true,
      fontSize,
      fontFamily: DenTerminal.getFontFamily(),
      scrollback,
      theme: DenTerminal.getXtermTheme(DenSettings.getPaneTheme('terminal-pane')),
    });

    fitAddon = new FitAddonClass();
    term.loadAddon(fitAddon);

    if (needsWebgl) DenTerminal.selectRenderer(term);

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
  let fitRetryCount = 0;
  let resizeObserver = null;
  let pendingFitOptions = { force: false, refresh: false };
  let lastFitContainerWidth = 0;
  let lastFitContainerHeight = 0;

  function flushFit({ force = false, refresh = false } = {}) {
    fitRafId = null;
    if (!term || !fitAddon || !visible) { fitRetryCount = 0; return; }
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

  function fitAndRefresh() {
    scheduleFit({ force: true, refresh: true });
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

  function cancelPendingFit() {
    if (fitRafId != null) {
      cancelAnimationFrame(fitRafId);
      fitRafId = null;
    }
    pendingFitOptions = { force: false, refresh: false };
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

  // --- WS connection ---
  let reconnectAttempts = 0;
  const MAX_RECONNECT = 3;
  let manualReconnectDisposable = null;
  let connectDelayTimer = null;

  function doConnect(delay = CONNECT_DELAY_MS) {
    if (!term || !currentSession) return;
    const generation = ++connectGeneration;
    reconnectAttempts = 0;
    lastSentCols = 0;
    lastSentRows = 0;
    if (connectDelayTimer) { clearTimeout(connectDelayTimer); connectDelayTimer = null; }
    if (manualReconnectDisposable) { manualReconnectDisposable.dispose(); manualReconnectDisposable = null; }
    const cols = term.cols;
    const rows = term.rows;
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    // Route WS through remote/relay proxy if connected to another Den
    let wsPath;
    if (!currentRemote) {
      wsPath = '/api/ws';
    } else {
      const conns = typeof FilerRemote !== 'undefined' ? FilerRemote.getDenConnections() : {};
      const conn = conns[currentRemote];
      wsPath = conn?.type === 'relay'
        ? `/api/relay/${currentRemote}/ws`
        : `/api/remote/${currentRemote}/ws`;
    }
    const url = `${proto}//${location.host}${wsPath}?cols=${cols}&rows=${rows}&session=${encodeURIComponent(currentSession)}`;

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

      // rAF batching: buffer incoming WS binary data and flush once per frame.
      // null sentinel is used instead of 0 because requestAnimationFrame() is
      // specified to return a positive integer, but null unambiguously means
      // "no pending rAF".
      let writeBuf = [];
      let writeRaf = null;

      ws.onopen = () => {
        if (stallTimer) { clearTimeout(stallTimer); stallTimer = null; }
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
          } catch (_) { /* text data */ }
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
        if (stallTimer) { clearTimeout(stallTimer); stallTimer = null; }
        // Cancel any pending rAF to prevent stale data from a closed connection
        // being written to the terminal after reconnect.
        if (writeRaf !== null) { cancelAnimationFrame(writeRaf); writeRaf = null; }
        writeBuf = [];
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
    if (pingTimer) { clearInterval(pingTimer); pingTimer = null; }
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
  async function show() {
    if (visible) { term?.focus(); return; }
    await ensureTerminal();
    visible = true;
    minimized = false;

    // Restore position/size from localStorage
    restorePosition();

    panel.hidden = false;
    restoreBtn.hidden = true;

    // If no session selected, pick the first available one
    if (!currentSession) {
      const sessions = await DenTerminal.fetchAllSessions();
      const alive = sessions.filter(s => s.alive);
      const target = alive.length > 0 ? alive[0] : (sessions.length > 0 ? sessions[0] : null);
      if (target) {
        currentSession = target.name;
        currentRemote = target.remote || null;
      }
    }

    // Connect WS (doConnect handles null currentSession gracefully)
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
  function switchSession(value) {
    if (!value) return;
    const { name, remote } = decodeSessionTarget(value);
    if (name === currentSession && remote === currentRemote) return;
    currentSession = name;
    currentRemote = remote;
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
    if (!currentSession) return;
    const displayName = currentRemote ? `${currentRemote}:${currentSession}` : currentSession;
    if (!(await Toast.confirm(`Kill session "${displayName}"?`))) return;
    const ok = await DenTerminal.destroySession(currentSession, currentRemote);
    if (!ok) { Toast.error('Failed to kill session'); return; }
    currentSession = null;
    currentRemote = null;
    await DenTerminal.refreshSessionList();
  }

  /** Build a composite key for session identity */
  function sessionKey(s) {
    return s.remote ? `${s.remote} / ${s.name}` : s.name;
  }

  function isCurrentSession(s) {
    return s.name === currentSession && (s.remote || null) === currentRemote;
  }

  async function refreshSessionList(sessions) {
    if (!sessionSelect) return;
    const gen = ++refreshGeneration;
    if (!sessions) {
      try {
        sessions = (await DenTerminal.fetchAllSessions()) ?? [];
      } catch {
        console.error('[FloatTerminal] Failed to fetch sessions');
        return;
      }
    }
    if (gen !== refreshGeneration) return; // stale response

    sessionSelect.innerHTML = '';
    if (sessions.length === 0) {
      const opt = document.createElement('option');
      opt.value = '';
      opt.textContent = '(no sessions)';
      opt.disabled = true;
      opt.selected = true;
      sessionSelect.appendChild(opt);
      if (currentSession !== null) {
        currentSession = null;
        currentRemote = null;
        disconnect();
        if (term) term.clear();
      }
    } else if (!currentSession) {
      // Recovery: sessions appeared while disconnected — auto-connect to first alive
      const alive = sessions.filter(s => s.alive);
      const target = alive.length > 0 ? alive[0] : sessions[0];
      currentSession = target.name;
      currentRemote = target.remote || null;
      if (term) term.clear();
      if (visible && !minimized) doConnect(0);
    }

    // If current session was renamed, follow DenTerminal's active session
    if (currentSession && sessions.length > 0 && !sessions.find(s => isCurrentSession(s))) {
      const mainSession = DenTerminal.getCurrentSession();
      const mainRemote = DenTerminal.getCurrentRemote();
      if (mainSession && sessions.find(s => s.name === mainSession && (s.remote || null) === mainRemote)) {
        currentSession = mainSession;
        currentRemote = mainRemote;
      }
    }

    if (sessions.length > 0) {
      for (const s of sessions) {
        const opt = document.createElement('option');
        opt.value = encodeSessionTarget(s.name, s.remote);
        const status = s.alive ? '' : ' (dead)';
        opt.textContent = `${sessionKey(s)}${status}`;
        if (isCurrentSession(s)) opt.selected = true;
        sessionSelect.appendChild(opt);
      }
    }

    // Update client count
    if (clientsSpan) {
      const current = sessions.find(s => isCurrentSession(s));
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
    term.options.theme = DenTerminal.getXtermTheme(DenSettings.getPaneTheme('terminal-pane'));
    fitAndRefresh();
  }

  function getTerminal() {
    return term;
  }

  function isVisible() {
    return visible && !minimized;
  }

  // Update xterm theme when Den theme changes
  document.addEventListener('den:theme-changed', () => {
    if (!term) return;
    term.options.theme = DenTerminal.getXtermTheme(DenSettings.getPaneTheme('terminal-pane'));
  });

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
