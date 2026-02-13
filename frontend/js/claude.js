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

      case 'claude_event':
        handleClaudeEvent(sessionId, msg.event);
        break;

      case 'session_completed':
      case 'session_stopped':
        updateHeader(sessionId);
        break;

      case 'switch_session':
        showSession(sessionId);
        updateHeader(sessionId);
        break;

      case 'error':
        appendError(msg.message);
        break;
    }
  }

  function handleClaudeEvent(sessionId, eventStr) {
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

    const history = messageHistory[sessionId] || [];
    for (const el of history) {
      container.appendChild(el);
    }
    scrollToBottom();
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
    header.innerHTML = `<span class="header-conn">${connLabel}</span>
      <span class="header-dir">${session.dir}</span>
      <span class="header-status ${statusClass}">${session.status}</span>`;
  }

  function appendError(message) {
    const container = document.getElementById('claude-messages');
    const div = document.createElement('div');
    div.className = 'msg msg-error';
    div.textContent = message;
    container.appendChild(div);
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
    if (session && session.status !== 'running') {
      // セッション完了後は新規セッションとして扱う
      ClaudeSession.openModal();
      return;
    }

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
    scrollToBottom();
  }

  function scrollToBottom() {
    const container = document.getElementById('claude-messages');
    container.scrollTop = container.scrollHeight;
  }

  return { init };
})();
