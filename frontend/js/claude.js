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
        break;

      case 'turn_completed':
        updateHeader(sessionId);
        setInputEnabled(true);
        break;

      case 'claude_event':
        handleClaudeEvent(sessionId, msg.event);
        break;

      case 'session_stopped':
        updateHeader(sessionId);
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
    header.append(connSpan, dirSpan, statusSpan);
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

  function scrollToBottom() {
    const container = document.getElementById('claude-messages');
    container.scrollTop = container.scrollHeight;
  }

  return { init };
})();
