/**
 * DenChat — Chat tab wrapping claude CLI via stream-json protocol.
 *
 * Backend spawns `claude -p --input-format stream-json --output-format stream-json --verbose`
 * and relays events over WebSocket.
 */
const DenChat = (() => {
  // ── State ──
  let ws = null;
  let sessionId = null;
  let messagesEl = null;
  let inputEl = null;
  let sendBtn = null;
  let sessionListEl = null;
  let newBtn = null;
  let continueBtn = null;
  let cwdToggle = null;
  let cwdInput = null;
  let cwdBar = null;
  let toolsToggle = null;
  let toolsBar = null;
  let toolsSelect = null;
  let toolsCustom = null;
  let searchInput = null;
  let mainTitle = null;
  let currentAssistantBubble = null;
  let currentThinkingBlock = null;
  let isStreaming = false;
  let composing = false;
  let renderPending = false;
  let thinkingRenderPending = false;

  // ── File attachment state ──
  let attachBtn = null;
  let attachChipsEl = null;
  let inputAreaEl = null;
  let imagePreviewEl = null;
  const attachedFiles = []; // array of file path strings
  const pendingImages = []; // array of {data: base64, mediaType: string, name: string, size: number}

  // Session search filter
  let searchFilter = '';

  // Cached session lists for client-side filtering
  let cachedActiveSessions = [];
  let cachedHistorySessions = [];

  // ── Tool notification state ──
  const MODIFYING_TOOLS = new Set([
    'Edit', 'Write', 'MultiEdit', 'Bash', 'NotebookEdit',
  ]);
  const DIFF_TOOLS = new Set(['Edit', 'Write', 'MultiEdit']);
  const toolInputMap = new Map();
  const autoDismissedTools = new Set();
  // Permission gate: tools auto-allowed by user for this session
  const autoAllowedTools = new Set();
  let permissionGateEnabled = false;
  let permissionGateCheckbox = null;

  // ── Push notification state ──
  let notificationsEnabled = false;

  // ── Cumulative cost state (#58) ──
  let sessionCostUsd = 0;
  let sessionInputTokens = 0;
  let sessionOutputTokens = 0;

  // ── Sidebar polling ──
  let pollTimer = null;
  const POLL_INTERVAL = 5000;

  // ── Frontend-side state for current session ──
  let currentSessionState = 'idle';

  // ── Auto-restart state (#75) ──
  // When claude process exits normally, we save the claude_session_id
  // so we can auto-restart with --continue on the next user message.
  let pendingClaudeSessionId = null;
  // Flag to distinguish explicit Stop from normal process exit.
  let explicitStop = false;
  // True when viewing a persisted history (read-only mode, no active session).
  let viewingHistory = false;
  // True when no session is active (welcome state — user must explicitly start one).
  let noSession = false;

  // ── WS keepalive ──
  let pingTimer = null;
  let pongReceived = true;
  const PING_INTERVAL = 30000; // 30s

  // ── Init ──
  function init() {
    messagesEl = document.getElementById('chat-messages');
    inputEl = document.getElementById('chat-input');
    sendBtn = document.getElementById('chat-send');
    sessionListEl = document.getElementById('chat-session-list');
    newBtn = document.getElementById('chat-new-btn');
    cwdToggle = document.getElementById('chat-cwd-toggle');
    cwdInput = document.getElementById('chat-cwd-input');
    cwdBar = document.getElementById('chat-cwd-bar');
    mainTitle = document.getElementById('chat-main-title');

    continueBtn = document.getElementById('chat-continue-btn');
    toolsToggle = document.getElementById('chat-tools-toggle');
    toolsBar = document.getElementById('chat-tools-bar');
    toolsSelect = document.getElementById('chat-tools-select');
    toolsCustom = document.getElementById('chat-tools-custom');
    searchInput = document.getElementById('chat-search-input');
    permissionGateCheckbox = document.getElementById('chat-permission-gate');
    attachBtn = document.getElementById('chat-attach-btn');
    attachChipsEl = document.getElementById('chat-attach-chips');
    inputAreaEl = document.getElementById('chat-input-area');
    imagePreviewEl = document.getElementById('chat-image-preview');

    sendBtn.addEventListener('click', handleSendOrStop);

    // File attachment
    attachBtn.addEventListener('click', handleAttachClick);
    initAttachDragDrop();
    initImagePaste();
    inputEl.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
        e.preventDefault();
        handleSendOrStop();
      }
    });
    inputEl.addEventListener('input', () => { if (!composing) autoResizeInput(); });
    inputEl.addEventListener('compositionstart', () => { composing = true; });
    inputEl.addEventListener('compositionend', () => {
      setTimeout(() => { composing = false; autoResizeInput(); }, 0);
    });
    inputEl.addEventListener('blur', () => { composing = false; });

    // Sidebar controls
    newBtn.addEventListener('click', () => startNewSession());
    continueBtn.addEventListener('click', () => continueLastSession());
    cwdToggle.addEventListener('click', () => openCwdPicker());
    toolsToggle.addEventListener('click', () => {
      toolsBar.hidden = !toolsBar.hidden;
      if (cwdBar) cwdBar.hidden = true;
    });
    toolsSelect.addEventListener('change', () => {
      toolsCustom.hidden = toolsSelect.value !== 'custom';
      if (!toolsCustom.hidden) toolsCustom.focus();
    });

    // Search filtering (debounced)
    let searchDebounce = null;
    searchInput.addEventListener('input', () => {
      clearTimeout(searchDebounce);
      searchDebounce = setTimeout(() => {
        searchFilter = searchInput.value.trim().toLowerCase();
        renderSessionList(cachedActiveSessions, cachedHistorySessions);
      }, 200);
    });

    // Desktop sidebar toggle (F011)
    const treeToggle = document.getElementById('chat-tree-toggle');
    const chatSidebar = document.querySelector('.chat-sidebar');
    if (treeToggle && chatSidebar) {
      const wasCollapsed = localStorage.getItem('chat-sidebar-collapsed') === 'true';
      if (wasCollapsed) chatSidebar.classList.add('collapsed');
      treeToggle.addEventListener('click', () => {
        chatSidebar.classList.toggle('collapsed');
        localStorage.setItem('chat-sidebar-collapsed', String(chatSidebar.classList.contains('collapsed')));
      });
    }

    // Listen for remote connection changes (F002)
    document.addEventListener('den:remote-changed', (e) => {
      const { mode, connectionId } = e.detail || {};
      if (mode === 'den' && connectionId) {
        const conns = typeof FilerRemote !== 'undefined' ? FilerRemote.getDenConnections() : {};
        const conn = conns[connectionId];
        setRemote(connectionId, conn?.type || 'direct');
      } else {
        setRemote(null);
      }
      disconnectWs();
      clearMessages();
      refreshSidebar().then(() => showWelcomeState());
    });

    // Request notification permission early
    requestNotificationPermission();

    // Handle visualViewport for mobile keyboard avoidance
    setupMobileViewport();

    // Initialize directory picker (registers listeners once)
    initCwdPicker();

    // Restore last session or create a new one
    refreshSidebar().then(() => restoreOrCreateSession());

    // Start polling for active session states (pauses when hidden)
    startPolling();
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) {
        stopPolling();
      } else {
        startPolling();
      }
    });
  }

  // ── Remote-aware URL helpers ──
  // chatRemoteId: if set, routes API calls through /api/remote/{id} or /api/relay/{id}
  let chatRemoteId = null;
  let chatRemoteType = null; // 'direct' or 'relay'

  function getApiBase() {
    if (!chatRemoteId) return '/api';
    if (chatRemoteType === 'relay') return `/api/relay/${chatRemoteId}/chat`;
    return `/api/remote/${chatRemoteId}/chat`;
  }

  function getChatWsUrl(sid) {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const query = `session=${encodeURIComponent(sid)}`;
    if (chatRemoteId) {
      if (chatRemoteType === 'relay') {
        return `${proto}//${location.host}/api/relay/${chatRemoteId}/chat-ws?${query}`;
      }
      return `${proto}//${location.host}/api/remote/${encodeURIComponent(chatRemoteId)}/chat-ws?${query}`;
    }
    return `${proto}//${location.host}/api/chat/ws?${query}`;
  }

  /** Set remote target for chat. Called externally when remote connections change. */
  function setRemote(remoteId, type) {
    chatRemoteId = remoteId || null;
    chatRemoteType = type || 'direct';
  }

  // ── Push notifications ──
  function requestNotificationPermission() {
    if (!('Notification' in window)) return;
    if (Notification.permission === 'granted') {
      notificationsEnabled = true;
    }
  }

  async function ensureNotificationPermission() {
    if (!('Notification' in window)) return false;
    if (Notification.permission === 'granted') {
      notificationsEnabled = true;
      return true;
    }
    if (Notification.permission === 'denied') return false;
    const result = await Notification.requestPermission();
    notificationsEnabled = result === 'granted';
    return notificationsEnabled;
  }

  function shouldNotify() {
    if (document.hidden) return true;
    const activeTab = document.querySelector('.tab.active');
    return activeTab && activeTab.dataset.tab !== 'chat';
  }

  function showNotification(title, body) {
    if (!notificationsEnabled || !shouldNotify()) return;
    try {
      const n = new Notification(title, {
        body: body,
        icon: '/favicon.ico',
        tag: 'den-chat',
        renotify: true,
      });
      n.onclick = () => {
        window.focus();
        if (window.DenApp) window.DenApp.switchTab('chat');
        n.close();
      };
      setTimeout(() => n.close(), 5000);
    } catch {
      // Notification API may fail in some contexts
    }
  }

  // ── Sidebar management ──
  async function refreshSidebar() {
    const base = getApiBase();

    try {
      const [activeResp, historyResp] = await Promise.all([
        fetch(`${base}/chat/sessions`, { credentials: 'same-origin' }),
        fetch(`${base}/chat/history`, { credentials: 'same-origin' }),
      ]);
      if (activeResp.ok) cachedActiveSessions = await activeResp.json();
      if (historyResp.ok) cachedHistorySessions = await historyResp.json();
    } catch (e) {
      console.warn('refreshSidebar failed:', e); // F008
    }

    renderSessionList(cachedActiveSessions, cachedHistorySessions);
  }

  function matchesSearch(s) {
    if (!searchFilter) return true;
    const name = (s.name || '').toLowerCase();
    const cwd = (s.cwd || '').toLowerCase();
    const id = (s.id || '').toLowerCase();
    const date = new Date(s.created_at || s.last_active || 0).toLocaleString().toLowerCase();
    return name.includes(searchFilter) || cwd.includes(searchFilter) || id.includes(searchFilter) || date.includes(searchFilter);
  }

  function renderSessionList(active, history) {
    // Don't destroy DOM if an inline rename is in progress
    if (renameInProgress) return;
    sessionListEl.innerHTML = '';

    const filteredActive = active.filter(matchesSearch);
    const filteredHistory = history.filter(matchesSearch);

    // Active sessions section
    if (filteredActive.length > 0) {
      const header = document.createElement('div');
      header.className = 'chat-session-section';
      header.textContent = 'Active';
      sessionListEl.appendChild(header);

      for (const s of filteredActive) {
        sessionListEl.appendChild(createActiveSessionItem(s));
      }
    }

    // History section
    if (filteredHistory.length > 0) {
      const header = document.createElement('div');
      header.className = 'chat-session-section';
      header.textContent = 'History';
      sessionListEl.appendChild(header);

      for (const s of filteredHistory) {
        sessionListEl.appendChild(createHistorySessionItem(s));
      }
    }

    // Highlight current session
    updateActiveHighlight();
  }

  function createActiveSessionItem(s) {
    const item = document.createElement('div');
    item.className = 'chat-session-item';
    item.dataset.sessionId = s.id;
    item.dataset.type = 'active';

    const stateEl = document.createElement('span');
    stateEl.className = 'chat-session-state';
    const stateKey = s.alive ? (s.state || 'idle') : 'dead';
    stateEl.dataset.state = stateKey;
    item.appendChild(stateEl);

    const info = document.createElement('div');
    info.className = 'chat-session-item-info';

    const label = document.createElement('span');
    label.className = 'chat-session-item-label';
    label.textContent = s.name || (s.cwd ? shortenPath(s.cwd) : s.id.substring(0, 8));
    info.appendChild(label);

    // Double-click to rename
    label.addEventListener('dblclick', (e) => {
      e.stopPropagation();
      startInlineRename(label, s.id, 'active');
    });

    const meta = document.createElement('span');
    meta.className = 'chat-session-item-meta';
    meta.textContent = formatTime(s.created_at);
    info.appendChild(meta);

    item.appendChild(info);

    const actions = document.createElement('div');
    actions.className = 'chat-session-actions';

    const delBtn = document.createElement('button');
    delBtn.className = 'chat-session-action-btn danger';
    delBtn.textContent = '\u00d7';
    delBtn.title = 'Destroy';
    delBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      destroyActiveSession(s.id);
    });
    actions.appendChild(delBtn);
    item.appendChild(actions);

    item.addEventListener('click', () => switchToActiveSession(s.id));
    return item;
  }

  function createHistorySessionItem(s) {
    const item = document.createElement('div');
    item.className = 'chat-session-item';
    item.dataset.sessionId = s.id;
    item.dataset.type = 'history';
    item.dataset.claudeSessionId = s.claude_session_id || '';

    const stateEl = document.createElement('span');
    stateEl.className = 'chat-session-state';
    stateEl.dataset.state = 'dead';
    item.appendChild(stateEl);

    const info = document.createElement('div');
    info.className = 'chat-session-item-info';

    const label = document.createElement('span');
    label.className = 'chat-session-item-label';
    const date = new Date(s.created_at).toLocaleString();
    label.textContent = s.name || date;
    info.appendChild(label);

    // Double-click to rename
    label.addEventListener('dblclick', (e) => {
      e.stopPropagation();
      startInlineRename(label, s.id, 'history');
    });

    const meta = document.createElement('span');
    meta.className = 'chat-session-item-meta';
    meta.textContent = `${s.message_count} events`;
    info.appendChild(meta);

    item.appendChild(info);

    const actions = document.createElement('div');
    actions.className = 'chat-session-actions';

    const resumeBtn = document.createElement('button');
    resumeBtn.className = 'chat-session-action-btn';
    resumeBtn.textContent = '\u25b6';
    resumeBtn.title = 'Resume';
    resumeBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      resumeSession(s.id, s.claude_session_id);
    });
    actions.appendChild(resumeBtn);

    const delBtn = document.createElement('button');
    delBtn.className = 'chat-session-action-btn danger';
    delBtn.textContent = '\u00d7';
    delBtn.title = 'Delete';
    delBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      deleteHistorySession(s.id);
    });
    actions.appendChild(delBtn);

    item.appendChild(actions);
    item.addEventListener('click', () => viewHistory(s.id));
    return item;
  }

  let renameInProgress = false;

  function startInlineRename(labelEl, id, type) {
    if (renameInProgress) return;
    renameInProgress = true;

    const oldText = labelEl.textContent;
    const input = document.createElement('input');
    input.type = 'text';
    input.className = 'chat-rename-input';
    input.value = oldText;
    labelEl.textContent = '';
    labelEl.appendChild(input);
    input.focus();
    input.select();

    let committed = false;
    function commit() {
      if (committed) return;
      committed = true;
      renameInProgress = false;

      const newName = input.value.trim();
      labelEl.textContent = newName || oldText;
      if (newName && newName !== oldText) {
        const base = getApiBase();
        const endpoint = type === 'active'
          ? `${base}/chat/sessions/${encodeURIComponent(id)}`
          : `${base}/chat/history/${encodeURIComponent(id)}`;
        fetch(endpoint, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          credentials: 'same-origin',
          body: JSON.stringify({ name: newName }),
        }).catch(() => {
          appendSystem('Failed to rename session.');
        });
      }
    }

    input.addEventListener('blur', commit);
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); input.blur(); }
      if (e.key === 'Escape') { input.value = oldText; input.blur(); }
    });
  }

  function updateActiveHighlight() {
    for (const el of sessionListEl.querySelectorAll('.chat-session-item')) {
      el.classList.toggle('active', el.dataset.sessionId === sessionId);
    }
  }

  function shortenPath(p) {
    const parts = p.replace(/\\/g, '/').split('/');
    return parts.length > 2 ? '.../' + parts.slice(-2).join('/') : p;
  }

  function formatTime(iso) {
    const d = new Date(iso);
    const now = new Date();
    if (d.toDateString() === now.toDateString()) {
      return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    }
    return d.toLocaleDateString([], { month: 'short', day: 'numeric' });
  }

  // ── Polling for active session states ──
  function startPolling() {
    stopPolling();
    pollTimer = setInterval(pollActiveSessions, POLL_INTERVAL);
  }

  function stopPolling() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
  }

  let pollInFlight = false;
  async function pollActiveSessions() {
    if (pollInFlight) return; // F015: prevent overlapping polls
    pollInFlight = true;
    const base = getApiBase();
    try {
      const resp = await fetch(`${base}/chat/sessions`, { credentials: 'same-origin' });
      if (!resp.ok) return;
      const sessions = await resp.json();
      updateActiveStates(sessions);
    } catch (e) {
      console.warn('Chat poll failed:', e); // F008
    } finally {
      pollInFlight = false;
    }
  }

  function updateActiveStates(sessions) {
    const map = new Map(sessions.map((s) => [s.id, s]));

    for (const el of sessionListEl.querySelectorAll('.chat-session-item[data-type="active"]')) {
      const sid = el.dataset.sessionId;
      const s = map.get(sid);
      const stateEl = el.querySelector('.chat-session-state');
      if (!stateEl) continue;

      if (s) {
        // Use frontend-tracked state for current session (more responsive)
        if (sid === sessionId) {
          stateEl.dataset.state = currentSessionState;
        } else {
          stateEl.dataset.state = s.alive ? (s.state || 'idle') : 'dead';
        }
      } else {
        // Session no longer active — mark dead
        stateEl.dataset.state = 'dead';
      }
    }
  }

  // ── Send or Stop ──
  function handleSendOrStop() {
    if (isStreaming) {
      stopSession();
    } else {
      sendMessage();
    }
  }

  async function stopSession() {
    if (!sessionId) return;
    explicitStop = true;
    pendingClaudeSessionId = null;
    const base = getApiBase();
    try {
      await fetch(`${base}/chat/sessions/${encodeURIComponent(sessionId)}/stop`, {
        method: 'POST',
        credentials: 'same-origin',
      });
    } catch (e) {
      console.warn('stopSession failed:', e);
    }
  }

  // ── Session auto-restore ──
  async function restoreOrCreateSession() {
    const savedId = localStorage.getItem('chat-active-session');
    if (savedId) {
      // Use cached sessions from refreshSidebar (already fetched)
      const alive = cachedActiveSessions.find((s) => s.id === savedId && s.alive);
      if (alive) {
        // Restore permission gate state from localStorage
        permissionGateEnabled = localStorage.getItem('chat-permission-gate') === 'true';
        if (permissionGateCheckbox) permissionGateCheckbox.checked = permissionGateEnabled;
        switchToActiveSession(savedId);
        return;
      }
      localStorage.removeItem('chat-active-session');
    }
    // Don't auto-create — show welcome state and let user explicitly start
    showWelcomeState();
  }

  function showWelcomeState() {
    noSession = true;
    sessionId = null;
    disconnectWs();
    clearMessages();
    appendSystem('Click "+" to start a new session, or select one from the sidebar.');
    updateInputState();
  }

  function saveSessionToStorage() {
    if (sessionId) {
      localStorage.setItem('chat-active-session', sessionId);
      localStorage.setItem('chat-permission-gate', String(permissionGateEnabled));
    } else {
      localStorage.removeItem('chat-active-session');
      localStorage.removeItem('chat-permission-gate');
    }
  }

  // ── Continue last session ──
  async function continueLastSession() {
    noSession = false;
    appendSystem('Continuing last session...');

    const base = getApiBase();
    try {
      const resp = await fetch(`${base}/chat/sessions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'same-origin',
        body: JSON.stringify({ continue_last: true }),
      });
      if (!resp.ok) {
        const err = await resp.json().catch(() => ({}));
        appendSystem('Failed to continue: ' + (err.error || resp.statusText));
        return;
      }
      // Only disconnect after successful creation
      disconnectWs();
      clearMessages();
      const data = await resp.json();
      sessionId = data.id;
      connectWs();
      refreshSidebar();
      saveSessionToStorage();
    } catch (e) {
      appendSystem('Failed to continue: ' + e.message);
    }
  }

  // ── Session actions ──
  async function startNewSession() {
    disconnectWs();
    clearMessages();
    await createSession();
  }

  // View a persisted session's history without creating a new active session (#71)
  async function viewHistory(persistedId) {
    noSession = false;
    disconnectWs();
    clearMessages();
    viewingHistory = true;
    updateInputState();

    const base = getApiBase();
    try {
      const resp = await fetch(`${base}/chat/history/${encodeURIComponent(persistedId)}`, {
        credentials: 'same-origin',
      });
      if (resp.ok) {
        const data = await resp.json();
        if (data.history) {
          suppressScroll = true; // F004: avoid forced reflow per event
          for (const line of data.history) {
            handleEvent(line);
          }
          suppressScroll = false;
          scrollToBottom();
        }
      } else {
        appendSystem('Failed to load history.');
      }
    } catch {
      appendSystem('Failed to load history.');
    }
    updateActiveHighlight();
  }

  function switchToActiveSession(id) {
    if (id === sessionId) return;
    noSession = false;
    disconnectWs();
    clearMessages();
    sessionId = id;
    connectWs();
    updateActiveHighlight();
    updateMainTitle();
    saveSessionToStorage();
  }

  async function resumeSession(persistedId, claudeSessionId) {
    if (!claudeSessionId) {
      appendSystem('Cannot resume: no claude session ID found.');
      return;
    }

    // Confirm before resuming — user may just want to view history
    if (!confirm('Resume this session? A new claude process will start with --continue.')) {
      return;
    }

    noSession = false;
    disconnectWs();
    clearMessages();
    appendSystem('Resuming session...');

    const base = getApiBase();

    // Load persisted history for display
    try {
      const histResp = await fetch(`${base}/chat/history/${encodeURIComponent(persistedId)}`, {
        credentials: 'same-origin',
      });
      if (histResp.ok) {
        const data = await histResp.json();
        if (data.history) {
          for (const line of data.history) {
            handleEvent(line);
          }
        }
      }
    } catch {
      // Continue even if history load fails
    }

    // Create a new active session with --resume
    try {
      const resp = await fetch(`${base}/chat/sessions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'same-origin',
        body: JSON.stringify({ resume_session_id: claudeSessionId }),
      });
      if (!resp.ok) {
        const err = await resp.json().catch(() => ({}));
        appendSystem('Failed to resume: ' + (err.error || resp.statusText));
        return;
      }
      const data = await resp.json();
      sessionId = data.id;
      connectWs();
      refreshSidebar();
      saveSessionToStorage();
    } catch (e) {
      appendSystem('Failed to resume: ' + e.message);
    } finally {
      isStreaming = false;
      currentAssistantBubble = null;
      currentThinkingBlock = null;
      updateInputState();
    }
  }

  async function destroyActiveSession(id) {
    const base = getApiBase();
    try {
      const resp = await fetch(`${base}/chat/sessions/${encodeURIComponent(id)}`, {
        method: 'DELETE',
        credentials: 'same-origin',
      });
      if (resp.ok || resp.status === 204) {
        if (id === sessionId) {
          disconnectWs();
          clearMessages();
        }
        refreshSidebar();
      } else {
        appendSystem('Failed to destroy session.'); // F007
      }
    } catch (e) {
      console.warn('destroyActiveSession failed:', e); // F008
      appendSystem('Failed to destroy session.');
    }
  }

  async function deleteHistorySession(id) {
    const base = getApiBase();
    try {
      const resp = await fetch(`${base}/chat/history/${encodeURIComponent(id)}`, {
        method: 'DELETE',
        credentials: 'same-origin',
      });
      if (resp.ok || resp.status === 204) {
        refreshSidebar();
      } else {
        appendSystem('Failed to delete session.');
      }
    } catch {
      appendSystem('Failed to delete session.');
    }
  }

  function getAllowedTools() {
    if (!toolsSelect) return undefined;
    const val = toolsSelect.value;
    if (!val) return undefined;
    if (val === 'custom') {
      const custom = toolsCustom ? toolsCustom.value.trim() : '';
      return custom ? custom.split(',').map((t) => t.trim()).filter(Boolean) : undefined;
    }
    return val.split(',').map((t) => t.trim());
  }

  async function createSession() {
    noSession = false;
    autoDismissedTools.clear();
    autoAllowedTools.clear();
    toolInputMap.clear();
    permissionGateEnabled = permissionGateCheckbox ? permissionGateCheckbox.checked : false;
    const base = getApiBase();
    const cwd = cwdInput ? cwdInput.value.trim() : '';
    const tools = getAllowedTools();

    try {
      const body = {};
      if (cwd) body.cwd = cwd;
      if (tools) body.allowed_tools = tools;
      if (permissionGateEnabled) body.permission_gate = true;

      const resp = await fetch(`${base}/chat/sessions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'same-origin',
        body: JSON.stringify(body),
      });
      if (!resp.ok) {
        const err = await resp.json().catch(() => ({}));
        appendSystem('Failed to create chat session: ' + (err.error || resp.statusText));
        return;
      }
      const data = await resp.json();
      sessionId = data.id;
      connectWs();
      refreshSidebar();
      updateMainTitle();
      saveSessionToStorage();
    } catch (e) {
      appendSystem('Failed to create chat session: ' + e.message);
    }
  }

  function updateMainTitle() {
    if (!mainTitle) return;
    if (sessionId) {
      const cwd = cwdInput ? cwdInput.value.trim() : '';
      mainTitle.textContent = cwd ? shortenPath(cwd) : sessionId.substring(0, 8);
    } else {
      mainTitle.textContent = '';
    }
  }

  // ── WebSocket ──
  function connectWs() {
    if (!sessionId) return;
    viewingHistory = false;
    const url = getChatWsUrl(sessionId);
    ws = new WebSocket(url);

    ws.onopen = () => {
      appendSystem('Session started.');
      currentSessionState = 'idle';
      noSession = false;
      updateInputState();
      startPing();
    };

    ws.onmessage = (e) => {
      // Treat any incoming message as proof of liveness
      pongReceived = true;
      if (e.data === 'pong') return; // keepalive response, don't process
      handleEvent(e.data);
    };

    ws.onclose = () => {
      ws = null;
      stopPing();
      isStreaming = false;
      currentSessionState = 'idle';
      autoDismissedTools.clear();
      toolInputMap.clear();
      if (pendingClaudeSessionId) {
        // Auto-restart available — keep input ready, no "ended" message
      } else if (explicitStop) {
        // F008: don't duplicate "Session stopped" from session_ended handler
      } else {
        appendSystem('Session ended.');
        // F005: no pending restart and not explicit stop — session is truly gone
        noSession = true;
      }
      explicitStop = false; // F003: reset after consumption
      updateInputState();
      refreshSidebar();
    };

    ws.onerror = () => {
      // onclose will fire after this
    };

    updateActiveHighlight();
  }

  function disconnectWs() {
    stopPing();
    if (ws) {
      ws.onclose = null;
      ws.close();
      ws = null;
    }
    sessionId = null;
    isStreaming = false;
    currentSessionState = 'idle';
    autoDismissedTools.clear();
    toolInputMap.clear();
    pendingClaudeSessionId = null;
    explicitStop = false;
    viewingHistory = false;
    updateInputState();
    // Don't clear localStorage here — only clear when session truly ends
  }

  const PING_MSG = JSON.stringify({ type: 'ping' }); // F008: constant

  function startPing() {
    stopPing();
    pongReceived = true;
    pingTimer = setInterval(() => {
      if (!ws || ws.readyState !== WebSocket.OPEN) {
        stopPing();
        return;
      }
      if (!pongReceived) {
        // No response since last ping — connection is dead
        // F001: clear sessionId so switchToActiveSession can reconnect
        const lostSessionId = sessionId;
        sessionId = null;
        // F004: set explicitStop to prevent duplicate "Session ended." from onclose
        explicitStop = true;
        appendSystem('Connection lost. Session may still be running — click it to reconnect.');
        ws.close();
        // F005: if no session, disable input properly
        noSession = !lostSessionId;
        updateInputState();
        return;
      }
      pongReceived = false;
      try {
        ws.send(PING_MSG);
      } catch {
        // send failed — onclose will handle
      }
    }, PING_INTERVAL);
  }

  function stopPing() {
    if (pingTimer) {
      clearInterval(pingTimer);
      pingTimer = null;
    }
  }

  function clearMessages() {
    if (messagesEl) messagesEl.innerHTML = '';
    currentAssistantBubble = null;
    currentThinkingBlock = null;
    toolInputMap.clear();
    sessionCostUsd = 0;
    sessionInputTokens = 0;
    sessionOutputTokens = 0;
  }

  // ── Event handling ──
  function handleEvent(raw) {
    let event;
    try {
      event = JSON.parse(raw);
    } catch {
      return;
    }

    // Track state for sidebar indicator
    trackState(event);

    switch (event.type) {
      case 'system':
        handleSystemEvent(event);
        break;
      case 'assistant':
        handleAssistantEvent(event);
        break;
      case 'user':
        handleUserEvent(event);
        break;
      case 'result':
        handleResultEvent(event);
        break;
      case 'permission_request':
        handlePermissionRequest(event);
        break;
      case 'session_ended':
        if (explicitStop) {
          appendSystem('Session stopped.');
          pendingClaudeSessionId = null;
        } else if (event.claude_session_id) {
          // Normal process exit — save for auto-restart
          pendingClaudeSessionId = event.claude_session_id;
        }
        currentSessionState = 'idle';
        break;
      case 'attach_warning':
        appendSystem(event.message || 'Attachment warning');
        break;
      default:
        break;
    }
  }

  function trackState(event) {
    if (event.type === 'assistant') {
      const content = event.message && event.message.content;
      if (Array.isArray(content)) {
        if (content.some((b) => b.type === 'thinking')) {
          currentSessionState = 'thinking';
        } else if (content.some((b) => b.type === 'tool_use')) {
          currentSessionState = 'tooluse';
        } else {
          currentSessionState = 'streaming';
        }
      } else {
        currentSessionState = 'streaming';
      }
    } else if (event.type === 'result') {
      currentSessionState = 'idle';
    }
    // Update sidebar indicator for current session immediately (F003/F012: avoid querySelector injection)
    if (!sessionId || !sessionListEl) return;
    for (const item of sessionListEl.querySelectorAll('.chat-session-item')) {
      if (item.dataset.sessionId === sessionId) {
        const stateEl = item.querySelector('.chat-session-state');
        if (stateEl) stateEl.dataset.state = currentSessionState;
        break;
      }
    }
  }

  function handleSystemEvent(event) {
    if (event.subtype === 'init') {
      const model = event.model || 'unknown';
      appendSystem(`Model: ${model}`);
    }
  }

  function handleAssistantEvent(event) {
    const msg = event.message;
    if (!msg || !msg.content) return;

    for (const block of msg.content) {
      switch (block.type) {
        case 'text':
          appendAssistantText(block.text);
          break;
        case 'thinking':
          appendThinking(block.thinking);
          break;
        case 'tool_use':
          if (block.name === 'AskUserQuestion') {
            appendAskDialog(block);
            ensureNotificationPermission().then(() => {
              showNotification('Claude has a question', block.input?.question || 'Action required');
            });
          } else {
            appendToolUse(block);
            const displayName = stripMcpPrefix(block.name);
            if (MODIFYING_TOOLS.has(displayName)) {
              ensureNotificationPermission().then(() => {
                showNotification(`Tool: ${displayName}`, JSON.stringify(block.input || {}).substring(0, 100));
              });
            }
          }
          break;
      }
    }
  }

  function handleUserEvent(event) {
    const msg = event.message;
    if (!msg || !msg.content) return;

    for (const item of msg.content) {
      if (item.type === 'tool_result' || item.tool_use_id) {
        appendToolResult(item);
      }
    }
  }

  function handleResultEvent(event) {
    isStreaming = false;
    currentAssistantBubble = null;
    currentThinkingBlock = null;
    updateInputState();

    if (event.total_cost_usd != null) {
      const cost = event.total_cost_usd.toFixed(4);
      const tokens = event.usage || {};
      const inp = tokens.input_tokens || 0;
      const out = tokens.output_tokens || 0;
      const cached = tokens.cache_read_input_tokens || 0;
      sessionCostUsd += event.total_cost_usd;
      sessionInputTokens += inp;
      sessionOutputTokens += out;
      appendCost(`$${cost} (total: $${sessionCostUsd.toFixed(4)}) | in:${inp} out:${out} cached:${cached}`);
    }

    ensureNotificationPermission().then(() => {
      showNotification('Claude finished', 'Response complete.');
    });
  }

  // ── DOM rendering ──
  function appendSystem(text) {
    const el = document.createElement('div');
    el.className = 'chat-msg chat-system';
    el.textContent = text;
    messagesEl.appendChild(el);
    scrollToBottom();
  }

  function appendCost(text) {
    const el = document.createElement('div');
    el.className = 'chat-msg chat-cost';
    el.textContent = text;
    messagesEl.appendChild(el);
    scrollToBottom();
  }

  function appendUserMessage(text, files, images) {
    const el = document.createElement('div');
    el.className = 'chat-msg chat-user';
    if (files && files.length > 0) {
      const filesDiv = document.createElement('div');
      filesDiv.className = 'chat-user-files';
      for (const fp of files) {
        const tag = document.createElement('span');
        tag.className = 'chat-user-file-tag';
        tag.textContent = fp.replace(/\\/g, '/').split('/').pop() || fp;
        tag.title = fp;
        filesDiv.appendChild(tag);
      }
      el.appendChild(filesDiv);
    }
    if (images && images.length > 0) {
      const imagesDiv = document.createElement('div');
      imagesDiv.className = 'chat-user-images';
      for (const img of images) {
        const imgEl = document.createElement('img');
        imgEl.src = 'data:' + img.mediaType + ';base64,' + img.data;
        imgEl.alt = img.name || 'image';
        imgEl.className = 'chat-user-image';
        imagesDiv.appendChild(imgEl);
      }
      el.appendChild(imagesDiv);
    }
    if (text) {
      const textNode = document.createTextNode(text);
      el.appendChild(textNode);
    }
    messagesEl.appendChild(el);
    scrollToBottom();
  }

  function appendAssistantText(text) {
    if (!currentAssistantBubble) {
      currentAssistantBubble = document.createElement('div');
      currentAssistantBubble.className = 'chat-msg chat-assistant';
      currentAssistantBubble._rawText = '';
      messagesEl.appendChild(currentAssistantBubble);
    }
    currentAssistantBubble._rawText += text;
    if (!renderPending) {
      renderPending = true;
      requestAnimationFrame(() => {
        renderPending = false;
        if (currentAssistantBubble) {
          if (typeof DenMarkdown !== 'undefined') {
            currentAssistantBubble.innerHTML = DenMarkdown.sanitize(
              DenMarkdown.renderMarkdown(currentAssistantBubble._rawText)
            );
            injectCopyButtons(currentAssistantBubble);
          } else {
            currentAssistantBubble.textContent = currentAssistantBubble._rawText;
          }
          scrollToBottom();
        }
      });
    }
  }

  function appendThinking(text) {
    if (!currentThinkingBlock) {
      const details = document.createElement('details');
      details.className = 'chat-msg chat-thinking';
      const summary = document.createElement('summary');
      summary.textContent = 'Thinking...';
      details.appendChild(summary);
      const content = document.createElement('div');
      content.className = 'chat-thinking-content';
      details.appendChild(content);
      messagesEl.appendChild(details);
      currentThinkingBlock = content;
    }
    currentThinkingBlock.textContent += text;
    if (!thinkingRenderPending) {
      thinkingRenderPending = true;
      requestAnimationFrame(() => {
        thinkingRenderPending = false;
        scrollToBottom();
      });
    }
  }

  function appendToolUse(block) {
    currentAssistantBubble = null;
    currentThinkingBlock = null;

    // Strip MCP prefix for display and matching
    const displayName = stripMcpPrefix(block.name);
    const displayBlock = { ...block, name: displayName };

    // Store input for diff rendering
    if (DIFF_TOOLS.has(displayName)) {
      toolInputMap.set(block.id, { name: displayName, input: block.input });
    }

    // If permission gate is active, gated tools go through the permission flow
    // (the MCP server handles blocking). We just show a collapsed block here.
    if (permissionGateEnabled && block.name.startsWith('mcp__den-gate__')) {
      appendToolBlock(displayBlock);
      return;
    }

    const isModifying = MODIFYING_TOOLS.has(displayName);
    const isDismissed = autoDismissedTools.has(displayName);

    if (isModifying && !isDismissed) {
      appendToolNotification(displayBlock);
    } else {
      appendToolBlock(displayBlock);
    }
  }

  function maybeAddRunInTerminal(parentEl, block) {
    const name = stripMcpPrefix(block.name);
    if (name !== 'Bash') return;
    // Do not show Run in Terminal for permission-gated tools
    if (permissionGateEnabled && block.name.startsWith('mcp__den-gate__')) return;
    const cmd = block.input?.command;
    if (!cmd) return;
    const btn = document.createElement('button');
    btn.className = 'chat-run-in-terminal-btn';
    btn.textContent = 'Run in Terminal';
    btn.addEventListener('click', () => {
      document.dispatchEvent(new CustomEvent('den:run-in-terminal', {
        detail: { command: cmd },
      }));
      if (window.DenApp) window.DenApp.switchTab('terminal');
    });
    parentEl.appendChild(btn);
  }

  function appendToolBlock(block) {
    const details = document.createElement('details');
    details.className = 'chat-msg chat-tool';
    details.id = 'tool-' + block.id;

    const summary = document.createElement('summary');
    summary.textContent = block.name;
    details.appendChild(summary);

    const inputPre = document.createElement('pre');
    inputPre.className = 'chat-tool-input';
    inputPre.textContent = JSON.stringify(block.input, null, 2);
    details.appendChild(inputPre);

    maybeAddRunInTerminal(details, block);

    // Group consecutive tool blocks into a collapsible accordion
    const lastChild = messagesEl.lastElementChild;
    if (lastChild && lastChild.classList.contains('chat-tool-group')) {
      // Add to existing group
      lastChild.appendChild(details);
      addToolToGroupMeta(lastChild, block.name);
    } else if (lastChild && lastChild.classList.contains('chat-tool')
               && !lastChild.classList.contains('chat-tool-notification')
               && !lastChild.classList.contains('chat-ask-dialog')) {
      // Promote previous single tool into a new group
      const prev = messagesEl.removeChild(lastChild);
      const group = document.createElement('details');
      group.className = 'chat-tool-group';
      group.appendChild(document.createElement('summary'));
      // Initialize meta from previous tool
      group._toolCount = 0;
      group._toolNames = [];
      group._errorCount = 0;
      const prevSummary = prev.querySelector('summary');
      addToolToGroupMeta(group, prevSummary ? prevSummary.textContent : '');
      group.appendChild(prev);
      // Add new tool
      addToolToGroupMeta(group, block.name);
      group.appendChild(details);
      messagesEl.appendChild(group);
    } else {
      // Single tool — append directly (no group wrapper)
      messagesEl.appendChild(details);
    }
    scrollToBottom();
  }

  function addToolToGroupMeta(group, name) {
    if (!group._toolCount) {
      group._toolCount = 0;
      group._toolNames = [];
      group._errorCount = 0;
    }
    group._toolCount++;
    if (!group._toolNames.includes(name)) {
      group._toolNames.push(name);
    }
    updateToolGroupLabel(group);
  }

  function updateToolGroupLabel(group) {
    const summary = group.querySelector(':scope > summary');
    if (!summary) return;
    const count = group._toolCount || 0;
    const names = group._toolNames || [];
    const errors = group._errorCount || 0;
    const label = names.length <= 3
      ? names.join(', ')
      : names.slice(0, 3).join(', ') + ', ...';
    let text = `${count} tool${count > 1 ? 's' : ''} used (${label})`;
    if (errors > 0) {
      text += ` — ${errors} failed`;
    }
    summary.textContent = text;
  }

  function appendToolNotification(block) {
    const card = document.createElement('div');
    card.className = 'chat-msg chat-tool chat-tool-notification';
    card.id = 'tool-' + block.id;

    const header = document.createElement('div');
    header.className = 'chat-tool-notification-header';
    header.textContent = block.name;
    card.appendChild(header);

    const inputPre = document.createElement('pre');
    inputPre.className = 'chat-tool-input';
    inputPre.textContent = JSON.stringify(block.input, null, 2);
    card.appendChild(inputPre);

    const actions = document.createElement('div');
    actions.className = 'chat-tool-notification-actions';

    const dismiss = document.createElement('button');
    dismiss.className = 'chat-notification-btn';
    dismiss.textContent = 'Dismiss';
    dismiss.addEventListener('click', () => {
      card.classList.remove('chat-tool-notification');
      card.classList.add('chat-tool-notification-resolved');
      actions.remove();
    });

    const autoDismiss = document.createElement('button');
    autoDismiss.className = 'chat-notification-btn primary';
    autoDismiss.textContent = 'Auto-dismiss';
    autoDismiss.addEventListener('click', () => {
      autoDismissedTools.add(block.name);
      card.classList.remove('chat-tool-notification');
      card.classList.add('chat-tool-notification-resolved');
      actions.remove();
    });

    actions.appendChild(dismiss);
    actions.appendChild(autoDismiss);
    card.appendChild(actions);

    maybeAddRunInTerminal(card, block);

    messagesEl.appendChild(card);
    scrollToBottom();
  }

  function appendAskDialog(block) {
    currentAssistantBubble = null;
    currentThinkingBlock = null;

    const card = document.createElement('div');
    card.className = 'chat-msg chat-tool chat-ask-dialog';
    card.id = 'tool-' + block.id;
    card.dataset.toolUseId = block.id;

    const question = (block.input && block.input.question) || 'Claude has a question:';
    const questionEl = document.createElement('div');
    questionEl.className = 'chat-ask-question';
    questionEl.textContent = question;
    card.appendChild(questionEl);

    const options = block.input && block.input.options;
    if (Array.isArray(options) && options.length > 0) {
      const optionsEl = document.createElement('div');
      optionsEl.className = 'chat-ask-options';
      for (const opt of options) {
        const btn = document.createElement('button');
        btn.className = 'chat-notification-btn';
        btn.textContent = opt;
        btn.addEventListener('click', () => {
          submitAskResponse(card, opt);
        });
        optionsEl.appendChild(btn);
      }
      card.appendChild(optionsEl);
    }

    const inputRow = document.createElement('div');
    inputRow.className = 'chat-ask-input-row';

    const input = document.createElement('input');
    input.type = 'text';
    input.className = 'chat-ask-input';
    input.placeholder = 'Type your answer...';
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !e.isComposing) {
        e.preventDefault();
        const text = input.value.trim();
        if (text) submitAskResponse(card, text);
      }
    });

    const sendAsk = document.createElement('button');
    sendAsk.className = 'chat-notification-btn primary';
    sendAsk.textContent = 'Send';
    sendAsk.addEventListener('click', () => {
      const text = input.value.trim();
      if (text) submitAskResponse(card, text);
    });

    inputRow.appendChild(input);
    inputRow.appendChild(sendAsk);
    card.appendChild(inputRow);

    messagesEl.appendChild(card);
    scrollToBottom();
    input.focus();
  }

  function submitAskResponse(card, answer) {
    if (!ws || ws.readyState !== WebSocket.OPEN) {
      appendSystem('Cannot send answer: connection lost.');
      return;
    }
    const toolUseId = card.dataset.toolUseId || null;
    ws.send(JSON.stringify({ type: 'ask_response', text: answer, tool_use_id: toolUseId }));

    card.classList.add('chat-ask-resolved');
    const resolved = document.createElement('div');
    resolved.className = 'chat-ask-answered';
    resolved.textContent = 'Answered: ' + answer;
    const interactive = card.querySelectorAll('.chat-ask-options, .chat-ask-input-row');
    interactive.forEach((el) => el.remove());
    card.appendChild(resolved);
    scrollToBottom();
  }

  // ── Permission gate ──

  /** Strip MCP server prefix from tool name (e.g. "mcp__den-gate__Bash" → "Bash") */
  function stripMcpPrefix(name) {
    const prefix = 'mcp__den-gate__';
    return name.startsWith(prefix) ? name.slice(prefix.length) : name;
  }

  function handlePermissionRequest(event) {
    const toolName = event.tool || 'Unknown';
    const requestId = event.request_id;

    // Auto-allow if user previously chose "Auto-allow" for this tool
    if (autoAllowedTools.has(toolName)) {
      sendPermissionResponse(requestId, true);
      appendSystem(`Auto-allowed: ${toolName}`);
      return;
    }

    appendPermissionDialog(requestId, toolName, event.input || {});
    ensureNotificationPermission().then(() => {
      showNotification(`Permission: ${toolName}`, 'Approval required');
    });
  }

  function appendPermissionDialog(requestId, toolName, toolInput) {
    currentAssistantBubble = null;
    currentThinkingBlock = null;

    const card = document.createElement('div');
    card.className = 'chat-msg chat-tool chat-permission-dialog';
    card.dataset.requestId = requestId;

    const header = document.createElement('div');
    header.className = 'chat-permission-header';
    header.textContent = `${toolName} — Approval Required`;
    card.appendChild(header);

    const inputPre = document.createElement('pre');
    inputPre.className = 'chat-tool-input';
    inputPre.textContent = JSON.stringify(toolInput, null, 2);
    card.appendChild(inputPre);

    const actions = document.createElement('div');
    actions.className = 'chat-permission-actions';

    const allowBtn = document.createElement('button');
    allowBtn.className = 'chat-notification-btn chat-permission-allow';
    allowBtn.textContent = 'Allow';
    allowBtn.addEventListener('click', () => {
      resolvePermission(card, requestId, true);
    });

    const denyBtn = document.createElement('button');
    denyBtn.className = 'chat-notification-btn chat-permission-deny';
    denyBtn.textContent = 'Deny';
    denyBtn.addEventListener('click', () => {
      resolvePermission(card, requestId, false);
    });

    const autoAllowBtn = document.createElement('button');
    autoAllowBtn.className = 'chat-notification-btn primary';
    autoAllowBtn.textContent = 'Auto-allow';
    autoAllowBtn.addEventListener('click', () => {
      autoAllowedTools.add(toolName);
      resolvePermission(card, requestId, true);
    });

    actions.appendChild(allowBtn);
    actions.appendChild(denyBtn);
    actions.appendChild(autoAllowBtn);
    card.appendChild(actions);

    messagesEl.appendChild(card);
    scrollToBottom();
  }

  function resolvePermission(card, requestId, allowed) {
    // Guard against double-click
    if (card.dataset.resolved) return;
    card.dataset.resolved = 'true';

    sendPermissionResponse(requestId, allowed);
    card.classList.add(allowed ? 'chat-permission-allowed' : 'chat-permission-denied');
    const status = document.createElement('div');
    status.className = 'chat-permission-status';
    status.textContent = allowed ? 'Allowed' : 'Denied';
    const actions = card.querySelector('.chat-permission-actions');
    if (actions) actions.remove();
    card.appendChild(status);
  }

  function sendPermissionResponse(requestId, allowed) {
    if (!ws || ws.readyState !== WebSocket.OPEN) {
      appendSystem('Cannot send permission response: connection lost.');
      return;
    }
    ws.send(JSON.stringify({ type: 'permission_response', request_id: requestId, allowed }));
  }

  function extractToolResultText(item) {
    let text = '';
    if (typeof item.content === 'string') {
      text = item.content;
    } else if (Array.isArray(item.content)) {
      text = item.content
        .filter((c) => c.type === 'text')
        .map((c) => c.text)
        .join('\n');
    }
    if (text.length > 2000) {
      text = text.substring(0, 2000) + '\n... (truncated)';
    }
    return text;
  }

  function appendToolResult(item) {
    const toolEl = document.getElementById('tool-' + item.tool_use_id);
    if (!toolEl) return;

    // Update group summary if tool errored
    if (item.is_error) {
      const group = toolEl.closest('.chat-tool-group');
      if (group) {
        group._errorCount = (group._errorCount || 0) + 1;
        updateToolGroupLabel(group);
      }
    }

    const toolInfo = toolInputMap.get(item.tool_use_id);
    const text = extractToolResultText(item);

    // Try diff rendering for Edit/Write/MultiEdit
    if (toolInfo && !item.is_error) {
      const diffEl = renderDiffResult(toolInfo);
      if (diffEl) {
        toolEl.appendChild(diffEl);
        // F005: append tool result text in a collapsible details
        if (text) {
          const details = document.createElement('details');
          details.className = 'chat-tool-result';
          const summary = document.createElement('summary');
          summary.textContent = 'Tool output';
          details.appendChild(summary);
          const pre = document.createElement('pre');
          pre.textContent = text;
          details.appendChild(pre);
          toolEl.appendChild(details);
        }
        toolInputMap.delete(item.tool_use_id);
        scrollToBottom();
        return;
      }
    }

    // Fallback: plain text rendering
    const resultDiv = document.createElement('div');
    resultDiv.className = 'chat-tool-result' + (item.is_error ? ' chat-tool-error' : '');
    const pre = document.createElement('pre');
    pre.textContent = text;
    resultDiv.appendChild(pre);
    toolEl.appendChild(resultDiv);
    if (toolInfo) toolInputMap.delete(item.tool_use_id);
    scrollToBottom();
  }

  function renderDiffResult(toolInfo) {
    const { name, input } = toolInfo;
    if (!input) return null;

    const container = document.createElement('div');
    container.className = 'chat-diff-viewer';

    if (name === 'Edit' && input.old_string != null && input.new_string != null) {
      if (!appendDiffHunk(container, input.file_path, input.old_string, input.new_string)) {
        return null; // old === new, fall back to plain text
      }
    } else if (name === 'MultiEdit' && Array.isArray(input.edits)) {
      let anyDiff = false;
      for (const edit of input.edits) {
        if (edit.old_string != null && edit.new_string != null) {
          if (appendDiffHunk(container, input.file_path, edit.old_string, edit.new_string)) {
            anyDiff = true;
          }
        }
      }
      if (!anyDiff) return null;
    } else if (name === 'Write' && input.file_path) {
      appendDiffFileHeader(container, input.file_path);
      const info = document.createElement('div');
      info.className = 'chat-diff-separator';
      info.textContent = 'File written';
      container.appendChild(info);
    } else {
      return null;
    }

    return container.children.length > 0 ? container : null;
  }

  function appendDiffFileHeader(container, filePath) {
    if (!filePath) return;
    const header = document.createElement('div');
    header.className = 'chat-diff-file-header';
    header.textContent = filePath;
    header.title = 'Open in Files tab';
    header.addEventListener('click', () => {
      if (window.FilerEditor) window.FilerEditor.openFile(filePath);
      if (window.DenApp) window.DenApp.switchTab('filer');
    });
    container.appendChild(header);
  }

  const MAX_DIFF_LINES = 200;

  function appendDiffHunk(container, filePath, oldStr, newStr) {
    // F008: skip diff if old === new (no actual change)
    if (oldStr === newStr) return false;

    appendDiffFileHeader(container, filePath);

    const diffLines = computeLineDiff(oldStr, newStr);
    const hunk = document.createElement('div');
    hunk.className = 'chat-diff-hunk';

    const limit = Math.min(diffLines.length, MAX_DIFF_LINES);
    for (let i = 0; i < limit; i++) {
      const line = diffLines[i];
      const div = document.createElement('div');
      div.className = 'chat-diff-line ' + line.type;
      const prefix = line.type === 'add' ? '+' : line.type === 'del' ? '-' : ' ';
      div.textContent = prefix + line.text;
      hunk.appendChild(div);
    }

    if (diffLines.length > MAX_DIFF_LINES) {
      const trunc = document.createElement('div');
      trunc.className = 'chat-diff-separator';
      trunc.textContent = `... ${diffLines.length - MAX_DIFF_LINES} more lines omitted`;
      hunk.appendChild(trunc);
    }

    container.appendChild(hunk);
    return true;
  }

  function splitLines(s) {
    if (s === '') return [];
    return s.split('\n');
  }

  function computeLineDiff(oldStr, newStr) {
    const oldLines = splitLines(oldStr);
    const newLines = splitLines(newStr);

    // Find common prefix lines
    let prefixLen = 0;
    while (prefixLen < oldLines.length && prefixLen < newLines.length
           && oldLines[prefixLen] === newLines[prefixLen]) {
      prefixLen++;
    }

    // Find common suffix lines
    let suffixLen = 0;
    while (suffixLen < oldLines.length - prefixLen
           && suffixLen < newLines.length - prefixLen
           && oldLines[oldLines.length - 1 - suffixLen] === newLines[newLines.length - 1 - suffixLen]) {
      suffixLen++;
    }

    const result = [];
    const ctxBefore = 3;
    const ctxAfter = 3;

    // Context before
    const ctxStart = Math.max(0, prefixLen - ctxBefore);
    for (let i = ctxStart; i < prefixLen; i++) {
      result.push({ type: 'ctx', text: oldLines[i] });
    }

    // Deleted lines
    for (let i = prefixLen; i < oldLines.length - suffixLen; i++) {
      result.push({ type: 'del', text: oldLines[i] });
    }

    // Added lines
    for (let i = prefixLen; i < newLines.length - suffixLen; i++) {
      result.push({ type: 'add', text: newLines[i] });
    }

    // Context after
    const ctxEnd = Math.min(oldLines.length, oldLines.length - suffixLen + ctxAfter);
    for (let i = oldLines.length - suffixLen; i < ctxEnd; i++) {
      result.push({ type: 'ctx', text: oldLines[i] });
    }

    return result;
  }

  // ── File attachment ──

  function handleAttachClick() {
    // F003: Block attach when Filer is in SFTP mode (paths are remote, not readable locally)
    if (typeof FilerRemote !== 'undefined' && FilerRemote.isRemote()) {
      const info = FilerRemote.getInfo();
      if (info.mode === 'sftp') {
        appendSystem('Cannot attach files from SFTP remote. Only local or Den-connected files are supported.');
        return;
      }
    }
    // Try to get the currently selected file from the filer tree, or active editor file
    let path = null;
    if (typeof FilerTree !== 'undefined') path = FilerTree.getSelectedPath();
    if (!path && typeof FilerEditor !== 'undefined') path = FilerEditor.getActivePath();
    if (path) {
      addAttachedFile(path);
    }
  }

  function addAttachedFile(filePath) {
    if (attachedFiles.includes(filePath)) return;
    attachedFiles.push(filePath);
    renderAttachChips();
  }

  function removeAttachedFile(filePath) {
    const idx = attachedFiles.indexOf(filePath);
    if (idx >= 0) attachedFiles.splice(idx, 1);
    renderAttachChips();
  }

  function clearAttachedFiles() {
    attachedFiles.length = 0;
    renderAttachChips();
  }

  function renderAttachChips() {
    if (!attachChipsEl) return;
    attachChipsEl.innerHTML = '';
    if (attachedFiles.length === 0) {
      attachChipsEl.hidden = true;
      return;
    }
    attachChipsEl.hidden = false;
    for (const fp of attachedFiles) {
      const chip = document.createElement('span');
      chip.className = 'chat-attach-chip';

      const name = document.createElement('span');
      name.className = 'chat-attach-chip-name';
      name.textContent = fp.replace(/\\/g, '/').split('/').pop() || fp;
      name.title = fp;
      chip.appendChild(name);

      const btn = document.createElement('button');
      btn.className = 'chat-attach-chip-remove';
      btn.textContent = '\u00d7';
      btn.type = 'button';
      btn.setAttribute('aria-label', 'Remove ' + name.textContent);
      btn.addEventListener('click', () => removeAttachedFile(fp));
      chip.appendChild(btn);

      attachChipsEl.appendChild(chip);
    }
  }

  function initAttachDragDrop() {
    if (!inputAreaEl) return;
    inputAreaEl.addEventListener('dragover', (e) => {
      if (e.dataTransfer.types.includes('text/x-den-path') || hasImageFiles(e.dataTransfer)) {
        e.preventDefault();
        e.dataTransfer.dropEffect = 'copy';
        inputAreaEl.classList.add('drag-over');
      }
    });
    inputAreaEl.addEventListener('dragleave', (e) => {
      if (!inputAreaEl.contains(e.relatedTarget)) {
        inputAreaEl.classList.remove('drag-over');
      }
    });
    inputAreaEl.addEventListener('drop', (e) => {
      inputAreaEl.classList.remove('drag-over');
      // Den filer path drop
      const path = e.dataTransfer.getData('text/x-den-path');
      if (path) {
        e.preventDefault();
        addAttachedFile(path);
        return;
      }
      // Native image file drop
      if (e.dataTransfer.files && e.dataTransfer.files.length > 0) {
        const imageFiles = [...e.dataTransfer.files].filter(f => ALLOWED_IMAGE_TYPES.has(f.type));
        if (imageFiles.length > 0) {
          e.preventDefault();
          for (const file of imageFiles) addImageFile(file);
        }
      }
    });
  }

  const ALLOWED_IMAGE_TYPES = new Set([
    'image/png', 'image/jpeg', 'image/gif', 'image/webp',
  ]);
  const MAX_IMAGE_SIZE = 5 * 1024 * 1024; // 5 MB
  const MAX_IMAGES = 10;

  function hasImageFiles(dt) {
    if (!dt || !dt.types.includes('Files')) return false;
    // items API available in modern browsers
    if (dt.items) {
      for (const item of dt.items) {
        if (item.kind === 'file' && ALLOWED_IMAGE_TYPES.has(item.type)) return true;
      }
    }
    return dt.types.includes('Files'); // fallback: accept Files drops and filter on drop
  }

  function initImagePaste() {
    if (!inputEl) return;
    inputEl.addEventListener('paste', (e) => {
      const items = e.clipboardData && e.clipboardData.items;
      if (!items) return;
      for (const item of items) {
        if (item.kind === 'file' && ALLOWED_IMAGE_TYPES.has(item.type)) {
          e.preventDefault();
          const file = item.getAsFile();
          if (file) addImageFile(file);
        }
      }
    });
  }

  function addImageFile(file) {
    if (pendingImages.length >= MAX_IMAGES) {
      appendSystem('Maximum ' + MAX_IMAGES + ' images per message.');
      return;
    }
    if (file.size > MAX_IMAGE_SIZE) {
      appendSystem('Image too large: ' + file.name + ' (' + (file.size / 1048576).toFixed(1) + ' MB, max 5 MB)');
      return;
    }
    const reader = new FileReader();
    reader.onload = () => {
      // F003: Re-check limit in async callback to prevent TOCTOU race
      if (pendingImages.length >= MAX_IMAGES) return;
      // result is data:image/png;base64,xxxx
      const dataUrl = reader.result;
      const commaIdx = dataUrl.indexOf(',');
      const base64 = dataUrl.substring(commaIdx + 1);
      pendingImages.push({
        data: base64,
        mediaType: file.type,
        name: file.name || 'image',
        size: file.size,
      });
      renderImagePreviews();
    };
    reader.readAsDataURL(file);
  }

  function removeImage(index) {
    pendingImages.splice(index, 1);
    renderImagePreviews();
  }

  function clearPendingImages() {
    pendingImages.length = 0;
    renderImagePreviews();
  }

  function renderImagePreviews() {
    if (!imagePreviewEl) return;
    imagePreviewEl.innerHTML = '';
    if (pendingImages.length === 0) {
      imagePreviewEl.hidden = true;
      return;
    }
    imagePreviewEl.hidden = false;
    for (let i = 0; i < pendingImages.length; i++) {
      const img = pendingImages[i];
      const thumb = document.createElement('div');
      thumb.className = 'chat-image-thumb';

      const imgEl = document.createElement('img');
      imgEl.src = 'data:' + img.mediaType + ';base64,' + img.data;
      imgEl.alt = img.name;
      thumb.appendChild(imgEl);

      const removeBtn = document.createElement('button');
      removeBtn.className = 'chat-image-thumb-remove';
      removeBtn.textContent = '\u00d7';
      removeBtn.type = 'button';
      removeBtn.setAttribute('aria-label', 'Remove ' + img.name);
      const idx = i;
      removeBtn.addEventListener('click', () => removeImage(idx));
      thumb.appendChild(removeBtn);

      imagePreviewEl.appendChild(thumb);
    }
  }

  // ── Send message ──
  let autoRestartInFlight = false;
  async function sendMessage() {
    const text = inputEl.value.trim();
    const files = [...attachedFiles];
    const images = [...pendingImages];
    if (!text && files.length === 0 && images.length === 0) return;

    // Auto-restart: WS closed but we can resume with --continue (#75)
    if ((!ws || ws.readyState !== WebSocket.OPEN) && pendingClaudeSessionId) {
      if (autoRestartInFlight) return; // F001: prevent double invocation
      autoRestartInFlight = true;

      const claudeSid = pendingClaudeSessionId;
      pendingClaudeSessionId = null;
      explicitStop = false;
      isStreaming = true;
      updateInputState();

      // F001: invalidate any lingering old WS before opening a new one
      if (ws) {
        ws.onclose = null;
        ws.close();
        ws = null;
      }

      const base = getApiBase();
      try {
        const resp = await fetch(`${base}/chat/sessions`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          credentials: 'same-origin',
          body: JSON.stringify({
            resume_session_id: claudeSid,
            permission_gate: permissionGateEnabled,
          }),
        });
        if (!resp.ok) {
          const err = await resp.json().catch(() => ({}));
          appendSystem('Failed to restart: ' + (err.error || resp.statusText));
          pendingClaudeSessionId = claudeSid; // F002: restore for retry
          isStreaming = false;
          updateInputState();
          return;
        }
        const data = await resp.json();
        sessionId = data.id;
        connectWs();
        refreshSidebar();
        saveSessionToStorage();

        // Wait for WS to open, then send the message (F005: 10s timeout)
        await new Promise((resolve, reject) => {
          const origOpen = ws.onopen;
          const origClose = ws.onclose;
          const timer = setTimeout(() => {
            ws.onopen = origOpen;
            ws.onclose = origClose;
            reject(new Error('WS open timeout'));
          }, 10000);
          ws.onopen = (e) => {
            clearTimeout(timer);
            ws.onopen = origOpen;
            ws.onclose = origClose;
            if (origOpen) origOpen.call(ws, e);
            resolve();
          };
          ws.onclose = (e) => {
            clearTimeout(timer);
            reject(new Error('WS closed before open'));
            if (origClose) origClose.call(ws, e);
          };
        });

        // F002: only show message and clear input after successful send
        appendUserMessage(text, files, images);
        inputEl.value = '';
        clearAttachedFiles();
        clearPendingImages();
        resetInputHeight();
        const cmd = { type: 'message', text };
        if (files.length > 0) cmd.files = files;
        if (images.length > 0) cmd.images = images.map(img => ({ data: img.data, media_type: img.mediaType }));
        ws.send(JSON.stringify(cmd));
        currentAssistantBubble = null;
        currentThinkingBlock = null;
        inputEl.focus();
      } catch (e) {
        appendSystem('Failed to restart: ' + e.message);
        pendingClaudeSessionId = claudeSid; // F002: restore for retry
        isStreaming = false;
        updateInputState();
      } finally {
        autoRestartInFlight = false;
      }
      return;
    }

    if (!ws || ws.readyState !== WebSocket.OPEN) return;

    appendUserMessage(text, files, images);
    const cmd = { type: 'message', text };
    if (files.length > 0) cmd.files = files;
    if (images.length > 0) cmd.images = images.map(img => ({ data: img.data, media_type: img.mediaType }));
    ws.send(JSON.stringify(cmd));
    inputEl.value = '';
    clearAttachedFiles();
    clearPendingImages();
    resetInputHeight();
    isStreaming = true;
    currentAssistantBubble = null;
    currentThinkingBlock = null;
    updateInputState();
    inputEl.focus();
  }

  function updateInputState() {
    if (noSession) {
      sendBtn.textContent = 'Send';
      sendBtn.classList.remove('chat-stop-btn');
      sendBtn.disabled = true;
      inputEl.disabled = true;
      inputEl.placeholder = 'No active session \u2014 click + to start';
    } else if (viewingHistory) {
      sendBtn.textContent = 'Send';
      sendBtn.classList.remove('chat-stop-btn');
      sendBtn.disabled = true;
      inputEl.disabled = true;
      inputEl.placeholder = 'Viewing history \u2014 click \u25b6 to resume';
    } else if (isStreaming) {
      sendBtn.textContent = 'Stop';
      sendBtn.classList.add('chat-stop-btn');
      sendBtn.disabled = false;
      inputEl.disabled = false;
      inputEl.placeholder = '';
    } else {
      sendBtn.textContent = 'Send';
      sendBtn.classList.remove('chat-stop-btn');
      sendBtn.disabled = false;
      inputEl.disabled = false;
      inputEl.placeholder = '';
    }
  }

  let suppressScroll = false; // F004: suppress during bulk history replay
  function scrollToBottom() {
    if (suppressScroll) return;
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }

  // ── Textarea auto-resize (#60) ──
  function autoResizeInput() {
    inputEl.style.height = 'auto';
    const maxRows = 5;
    const lineHeight = parseInt(getComputedStyle(inputEl).lineHeight) || 20;
    const maxHeight = lineHeight * maxRows;
    inputEl.style.height = Math.min(inputEl.scrollHeight, maxHeight) + 'px';
  }

  function resetInputHeight() {
    inputEl.style.height = '';
  }

  // ── Code block copy buttons (#61) ──
  function injectCopyButtons(container) {
    for (const pre of container.querySelectorAll('pre')) {
      if (pre.querySelector('.code-copy-btn')) continue;
      const btn = document.createElement('button');
      btn.className = 'code-copy-btn';
      btn.textContent = 'Copy';
      btn.type = 'button';
      btn.addEventListener('click', () => {
        const code = pre.querySelector('code');
        const text = code ? code.textContent : pre.textContent;
        navigator.clipboard.writeText(text).then(() => {
          btn.textContent = '\u2713';
          setTimeout(() => { btn.textContent = 'Copy'; }, 1500);
        });
      });
      pre.style.position = 'relative';
      pre.appendChild(btn);
    }
  }

  // ── Bulk collapse/expand (#62) ──
  function toggleAllDetails(expand) {
    for (const d of messagesEl.querySelectorAll('details')) {
      d.open = expand;
    }
  }

  // ── Export conversation (#59) ──
  function exportConversation(format) {
    const msgs = messagesEl.querySelectorAll('.chat-msg');
    let content, filename, mime;

    if (format === 'json') {
      const items = [];
      for (const el of msgs) {
        const role = el.classList.contains('chat-user') ? 'user'
          : el.classList.contains('chat-assistant') ? 'assistant'
          : el.classList.contains('chat-system') ? 'system'
          : el.classList.contains('chat-cost') ? 'cost'
          : el.classList.contains('chat-thinking') ? 'thinking'
          : 'tool';
        items.push({ role, text: el.textContent });
      }
      content = JSON.stringify(items, null, 2);
      filename = `chat-${sessionId || 'unknown'}-${new Date().toISOString().slice(0, 10)}.json`;
      mime = 'application/json';
    } else {
      const lines = [];
      for (const el of msgs) {
        if (el.classList.contains('chat-user')) {
          lines.push('## User\n\n' + el.textContent + '\n');
        } else if (el.classList.contains('chat-assistant')) {
          lines.push('## Assistant\n\n' + (el._rawText || el.textContent) + '\n');
        } else if (el.classList.contains('chat-cost')) {
          lines.push('> ' + el.textContent + '\n');
        } else if (el.classList.contains('chat-system')) {
          lines.push('*' + el.textContent + '*\n');
        } else {
          lines.push('### Tool\n\n```\n' + el.textContent + '\n```\n');
        }
      }
      content = lines.join('\n---\n\n');
      filename = `chat-${sessionId || 'unknown'}-${new Date().toISOString().slice(0, 10)}.md`;
      mime = 'text/markdown';
    }

    const blob = new Blob([content], { type: mime });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    a.click();
    URL.revokeObjectURL(url);
  }

  // ── Mobile viewport handling ──
  function setupMobileViewport() {
    if (!window.visualViewport) return;
    const chatPane = document.getElementById('chat-pane');
    if (!chatPane) return;
    let rafId = null;
    const onViewportChange = () => {
      if (rafId) return;
      rafId = requestAnimationFrame(() => {
        rafId = null;
        if (chatPane.hidden) return;
        // Adjust pane height when keyboard is visible
        const vvHeight = window.visualViewport.height;
        const windowHeight = window.innerHeight;
        if (vvHeight < windowHeight * 0.8) {
          chatPane.style.height = vvHeight + 'px';
        } else {
          chatPane.style.height = '';
        }
        // Only scroll to bottom if user was already near the bottom
        const gap = messagesEl.scrollHeight - messagesEl.scrollTop - messagesEl.clientHeight;
        if (gap < 80) scrollToBottom();
      });
    };
    window.visualViewport.addEventListener('resize', onViewportChange);
    window.visualViewport.addEventListener('scroll', onViewportChange);
  }

  // ── Directory picker modal ──
  // State scoped to the picker lifecycle (reset on each open)
  let cwdPickerPath = '';
  let cwdPickerAbort = null; // AbortController for in-flight fetches
  let cwdPickerDrivesLoaded = false;

  function getFilerApiBase() {
    if (!chatRemoteId) return '/api/filer';
    if (chatRemoteType === 'relay') return `/api/relay/${chatRemoteId}/filer`;
    return `/api/remote/${chatRemoteId}/filer`;
  }

  // F001: Register listeners once in init, not per-open
  function initCwdPicker() {
    const modal = document.getElementById('chat-cwd-picker-modal');
    const cancelBtn = document.getElementById('cwd-picker-cancel');
    const selectBtn = document.getElementById('cwd-picker-select');
    const upBtn = document.getElementById('cwd-picker-up');
    const listEl = document.getElementById('cwd-picker-list');
    if (!modal || !cancelBtn || !selectBtn || !upBtn || !listEl) return;

    cancelBtn.addEventListener('click', closeCwdPicker);
    selectBtn.addEventListener('click', () => {
      const selected = listEl.querySelector('.cwd-picker-item.selected');
      const finalPath = selected ? selected.dataset.path : cwdPickerPath;
      cwdInput.value = finalPath;
      cwdBar.hidden = false;
      if (toolsBar) toolsBar.hidden = true;
      closeCwdPicker();
    });
    upBtn.addEventListener('click', () => {
      const parent = getParentDir(cwdPickerPath);
      if (parent && parent !== cwdPickerPath) loadCwdPickerDir(parent);
    });

    // Single click navigates into folder, double click selects and closes
    listEl.addEventListener('click', (e) => {
      const item = e.target.closest('.cwd-picker-item');
      if (!item) return;
      loadCwdPickerDir(item.dataset.path);
    });
    listEl.addEventListener('dblclick', (e) => {
      const item = e.target.closest('.cwd-picker-item');
      if (!item) return;
      cwdInput.value = item.dataset.path;
      cwdBar.hidden = false;
      if (toolsBar) toolsBar.hidden = true;
      closeCwdPicker();
    });
  }

  function openCwdPicker() {
    const modal = document.getElementById('chat-cwd-picker-modal');
    if (!modal) return;
    modal.hidden = false;
    cwdPickerPath = cwdInput.value.trim() || '~';
    cwdPickerDrivesLoaded = false;
    loadCwdPickerDir(cwdPickerPath);
  }

  function closeCwdPicker() {
    const modal = document.getElementById('chat-cwd-picker-modal');
    if (modal) modal.hidden = true;
    // F003: Cancel any in-flight fetch
    if (cwdPickerAbort) { cwdPickerAbort.abort(); cwdPickerAbort = null; }
  }

  async function loadCwdPickerDir(dirPath) {
    const listEl = document.getElementById('cwd-picker-list');
    const currentEl = document.getElementById('cwd-picker-current');
    if (!listEl || !currentEl) return;

    // F003: Abort previous in-flight request to prevent stale responses
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

      // Populate drives bar (once per open)
      if (!cwdPickerDrivesLoaded && data.drives && data.drives.length > 0) {
        cwdPickerDrivesLoaded = true;
        renderCwdPickerDrives(data.drives);
      } else if (!cwdPickerDrivesLoaded) {
        fetchCwdPickerDrives(data.path);
      }

      const dirs = (data.entries || []).filter(e => e.is_dir);
      if (dirs.length === 0) {
        listEl.innerHTML = '<div class="cwd-picker-empty">No subdirectories</div>';
      } else {
        listEl.innerHTML = '';
        const sep = data.path.includes('/') ? '/' : '\\';
        const base = data.path.replace(/[\\/]$/, '');
        for (const d of dirs) {
          listEl.appendChild(createPickerItem(d.name, base + sep + d.name, false));
        }
      }
    } catch (err) {
      if (err.name === 'AbortError') return; // Superseded by newer request
      listEl.innerHTML = '<div class="cwd-picker-empty">Error loading directory</div>';
    }
  }

  function createPickerItem(label, path, isDrive) {
    const item = document.createElement('div');
    item.className = 'cwd-picker-item';
    item.setAttribute('role', 'option');
    item.dataset.path = path;

    const icon = document.createElement('span');
    icon.className = 'cwd-picker-item-icon';
    icon.textContent = isDrive ? '\uD83D\uDCBF' : '\uD83D\uDCC1';

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

  // F006: Handle drive root boundary (e.g., "C:" → "C:\", "/" stays "/")
  function getParentDir(p) {
    const normalized = p.replace(/[\\/]+$/, '');
    const lastSep = Math.max(normalized.lastIndexOf('/'), normalized.lastIndexOf('\\'));
    if (lastSep < 0) return p; // No separator at all (e.g., "C:") — can't go higher
    if (lastSep === 0) return normalized.substring(0, 1); // Unix root "/"
    // Windows drive root: "C:\foo" → "C:\"
    if (normalized.length >= 2 && normalized[1] === ':' && lastSep === 2) {
      return normalized.substring(0, 3); // "C:\"
    }
    return normalized.substring(0, lastSep);
  }

  function prefillInput(text) {
    if (!inputEl) return;
    // Build code fence that won't collide with content (CommonMark)
    const maxRun = (text.match(/`+/g) || []).reduce((m, s) => Math.max(m, s.length), 2);
    const fence = '`'.repeat(maxRun + 1);
    const prefill = fence + '\n' + text + '\n' + fence + '\n';
    // Append to existing input instead of overwriting
    inputEl.value = inputEl.value ? inputEl.value + '\n' + prefill : prefill;
    inputEl.focus();
    autoResizeInput();
    inputEl.setSelectionRange(inputEl.value.length, inputEl.value.length);
  }

  // ── Public API ──
  return {
    init,
    toggleAllDetails,
    exportConversation,
    setRemote,
    prefillInput,
    attachFile: addAttachedFile,
  };
})();
