/* global DenSettings, DenTerminal, Toast */
// Den - スニペット管理モジュール
// eslint-disable-next-line no-unused-vars
const DenSnippet = (() => {
  let btn = null;
  let popup = null;

  function init(button) {
    btn = button;
    if (!btn) return;
    btn.addEventListener('click', toggle);
  }

  function toggle() {
    if (popup) {
      close();
    } else {
      open();
    }
  }

  function open() {
    const snippets = DenSettings.get('snippets');
    if (!snippets || snippets.length === 0) {
      Toast.info('No snippets configured. Add snippets in Settings.');
      return;
    }
    close();

    popup = document.createElement('div');
    popup.className = 'snippet-popup';

    snippets.forEach((s) => {
      const item = document.createElement('button');
      item.className = 'snippet-popup-item';
      item.type = 'button';

      const label = document.createElement('span');
      label.className = 'snippet-popup-label';
      label.textContent = s.label;

      const cmd = document.createElement('span');
      cmd.className = 'snippet-popup-cmd';
      cmd.textContent = s.command;

      item.appendChild(label);
      item.appendChild(cmd);

      if (s.auto_run) {
        const auto = document.createElement('span');
        auto.className = 'snippet-popup-auto';
        auto.textContent = '\u23CE';
        auto.title = 'Auto-run';
        item.appendChild(auto);
      }

      item.addEventListener('click', () => {
        execute(s);
        close();
      });

      popup.appendChild(item);
    });

    // Position relative to button
    document.body.appendChild(popup);
    positionPopup();

    // Close on outside click (delayed to avoid catching the opening click)
    requestAnimationFrame(() => {
      document.addEventListener('pointerdown', onOutsideClick, true);
    });
  }

  function positionPopup() {
    if (!btn || !popup) return;
    const rect = btn.getBoundingClientRect();
    popup.style.top = (rect.bottom + 4) + 'px';
    // Align right edge to button right edge
    popup.style.right = (window.innerWidth - rect.right) + 'px';
  }

  function close() {
    if (popup) {
      popup.remove();
      popup = null;
    }
    document.removeEventListener('pointerdown', onOutsideClick, true);
  }

  function onOutsideClick(e) {
    if (popup && !popup.contains(e.target) && e.target !== btn && !btn.contains(e.target)) {
      close();
    }
  }

  function execute(snippet) {
    const data = snippet.auto_run ? snippet.command + '\r' : snippet.command;
    DenTerminal.sendInput(data);
    DenTerminal.focus();
  }

  function reload() {
    // If popup is open, close and reopen to reflect changes
    if (popup) {
      close();
      open();
    }
  }

  return { init, open, close, reload };
})();
