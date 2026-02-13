// Den - ターミナルモジュール
const DenTerminal = (() => {
  let term = null;
  let fitAddon = null;
  let ws = null;

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

    // WebGL レンダラー（Safari/iOS では不安定なためスキップ）
    const isIOS = /iPad|iPhone|iPod/.test(navigator.userAgent)
      || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
    const isSafari = !isIOS && /^((?!chrome|android).)*safari/i.test(navigator.userAgent);
    if (!isIOS && !isSafari) {
      try {
        const webglAddon = new WebglAddon.WebglAddon();
        webglAddon.onContextLost(() => webglAddon.dispose());
        term.loadAddon(webglAddon);
      } catch (_e) {
        console.warn('WebGL not available, using canvas renderer');
      }
    }

    term.open(container);
    fitAddon.fit();

    // iPad 等でレイアウト確定前に fit() が空振りする対策
    requestAnimationFrame(() => {
      fitAddon.fit();
      sendResize();
    });
    setTimeout(() => {
      fitAddon.fit();
      sendResize();
    }, 200);

    // リサイズ監視
    const resizeObserver = new ResizeObserver(() => {
      fitAddon.fit();
      sendResize();
    });
    resizeObserver.observe(container);

    // キー入力 → WebSocket
    term.onData((data) => {
      if (ws && ws.readyState === WebSocket.OPEN) {
        // バイナリで送信
        const encoder = new TextEncoder();
        ws.send(encoder.encode(data));
      }
    });

    // バイナリデータ → WebSocket
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

    ws = new WebSocket(url);
    ws.binaryType = 'arraybuffer';

    ws.onopen = () => {
      term.writeln('\x1b[32mConnected.\x1b[0m');
      term.focus();
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

    ws.onerror = () => {
      term.writeln('\r\n\x1b[31mConnection error.\x1b[0m');
    };
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

  return { init, connect, sendInput, sendResize, focus, getTerminal };
})();
