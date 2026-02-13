// Den - ターミナルモジュール
const DenTerminal = (() => {
  let term = null;
  let fitAddon = null;
  let ws = null;

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
      // xterm.js v6 は WebGL/Canvas アドオン未ロードだと DOM レンダラーになり描画されない
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

  function connect(token) {
    const cols = term.cols;
    const rows = term.rows;
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${proto}//${location.host}/api/ws?token=${encodeURIComponent(token)}&cols=${cols}&rows=${rows}`;

    let retries = 0;

    const doConnect = () => {
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
        term.writeln('\x1b[32mConnected.\x1b[0m');
        term.focus();
        fitAndRefresh();
      };

      ws.onmessage = (event) => {
        if (event.data instanceof ArrayBuffer) {
          term.write(new Uint8Array(event.data));
        } else {
          term.write(event.data);
        }
      };

      ws.onclose = () => {
        term.writeln('\r\n\x1b[31mDisconnected.\x1b[0m');
      };

      ws.onerror = () => {};

      // Safari: WebSocket が CONNECTING のまま stall する問題のリトライ
      setTimeout(() => {
        if (ws && ws.readyState === WebSocket.CONNECTING && retries < 3) {
          retries++;
          doConnect();
        }
      }, 3000);
    };

    // 少し遅延させてから接続（Safari の初回 WS stall 軽減）
    setTimeout(doConnect, 200);
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

  return { init, connect, sendInput, sendResize, focus, fitAndRefresh, getTerminal };
})();
