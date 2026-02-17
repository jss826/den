// Den - Claude チャット UI メインモジュール
const DenClaude = (() => {
  let initialized = false;
  // セッションごとのメッセージ履歴 { sessionId: [element, ...] }
  let messageHistory = {};

  function init(token) {
    if (initialized) return;
    initialized = true;

    ClaudeSession.init(token, handleEvent);
    bindInput();
  }

  function handleEvent(sessionId, msg) {
    switch (msg.type) {
      case 'session_created':
        messageHistory[sessionId] = [];
        showSession(sessionId);
        updateHeader(sessionId);
        break;

      case 'turn_started':
        updateHeader(sessionId);
        setInputEnabled(false);
        appendThinkingIndicator(sessionId);
        break;

      case 'turn_completed':
        removeThinkingIndicator();
        updateHeader(sessionId);
        setInputEnabled(true);
        break;

      case 'claude_event':
        handleClaudeEvent(sessionId, msg.event);
        break;

      case 'session_stopped':
        updateHeader(sessionId);
        break;

      case 'process_died':
        removeThinkingIndicator();
        appendSystemMessage(sessionId, 'Process ended');
        updateHeader(sessionId);
        setInputEnabled(false);
        break;

      case 'session_closed':
        delete messageHistory[sessionId];
        if (!ClaudeSession.getActiveSessionId()) {
          document.getElementById('claude-messages').innerHTML = '';
          document.getElementById('claude-header').innerHTML = '<span class="header-hint">Start a new session</span>';
          setInputEnabled(false);
        } else {
          showSession(ClaudeSession.getActiveSessionId());
          updateHeader(ClaudeSession.getActiveSessionId());
        }
        break;

      case 'switch_session':
        showSession(sessionId);
        updateHeader(sessionId);
        break;

      case 'replay_session':
        replaySession(msg.meta, msg.events);
        break;

      case 'error':
        appendError(msg.message);
        break;
    }
  }

  function handleClaudeEvent(sessionId, eventStr) {
    removeThinkingIndicator();
    const event = ClaudeParser.parse(eventStr);
    if (!event) return;

    const element = ClaudeParser.renderEvent(event);
    if (!element) return;

    // ツール結果は既存要素に追記されるので、fragmentの場合は記録不要
    if (element instanceof DocumentFragment) return;

    if (!messageHistory[sessionId]) {
      messageHistory[sessionId] = [];
    }
    messageHistory[sessionId].push(element);

    // アクティブセッションなら表示
    if (sessionId === ClaudeSession.getActiveSessionId()) {
      const container = document.getElementById('claude-messages');
      container.appendChild(element);
      scrollToBottom();
    }
  }

  function showSession(sessionId) {
    const container = document.getElementById('claude-messages');
    container.innerHTML = '';

    // セッション状態に応じて入力を有効化/無効化
    const session = ClaudeSession.getSession(sessionId);
    const canInput = session && (session.status === 'idle' || session.status === 'running');
    setInputEnabled(canInput && session.status !== 'running');

    const history = messageHistory[sessionId] || [];
    for (const el of history) {
      container.appendChild(el);
    }
    scrollToBottom();
  }

  function setInputEnabled(enabled) {
    document.getElementById('claude-input').disabled = !enabled;
    document.getElementById('claude-send').disabled = !enabled;
  }

  function updateHeader(sessionId) {
    const header = document.getElementById('claude-header');
    const session = ClaudeSession.getSession(sessionId);
    if (!session) {
      header.innerHTML = '<span class="header-hint">Start a new session</span>';
      return;
    }

    const connLabel = session.connection.type === 'local' ? 'Local' : session.connection.host;
    const statusClass = session.status === 'running' ? 'running' : 'done';
    header.innerHTML = '';
    const connSpan = document.createElement('span');
    connSpan.className = 'header-conn';
    connSpan.textContent = connLabel;
    const dirSpan = document.createElement('span');
    dirSpan.className = 'header-dir';
    dirSpan.textContent = session.dir;
    const statusSpan = document.createElement('span');
    statusSpan.className = 'header-status ' + statusClass;
    statusSpan.textContent = session.status;

    const exportBtn = document.createElement('button');
    exportBtn.className = 'header-export-btn';
    exportBtn.innerHTML = DenIcons.download(14);
    exportBtn.title = 'Export as Markdown';
    exportBtn.addEventListener('click', () => exportSession(sessionId));

    header.append(connSpan, dirSpan, statusSpan, exportBtn);
  }

  function appendError(message) {
    const container = document.getElementById('claude-messages');
    const div = document.createElement('div');
    div.className = 'msg msg-error';
    div.textContent = message;
    container.appendChild(div);
    scrollToBottom();
  }

  function appendSystemMessage(sessionId, message) {
    const container = document.getElementById('claude-messages');
    const div = document.createElement('div');
    div.className = 'msg msg-system';
    div.textContent = message;
    container.appendChild(div);
    if (!messageHistory[sessionId]) messageHistory[sessionId] = [];
    messageHistory[sessionId].push(div);
    scrollToBottom();
  }

  function bindInput() {
    const input = document.getElementById('claude-input');
    const sendBtn = document.getElementById('claude-send');

    sendBtn.addEventListener('click', sendMessage);

    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        sendMessage();
      }
    });

    // コードブロック コピーボタン（イベント委譲）
    const container = document.getElementById('claude-messages');
    container.addEventListener('click', (e) => {
      if (!e.target.classList.contains('code-copy-btn')) return;
      const wrapper = e.target.closest('.code-block-wrapper');
      if (!wrapper) return;
      const code = wrapper.querySelector('code');
      if (!code) return;
      navigator.clipboard.writeText(code.textContent).then(() => {
        Toast.success('Copied!');
        e.target.textContent = 'Copied!';
        setTimeout(() => { e.target.textContent = 'Copy'; }, 2000);
      }).catch(() => {
        Toast.error('Copy failed');
      });
    });
  }

  function sendMessage() {
    const input = document.getElementById('claude-input');
    const prompt = input.value.trim();
    if (!prompt) return;

    const sessionId = ClaudeSession.getActiveSessionId();
    if (!sessionId) {
      // アクティブセッションがなければモーダルを開く
      ClaudeSession.openModal();
      return;
    }

    const session = ClaudeSession.getSession(sessionId);
    if (!session) {
      ClaudeSession.openModal();
      return;
    }

    // 処理中の場合は Toast で通知
    if (session.status === 'running') {
      Toast.info('Processing a previous prompt...');
      return;
    }

    // completed/stopped の場合はモーダルで新規セッション
    if (session.status === 'completed' || session.status === 'stopped') {
      ClaudeSession.openModal();
      return;
    }

    // idle の場合は send_prompt で新ターンを開始
    // ユーザーメッセージを表示
    const container = document.getElementById('claude-messages');
    const userMsg = document.createElement('div');
    userMsg.className = 'msg msg-user';
    userMsg.textContent = prompt;
    container.appendChild(userMsg);

    if (!messageHistory[sessionId]) messageHistory[sessionId] = [];
    messageHistory[sessionId].push(userMsg);

    ClaudeSession.sendPrompt(sessionId, prompt);
    input.value = '';
    setInputEnabled(false);
    scrollToBottom();
  }

  function replaySession(meta, events) {
    const container = document.getElementById('claude-messages');
    container.innerHTML = '';

    // ヘッダーを読み取り専用で更新
    const header = document.getElementById('claude-header');
    const connLabel = meta.connection?.type === 'local' ? 'Local' : (meta.connection?.host || '?');
    header.innerHTML = '';
    const connSpan = document.createElement('span');
    connSpan.className = 'header-conn';
    connSpan.textContent = connLabel;
    const dirSpan = document.createElement('span');
    dirSpan.className = 'header-dir';
    dirSpan.textContent = meta.working_dir;
    const statusSpan = document.createElement('span');
    statusSpan.className = 'header-status';
    statusSpan.textContent = meta.status;
    const replaySpan = document.createElement('span');
    replaySpan.className = 'header-replay';
    replaySpan.textContent = 'replay';
    header.append(connSpan, dirSpan, statusSpan, replaySpan);

    // 入力エリアを無効化
    setInputEnabled(false);

    // イベントを順番にレンダリング
    for (const eventStr of events) {
      const event = ClaudeParser.parse(eventStr);
      if (!event) continue;
      const element = ClaudeParser.renderEvent(event);
      if (!element) continue;
      if (element instanceof DocumentFragment) continue;
      container.appendChild(element);
    }
    scrollToBottom();
  }

  function appendThinkingIndicator(sessionId) {
    if (sessionId !== ClaudeSession.getActiveSessionId()) return;
    removeThinkingIndicator();
    const container = document.getElementById('claude-messages');
    const indicator = document.createElement('div');
    indicator.className = 'claude-thinking';
    indicator.innerHTML = '<div class="spinner-ring"></div><span>Thinking...</span>';
    container.appendChild(indicator);
    scrollToBottom();
  }

  function removeThinkingIndicator() {
    const container = document.getElementById('claude-messages');
    const indicator = container.querySelector('.claude-thinking');
    if (indicator) indicator.remove();
  }

  async function exportSession(sessionId) {
    try {
      const [metaResp, eventsResp] = await Promise.all([
        fetch(`/api/sessions/${sessionId}`, {
          headers: { 'Authorization': `Bearer ${Auth.getToken()}` },
        }),
        fetch(`/api/sessions/${sessionId}/events`, {
          headers: { 'Authorization': `Bearer ${Auth.getToken()}` },
        }),
      ]);
      if (!metaResp.ok || !eventsResp.ok) {
        Toast.error('Failed to load session data');
        return;
      }
      const meta = await metaResp.json();
      const events = await eventsResp.json();
      const md = eventsToMarkdown(meta, events);
      const date = new Date(meta.created_at).toISOString().slice(0, 10);
      downloadText(md, `claude-session-${date}-${sessionId.slice(0, 8)}.md`);
    } catch {
      Toast.error('Export failed');
    }
  }

  function eventsToMarkdown(meta, events) {
    const lines = [];
    const connLabel = meta.connection?.type === 'local' ? 'Local' : (meta.connection?.host || '?');
    const date = new Date(meta.created_at).toLocaleString();
    const cost = meta.total_cost != null ? `$${meta.total_cost.toFixed(4)}` : '-';
    const duration = meta.duration_ms ? `${(meta.duration_ms / 1000).toFixed(1)}s` : '-';

    lines.push(`# Claude Session`);
    lines.push('');
    lines.push(`- **Date:** ${date}`);
    lines.push(`- **Connection:** ${connLabel}`);
    lines.push(`- **Working Directory:** ${meta.working_dir}`);
    lines.push(`- **Cost:** ${cost}`);
    lines.push(`- **Duration:** ${duration}`);
    lines.push(`- **Status:** ${meta.status}`);
    lines.push('');
    lines.push('---');
    lines.push('');

    for (const eventStr of events) {
      let event;
      try { event = JSON.parse(eventStr); } catch { continue; }

      if (event.type === 'user_prompt') {
        lines.push(`## User`);
        lines.push('');
        lines.push(event.prompt || '');
        lines.push('');
      } else if (event.type === 'assistant') {
        lines.push(`## Claude`);
        lines.push('');
        const contents = event.message?.content || [];
        for (const block of contents) {
          if (block.type === 'text') {
            lines.push(block.text);
            lines.push('');
          } else if (block.type === 'tool_use') {
            lines.push(`### Tool: ${block.name}`);
            lines.push('');
            lines.push('```json');
            lines.push(JSON.stringify(block.input, null, 2));
            lines.push('```');
            lines.push('');
          }
        }
      } else if (event.type === 'user') {
        const contents = event.message?.content || [];
        for (const block of contents) {
          if (block.type === 'tool_result') {
            const content = typeof block.content === 'string'
              ? block.content
              : JSON.stringify(block.content, null, 2);
            const truncated = content.length > 2000 ? content.slice(0, 2000) + '\n... (truncated)' : content;
            lines.push('<details>');
            lines.push(`<summary>Tool Result${block.is_error ? ' (Error)' : ''}</summary>`);
            lines.push('');
            lines.push('```');
            lines.push(truncated);
            lines.push('```');
            lines.push('');
            lines.push('</details>');
            lines.push('');
          }
        }
      } else if (event.type === 'result') {
        lines.push('---');
        lines.push('');
        const resultCost = event.total_cost_usd != null ? `$${event.total_cost_usd.toFixed(4)}` : '-';
        const turns = event.num_turns || 0;
        const dur = event.duration_ms ? `${(event.duration_ms / 1000).toFixed(1)}s` : '-';
        lines.push(`**Result:** ${event.is_error ? 'Error' : 'Done'} | ${turns} turns | ${dur} | ${resultCost}`);
        lines.push('');
      }
    }

    return lines.join('\n');
  }

  function downloadText(text, filename) {
    const blob = new Blob([text], { type: 'text/markdown;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
  }

  function scrollToBottom() {
    const container = document.getElementById('claude-messages');
    container.scrollTop = container.scrollHeight;
  }

  return { init };
})();
