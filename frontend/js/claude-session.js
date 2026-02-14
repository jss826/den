// Den - Claude セッション管理 UI
const ClaudeSession = (() => {
  let ws = null;
  let sessions = {};         // { id: { id, connection, dir, status } }
  let activeSessionId = null;
  let selectedConnection = { type: 'local' };
  let currentDirPath = '~';
  let currentDirParent = null; // 親ディレクトリ（サーバーレスポンスから取得）
  let onEvent = null;        // コールバック: (sessionId, event) => void
  let pendingSend = [];      // WebSocket 接続前のメッセージキュー

  function init(token, eventCallback) {
    onEvent = eventCallback;
    connectWs(token);
    bindUI();
    loadHistory();
  }

  function connectWs(token) {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    ws = new WebSocket(`${proto}//${location.host}/api/claude/ws?token=${token}`);

    ws.onopen = () => {
      // SSH ホスト一覧を取得
      send({ type: 'get_ssh_hosts' });
      // 接続前にキューされたメッセージを送信
      const pending = [...pendingSend];
      pendingSend = [];
      for (const msg of pending) {
        send(msg);
      }
    };

    ws.onmessage = (e) => {
      let msg;
      try {
        msg = JSON.parse(e.data);
      } catch {
        console.warn('Invalid JSON from WebSocket:', e.data);
        return;
      }
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
    } else {
      pendingSend.push(obj);
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
          prompt: msg.prompt || '',
          status: msg.status || 'idle',
        };
        activeSessionId = msg.session_id;
        renderSessionList();
        if (onEvent) onEvent(msg.session_id, msg);
        break;

      case 'turn_started':
        if (sessions[msg.session_id]) {
          sessions[msg.session_id].status = 'running';
          renderSessionList();
        }
        if (onEvent) onEvent(msg.session_id, msg);
        break;

      case 'turn_completed':
        if (sessions[msg.session_id]) {
          sessions[msg.session_id].status = 'idle';
          renderSessionList();
        }
        if (onEvent) onEvent(msg.session_id, msg);
        // 履歴を再取得
        loadHistory();
        break;

      case 'claude_event':
        if (onEvent) onEvent(msg.session_id, msg);
        break;

      case 'session_stopped':
        if (sessions[msg.session_id]) {
          sessions[msg.session_id].status = 'stopped';
          renderSessionList();
        }
        if (onEvent) onEvent(msg.session_id, msg);
        loadHistory();
        break;

      case 'error':
        if (onEvent) onEvent(null, msg);
        break;
    }
  }

  async function loadHistory() {
    await SessionHistory.load();
    const container = document.getElementById('claude-history-list');
    if (container) {
      SessionHistory.render(container);
    }
  }

  function bindUI() {
    // 新規セッションボタン
    document.getElementById('claude-new-session').addEventListener('click', openModal);
    document.getElementById('modal-cancel').addEventListener('click', closeModal);
    document.getElementById('modal-start').addEventListener('click', startSession);
    document.getElementById('dir-up').addEventListener('click', navigateUp);

    // パス直接入力
    document.getElementById('dir-go').addEventListener('click', () => {
      const input = document.getElementById('dir-path-input');
      const path = input.value.trim();
      if (path) navigateToPath(path);
    });
    document.getElementById('dir-path-input').addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        const path = e.target.value.trim();
        if (path) navigateToPath(path);
      }
    });

    // モーダル外クリックで閉じる
    document.getElementById('claude-modal').addEventListener('click', (e) => {
      if (e.target.id === 'claude-modal') closeModal();
    });

    // 履歴リプレイコールバック
    SessionHistory.setReplayCallback((meta, events) => {
      if (onEvent) onEvent(meta.id, {
        type: 'replay_session',
        session_id: meta.id,
        meta: meta,
        events: events,
      });
    });
  }

  function openModal() {
    document.getElementById('claude-modal').hidden = false;
    document.getElementById('modal-prompt').value = '';

    // デフォルト接続先を適用
    const defaultConn = DenSettings.get('claude_default_connection');
    if (defaultConn) {
      selectedConnection = defaultConn;
    }

    // デフォルトディレクトリを適用
    const defaultDir = DenSettings.get('claude_default_dir');
    currentDirPath = defaultDir || '~';

    send({ type: 'list_dirs', connection: selectedConnection, path: currentDirPath });
  }

  function closeModal() {
    document.getElementById('claude-modal').hidden = true;
  }

  function startSession() {
    // プロンプトは任意（空でも OK）
    const prompt = document.getElementById('modal-prompt').value.trim();

    send({
      type: 'start_session',
      connection: selectedConnection,
      dir: currentDirPath,
      prompt: prompt,
    });
    closeModal();
  }

  function selectConnection(conn, clickedBtn) {
    selectedConnection = conn;
    // ボタンの active 状態更新
    document.querySelectorAll('.conn-btn').forEach(btn => btn.classList.remove('active'));
    if (clickedBtn) clickedBtn.classList.add('active');
    // ディレクトリ再取得
    currentDirPath = '~';
    send({ type: 'list_dirs', connection: conn, path: '~' });
  }

  function navigateUp() {
    if (currentDirParent) {
      send({ type: 'list_dirs', connection: selectedConnection, path: currentDirParent });
    }
  }

  function navigateDir(name) {
    // Windows パスかどうかをカレントパスから判定
    const sep = currentDirPath.includes('\\') ? '\\' : '/';
    const newPath = currentDirPath.endsWith(sep)
      ? currentDirPath + name
      : currentDirPath + sep + name;
    send({ type: 'list_dirs', connection: selectedConnection, path: newPath });
  }

  function navigateToPath(path) {
    send({ type: 'list_dirs', connection: selectedConnection, path: path });
  }

  function renderSshHosts(hosts) {
    const container = document.getElementById('modal-connections');
    // ローカルボタンは残す
    container.innerHTML = '<button class="conn-btn active" data-conn="local">Local</button>';
    container.querySelector('[data-conn="local"]').addEventListener('click', (e) => {
      selectConnection({ type: 'local' }, e.currentTarget);
    });

    hosts.forEach(h => {
      const btn = document.createElement('button');
      btn.className = 'conn-btn';
      btn.textContent = h.name;
      btn.addEventListener('click', (e) => {
        selectConnection({ type: 'ssh', host: h.name }, e.currentTarget);
      });
      container.appendChild(btn);
    });
  }

  function renderDirList(listing) {
    currentDirPath = listing.path;
    currentDirParent = listing.parent || null;
    document.getElementById('dir-path-input').value = listing.path;

    // 親がない場合は上移動ボタンを無効化
    const upBtn = document.getElementById('dir-up');
    upBtn.disabled = !currentDirParent;
    upBtn.style.opacity = currentDirParent ? '1' : '0.4';

    // ドライブボタン（既存の drives コンテナがなければ作成）
    let drivesContainer = document.getElementById('modal-dir-drives');
    if (!drivesContainer) {
      drivesContainer = document.createElement('div');
      drivesContainer.id = 'modal-dir-drives';
      drivesContainer.className = 'dir-drives';
      const dirList = document.getElementById('dir-list');
      dirList.parentNode.insertBefore(drivesContainer, dirList);
    }
    drivesContainer.innerHTML = '';
    if (listing.drives && listing.drives.length > 0) {
      listing.drives.forEach(d => {
        const btn = document.createElement('button');
        btn.className = 'dir-drive-btn';
        btn.textContent = d;
        btn.addEventListener('click', () => navigateToPath(d));
        drivesContainer.appendChild(btn);
      });
    }

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

      // idle=○ running=● completed/stopped=—
      let statusDot;
      if (s.status === 'running') statusDot = '\u25CF';
      else if (s.status === 'idle') statusDot = '\u25CB';
      else statusDot = '\u2014';

      const statusSpan = document.createElement('span');
      statusSpan.className = 'session-status';
      statusSpan.textContent = statusDot;
      const infoSpan = document.createElement('span');
      infoSpan.className = 'session-info';
      const nameSpan = document.createElement('span');
      nameSpan.className = 'session-name';
      nameSpan.textContent = shortDir;
      nameSpan.title = s.dir;
      const connSpan = document.createElement('span');
      connSpan.className = 'session-conn';
      connSpan.textContent = connLabel;
      infoSpan.append(nameSpan, connSpan);

      if (s.prompt) {
        const previewSpan = document.createElement('span');
        previewSpan.className = 'session-prompt-preview';
        previewSpan.textContent = s.prompt.length > 30 ? s.prompt.slice(0, 30) + '...' : s.prompt;
        infoSpan.appendChild(previewSpan);
      }

      div.append(statusSpan, infoSpan);

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
