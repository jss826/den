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
  let currentAssistantBubble = null;
  let currentThinkingBlock = null;
  let isStreaming = false;
  let renderPending = false;

  // ── Init ──
  function init() {
    messagesEl = document.getElementById('chat-messages');
    inputEl = document.getElementById('chat-input');
    sendBtn = document.getElementById('chat-send');

    sendBtn.addEventListener('click', sendMessage);
    inputEl.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        sendMessage();
      }
    });

    // Auto-create a session on first init
    createSession();
  }

  // ── Session management ──
  async function createSession() {
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
      updateInputState();
    };

    ws.onerror = () => {
      // onclose will fire after this
    };
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
          appendToolUse(block);
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
    // F005 + F009: correct API + rAF batch rendering
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
    scrollToBottom();
  }

  function appendToolUse(block) {
    // Finalize any current assistant bubble
    currentAssistantBubble = null;
    currentThinkingBlock = null;

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

  // ── Public API ──
  return {
    init,
  };
})();
