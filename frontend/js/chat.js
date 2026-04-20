/**
 * DenChat — Chat tab using Channel API (Chat v2).
 *
 * Session-aware: each chat session has its own Claude Code process,
 * message queue, and WebSocket connection.
 *
 * Communicates through /api/channel/* endpoints:
 * - POST /api/channel/sessions    — create session
 * - GET  /api/channel/sessions    — list sessions
 * - DELETE /api/channel/sessions/{id} — stop session
 * - POST /api/channel/message     — send user message
 * - POST /api/channel/verdict     — approve/deny permission request
 * - WS   /api/channel/ws?session= — receive replies + permission requests
 */
/* global DenMarkdown, DenFiler */
const DenChat = (() => {
  // ── DOM refs ──
  let messagesEl = null;
  let inputEl = null;
  let sendBtn = null;
  let clearBtn = null;
  let stopBtn = null;
  let sessionListEl = null;
  let newSessionBtn = null;
  let newSessionModal = null;
  let permissionModeSelect = null;
  let cwdDisplayText = null;
  let cwdBrowseBtn = null;
  let cwdUseFilerBtn = null;

  // ── Working directory picker state ──
  // selectedCwd is the value that will be sent on session creation.
  // Empty string means "home directory (default)".
  let selectedCwd = '';
  let cwdPickerModal = null;
  let cwdPickerPath = '';
  let cwdPickerAbort = null;
  let cwdPickerDrivesLoaded = false;

  // ── State ──
  let ws = null;
  let composing = false;
  let currentAssistantBubble = null;
  let renderPending = false;
  let pendingText = '';
  let activeSessionId = null;
  let sessions = []; // cached session list
  let pollTimer = null;
  let sendingInFlight = false;
  let wsReconnectDelay = 1000;
  const WS_RECONNECT_MAX = 30000;
  let visibilityListenerRegistered = false;

  // ── Remote connection state ──
  let chatRemoteId = null;
  let chatRemoteType = null; // 'direct' | 'relay'

  // ── Notification permission ──
  let notificationsEnabled = false;

  // ── API helpers ──

  function getApiBase() {
    if (!chatRemoteId) return '/api';
    if (chatRemoteType === 'relay') return `/api/relay/${chatRemoteId}`;
    return `/api/remote/${chatRemoteId}`;
  }

  function getChannelWsUrl(sessionId) {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const base = `${proto}//${location.host}`;
    if (chatRemoteId) {
      if (chatRemoteType === 'relay') {
        return `${base}/api/relay/${chatRemoteId}/chat-ws?session=${encodeURIComponent(sessionId)}`;
      }
      return `${base}/api/remote/${encodeURIComponent(chatRemoteId)}/chat-ws?session=${encodeURIComponent(sessionId)}`;
    }
    return `${base}/api/channel/ws?session=${encodeURIComponent(sessionId)}`;
  }

  // ── Init ──

  function init() {
    messagesEl = document.getElementById('chat-messages');
    inputEl = document.getElementById('chat-input');
    sendBtn = document.getElementById('chat-send');
    clearBtn = document.getElementById('chat-clear-btn');
    stopBtn = document.getElementById('chat-stop-btn');
    sessionListEl = document.getElementById('chat-session-list');
    newSessionBtn = document.getElementById('chat-new-session-btn');
    newSessionModal = document.getElementById('chat-new-session-modal');
    permissionModeSelect = document.getElementById('chat-permission-mode');
    cwdDisplayText = document.getElementById('chat-cwd-display-text');
    cwdBrowseBtn = document.getElementById('chat-cwd-browse');
    cwdUseFilerBtn = document.getElementById('chat-cwd-use-filer');
    cwdPickerModal = document.getElementById('chat-cwd-picker-modal');

    sendBtn.addEventListener('click', handleSend);
    inputEl.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !e.shiftKey && !composing) {
        e.preventDefault();
        handleSend();
      }
    });
    inputEl.addEventListener('compositionstart', () => { composing = true; });
    inputEl.addEventListener('compositionend', () => { composing = false; });

    if (clearBtn) clearBtn.addEventListener('click', handleClear);
    if (stopBtn) stopBtn.addEventListener('click', handleStop);

    // New session modal
    if (newSessionBtn) {
      newSessionBtn.addEventListener('click', () => {
        selectedCwd = '';
        updateCwdDisplay();
        // Reset the advanced-settings accordion so the previous session's
        // auto/escalate lists don't silently carry over into the next one.
        const autoEl = document.getElementById('chat-auto-tools');
        const escalateEl = document.getElementById('chat-escalate-tools');
        if (autoEl) autoEl.value = '';
        if (escalateEl) escalateEl.value = '';
        newSessionModal.hidden = false;
      });
    }
    const cancelBtn = document.getElementById('chat-session-cancel');
    const createBtn = document.getElementById('chat-session-create');
    if (cancelBtn) cancelBtn.addEventListener('click', () => { newSessionModal.hidden = true; });
    if (createBtn) createBtn.addEventListener('click', handleCreateSession);

    if (cwdBrowseBtn) {
      cwdBrowseBtn.addEventListener('click', openCwdPicker);
    }

    if (cwdUseFilerBtn) {
      cwdUseFilerBtn.addEventListener('click', () => {
        // Pull the directory currently open in the Files tab, if the filer
        // module exposes it. Falls back gracefully when Files hasn't resolved
        // a real path yet (e.g. still showing "~").
        const dir = typeof DenFiler !== 'undefined' && DenFiler.getCurrentDir
          ? DenFiler.getCurrentDir()
          : null;
        if (!dir || dir === '~' || dir === '/') {
          appendSystemMessage('Open a folder in the Files tab first to use it here.');
          return;
        }
        selectedCwd = dir;
        updateCwdDisplay();
      });
    }

    // Initialize directory picker (registers listeners once)
    initCwdPicker();

    // Remote connection awareness
    document.addEventListener('den:remote-changed', (e) => {
      const { connectionId, connection: conn } = e.detail || {};
      if (connectionId) {
        setRemote(connectionId, conn?.type || 'direct');
      } else {
        setRemote(null);
      }
    });

    // Request notification permission
    requestNotificationPermission();

    // Start polling sessions
    fetchSessions();
    startSessionPoll();
  }

  // ── Session management ──

  async function fetchSessions() {
    try {
      const resp = await fetch(`${getApiBase()}/channel/sessions`, {
        credentials: 'same-origin',
      });
      if (resp.ok) {
        sessions = await resp.json();
        renderSessionList();
      }
    } catch { /* network error, retry on next poll */ }
  }

  function startSessionPoll() {
    if (pollTimer) clearInterval(pollTimer);
    pollTimer = setInterval(fetchSessions, 5000);
    // Register visibilitychange once at module level
    if (!visibilityListenerRegistered) {
      visibilityListenerRegistered = true;
      document.addEventListener('visibilitychange', () => {
        if (document.hidden) {
          clearInterval(pollTimer);
          pollTimer = null;
        } else {
          fetchSessions();
          startSessionPoll();
        }
      });
    }
  }

  /**
   * Return the last path segment (trailing directory name) of an absolute
   * path. Handles both POSIX (`/`) and Windows (`\`) separators, and strips a
   * single trailing separator. Falls back to the full path if no segment
   * can be extracted (e.g. a bare drive letter like `C:\`).
   */
  function lastPathSegment(path) {
    if (!path) return '';
    const cleaned = path.replace(/[\\/]+$/, '');
    const match = cleaned.match(/[^\\/]+$/);
    return match ? match[0] : path;
  }

  /**
   * Split the auto/escalate textarea contents into a clean tool-name array.
   * Accepts commas, newlines, and whitespace as separators and drops empties
   * so a trailing comma or stray newline doesn't produce a bogus entry.
   */
  function parseToolList(el) {
    if (!el || typeof el.value !== 'string') return [];
    return el.value
      .split(/[\s,]+/)
      .map(t => t.trim())
      .filter(Boolean);
  }

  function renderSessionList() {
    if (!sessionListEl) return;
    sessionListEl.innerHTML = '';

    if (sessions.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'chat-session-empty';
      empty.textContent = 'No active sessions';
      sessionListEl.appendChild(empty);
      return;
    }

    for (const s of sessions) {
      const item = document.createElement('div');
      item.className = 'chat-session-item';
      if (s.id === activeSessionId) item.classList.add('active');
      if (!s.alive) item.classList.add('dead');

      const label = document.createElement('span');
      label.className = 'chat-session-label';
      label.textContent = s.id.slice(0, 8);
      const cwdFull = s.cwd || '';
      label.title = `${s.permission_mode}${cwdFull ? ` — ${cwdFull}` : ''} — ${s.id}`;

      const badge = document.createElement('span');
      badge.className = 'chat-session-badge';
      badge.textContent = s.alive ? s.permission_mode : 'stopped';

      item.appendChild(label);
      if (cwdFull) {
        const cwdBadge = document.createElement('span');
        cwdBadge.className = 'chat-session-cwd';
        cwdBadge.textContent = lastPathSegment(cwdFull);
        cwdBadge.title = cwdFull;
        item.appendChild(cwdBadge);
      }
      item.appendChild(badge);
      if (s.alive) {
        item.addEventListener('click', () => switchSession(s.id));
      } else {
        item.style.cursor = 'default';
      }
      sessionListEl.appendChild(item);
    }

    // Update UI state based on active session
    const active = sessions.find(s => s.id === activeSessionId);
    updateInputState(!!active);
    if (stopBtn) stopBtn.hidden = !active;
  }

  async function handleCreateSession() {
    const mode = permissionModeSelect ? permissionModeSelect.value : 'default';
    const cwd = selectedCwd.trim();
    const autoTools = parseToolList(document.getElementById('chat-auto-tools'));
    const escalateTools = parseToolList(document.getElementById('chat-escalate-tools'));
    newSessionModal.hidden = true;

    const summaryBits = [mode];
    if (autoTools.length) summaryBits.push(`auto=${autoTools.join(',')}`);
    if (escalateTools.length) summaryBits.push(`escalate=${escalateTools.join(',')}`);
    appendSystemMessage(`Creating session (${summaryBits.join(' | ')})${cwd ? ` in ${cwd}` : ''}...`);

    const body = { permission_mode: mode };
    if (cwd) body.cwd = cwd;
    if (autoTools.length) body.auto_tools = autoTools;
    if (escalateTools.length) body.escalate_tools = escalateTools;

    try {
      const resp = await fetch(`${getApiBase()}/channel/sessions`, {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });

      if (!resp.ok) {
        const text = await resp.text();
        appendSystemMessage(`Failed to create session: ${text}`);
        return;
      }

      const session = await resp.json();
      appendSystemMessage(`Session created: ${session.id.slice(0, 8)} (${session.cwd})`);
      selectedCwd = '';
      updateCwdDisplay();
      await fetchSessions();
      switchSession(session.id);
    } catch (err) {
      appendSystemMessage(`Error: ${err.message}`);
    }
  }

  function updateCwdDisplay() {
    if (!cwdDisplayText) return;
    if (selectedCwd) {
      cwdDisplayText.textContent = selectedCwd;
      cwdDisplayText.classList.remove('is-placeholder');
      if (cwdBrowseBtn) cwdBrowseBtn.title = selectedCwd;
    } else {
      cwdDisplayText.textContent = 'Home directory (default)';
      cwdDisplayText.classList.add('is-placeholder');
      if (cwdBrowseBtn) cwdBrowseBtn.title = 'Click to browse';
    }
  }

  async function handleStop() {
    if (!activeSessionId) return;
    const id = activeSessionId;

    try {
      const resp = await fetch(`${getApiBase()}/channel/sessions/${encodeURIComponent(id)}`, {
        method: 'DELETE',
        credentials: 'same-origin',
      });
      if (!resp.ok) {
        appendSystemMessage(`Failed to stop session (${resp.status})`);
        return;
      }
      appendSystemMessage(`Session ${id.slice(0, 8)} stopped.`);
    } catch (err) {
      appendSystemMessage(`Error stopping session: ${err.message}`);
    }

    activeSessionId = null;
    disconnectWs();
    updateInputState(false);
    if (stopBtn) stopBtn.hidden = true;
    await fetchSessions();
  }

  function switchSession(id) {
    if (id === activeSessionId) return;
    activeSessionId = id;

    // Clear messages for new session view
    handleClear();

    // Reconnect WebSocket
    connectWs();

    // Re-render session list to update active state
    renderSessionList();

    // On mobile, collapse the sidebar after choosing a session
    closeMobileSidebar();
  }

  function closeMobileSidebar() {
    if (window.innerWidth > 768) return;
    const sidebar = document.getElementById('chat-sidebar');
    if (!sidebar || !sidebar.classList.contains('sidebar-expanded')) return;
    sidebar.classList.remove('sidebar-expanded');
    const layout = sidebar.closest('.chat-layout');
    const overlay = layout && layout.querySelector('.sidebar-overlay');
    if (overlay) overlay.classList.remove('visible');
  }

  function updateInputState(enabled) {
    if (inputEl) inputEl.disabled = !enabled;
    if (sendBtn) sendBtn.disabled = !enabled;
    if (inputEl && !enabled) {
      inputEl.placeholder = 'Create a session to start chatting...';
    } else if (inputEl) {
      inputEl.placeholder = 'Ask Claude...';
    }
  }

  // ── WebSocket ──

  function connectWs() {
    disconnectWs();
    if (!activeSessionId) return;

    const url = getChannelWsUrl(activeSessionId);
    ws = new WebSocket(url);

    ws.addEventListener('open', () => {
      wsReconnectDelay = 1000; // reset on successful connection
      updateConnectionStatus(true);
    });

    ws.addEventListener('message', (e) => {
      let data;
      try {
        data = JSON.parse(e.data);
      } catch {
        return;
      }
      handleWsEvent(data);
    });

    ws.addEventListener('close', () => {
      updateConnectionStatus(false);
      ws = null;
      // Reconnect with exponential backoff if session still active
      const delay = wsReconnectDelay;
      wsReconnectDelay = Math.min(wsReconnectDelay * 2, WS_RECONNECT_MAX);
      setTimeout(() => {
        if (!ws && activeSessionId) connectWs();
      }, delay);
    });

    ws.addEventListener('error', () => {
      // close event will fire after error
    });
  }

  function disconnectWs() {
    if (ws) {
      ws.close();
      ws = null;
    }
  }

  function handleWsEvent(event) {
    switch (event.type) {
    case 'reply':
      handleReply(event);
      break;
    case 'permission_request':
      handlePermissionRequest(event);
      break;
    }
  }

  // ── Sending messages ──

  async function handleSend() {
    if (!activeSessionId || sendingInFlight) return;
    const text = inputEl.value.trim();
    if (!text) return;

    inputEl.value = '';
    inputEl.style.height = 'auto';

    // Show user message
    appendUserMessage(text);

    // Reset assistant bubble for new response
    currentAssistantBubble = null;
    pendingText = '';

    // Send via Channel API with double-send guard
    sendingInFlight = true;
    try {
      const resp = await fetch(`${getApiBase()}/channel/message`, {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ session: activeSessionId, text }),
      });
      if (!resp.ok) {
        appendSystemMessage(`Failed to send message (${resp.status})`);
      }
    } catch (err) {
      appendSystemMessage(`Network error: ${err.message}`);
    } finally {
      sendingInFlight = false;
    }
  }

  // ── Handling replies ──

  function handleReply(event) {
    if (!currentAssistantBubble) {
      currentAssistantBubble = createAssistantBubble();
      pendingText = '';
    }
    pendingText += event.text;
    scheduleRender();
  }

  function scheduleRender() {
    if (renderPending) return;
    renderPending = true;
    requestAnimationFrame(() => {
      renderPending = false;
      if (currentAssistantBubble && pendingText) {
        currentAssistantBubble.innerHTML = DenMarkdown.sanitize(DenMarkdown.renderMarkdown(pendingText));
        scrollToBottom();
      }
    });
  }

  // ── Permission requests ──

  function handlePermissionRequest(event) {
    const card = document.createElement('div');
    card.className = 'chat-permission-dialog';
    card.dataset.requestId = event.request_id;

    const header = document.createElement('div');
    header.className = 'chat-permission-header';
    header.textContent = `Permission: ${event.tool_name}`;
    card.appendChild(header);

    if (event.description) {
      const desc = document.createElement('div');
      desc.className = 'chat-permission-desc';
      desc.textContent = event.description;
      card.appendChild(desc);
    }

    if (event.input_preview) {
      const preview = document.createElement('pre');
      preview.className = 'chat-permission-preview';
      preview.textContent = event.input_preview;
      card.appendChild(preview);
    }

    const actions = document.createElement('div');
    actions.className = 'chat-permission-actions';

    const allowBtn = document.createElement('button');
    allowBtn.className = 'chat-permission-allow';
    allowBtn.textContent = 'Allow';
    allowBtn.addEventListener('click', () => sendVerdict(event.request_id, 'allow', card));

    const denyBtn = document.createElement('button');
    denyBtn.className = 'chat-permission-deny';
    denyBtn.textContent = 'Deny';
    denyBtn.addEventListener('click', () => sendVerdict(event.request_id, 'deny', card));

    actions.appendChild(allowBtn);
    actions.appendChild(denyBtn);
    card.appendChild(actions);

    messagesEl.appendChild(card);
    scrollToBottom();

    // Browser notification
    if (notificationsEnabled && document.hidden) {
      try {
        new Notification('Den — Permission Request', {
          body: `${event.tool_name}: ${event.description || event.input_preview || ''}`.slice(0, 200),
          tag: 'den-permission-' + event.request_id,
        });
      } catch { /* notifications may fail */ }
    }
  }

  async function sendVerdict(requestId, behavior, card) {
    if (!activeSessionId) return;

    // Update UI immediately
    const actions = card.querySelector('.chat-permission-actions');
    if (actions) {
      actions.innerHTML = '';
      const status = document.createElement('span');
      status.className = behavior === 'allow' ? 'chat-permission-allowed' : 'chat-permission-denied';
      status.textContent = behavior === 'allow' ? 'Allowed' : 'Denied';
      actions.appendChild(status);
    }

    try {
      await fetch(`${getApiBase()}/channel/verdict`, {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          session: activeSessionId,
          request_id: requestId,
          behavior,
        }),
      });
    } catch (err) {
      appendSystemMessage(`Failed to send verdict: ${err.message}`);
    }
  }

  // ── DOM helpers ──

  function appendUserMessage(text) {
    const msg = document.createElement('div');
    msg.className = 'chat-msg chat-user';
    msg.textContent = text;
    messagesEl.appendChild(msg);
    scrollToBottom();
  }

  function createAssistantBubble() {
    const msg = document.createElement('div');
    msg.className = 'chat-msg chat-assistant';
    messagesEl.appendChild(msg);
    scrollToBottom();
    return msg;
  }

  function appendSystemMessage(text) {
    const msg = document.createElement('div');
    msg.className = 'chat-msg chat-system';
    msg.textContent = text;
    messagesEl.appendChild(msg);
    scrollToBottom();
  }

  function scrollToBottom() {
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }

  function updateConnectionStatus(connected) {
    const title = document.getElementById('chat-main-title');
    if (title) {
      if (!activeSessionId) {
        title.textContent = 'Chat';
      } else {
        title.textContent = connected ? `Chat — ${activeSessionId.slice(0, 8)}` : 'Chat (disconnected)';
      }
    }
  }

  // ── Clear ──

  function handleClear() {
    if (messagesEl) {
      messagesEl.innerHTML = '';
    }
    currentAssistantBubble = null;
    pendingText = '';
  }

  // ── Remote ──

  function setRemote(remoteId = null, type = null) {
    chatRemoteId = remoteId;
    chatRemoteType = type;
    // Clear stale state from previous connection
    activeSessionId = null;
    disconnectWs();
    handleClear();
    // Refetch sessions from new target
    fetchSessions();
  }

  // ── Notifications ──

  function requestNotificationPermission() {
    if (!('Notification' in window)) return;
    if (Notification.permission === 'granted') {
      notificationsEnabled = true;
    } else if (Notification.permission !== 'denied') {
      Notification.requestPermission().then((p) => {
        notificationsEnabled = p === 'granted';
      });
    }
  }

  // ── Prefill (from terminal selection via Ctrl+Shift+Enter) ──

  function prefillInput(text) {
    if (!inputEl) return;
    const maxRun = (text.match(/`+/g) || []).reduce((m, s) => Math.max(m, s.length), 2);
    const fence = '`'.repeat(maxRun + 1);
    const prefill = fence + '\n' + text + '\n' + fence + '\n';
    inputEl.value += prefill;
    inputEl.focus();
    inputEl.scrollTop = inputEl.scrollHeight;
  }

  // ── Directory picker modal ──

  // Filer API base mirrors getApiBase() but points at the filer subtree.
  // Uses the chat tab's remote state so picking runs against the same host
  // where the session will be created.
  function getFilerApiBase() {
    if (!chatRemoteId) return '/api/filer';
    if (chatRemoteType === 'relay') return `/api/relay/${chatRemoteId}/filer`;
    return `/api/remote/${chatRemoteId}/filer`;
  }

  function initCwdPicker() {
    if (!cwdPickerModal) return;
    const cancelBtn = document.getElementById('cwd-picker-cancel');
    const selectBtn = document.getElementById('cwd-picker-select');
    const clearBtn = document.getElementById('cwd-picker-clear');
    const upBtn = document.getElementById('cwd-picker-up');
    const listEl = document.getElementById('cwd-picker-list');
    if (!cancelBtn || !selectBtn || !clearBtn || !upBtn || !listEl) return;

    cancelBtn.addEventListener('click', closeCwdPicker);

    selectBtn.addEventListener('click', () => {
      if (cwdPickerPath) {
        selectedCwd = cwdPickerPath;
        updateCwdDisplay();
      }
      closeCwdPicker();
    });

    clearBtn.addEventListener('click', () => {
      selectedCwd = '';
      updateCwdDisplay();
      closeCwdPicker();
    });

    upBtn.addEventListener('click', () => {
      const parent = getParentDir(cwdPickerPath);
      if (parent && parent !== cwdPickerPath) loadCwdPickerDir(parent);
    });

    // Single click enters a folder; double click enters and picks immediately.
    listEl.addEventListener('click', (e) => {
      const item = e.target.closest('.cwd-picker-item');
      if (!item) return;
      loadCwdPickerDir(item.dataset.path);
    });
    listEl.addEventListener('dblclick', (e) => {
      const item = e.target.closest('.cwd-picker-item');
      if (!item) return;
      selectedCwd = item.dataset.path;
      updateCwdDisplay();
      closeCwdPicker();
    });
  }

  function openCwdPicker() {
    if (!cwdPickerModal) return;
    cwdPickerModal.hidden = false;
    cwdPickerDrivesLoaded = false;
    const drivesEl = document.getElementById('cwd-picker-drives');
    if (drivesEl) drivesEl.hidden = true;
    const startPath = selectedCwd || '~';
    loadCwdPickerDir(startPath);
  }

  function closeCwdPicker() {
    if (cwdPickerModal) cwdPickerModal.hidden = true;
    if (cwdPickerAbort) { cwdPickerAbort.abort(); cwdPickerAbort = null; }
  }

  async function loadCwdPickerDir(dirPath) {
    const listEl = document.getElementById('cwd-picker-list');
    const currentEl = document.getElementById('cwd-picker-current');
    if (!listEl || !currentEl) return;

    if (cwdPickerAbort) cwdPickerAbort.abort();
    const controller = new AbortController();
    cwdPickerAbort = controller;

    listEl.innerHTML = '<div class="cwd-picker-empty">Loading...</div>';
    try {
      const resp = await fetch(
        `${getFilerApiBase()}/list?path=${encodeURIComponent(dirPath)}&show_hidden=false`,
        { credentials: 'same-origin', signal: controller.signal }
      );
      if (!resp.ok) {
        listEl.innerHTML = '<div class="cwd-picker-empty">Failed to load directory</div>';
        return;
      }
      const data = await resp.json();
      cwdPickerPath = data.path;
      currentEl.textContent = data.path;
      currentEl.title = data.path;

      if (!cwdPickerDrivesLoaded && data.drives && data.drives.length > 0) {
        cwdPickerDrivesLoaded = true;
        renderCwdPickerDrives(data.drives);
      } else if (!cwdPickerDrivesLoaded) {
        fetchCwdPickerDrives(data.path);
      }

      const dirs = (data.entries || []).filter((e) => e.is_dir);
      if (dirs.length === 0) {
        listEl.innerHTML = '<div class="cwd-picker-empty">No subdirectories</div>';
      } else {
        listEl.innerHTML = '';
        const sep = data.path.includes('/') ? '/' : '\\';
        const base = data.path.replace(/[\\/]$/, '');
        for (const d of dirs) {
          listEl.appendChild(createPickerItem(d.name, base + sep + d.name));
        }
      }
    } catch (err) {
      if (err.name === 'AbortError') return;
      listEl.innerHTML = '<div class="cwd-picker-empty">Error loading directory</div>';
    }
  }

  function createPickerItem(label, path) {
    const item = document.createElement('div');
    item.className = 'cwd-picker-item';
    item.setAttribute('role', 'option');
    item.dataset.path = path;

    const icon = document.createElement('span');
    icon.className = 'cwd-picker-item-icon';
    icon.textContent = '\uD83D\uDCC1';

    const name = document.createElement('span');
    name.textContent = label;

    item.appendChild(icon);
    item.appendChild(name);
    return item;
  }

  function renderCwdPickerDrives(drives) {
    const container = document.getElementById('cwd-picker-drives');
    if (!container) return;
    container.innerHTML = '';
    container.hidden = !drives || drives.length === 0;
    for (const d of drives) {
      const btn = document.createElement('button');
      btn.className = 'cwd-picker-drive-btn';
      btn.textContent = d;
      btn.type = 'button';
      btn.addEventListener('click', () => loadCwdPickerDir(d));
      container.appendChild(btn);
    }
  }

  async function fetchCwdPickerDrives(resolvedPath) {
    if (cwdPickerDrivesLoaded) return;
    const match = resolvedPath.match(/^([A-Za-z]:\\)/);
    if (!match) return;
    try {
      const resp = await fetch(
        `${getFilerApiBase()}/list?path=${encodeURIComponent(match[1])}&show_hidden=false`,
        { credentials: 'same-origin' }
      );
      if (!resp.ok) return;
      const data = await resp.json();
      if (data.drives && data.drives.length > 0) {
        cwdPickerDrivesLoaded = true;
        renderCwdPickerDrives(data.drives);
      }
    } catch { /* ignore */ }
  }

  // Handle drive/root boundaries so "Up" stops instead of looping.
  function getParentDir(p) {
    if (!p) return p;
    const normalized = p.replace(/[\\/]+$/, '');
    const lastSep = Math.max(normalized.lastIndexOf('/'), normalized.lastIndexOf('\\'));
    if (lastSep < 0) return p;
    if (lastSep === 0) return normalized.substring(0, 1);
    if (normalized.length >= 2 && normalized[1] === ':' && lastSep === 2) {
      return normalized.substring(0, 3);
    }
    return normalized.substring(0, lastSep);
  }

  // ── Public API ──

  return {
    init,
    setRemote,
    prefillInput,
  };
})();
