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

  // Session search filter
  let searchFilter = '';

  // Cached session lists for client-side filtering
  let cachedActiveSessions = [];
  let cachedHistorySessions = [];

  // ── Tool notification state ──
  const MODIFYING_TOOLS = new Set([
    'Edit', 'Write', 'MultiEdit', 'Bash', 'NotebookEdit',
  ]);
  const autoDismissedTools = new Set();

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

    sendBtn.addEventListener('click', handleSendOrStop);
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
      refreshSidebar().then(() => createSession());
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
        switchToActiveSession(savedId);
        return;
      }
      localStorage.removeItem('chat-active-session');
    }
    await createSession();
  }

  function saveSessionToStorage() {
    if (sessionId) {
      localStorage.setItem('chat-active-session', sessionId);
    } else {
      localStorage.removeItem('chat-active-session');
    }
  }

  // ── Continue last session ──
  async function continueLastSession() {
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
    autoDismissedTools.clear();
    const base = getApiBase();
    const cwd = cwdInput ? cwdInput.value.trim() : '';
    const tools = getAllowedTools();

    try {
      const body = {};
      if (cwd) body.cwd = cwd;
      if (tools) body.allowed_tools = tools;

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
    };

    ws.onmessage = (e) => {
      handleEvent(e.data);
    };

    ws.onclose = () => {
      ws = null;
      isStreaming = false;
      currentSessionState = 'idle';
      autoDismissedTools.clear();
      if (pendingClaudeSessionId) {
        // Auto-restart available — keep input ready, no "ended" message
      } else if (explicitStop) {
        // F008: don't duplicate "Session stopped" from session_ended handler
      } else {
        appendSystem('Session ended.');
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
    if (ws) {
      ws.onclose = null;
      ws.close();
      ws = null;
    }
    sessionId = null;
    isStreaming = false;
    currentSessionState = 'idle';
    autoDismissedTools.clear();
    pendingClaudeSessionId = null;
    explicitStop = false;
    viewingHistory = false;
    updateInputState();
    // Don't clear localStorage here — only clear when session truly ends
  }

  function clearMessages() {
    if (messagesEl) messagesEl.innerHTML = '';
    currentAssistantBubble = null;
    currentThinkingBlock = null;
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
            if (MODIFYING_TOOLS.has(block.name)) {
              ensureNotificationPermission().then(() => {
                showNotification(`Tool: ${block.name}`, JSON.stringify(block.input || {}).substring(0, 100));
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

  function appendUserMessage(text) {
    const el = document.createElement('div');
    el.className = 'chat-msg chat-user';
    el.textContent = text;
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

    const isModifying = MODIFYING_TOOLS.has(block.name);
    const isDismissed = autoDismissedTools.has(block.name);

    if (isModifying && !isDismissed) {
      appendToolNotification(block);
    } else {
      appendToolBlock(block);
    }
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

    messagesEl.appendChild(details);
    scrollToBottom();
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

  function appendToolResult(item) {
    const toolEl = document.getElementById('tool-' + item.tool_use_id);
    if (toolEl) {
      const resultDiv = document.createElement('div');
      resultDiv.className = 'chat-tool-result' + (item.is_error ? ' chat-tool-error' : '');
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
      const pre = document.createElement('pre');
      pre.textContent = text;
      resultDiv.appendChild(pre);
      toolEl.appendChild(resultDiv);
      scrollToBottom();
    }
  }

  // ── Send message ──
  let autoRestartInFlight = false;
  async function sendMessage() {
    const text = inputEl.value.trim();
    if (!text) return;

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
          body: JSON.stringify({ resume_session_id: claudeSid }),
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
        appendUserMessage(text);
        inputEl.value = '';
        resetInputHeight();
        ws.send(JSON.stringify({ type: 'message', text }));
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

    appendUserMessage(text);
    ws.send(JSON.stringify({ type: 'message', text }));
    inputEl.value = '';
    resetInputHeight();
    isStreaming = true;
    currentAssistantBubble = null;
    currentThinkingBlock = null;
    updateInputState();
    inputEl.focus();
  }

  function updateInputState() {
    if (viewingHistory) {
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

    // F004: Event delegation for directory item click/dblclick
    listEl.addEventListener('click', (e) => {
      const item = e.target.closest('.cwd-picker-item');
      if (!item) return;
      for (const el of listEl.querySelectorAll('.cwd-picker-item.selected')) {
        el.classList.remove('selected');
      }
      item.classList.add('selected');
    });
    listEl.addEventListener('dblclick', (e) => {
      const item = e.target.closest('.cwd-picker-item');
      if (!item) return;
      loadCwdPickerDir(item.dataset.path);
    });
  }

  function openCwdPicker() {
    const modal = document.getElementById('chat-cwd-picker-modal');
    if (!modal) return;
    modal.hidden = false;
    cwdPickerPath = cwdInput.value.trim() || '~';
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

      const dirs = (data.entries || []).filter(e => e.is_dir);

      // Prepend drives if available (Windows root)
      if (data.drives && data.drives.length > 0 && !data.parent) {
        listEl.innerHTML = '';
        for (const drive of data.drives) {
          listEl.appendChild(createPickerItem(drive, drive, true));
        }
        for (const d of dirs) {
          const fullPath = data.path.replace(/[\\/]$/, '') + '\\' + d.name;
          listEl.appendChild(createPickerItem(d.name, fullPath, false));
        }
      } else if (dirs.length === 0) {
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

  // ── Public API ──
  return {
    init,
    toggleAllDetails,
    exportConversation,
    setRemote,
  };
})();
