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
/* global DenMarkdown */
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

  // ── State ──
  let ws = null;
  let composing = false;
  let currentAssistantBubble = null;
  let renderPending = false;
  let pendingText = '';
  let activeSessionId = null;
  let sessions = []; // cached session list
  let pollTimer = null;

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
        newSessionModal.hidden = false;
      });
    }
    const cancelBtn = document.getElementById('chat-session-cancel');
    const createBtn = document.getElementById('chat-session-create');
    if (cancelBtn) cancelBtn.addEventListener('click', () => { newSessionModal.hidden = true; });
    if (createBtn) createBtn.addEventListener('click', handleCreateSession);

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
    // Pause when tab hidden
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
      label.title = `${s.permission_mode} — ${s.id}`;

      const badge = document.createElement('span');
      badge.className = 'chat-session-badge';
      badge.textContent = s.alive ? s.permission_mode : 'stopped';

      item.appendChild(label);
      item.appendChild(badge);
      item.addEventListener('click', () => switchSession(s.id));
      sessionListEl.appendChild(item);
    }

    // Update UI state based on active session
    const active = sessions.find(s => s.id === activeSessionId);
    updateInputState(!!active);
    if (stopBtn) stopBtn.hidden = !active;
  }

  async function handleCreateSession() {
    const mode = permissionModeSelect ? permissionModeSelect.value : 'default';
    newSessionModal.hidden = true;

    appendSystemMessage(`Creating session (${mode})...`);

    try {
      const resp = await fetch(`${getApiBase()}/channel/sessions`, {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ permission_mode: mode }),
      });

      if (!resp.ok) {
        const text = await resp.text();
        appendSystemMessage(`Failed to create session: ${text}`);
        return;
      }

      const session = await resp.json();
      appendSystemMessage(`Session created: ${session.id.slice(0, 8)}`);
      await fetchSessions();
      switchSession(session.id);
    } catch (err) {
      appendSystemMessage(`Error: ${err.message}`);
    }
  }

  async function handleStop() {
    if (!activeSessionId) return;
    const id = activeSessionId;

    try {
      await fetch(`${getApiBase()}/channel/sessions/${encodeURIComponent(id)}`, {
        method: 'DELETE',
        credentials: 'same-origin',
      });
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
      // Reconnect after delay if session still active
      setTimeout(() => {
        if (!ws && activeSessionId) connectWs();
      }, 3000);
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
    if (!activeSessionId) return;
    const text = inputEl.value.trim();
    if (!text) return;

    inputEl.value = '';
    inputEl.style.height = 'auto';

    // Show user message
    appendUserMessage(text);

    // Reset assistant bubble for new response
    currentAssistantBubble = null;
    pendingText = '';

    // Send via Channel API
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
        currentAssistantBubble.innerHTML = DenMarkdown.renderMarkdown(pendingText);
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

  function setRemote(remoteId, type) {
    chatRemoteId = remoteId || null;
    chatRemoteType = type || null;
    // Refetch sessions from new target + reconnect WS
    fetchSessions();
    connectWs();
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

  // ── Public API ──

  return {
    init,
    setRemote,
    prefillInput,
  };
})();
