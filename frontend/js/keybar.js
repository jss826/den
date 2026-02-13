// Den - タッチキーバーモジュール
const Keybar = (() => {
  let container = null;
  let modifiers = { ctrl: false, alt: false };

  // キー定義
  const keys = [
    { label: 'Ctrl', type: 'modifier', mod: 'ctrl' },
    { label: 'Alt', type: 'modifier', mod: 'alt' },
    { label: 'Tab', send: '\t' },
    { label: 'Esc', send: '\x1b' },
    { label: '↑', send: '\x1b[A' },
    { label: '↓', send: '\x1b[B' },
    { label: '→', send: '\x1b[C' },
    { label: '←', send: '\x1b[D' },
    { label: '|', send: '|' },
    { label: '~', send: '~' },
    { label: '/', send: '/' },
    { label: '-', send: '-' },
    { label: 'C-c', send: '\x03' },
    { label: 'C-d', send: '\x04' },
    { label: 'C-z', send: '\x1a' },
    { label: 'C-l', send: '\x0c' },
  ];

  function init(el) {
    container = el;
    render();

    // タッチデバイス検出
    if (isTouchDevice()) {
      container.classList.add('visible');
    }
  }

  function isTouchDevice() {
    return 'ontouchstart' in window
      || navigator.maxTouchPoints > 0
      || window.matchMedia('(hover: none) and (pointer: coarse)').matches;
  }

  function render() {
    container.innerHTML = '';
    keys.forEach((key) => {
      const btn = document.createElement('button');
      btn.className = 'key-btn';
      btn.textContent = key.label;

      if (key.type === 'modifier') {
        btn.classList.add('modifier');
        btn.addEventListener('click', (e) => {
          e.preventDefault();
          modifiers[key.mod] = !modifiers[key.mod];
          btn.classList.toggle('active', modifiers[key.mod]);
        });
      } else {
        btn.addEventListener('click', (e) => {
          e.preventDefault();
          let data = key.send;

          // Ctrl 修飾
          if (modifiers.ctrl && data.length === 1) {
            const code = data.toUpperCase().charCodeAt(0);
            if (code >= 0x40 && code <= 0x5f) {
              data = String.fromCharCode(code - 0x40);
            }
          }

          // Alt 修飾
          if (modifiers.alt) {
            data = '\x1b' + data;
          }

          DenTerminal.sendInput(data);
          DenTerminal.focus();

          // 修飾キーをリセット
          resetModifiers();
        });
      }

      container.appendChild(btn);
    });
  }

  function resetModifiers() {
    modifiers.ctrl = false;
    modifiers.alt = false;
    container.querySelectorAll('.modifier').forEach((btn) => {
      btn.classList.remove('active');
    });
  }

  return { init, isTouchDevice };
})();
