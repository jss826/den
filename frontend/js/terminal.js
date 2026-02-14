// Den - ターミナルモジュール
const DenTerminal = (() => {
  let term = null;
  let fitAddon = null;
  let ws = null;
  let currentSession = 'default';
  let authToken = null;
  let connectGeneration = 0; // doConnect の世代カウンタ（高速切り替え時の race 防止）

  /** fit + refresh + resize 通知をまとめて実行 */
  let fitRetryCount = 0;
  function fitAndRefresh() {
    if (!term || !fitAddon) return;
    const container = term.element?.parentElement;
    if (container && container.clientWidth === 0) {
      if (fitRetryCount < 10) {
        fitRetryCount++;
        requestAnimationFrame(() => fitAndRefresh());
      }
      return;
    }
    fitRetryCount = 0;
    fitAddon.fit();
    term.refresh(0, term.rows - 1);
    sendResize();
  }

  function init(container) {
    term = new Terminal({
      cursorBlink: true,
      fontSize: 15,
      fontFamily: '"Cascadia Code", "Fira Code", "Source Code Pro", "Menlo", monospace',
      theme: {
        background: '#1a1b26',
        foreground: '#c0caf5',
        cursor: '#c0caf5',
        selectionBackground: '#33467c',
        black: '#15161e',
        red: '#f7768e',
        green: '#9ece6a',
        yellow: '#e0af68',
        blue: '#7aa2f7',
        magenta: '#bb9af7',
        cyan: '#7dcfff',
        white: '#a9b1d6',
        brightBlack: '#414868',
        brightRed: '#f7768e',
        brightGreen: '#9ece6a',
        brightYellow: '#e0af68',
        brightBlue: '#7aa2f7',
        brightMagenta: '#bb9af7',
        brightCyan: '#7dcfff',
        brightWhite: '#c0caf5',
      },
      allowProposedApi: true,
    });

    fitAddon = new FitAddon.FitAddon();
    term.loadAddon(fitAddon);

    // レンダラー選択: デスクトップ → WebGL、iOS/Safari → Canvas
    const isIOS = /iPad|iPhone|iPod/.test(navigator.userAgent)
      || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
    const isSafari = !isIOS && /^((?!chrome|android).)*safari/i.test(navigator.userAgent);
    if (!isIOS && !isSafari) {
      try {
        const webglAddon = new WebglAddon.WebglAddon();
        webglAddon.onContextLost(() => webglAddon.dispose());
        term.loadAddon(webglAddon);
      } catch (_e) {
        console.warn('WebGL not available, falling back to canvas renderer');
        try {
          term.loadAddon(new CanvasAddon.CanvasAddon());
        } catch (_e2) { /* DOM fallback */ }
      }
    } else {
      // iOS/Safari: Canvas レンダラーを明示的にロード
      try {
        term.loadAddon(new CanvasAddon.CanvasAddon());
      } catch (_e) {
        console.warn('Canvas addon not available, using DOM renderer');
      }
    }

    term.open(container);
    fitAndRefresh();
    requestAnimationFrame(() => fitAndRefresh());
    setTimeout(() => fitAndRefresh(), 300);
    setTimeout(() => fitAndRefresh(), 1000);

    // フォント読み込み完了後に再 fit
    if (document.fonts?.ready) {
      document.fonts.ready.then(() => fitAndRefresh());
    }
    window.addEventListener('pageshow', () => fitAndRefresh());
    const resizeObserver = new ResizeObserver(() => fitAndRefresh());
    resizeObserver.observe(container);

    // キー入力 → WebSocket
    term.onData((data) => {
      if (ws && ws.readyState === WebSocket.OPEN) {
        const encoder = new TextEncoder();
        ws.send(encoder.encode(data));
      }
    });

    term.onBinary((data) => {
      if (ws && ws.readyState === WebSocket.OPEN) {
        const bytes = new Uint8Array(data.length);
        for (let i = 0; i < data.length; i++) {
          bytes[i] = data.charCodeAt(i) & 0xff;
        }
        ws.send(bytes);
      }
    });

    return term;
  }

  function connect(token, sessionName) {
    authToken = token;
    currentSession = sessionName || 'default';
    doConnect();
  }

  function doConnect() {
    const generation = ++connectGeneration;
    const cols = term.cols;
    const rows = term.rows;
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${proto}//${location.host}/api/ws?token=${encodeURIComponent(authToken)}&cols=${cols}&rows=${rows}&session=${encodeURIComponent(currentSession)}`;

    let retries = 0;

    const attemptConnect = () => {
      // 世代チェック: 新しい doConnect() が呼ばれていたら中断
      if (generation !== connectGeneration) return;
      // 古い接続を破棄
      if (ws) {
        ws.onopen = ws.onclose = ws.onerror = ws.onmessage = null;
        ws.close();
        ws = null;
      }

      ws = new WebSocket(url);
      ws.binaryType = 'arraybuffer';

      ws.onopen = () => {
        retries = 0;
        term.focus();
        fitAndRefresh();
      };

      ws.onmessage = (event) => {
        if (typeof event.data === 'string') {
          // JSON メッセージ
          try {
            const msg = JSON.parse(event.data);
            if (msg.type === 'session_ended') {
              term.writeln('\r\n\x1b[33mSession ended.\x1b[0m');
              refreshSessionList();
              return;
            }
          } catch (_) {
            // テキストデータとして扱う
          }
          term.write(event.data);
        } else if (event.data instanceof ArrayBuffer) {
          term.write(new Uint8Array(event.data));
        }
      };

      ws.onclose = () => {
        term.writeln('\r\n\x1b[31mDisconnected.\x1b[0m');
      };

      ws.onerror = () => {};

      // Safari: WebSocket が CONNECTING のまま stall する問題のリトライ
      setTimeout(() => {
        if (generation !== connectGeneration) return;
        if (ws && ws.readyState === WebSocket.CONNECTING && retries < 3) {
          retries++;
          attemptConnect();
        }
      }, 3000);
    };

    // 少し遅延させてから接続（Safari の初回 WS stall 軽減）
    setTimeout(attemptConnect, 200);
  }

  /** セッションを切り替え */
  function switchSession(name) {
    if (name === currentSession) return;
    currentSession = name;
    term.clear();
    doConnect();
  }

  function sendResize() {
    if (ws && ws.readyState === WebSocket.OPEN && term) {
      ws.send(JSON.stringify({
        type: 'resize',
        cols: term.cols,
        rows: term.rows,
      }));
    }
  }

  function sendInput(data) {
    if (ws && ws.readyState === WebSocket.OPEN) {
      const encoder = new TextEncoder();
      ws.send(encoder.encode(data));
    }
  }

  function focus() {
    if (term) term.focus();
  }

  function getTerminal() {
    return term;
  }

  function getCurrentSession() {
    return currentSession;
  }

  // --- Session management ---

  async function fetchSessions() {
    if (!authToken) return [];
    try {
      const resp = await fetch('/api/terminal/sessions', {
        headers: { 'Authorization': `Bearer ${authToken}` },
      });
      if (resp.ok) return await resp.json();
    } catch (_) { /* ignore */ }
    return [];
  }

  async function createSession(name) {
    if (!authToken) return false;
    try {
      const resp = await fetch('/api/terminal/sessions', {
        method: 'POST',
        headers: {
          'Authorization': `Bearer ${authToken}`,
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({ name }),
      });
      return resp.ok || resp.status === 201;
    } catch (_) {
      return false;
    }
  }

  async function destroySession(name) {
    if (!authToken) return false;
    try {
      const resp = await fetch(`/api/terminal/sessions/${encodeURIComponent(name)}`, {
        method: 'DELETE',
        headers: { 'Authorization': `Bearer ${authToken}` },
      });
      return resp.ok || resp.status === 204;
    } catch (_) {
      return false;
    }
  }

  async function refreshSessionList() {
    const select = document.getElementById('session-select');
    const clientsSpan = document.getElementById('session-clients');
    if (!select) return;

    const sessions = await fetchSessions();

    select.innerHTML = '';
    if (sessions.length === 0) {
      // No sessions — just show "default" as placeholder
      const opt = document.createElement('option');
      opt.value = 'default';
      opt.textContent = 'default';
      select.appendChild(opt);
    } else {
      for (const s of sessions) {
        const opt = document.createElement('option');
        opt.value = s.name;
        const status = s.alive ? '' : ' (dead)';
        opt.textContent = `${s.name}${status}`;
        if (s.name === currentSession) opt.selected = true;
        select.appendChild(opt);
      }
    }

    // Update client count display
    const current = sessions.find(s => s.name === currentSession);
    if (current) {
      clientsSpan.textContent = `${current.client_count} client${current.client_count !== 1 ? 's' : ''}`;
    } else {
      clientsSpan.textContent = '';
    }
  }

  function initSessionBar() {
    const select = document.getElementById('session-select');
    const newBtn = document.getElementById('session-new-btn');
    const killBtn = document.getElementById('session-kill-btn');

    if (select) {
      select.addEventListener('change', () => {
        switchSession(select.value);
      });
    }

    if (newBtn) {
      newBtn.addEventListener('click', async () => {
        const name = prompt('Session name:');
        if (!name || !name.trim()) return;
        const trimmed = name.trim();
        if (!/^[a-zA-Z0-9-]+$/.test(trimmed)) {
          alert('Session name must be alphanumeric + hyphens only');
          return;
        }
        if (trimmed.length > 64) {
          alert('Session name too long (max 64 characters)');
          return;
        }
        const ok = await createSession(trimmed);
        if (!ok) {
          alert('Failed to create session');
          return;
        }
        await refreshSessionList();
        switchSession(trimmed);
      });
    }

    if (killBtn) {
      killBtn.addEventListener('click', async () => {
        if (!confirm(`Kill session "${currentSession}"?`)) return;
        await destroySession(currentSession);
        currentSession = 'default';
        term.clear();
        doConnect();
        await refreshSessionList();
      });
    }

    // 定期更新
    setInterval(refreshSessionList, 5000);
  }

  return {
    init, connect, sendInput, sendResize, focus, fitAndRefresh, getTerminal,
    getCurrentSession, switchSession, refreshSessionList, initSessionBar,
  };
})();
