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
  let sessionSelect = null;
  let newBtn = null;
  let resumeBtn = null;
  let deleteBtn = null;
  let currentAssistantBubble = null;
  let currentThinkingBlock = null;
  let isStreaming = false;
  let renderPending = false;
  let thinkingRenderPending = false;

  // ── Tool notification state ──
  // Tools that modify files or run commands — shown with expanded notification card.
  // Note: In -p mode, claude CLI auto-executes tools. These cards are notifications,
  // not blocking approvals. The user can dismiss or auto-dismiss future notifications.
  const MODIFYING_TOOLS = new Set([
    'Edit', 'Write', 'MultiEdit', 'Bash', 'NotebookEdit',
  ]);
  const autoDismissedTools = new Set();

  // ── Push notification state ──
  let notificationsEnabled = false;

  // ── Init ──
  function init() {
    messagesEl = document.getElementById('chat-messages');
    inputEl = document.getElementById('chat-input');
    sendBtn = document.getElementById('chat-send');
    sessionSelect = document.getElementById('chat-session-select');
    newBtn = document.getElementById('chat-new-btn');
    resumeBtn = document.getElementById('chat-resume-btn');
    deleteBtn = document.getElementById('chat-delete-btn');

    sendBtn.addEventListener('click', sendMessage);
    inputEl.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        sendMessage();
      }
    });

    // Session bar controls
    newBtn.addEventListener('click', () => startNewSession());
    resumeBtn.addEventListener('click', resumeSelected);
    deleteBtn.addEventListener('click', deleteSelected);
    sessionSelect.addEventListener('change', onSessionSelectChange);

    // Request notification permission early
    requestNotificationPermission();

    // Handle visualViewport for mobile keyboard avoidance
    setupMobileViewport();

    // Load persisted sessions and auto-create a new one
    refreshSessionList().then(() => createSession());
  }

  // ── Push notifications ──
  function requestNotificationPermission() {
    if (!('Notification' in window)) return;
    if (Notification.permission === 'granted') {
      notificationsEnabled = true;
    } else if (Notification.permission !== 'denied') {
      // Will request on first relevant event
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
    // Notify if page is hidden OR chat tab is not active
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
      // Auto-close after 5 seconds
      setTimeout(() => n.close(), 5000);
    } catch {
      // Notification API may fail in some contexts
    }
  }

  // ── Session management ──
  async function refreshSessionList() {
    try {
      const resp = await fetch('/api/chat/history', { credentials: 'same-origin' });
      if (!resp.ok) return;
      const sessions = await resp.json();
      // Clear existing options except the first "New Session"
      while (sessionSelect.options.length > 1) {
        sessionSelect.remove(1);
      }
      for (const s of sessions) {
        const opt = document.createElement('option');
        opt.value = s.id;
        opt.dataset.claudeSessionId = s.claude_session_id || '';
        const date = new Date(s.created_at).toLocaleString();
        const msgs = s.message_count;
        opt.textContent = `${date} (${msgs} events)`;
        sessionSelect.appendChild(opt);
      }
    } catch {
      // Silently ignore
    }
  }

  function onSessionSelectChange() {
    const val = sessionSelect.value;
    resumeBtn.hidden = !val;
    deleteBtn.hidden = !val;
  }

  async function startNewSession() {
    sessionSelect.value = '';
    resumeBtn.hidden = true;
    deleteBtn.hidden = true;
    disconnectWs();
    clearMessages();
    await createSession();
  }

  async function resumeSelected() {
    const selectedOpt = sessionSelect.selectedOptions[0];
    if (!selectedOpt || !selectedOpt.value) return;

    const persistedId = selectedOpt.value;
    const claudeSessionId = selectedOpt.dataset.claudeSessionId;

    if (!claudeSessionId) {
      appendSystem('Cannot resume: no claude session ID found.');
      return;
    }

    disconnectWs();
    clearMessages();
    appendSystem('Resuming session...');

    // Load persisted history for display
    try {
      const histResp = await fetch(`/api/chat/history/${encodeURIComponent(persistedId)}`, {
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
      const resp = await fetch('/api/chat/sessions', {
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
      // Reset streaming state after history replay
      isStreaming = false;
      currentAssistantBubble = null;
      currentThinkingBlock = null;
      connectWs();
    } catch (e) {
      appendSystem('Failed to resume: ' + e.message);
    }
  }

  async function deleteSelected() {
    const selectedOpt = sessionSelect.selectedOptions[0];
    if (!selectedOpt || !selectedOpt.value) return;

    const persistedId = selectedOpt.value;
    try {
      await fetch(`/api/chat/history/${encodeURIComponent(persistedId)}`, {
        method: 'DELETE',
        credentials: 'same-origin',
      });
      selectedOpt.remove();
      sessionSelect.value = '';
      resumeBtn.hidden = true;
      deleteBtn.hidden = true;
    } catch {
      // Silently ignore
    }
  }

  async function createSession() {
    // F006: Reset auto-dismissed tools on new session
    autoDismissedTools.clear();

    try {
      const resp = await fetch('/api/chat/sessions', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'same-origin',
        body: '{}',
      });
      if (!resp.ok) {
        const err = await resp.json().catch(() => ({}));
        appendSystem('Failed to create chat session: ' + (err.error || resp.statusText));
        return;
      }
      const data = await resp.json();
      sessionId = data.id;
      connectWs();
    } catch (e) {
      appendSystem('Failed to create chat session: ' + e.message);
    }
  }

  // ── WebSocket ──
  function connectWs() {
    if (!sessionId) return;
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${proto}//${location.host}/api/chat/ws?session=${encodeURIComponent(sessionId)}`;
    ws = new WebSocket(url);

    ws.onopen = () => {
      appendSystem('Session started.');
    };

    ws.onmessage = (e) => {
      handleEvent(e.data);
    };

    ws.onclose = () => {
      appendSystem('Session ended.');
      ws = null;
      isStreaming = false;
      // F006: Clear auto-dismissed tools on disconnect
      autoDismissedTools.clear();
      updateInputState();
      // Refresh session list to show the newly persisted session
      refreshSessionList();
    };

    ws.onerror = () => {
      // onclose will fire after this
    };
  }

  function disconnectWs() {
    if (ws) {
      ws.onclose = null; // Prevent appendSystem('Session ended.')
      ws.close();
      ws = null;
    }
    sessionId = null;
    isStreaming = false;
    autoDismissedTools.clear();
    updateInputState();
  }

  function clearMessages() {
    if (messagesEl) messagesEl.innerHTML = '';
    currentAssistantBubble = null;
    currentThinkingBlock = null;
  }

  // ── Event handling ──
  function handleEvent(raw) {
    let event;
    try {
      event = JSON.parse(raw);
    } catch {
      return;
    }

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
        break;
      default:
        break;
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
            // Push notification for AskUserQuestion
            ensureNotificationPermission().then(() => {
              showNotification('Claude has a question', block.input?.question || 'Action required');
            });
          } else {
            appendToolUse(block);
            // Push notification for modifying tools
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
    // Tool results from Claude's tool execution
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
      appendCost(`$${cost} | in:${inp} out:${out} cached:${cached}`);
    }

    // Push notification for completion
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
          } else {
            currentAssistantBubble.textContent = currentAssistantBubble._rawText;
          }
          scrollToBottom();
        }
      });
    }
  }

  // F005: rAF batch rendering for thinking blocks (same pattern as appendAssistantText)
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
    // Finalize any current assistant bubble
    currentAssistantBubble = null;
    currentThinkingBlock = null;

    const isModifying = MODIFYING_TOOLS.has(block.name);
    const isDismissed = autoDismissedTools.has(block.name);

    // F001: Notification card (not approval — tools already executed in -p mode)
    if (isModifying && !isDismissed) {
      appendToolNotification(block);
    } else {
      appendToolBlock(block);
    }
  }

  /** Standard collapsible tool block (safe or auto-dismissed tools). */
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

  /**
   * F001: Notification card for modifying tools — shown expanded with dismiss buttons.
   * This is NOT a blocking approval. In -p mode, claude CLI auto-executes tools.
   * The card informs the user what was executed and lets them control future notifications.
   */
  function appendToolNotification(block) {
    const card = document.createElement('div');
    card.className = 'chat-msg chat-tool chat-tool-notification';
    card.id = 'tool-' + block.id;

    // Header
    const header = document.createElement('div');
    header.className = 'chat-tool-notification-header';
    header.textContent = block.name;
    card.appendChild(header);

    // Input preview
    const inputPre = document.createElement('pre');
    inputPre.className = 'chat-tool-input';
    inputPre.textContent = JSON.stringify(block.input, null, 2);
    card.appendChild(inputPre);

    // Action buttons
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

  /** Inline dialog for AskUserQuestion tool_use events. */
  function appendAskDialog(block) {
    currentAssistantBubble = null;
    currentThinkingBlock = null;

    const card = document.createElement('div');
    card.className = 'chat-msg chat-tool chat-ask-dialog';
    card.id = 'tool-' + block.id;
    // F003: Store tool_use_id on the card element for submitAskResponse
    card.dataset.toolUseId = block.id;

    // Question text
    const question = (block.input && block.input.question) || 'Claude has a question:';
    const questionEl = document.createElement('div');
    questionEl.className = 'chat-ask-question';
    questionEl.textContent = question;
    card.appendChild(questionEl);

    // Options (if provided)
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

    // Free-text input
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

  /** Submit an AskUserQuestion response via WebSocket. */
  function submitAskResponse(card, answer) {
    // F002: Only resolve UI if WS send succeeds
    if (!ws || ws.readyState !== WebSocket.OPEN) {
      appendSystem('Cannot send answer: connection lost.');
      return;
    }
    // F003: Include tool_use_id in the response
    const toolUseId = card.dataset.toolUseId || null;
    ws.send(JSON.stringify({ type: 'ask_response', text: answer, tool_use_id: toolUseId }));

    // Replace card content with resolved state
    card.classList.add('chat-ask-resolved');
    const resolved = document.createElement('div');
    resolved.className = 'chat-ask-answered';
    resolved.textContent = 'Answered: ' + answer;
    // Remove interactive elements
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
      // Extract text content from various formats
      let text = '';
      if (typeof item.content === 'string') {
        text = item.content;
      } else if (Array.isArray(item.content)) {
        text = item.content
          .filter((c) => c.type === 'text')
          .map((c) => c.text)
          .join('\n');
      }
      // Truncate very long results
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

  // ── Mobile viewport handling ──
  function setupMobileViewport() {
    if (!window.visualViewport) return;
    window.visualViewport.addEventListener('resize', () => {
      // Adjust chat layout when virtual keyboard appears/disappears
      const chatPane = document.getElementById('chat-pane');
      if (!chatPane || chatPane.hidden) return;
      const vvHeight = window.visualViewport.height;
      const windowHeight = window.innerHeight;
      // If viewport is significantly smaller than window, keyboard is likely showing
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
  };
})();
