// Den - Claude セッション管理 UI
const ClaudeSession = (() => {
  let ws = null;
  let sessions = {};         // { id: { id, connection, dir, status } }
  let activeSessionId = null;
  let selectedConnection = { type: 'local' };
  let currentDirPath = '~';
  let onEvent = null;        // コールバック: (sessionId, event) => void

  function init(token, eventCallback) {
    onEvent = eventCallback;
    connectWs(token);
    bindUI();
  }

  function connectWs(token) {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    ws = new WebSocket(`${proto}//${location.host}/api/claude/ws?token=${token}`);

    ws.onopen = () => {
      // SSH ホスト一覧を取得
      send({ type: 'get_ssh_hosts' });
    };

    ws.onmessage = (e) => {
      const msg = JSON.parse(e.data);
      handleMessage(msg);
    };

    ws.onclose = () => {
      // 5秒後に再接続
      setTimeout(() => connectWs(token), 5000);
    };
  }

  function send(obj) {
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(obj));
    }
  }

  function handleMessage(msg) {
    switch (msg.type) {
      case 'ssh_hosts':
        renderSshHosts(msg.hosts);
        break;

      case 'dir_list':
        renderDirList(msg.listing);
        break;

      case 'session_created':
        sessions[msg.session_id] = {
          id: msg.session_id,
          connection: msg.connection,
          dir: msg.dir,
          status: 'running',
        };
        activeSessionId = msg.session_id;
        renderSessionList();
        if (onEvent) onEvent(msg.session_id, msg);
        break;

      case 'claude_event':
        if (onEvent) onEvent(msg.session_id, msg);
        break;

      case 'session_completed':
        if (sessions[msg.session_id]) {
          sessions[msg.session_id].status = 'completed';
          renderSessionList();
        }
        if (onEvent) onEvent(msg.session_id, msg);
        break;

      case 'session_stopped':
        if (sessions[msg.session_id]) {
          sessions[msg.session_id].status = 'stopped';
          renderSessionList();
        }
        if (onEvent) onEvent(msg.session_id, msg);
        break;

      case 'error':
        if (onEvent) onEvent(null, msg);
        break;
    }
  }

  function bindUI() {
    // 新規セッションボタン
    document.getElementById('claude-new-session').addEventListener('click', openModal);
    document.getElementById('modal-cancel').addEventListener('click', closeModal);
    document.getElementById('modal-start').addEventListener('click', startSession);
    document.getElementById('dir-up').addEventListener('click', navigateUp);

    // モーダル外クリックで閉じる
    document.getElementById('claude-modal').addEventListener('click', (e) => {
      if (e.target.id === 'claude-modal') closeModal();
    });
  }

  function openModal() {
    document.getElementById('claude-modal').hidden = false;
    document.getElementById('modal-prompt').value = '';
    // 初期ディレクトリ取得
    currentDirPath = '~';
    send({ type: 'list_dirs', connection: selectedConnection, path: '~' });
  }

  function closeModal() {
    document.getElementById('claude-modal').hidden = true;
  }

  function startSession() {
    const prompt = document.getElementById('modal-prompt').value.trim();
    if (!prompt) return;

    send({
      type: 'start_session',
      connection: selectedConnection,
      dir: currentDirPath,
      prompt: prompt,
    });
    closeModal();
  }

  function selectConnection(conn) {
    selectedConnection = conn;
    // ボタンの active 状態更新
    document.querySelectorAll('.conn-btn').forEach(btn => btn.classList.remove('active'));
    event.target.classList.add('active');
    // ディレクトリ再取得
    currentDirPath = '~';
    send({ type: 'list_dirs', connection: conn, path: '~' });
  }

  function navigateUp() {
    send({ type: 'list_dirs', connection: selectedConnection, path: currentDirPath + '/..' });
  }

  function navigateDir(name) {
    const newPath = currentDirPath === '/' ? '/' + name : currentDirPath + '/' + name;
    send({ type: 'list_dirs', connection: selectedConnection, path: newPath });
  }

  function renderSshHosts(hosts) {
    const container = document.getElementById('modal-connections');
    // ローカルボタンは残す
    container.innerHTML = '<button class="conn-btn active" data-conn="local">Local</button>';
    container.querySelector('[data-conn="local"]').addEventListener('click', () => {
      selectConnection({ type: 'local' });
    });

    hosts.forEach(h => {
      const btn = document.createElement('button');
      btn.className = 'conn-btn';
      btn.textContent = h.name;
      btn.addEventListener('click', () => {
        selectConnection({ type: 'ssh', host: h.name });
      });
      container.appendChild(btn);
    });
  }

  function renderDirList(listing) {
    currentDirPath = listing.path;
    document.getElementById('dir-current').textContent = listing.path;

    const container = document.getElementById('dir-list');
    container.innerHTML = '';
    listing.entries.forEach(entry => {
      if (!entry.is_dir) return;
      const div = document.createElement('div');
      div.className = 'dir-item';
      div.textContent = entry.name;
      div.addEventListener('click', () => navigateDir(entry.name));
      container.appendChild(div);
    });
  }

  function renderSessionList() {
    const container = document.getElementById('claude-session-list');
    container.innerHTML = '';

    Object.values(sessions).forEach(s => {
      const div = document.createElement('div');
      div.className = 'session-item' + (s.id === activeSessionId ? ' active' : '');

      const connLabel = s.connection.type === 'local' ? 'Local' : s.connection.host;
      const shortDir = s.dir.split(/[/\\]/).pop() || s.dir;
      const statusDot = s.status === 'running' ? '●' : '○';

      div.innerHTML = `<span class="session-status">${statusDot}</span>
        <span class="session-info"><span class="session-name">${shortDir}</span>
        <span class="session-conn">${connLabel}</span></span>`;

      div.addEventListener('click', () => {
        activeSessionId = s.id;
        renderSessionList();
        if (onEvent) onEvent(s.id, { type: 'switch_session', session_id: s.id });
      });
      container.appendChild(div);
    });
  }

  function sendPrompt(sessionId, prompt) {
    send({ type: 'send_prompt', session_id: sessionId, prompt: prompt });
  }

  function stopSession(sessionId) {
    send({ type: 'stop_session', session_id: sessionId });
  }

  function getActiveSessionId() { return activeSessionId; }
  function getSession(id) { return sessions[id]; }

  return {
    init,
    sendPrompt,
    stopSession,
    getActiveSessionId,
    getSession,
    openModal,
  };
})();
