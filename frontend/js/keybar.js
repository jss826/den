// Den - タッチキーバーモジュール
const Keybar = (() => {
  let container = null;
  let modifiers = { ctrl: false, alt: false };
  let activeKeys = [];

  // デフォルトキー配列
  const DEFAULT_KEYS = [
    { label: 'Ctrl', send: '', type: 'modifier', mod_key: 'ctrl' },
    { label: 'Alt', send: '', type: 'modifier', mod_key: 'alt' },
    { label: 'Tab', send: '\t' },
    { label: 'Esc', send: '\x1b' },
    { label: '\u2191', send: '\x1b[A' },
    { label: '\u2193', send: '\x1b[B' },
    { label: '\u2192', send: '\x1b[C' },
    { label: '\u2190', send: '\x1b[D' },
    { label: '|', send: '|' },
    { label: '~', send: '~' },
    { label: '/', send: '/' },
    { label: '-', send: '-' },
    { label: 'C-c', send: '\x03' },
    { label: 'C-d', send: '\x04' },
    { label: 'C-z', send: '\x1a' },
    { label: 'C-l', send: '\x0c' },
  ];

  function init(el, customKeys) {
    container = el;
    activeKeys = customKeys && customKeys.length > 0 ? customKeys : DEFAULT_KEYS;
    render();

    if (isTouchDevice()) {
      container.classList.add('visible');
    }
  }

  function reload(customKeys) {
    activeKeys = customKeys && customKeys.length > 0 ? customKeys : DEFAULT_KEYS;
    render();
  }

  function getDefaultKeys() {
    return DEFAULT_KEYS.map(k => ({ ...k }));
  }

  function isTouchDevice() {
    return 'ontouchstart' in window
      || navigator.maxTouchPoints > 0
      || window.matchMedia('(hover: none) and (pointer: coarse)').matches;
  }

  function render() {
    container.innerHTML = '';
    modifiers = { ctrl: false, alt: false };
    activeKeys.forEach((key) => {
      const btn = document.createElement('button');
      btn.className = 'key-btn';
      btn.textContent = key.label;

      const isModifier = key.type === 'modifier' || key.btn_type === 'modifier';

      if (isModifier) {
        const modKey = key.mod || key.mod_key;
        btn.classList.add('modifier');
        btn.addEventListener('click', (e) => {
          e.preventDefault();
          modifiers[modKey] = !modifiers[modKey];
          btn.classList.toggle('active', modifiers[modKey]);
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

  return { init, reload, getDefaultKeys, isTouchDevice };
})();
