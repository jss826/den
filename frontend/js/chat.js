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
  let cwdToggle = null;
  let cwdInput = null;
  let cwdBar = null;
  let mainTitle = null;
  let currentAssistantBubble = null;
  let currentThinkingBlock = null;
  let isStreaming = false;
  let renderPending = false;
  let thinkingRenderPending = false;

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

    sendBtn.addEventListener('click', sendMessage);
    inputEl.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        sendMessage();
      }
    });
    inputEl.addEventListener('input', autoResizeInput);

    // Sidebar controls
    newBtn.addEventListener('click', () => startNewSession());
    cwdToggle.addEventListener('click', () => {
      cwdBar.hidden = !cwdBar.hidden;
      if (!cwdBar.hidden) cwdInput.focus();
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

    // Load sessions and auto-create a new one
    refreshSidebar().then(() => createSession());

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
    let activeSessions = [];
    let historySessions = [];

    try {
      const [activeResp, historyResp] = await Promise.all([
        fetch(`${base}/chat/sessions`, { credentials: 'same-origin' }),
        fetch(`${base}/chat/history`, { credentials: 'same-origin' }),
      ]);
      if (activeResp.ok) activeSessions = await activeResp.json();
      if (historyResp.ok) historySessions = await historyResp.json();
    } catch (e) {
      console.warn('refreshSidebar failed:', e); // F008
    }

    renderSessionList(activeSessions, historySessions);
  }

  function renderSessionList(active, history) {
    sessionListEl.innerHTML = '';

    // Active sessions section
    if (active.length > 0) {
      const header = document.createElement('div');
      header.className = 'chat-session-section';
      header.textContent = 'Active';
      sessionListEl.appendChild(header);

      for (const s of active) {
        sessionListEl.appendChild(createActiveSessionItem(s));
      }
    }

    // History section
    if (history.length > 0) {
      const header = document.createElement('div');
      header.className = 'chat-session-section';
      header.textContent = 'History';
      sessionListEl.appendChild(header);

      for (const s of history) {
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
    label.textContent = s.cwd ? shortenPath(s.cwd) : s.id.substring(0, 8);
    info.appendChild(label);

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
    label.textContent = `${date}`;
    info.appendChild(label);

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
    item.addEventListener('click', () => resumeSession(s.id, s.claude_session_id));
    return item;
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

  // ── Session actions ──
  async function startNewSession() {
    disconnectWs();
    clearMessages();
    await createSession();
  }

  function switchToActiveSession(id) {
    if (id === sessionId) return;
    disconnectWs();
    clearMessages();
    sessionId = id;
    connectWs();
    updateActiveHighlight();
    updateMainTitle();
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

  async function createSession() {
    autoDismissedTools.clear();
    const base = getApiBase();
    const cwd = cwdInput ? cwdInput.value.trim() : '';

    try {
      const body = {};
      if (cwd) body.cwd = cwd;

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
      appendSystem('Session ended.');
      ws = null;
      isStreaming = false;
      currentSessionState = 'idle';
      autoDismissedTools.clear();
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
    updateInputState();
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
        appendSystem('Claude process ended.');
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
      if (e.key === 'Enter') {
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
  function sendMessage() {
    const text = inputEl.value.trim();
    if (!text || !ws || ws.readyState !== WebSocket.OPEN) return;

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
    sendBtn.disabled = isStreaming;
  }

  function scrollToBottom() {
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
    window.visualViewport.addEventListener('resize', () => {
      const chatPane = document.getElementById('chat-pane');
      if (!chatPane || chatPane.hidden) return;
      const vvHeight = window.visualViewport.height;
      const windowHeight = window.innerHeight;
      if (vvHeight < windowHeight * 0.8) {
        chatPane.style.height = vvHeight + 'px';
      } else {
        chatPane.style.height = '';
      }
    });
  }

  // ── Public API ──
  return {
    init,
    toggleAllDetails,
    exportConversation,
    setRemote,
  };
})();
