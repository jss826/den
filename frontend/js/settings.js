// Den - 設定管理モジュール
const DenSettings = (() => {
  let current = {
    font_size: 14,
    theme: 'dark',
    terminal_scrollback: 1000,
    keybar_buttons: null,
    keybar_secondary_buttons: null,
    ssh_agent_forwarding: false,
    keybar_position: null,
    snippets: null,
    sleep_prevention_mode: 'user-activity',
    sleep_prevention_timeout: 30,
  };

  // キーバー設定で使用する一時配列
  let editingKeybarButtons = null;
  let editingKeybarSecondaryButtons = null;

  // Add form のターゲット（'primary' | 'secondary'）
  let addTarget = 'primary';

  // スニペット設定で使用する一時配列
  let editingSnippets = [];

  // プリセットキー一覧 (label → send のマッピング)
  const KEY_PRESETS = [
    { label: 'Tab', send: '\\t', display: 'Tab (\\t)' },
    { label: 'Esc', send: '\\x1b', display: 'Esc (\\x1b)' },
    { label: '\u2191', send: '\\x1b[A', display: '\u2191 (Up arrow)' },
    { label: '\u2193', send: '\\x1b[B', display: '\u2193 (Down arrow)' },
    { label: '\u2192', send: '\\x1b[C', display: '\u2192 (Right arrow)' },
    { label: '\u2190', send: '\\x1b[D', display: '\u2190 (Left arrow)' },
    { label: '|', send: '|', display: '| (Pipe)' },
    { label: '~', send: '~', display: '~ (Tilde)' },
    { label: '/', send: '/', display: '/ (Slash)' },
    { label: '-', send: '-', display: '- (Hyphen)' },
    { label: '_', send: '_', display: '_ (Underscore)' },
    { label: '.', send: '.', display: '. (Dot)' },
    { label: ':', send: ':', display: ': (Colon)' },
    { label: '=', send: '=', display: '= (Equals)' },
    { label: 'C-c', send: '\\x03', display: 'Ctrl+C (\\x03)' },
    { label: 'C-d', send: '\\x04', display: 'Ctrl+D (\\x04)' },
    { label: 'C-z', send: '\\x1a', display: 'Ctrl+Z (\\x1a)' },
    { label: 'C-l', send: '\\x0c', display: 'Ctrl+L (\\x0c)' },
    { label: 'C-a', send: '\\x01', display: 'Ctrl+A (\\x01)' },
    { label: 'C-e', send: '\\x05', display: 'Ctrl+E (\\x05)' },
    { label: 'C-r', send: '\\x12', display: 'Ctrl+R (\\x12)' },
    { label: 'C-w', send: '\\x17', display: 'Ctrl+W (\\x17)' },
    { label: 'C-u', send: '\\x15', display: 'Ctrl+U (\\x15)' },
    { label: 'C-k', send: '\\x0b', display: 'Ctrl+K (\\x0b)' },
    { label: 'Enter', send: '\\r', display: 'Enter (\\r)' },
    { label: 'BS', send: '\\x7f', display: 'Backspace (\\x7f)' },
    { label: 'Del', send: '\\x1b[3~', display: 'Delete' },
    { label: 'Ins', send: '\\x1b[2~', display: 'Insert' },
    { label: 'Home', send: '\\x1b[H', display: 'Home' },
    { label: 'End', send: '\\x1b[F', display: 'End' },
    { label: 'PgUp', send: '\\x1b[5~', display: 'Page Up' },
    { label: 'PgDn', send: '\\x1b[6~', display: 'Page Down' },
    { label: 'F1', send: '\\x1bOP', display: 'F1' },
    { label: 'F2', send: '\\x1bOQ', display: 'F2' },
    { label: 'F3', send: '\\x1bOR', display: 'F3' },
    { label: 'F4', send: '\\x1bOS', display: 'F4' },
    { label: 'F5', send: '\\x1b[15~', display: 'F5' },
    { label: 'F6', send: '\\x1b[17~', display: 'F6' },
    { label: 'F7', send: '\\x1b[18~', display: 'F7' },
    { label: 'F8', send: '\\x1b[19~', display: 'F8' },
    { label: 'F9', send: '\\x1b[20~', display: 'F9' },
    { label: 'F10', send: '\\x1b[21~', display: 'F10' },
    { label: 'F11', send: '\\x1b[23~', display: 'F11' },
    { label: 'F12', send: '\\x1b[24~', display: 'F12' },
    { label: 'S-Tab', send: '\\x1b[Z', display: 'Shift+Tab' },
    { label: 'Copy', send: '', display: 'Copy (selection)', type: 'action', action: 'copy' },
    { label: 'Paste', send: '', display: 'Paste (clipboard)', type: 'action', action: 'paste' },
    { label: 'Sel', send: '', display: 'Select mode (tap lines)', type: 'action', action: 'select' },
    { label: 'Screen', send: '', display: 'Copy screen (visible)', type: 'action', action: 'copy-screen' },
    { label: 'Sc\u2191', send: '', display: 'Scroll page up', type: 'action', action: 'scroll-page-up' },
    { label: 'Sc\u2193', send: '', display: 'Scroll page down', type: 'action', action: 'scroll-page-down' },
    { label: 'Top', send: '', display: 'Scroll to top', type: 'action', action: 'scroll-top' },
    { label: 'Bot', send: '', display: 'Scroll to bottom', type: 'action', action: 'scroll-bottom' },
    // スタックプリセット
    { display: 'C-c/C-z (Interrupt/Suspend)', type: 'stack', items: [
        { label: 'C-c', send: '\\x03', display: 'Ctrl+C' },
        { label: 'C-z', send: '\\x1a', display: 'Ctrl+Z' },
      ] },
    { display: 'C-d/C-l (EOF/Clear)', type: 'stack', items: [
        { label: 'C-d', send: '\\x04', display: 'Ctrl+D' },
        { label: 'C-l', send: '\\x0c', display: 'Ctrl+L' },
      ] },
    { display: 'Top/Bot (Scroll Top/Bottom)', type: 'stack', items: [
        { label: 'Top', send: '', type: 'action', action: 'scroll-top', display: 'Scroll to top' },
        { label: 'Bot', send: '', type: 'action', action: 'scroll-bottom', display: 'Scroll to bottom' },
      ] },
    { display: 'Enter/A-Ent/C-j (Enter / Alt+Enter / Newline)', type: 'stack', items: [
        { label: 'Enter', send: '\\r' },
        { label: 'A-Ent', send: '\\x1b\\r', display: 'Alt+Enter' },
        { label: 'C-j', send: '\\x0a', display: 'Ctrl+J (Newline)' },
      ] },
  ];

  // unescapeSend は keybar.js の executeNormalKey 内で実行時に適用される。
  // 設定保存時にはエスケープ形式のまま保持する。

  async function load() {
    try {
      const resp = await fetch('/api/settings', {
        credentials: 'same-origin',
      });
      if (resp.ok) {
        current = await resp.json();
      }
    } catch (e) {
      console.warn('Failed to load settings:', e);
    }
    return current;
  }

  let saveInFlight = false;
  let savePending = false;

  /**
   * Save settings to server. Merges `updates` into the in-memory `current` state
   * and PUTs the full object. Serializes concurrent calls to prevent race conditions
   * where an earlier response overwrites a later one.
   * @param {Object} updates - partial settings to merge
   * @param {Object} [opts] - options (e.g. { keepalive: true } for page-hide saves)
   */
  async function save(updates, opts) {
    Object.assign(current, updates);

    if (saveInFlight) {
      // Another save is in progress — mark pending so it re-saves after completion.
      savePending = true;
      return true;
    }

    saveInFlight = true;
    const snapshot = { ...current };
    try {
      const resp = await fetch('/api/settings', {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(current),
        keepalive: !!(opts && opts.keepalive),
      });
      if (!resp.ok) {
        throw new Error(`HTTP ${resp.status}`);
      }
      return true;
    } catch (e) {
      // Restore only fields from this batch that failed
      Object.assign(current, snapshot);
      if (typeof Toast !== 'undefined' && Toast.error) {
        Toast.error('Failed to save settings');
      }
      console.warn('Failed to save settings:', e);
      return false;
    } finally {
      saveInFlight = false;
      if (savePending) {
        savePending = false;
        // Re-save with the latest accumulated state
        save({}, opts);
      }
    }
  }

  let mediaQuery = null;

  function apply() {
    document.documentElement.style.setProperty('--den-font-size', current.font_size + 'px');
    applyTheme();
  }

  function applyTheme() {
    const theme = current.theme || 'dark';
    // 既存の mediaQuery リスナーを破棄
    if (mediaQuery) {
      mediaQuery.removeEventListener('change', onSystemThemeChange);
      mediaQuery = null;
    }

    if (theme === 'system') {
      mediaQuery = window.matchMedia('(prefers-color-scheme: light)');
      mediaQuery.addEventListener('change', onSystemThemeChange);
      const resolved = mediaQuery.matches ? 'light' : 'dark';
      document.documentElement.setAttribute('data-theme', resolved);
    } else {
      document.documentElement.setAttribute('data-theme', theme);
    }
    // light 系テーマでは color-scheme を light に
    const lightThemes = ['light', 'solarized-light', 'gruvbox-light'];
    const resolved = theme === 'system'
      ? (window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark')
      : theme;
    document.documentElement.style.colorScheme = lightThemes.includes(resolved) ? 'light' : 'dark';
  }

  function onSystemThemeChange(e) {
    document.documentElement.setAttribute('data-theme', e.matches ? 'light' : 'dark');
  }

  function get(key) {
    return current[key];
  }

  function getAll() {
    return { ...current };
  }

  // --- Keybar 設定 UI ---

  // Event delegation 用の状態管理（リスト ID → { array, render }）
  const keybarBtnListState = {};

  function renderKeybarBtnList(listId, editingArray, renderFn) {
    const list = document.getElementById(listId);
    if (!list) return;

    keybarBtnListState[listId] = { array: editingArray, render: renderFn };
    list.innerHTML = '';

    editingArray.forEach((key, idx) => {
      const item = document.createElement('div');
      item.className = 'keybar-btn-item';
      const isStack = key.type === 'stack' || key.btn_type === 'stack';
      if (isStack) {
        item.classList.add('stack');
      } else if (key.type === 'modifier' || key.btn_type === 'modifier') {
        item.classList.add('modifier');
      }
      if (key.type === 'action' || key.btn_type === 'action') {
        item.classList.add('action');
      }
      item.setAttribute('draggable', 'true');
      item.dataset.index = idx;

      const labelSpan = document.createElement('span');
      if (isStack && key.items && key.items.length > 0) {
        labelSpan.textContent = key.items.map(i => i.label).join('/');
      } else {
        labelSpan.textContent = key.label;
      }
      item.appendChild(labelSpan);

      const removeBtn = document.createElement('button');
      removeBtn.className = 'keybar-btn-remove';
      removeBtn.textContent = '\u00d7';
      removeBtn.type = 'button';
      removeBtn.setAttribute('data-tooltip', 'Remove');
      item.appendChild(removeBtn);

      list.appendChild(item);
    });

    // Event delegation: attach once per list element
    if (!list._delegated) {
      list._delegated = true;
      let currentDragOverEl = null;
      let touchStartIdx = null;
      let touchClone = null;
      let touchCurrentOverIdx = null;
      let touchTimer = null;
      let touchDragItem = null;

      function getState() { return keybarBtnListState[listId]; }
      function getItemIndex(el) {
        const item = el.closest('.keybar-btn-item');
        if (!item || item.dataset.index === undefined) return -1;
        return parseInt(item.dataset.index, 10);
      }
      function clearDragOver() {
        if (currentDragOverEl) {
          currentDragOverEl.classList.remove('drag-over');
          currentDragOverEl = null;
        }
      }
      function cleanupTouch() {
        clearTimeout(touchTimer);
        touchTimer = null;
        if (touchDragItem) {
          touchDragItem.classList.remove('dragging');
          touchDragItem = null;
        }
        clearDragOver();
        if (touchClone) { touchClone.remove(); touchClone = null; }
        touchStartIdx = null;
        touchCurrentOverIdx = null;
      }

      // Click delegation (remove)
      list.addEventListener('click', (e) => {
        const btn = e.target.closest('.keybar-btn-remove');
        if (!btn) return;
        e.stopPropagation();
        const idx = getItemIndex(btn);
        if (idx < 0) return;
        const s = getState();
        s.array.splice(idx, 1);
        s.render();
      });

      // Desktop drag & drop delegation
      list.addEventListener('dragstart', (e) => {
        const idx = getItemIndex(e.target);
        if (idx < 0) return;
        e.dataTransfer.effectAllowed = 'move';
        e.dataTransfer.setData('text/plain', String(idx));
        e.target.closest('.keybar-btn-item').classList.add('dragging');
      });
      list.addEventListener('dragend', (e) => {
        const item = e.target.closest('.keybar-btn-item');
        if (item) item.classList.remove('dragging');
        clearDragOver();
      });
      list.addEventListener('dragover', (e) => {
        const item = e.target.closest('.keybar-btn-item');
        if (!item) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = 'move';
        if (currentDragOverEl !== item) {
          clearDragOver();
          item.classList.add('drag-over');
          currentDragOverEl = item;
        }
      });
      list.addEventListener('dragleave', (e) => {
        const item = e.target.closest('.keybar-btn-item');
        if (item && currentDragOverEl === item) clearDragOver();
      });
      list.addEventListener('drop', (e) => {
        e.preventDefault();
        clearDragOver();
        const toItem = e.target.closest('.keybar-btn-item');
        if (!toItem) return;
        const s = getState();
        const fromIdx = parseInt(e.dataTransfer.getData('text/plain'), 10);
        if (isNaN(fromIdx) || fromIdx < 0 || fromIdx >= s.array.length) return;
        const toIdx = parseInt(toItem.dataset.index, 10);
        if (fromIdx !== toIdx) {
          const moved = s.array.splice(fromIdx, 1)[0];
          s.array.splice(toIdx, 0, moved);
          s.render();
        }
      });

      // Touch drag & drop delegation
      list.addEventListener('touchstart', (e) => {
        if (e.target.closest('.keybar-btn-remove')) return;
        const item = e.target.closest('.keybar-btn-item');
        if (!item) return;
        touchStartIdx = parseInt(item.dataset.index, 10);
        const touch = e.touches[0];
        touchTimer = setTimeout(() => {
          touchTimer = null;
          touchDragItem = item;
          item.classList.add('dragging');
          touchClone = item.cloneNode(true);
          touchClone.style.position = 'fixed';
          touchClone.style.zIndex = '999';
          touchClone.style.pointerEvents = 'none';
          touchClone.style.opacity = '0.8';
          touchClone.style.left = (touch.clientX - 20) + 'px';
          touchClone.style.top = (touch.clientY - 20) + 'px';
          document.body.appendChild(touchClone);
        }, 200);
      }, { passive: true });

      list.addEventListener('touchmove', (e) => {
        if (!touchClone) return;
        e.preventDefault();
        const touch = e.touches[0];
        touchClone.style.left = (touch.clientX - 20) + 'px';
        touchClone.style.top = (touch.clientY - 20) + 'px';
        const overEl = document.elementFromPoint(touch.clientX, touch.clientY);
        const overItem = overEl ? overEl.closest('.keybar-btn-item') : null;
        if (overItem && overItem.dataset.index !== undefined) {
          if (currentDragOverEl !== overItem) {
            clearDragOver();
            overItem.classList.add('drag-over');
            currentDragOverEl = overItem;
          }
          touchCurrentOverIdx = parseInt(overItem.dataset.index, 10);
        } else {
          clearDragOver();
          touchCurrentOverIdx = null;
        }
      }, { passive: false });

      list.addEventListener('touchend', () => {
        const startIdx = touchStartIdx;
        const overIdx = touchCurrentOverIdx;
        const hadClone = !!touchClone;
        cleanupTouch();
        if (hadClone && overIdx !== null && startIdx !== overIdx) {
          const s = getState();
          const moved = s.array.splice(startIdx, 1)[0];
          s.array.splice(overIdx, 0, moved);
          s.render();
        }
      });

      list.addEventListener('touchcancel', () => {
        cleanupTouch();
      });
    }
  }

  function renderKeybarList() {
    renderKeybarBtnList('keybar-btn-list', editingKeybarButtons, renderKeybarList);
  }

  function renderKeybarSecondaryList() {
    renderKeybarBtnList('keybar-secondary-btn-list', editingKeybarSecondaryButtons, renderKeybarSecondaryList);
  }

  function getEditingButtons() {
    return editingKeybarButtons.map(k => {
      const copy = { ...k };
      if (copy.items) copy.items = copy.items.map(i => ({ ...i }));
      return copy;
    });
  }

  function getEditingSecondaryButtons() {
    return editingKeybarSecondaryButtons.map(k => {
      const copy = { ...k };
      if (copy.items) copy.items = copy.items.map(i => ({ ...i }));
      return copy;
    });
  }

  // --- Snippet 設定 UI ---

  let snippetListDelegated = false;

  function renderSnippetList() {
    const list = document.getElementById('snippet-list');
    if (!list) return;
    list.innerHTML = '';

    editingSnippets.forEach((s, idx) => {
      const item = document.createElement('div');
      item.className = 'snippet-item';
      item.setAttribute('draggable', 'true');
      item.dataset.index = idx;

      const label = document.createElement('span');
      label.className = 'snippet-item-label';
      label.textContent = s.label;
      item.appendChild(label);

      const cmd = document.createElement('span');
      cmd.className = 'snippet-item-cmd';
      cmd.textContent = s.command;
      item.appendChild(cmd);

      if (s.auto_run) {
        const auto = document.createElement('span');
        auto.className = 'snippet-item-auto';
        auto.textContent = '\u23CE';
        auto.title = 'Auto-run';
        item.appendChild(auto);
      }

      const removeBtn = document.createElement('button');
      removeBtn.className = 'snippet-item-remove';
      removeBtn.textContent = '\u00d7';
      removeBtn.type = 'button';
      removeBtn.setAttribute('data-tooltip', 'Remove');
      removeBtn.setAttribute('aria-label', 'Remove snippet');
      item.appendChild(removeBtn);

      list.appendChild(item);
    });

    // Event delegation: attach once on the list element
    if (!snippetListDelegated) {
      snippetListDelegated = true;
      let currentDragOverEl = null;
      let touchStartIdx = null;
      let touchClone = null;
      let touchCurrentOverIdx = null;
      let touchTimer = null;
      let touchDragItem = null;

      function getItemIndex(el) {
        const item = el.closest('.snippet-item');
        if (!item || item.dataset.index === undefined) return -1;
        return parseInt(item.dataset.index, 10);
      }

      function clearDragOver() {
        if (currentDragOverEl) {
          currentDragOverEl.classList.remove('drag-over');
          currentDragOverEl = null;
        }
      }

      function cleanupTouch() {
        clearTimeout(touchTimer);
        touchTimer = null;
        if (touchDragItem) {
          touchDragItem.classList.remove('dragging');
          touchDragItem = null;
        }
        clearDragOver();
        if (touchClone) { touchClone.remove(); touchClone = null; }
        touchStartIdx = null;
        touchCurrentOverIdx = null;
      }

      // Click delegation (remove button)
      list.addEventListener('click', (e) => {
        const removeBtn = e.target.closest('.snippet-item-remove');
        if (!removeBtn) return;
        e.stopPropagation();
        const idx = getItemIndex(removeBtn);
        if (idx < 0) return;
        editingSnippets.splice(idx, 1);
        renderSnippetList();
      });

      // Desktop drag & drop delegation
      list.addEventListener('dragstart', (e) => {
        const idx = getItemIndex(e.target);
        if (idx < 0) return;
        e.dataTransfer.effectAllowed = 'move';
        e.dataTransfer.setData('text/plain', String(idx));
        e.target.closest('.snippet-item').classList.add('dragging');
      });
      list.addEventListener('dragend', (e) => {
        const item = e.target.closest('.snippet-item');
        if (item) item.classList.remove('dragging');
        clearDragOver();
      });
      list.addEventListener('dragover', (e) => {
        const item = e.target.closest('.snippet-item');
        if (!item) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = 'move';
        if (currentDragOverEl !== item) {
          clearDragOver();
          item.classList.add('drag-over');
          currentDragOverEl = item;
        }
      });
      list.addEventListener('dragleave', (e) => {
        const item = e.target.closest('.snippet-item');
        if (item && currentDragOverEl === item) {
          clearDragOver();
        }
      });
      list.addEventListener('drop', (e) => {
        e.preventDefault();
        clearDragOver();
        const toItem = e.target.closest('.snippet-item');
        if (!toItem) return;
        const fromIdx = parseInt(e.dataTransfer.getData('text/plain'), 10);
        if (isNaN(fromIdx) || fromIdx < 0 || fromIdx >= editingSnippets.length) return;
        const toIdx = parseInt(toItem.dataset.index, 10);
        if (fromIdx !== toIdx) {
          const moved = editingSnippets.splice(fromIdx, 1)[0];
          editingSnippets.splice(toIdx, 0, moved);
          renderSnippetList();
        }
      });

      // Touch drag & drop delegation
      list.addEventListener('touchstart', (e) => {
        if (e.target.closest('.snippet-item-remove')) return;
        const item = e.target.closest('.snippet-item');
        if (!item) return;
        const idx = parseInt(item.dataset.index, 10);
        touchStartIdx = idx;
        const touch = e.touches[0];
        touchTimer = setTimeout(() => {
          touchTimer = null;
          touchDragItem = item;
          item.classList.add('dragging');
          touchClone = item.cloneNode(true);
          const rect = item.getBoundingClientRect();
          touchClone.style.position = 'fixed';
          touchClone.style.zIndex = '999';
          touchClone.style.pointerEvents = 'none';
          touchClone.style.opacity = '0.8';
          touchClone.style.width = rect.width + 'px';
          touchClone.style.left = (touch.clientX - 20) + 'px';
          touchClone.style.top = (touch.clientY - 20) + 'px';
          document.body.appendChild(touchClone);
        }, 200);
      }, { passive: true });

      list.addEventListener('touchmove', (e) => {
        if (!touchClone) return;
        e.preventDefault();
        const touch = e.touches[0];
        touchClone.style.left = (touch.clientX - 20) + 'px';
        touchClone.style.top = (touch.clientY - 20) + 'px';
        const overEl = document.elementFromPoint(touch.clientX, touch.clientY);
        const overItem = overEl ? overEl.closest('.snippet-item') : null;
        if (overItem && overItem.dataset.index !== undefined) {
          if (currentDragOverEl !== overItem) {
            clearDragOver();
            overItem.classList.add('drag-over');
            currentDragOverEl = overItem;
          }
          touchCurrentOverIdx = parseInt(overItem.dataset.index, 10);
        } else {
          clearDragOver();
          touchCurrentOverIdx = null;
        }
      }, { passive: false });

      list.addEventListener('touchend', () => {
        const startIdx = touchStartIdx;
        const overIdx = touchCurrentOverIdx;
        const hadClone = !!touchClone;
        cleanupTouch();
        if (hadClone && overIdx !== null && startIdx !== overIdx) {
          const moved = editingSnippets.splice(startIdx, 1)[0];
          editingSnippets.splice(overIdx, 0, moved);
          renderSnippetList();
        }
      });

      list.addEventListener('touchcancel', () => {
        cleanupTouch();
      });
    }
  }

  function openModal() {
    const modal = document.getElementById('settings-modal');
    document.getElementById('setting-font-size').value = current.font_size;
    document.getElementById('setting-scrollback').value = current.terminal_scrollback;
    const themeSelect = document.getElementById('setting-theme');
    if (themeSelect) themeSelect.value = current.theme || 'dark';

    const agentFwdCheck = document.getElementById('setting-ssh-agent-fwd');
    if (agentFwdCheck) agentFwdCheck.checked = !!current.ssh_agent_forwarding;

    // Sleep prevention 設定
    const sleepMode = document.getElementById('setting-sleep-mode');
    if (sleepMode) sleepMode.value = current.sleep_prevention_mode || 'user-activity';
    const sleepTimeout = document.getElementById('setting-sleep-timeout');
    if (sleepTimeout) sleepTimeout.value = current.sleep_prevention_timeout || 30;
    const timeoutRow = document.getElementById('sleep-timeout-row');
    if (timeoutRow) timeoutRow.hidden = (sleepMode && sleepMode.value !== 'user-activity');

    // キーバー設定の初期化（items を deep clone）
    if (current.keybar_buttons && current.keybar_buttons.length > 0) {
      editingKeybarButtons = current.keybar_buttons.map(k => {
        const copy = { ...k };
        if (copy.items) copy.items = copy.items.map(i => ({ ...i }));
        return copy;
      });
    } else {
      editingKeybarButtons = Keybar.getDefaultKeys();
    }
    renderKeybarList();

    // サブ行キーバー設定の初期化
    if (current.keybar_secondary_buttons && current.keybar_secondary_buttons.length > 0) {
      editingKeybarSecondaryButtons = current.keybar_secondary_buttons.map(k => {
        const copy = { ...k };
        if (copy.items) copy.items = copy.items.map(i => ({ ...i }));
        return copy;
      });
    } else {
      editingKeybarSecondaryButtons = Keybar.getDefaultSecondaryKeys();
    }
    renderKeybarSecondaryList();

    setupAddForm();

    // Add form を閉じた状態にリセット
    addTarget = 'primary';
    const addForm = document.getElementById('keybar-add-form');
    if (addForm) addForm.hidden = true;

    // スニペット設定の初期化
    editingSnippets = current.snippets ? current.snippets.map(s => ({ ...s })) : [];
    renderSnippetList();
    const snippetAddForm = document.getElementById('snippet-add-form');
    if (snippetAddForm) snippetAddForm.hidden = true;

    modal.hidden = false;
  }

  // スタックビルダーの一時アイテム配列
  let editingStackItems = [];

  function setupAddForm() {
    const presetSelect = document.getElementById('keybar-preset-select');
    const stackPreset = document.getElementById('keybar-stack-preset');
    if (!presetSelect) return;
    // プリセット option を生成（初回のみ）
    if (presetSelect.options.length <= 1) {
      // Non-stack presets for single-key and stack-item selection
      const nonStackPresets = KEY_PRESETS.filter(p => p.type !== 'stack');
      const stackPresets = KEY_PRESETS.filter(p => p.type === 'stack');

      nonStackPresets.forEach(p => {
        const opt = document.createElement('option');
        if (p.type === 'action') {
          opt.value = '__action:' + p.action;
          opt.dataset.btnType = p.type;
          opt.dataset.btnAction = p.action;
        } else {
          opt.value = p.send;
        }
        opt.textContent = p.display;
        if (p.label) opt.dataset.label = p.label;
        presetSelect.appendChild(opt);
      });

      // Stack presets in single-key dropdown
      stackPresets.forEach(p => {
        const opt = document.createElement('option');
        opt.value = '__stack:' + p.display;
        opt.dataset.btnType = 'stack';
        opt.dataset.stackItems = JSON.stringify(p.items);
        opt.textContent = p.display;
        presetSelect.appendChild(opt);
      });

      // Populate stack-item preset dropdown (non-stack only)
      if (stackPreset) {
        nonStackPresets.forEach(p => {
          const opt = document.createElement('option');
          opt.value = p.send;
          opt.textContent = p.display;
          if (p.label) opt.dataset.label = p.label;
          if (p.type === 'action') {
            opt.dataset.btnType = 'action';
            opt.dataset.btnAction = p.action;
          }
          stackPreset.appendChild(opt);
        });
      }
    }
  }

  function renderStackItems() {
    const container = document.getElementById('keybar-stack-items');
    if (!container) return;
    container.innerHTML = '';
    editingStackItems.forEach((item, idx) => {
      const div = document.createElement('div');
      div.className = 'keybar-stack-item';
      const label = document.createElement('span');
      label.textContent = item.display || item.label;
      div.appendChild(label);
      const removeBtn = document.createElement('button');
      removeBtn.className = 'keybar-stack-item-remove';
      removeBtn.type = 'button';
      removeBtn.textContent = '\u00d7';
      removeBtn.addEventListener('click', () => {
        editingStackItems.splice(idx, 1);
        renderStackItems();
      });
      div.appendChild(removeBtn);
      container.appendChild(div);
    });
  }

  function closeModal() {
    document.getElementById('settings-modal').hidden = true;
  }

  function bindUI() {
    const btn = document.getElementById('settings-btn');
    if (btn) btn.addEventListener('click', openModal);

    const cancelBtn = document.getElementById('settings-cancel');
    if (cancelBtn) cancelBtn.addEventListener('click', closeModal);

    const saveBtn = document.getElementById('settings-save');
    if (saveBtn) saveBtn.addEventListener('click', async () => {
      const fontSize = parseInt(document.getElementById('setting-font-size').value, 10) || 14;
      const scrollback = parseInt(document.getElementById('setting-scrollback').value, 10) || 1000;
      const themeSelect = document.getElementById('setting-theme');
      const theme = themeSelect ? themeSelect.value : 'dark';

      // キーバーボタン: 保存用に items を deep clone
      const keybarButtons = getEditingButtons();
      const keybarSecondaryButtons = getEditingSecondaryButtons();

      const agentFwdCheck = document.getElementById('setting-ssh-agent-fwd');
      const sshAgentFwd = agentFwdCheck ? agentFwdCheck.checked : false;

      const snippets = editingSnippets.length > 0 ? editingSnippets.map(s => ({ ...s })) : null;

      const sleepModeEl = document.getElementById('setting-sleep-mode');
      const sleepMode = sleepModeEl ? sleepModeEl.value : 'user-activity';
      const sleepTimeoutEl = document.getElementById('setting-sleep-timeout');
      const sleepTimeout = sleepTimeoutEl ? Math.max(1, Math.min(480, parseInt(sleepTimeoutEl.value, 10) || 30)) : 30;

      const ok = await save({
        font_size: Math.max(8, Math.min(32, fontSize)),
        terminal_scrollback: Math.max(100, Math.min(50000, scrollback)),
        theme: theme,
        keybar_buttons: keybarButtons,
        keybar_secondary_buttons: keybarSecondaryButtons,
        ssh_agent_forwarding: sshAgentFwd,
        snippets: snippets,
        sleep_prevention_mode: sleepMode,
        sleep_prevention_timeout: sleepTimeout,
      });
      if (!ok) return;
      apply();

      // scrollback / fontSize を即時反映（xterm.js は options の動的変更に対応）
      const t = DenTerminal.getTerminal();
      if (t) {
        t.options.scrollback = Math.max(100, Math.min(50000, scrollback));
        t.options.fontSize = Math.max(8, Math.min(32, fontSize));
        DenTerminal.fitAndRefresh();
      }

      // フローティングターミナルにも設定反映
      if (typeof FloatTerminal !== 'undefined') FloatTerminal.applySettings();

      // キーバーを即時反映
      Keybar.reload(keybarButtons, keybarSecondaryButtons);

      // スニペットを即時反映
      if (typeof DenSnippet !== 'undefined') DenSnippet.reload();

      closeModal();
    });

    const modal = document.getElementById('settings-modal');
    if (modal) modal.addEventListener('click', (e) => {
      if (e.target === modal) closeModal();
    });

    // --- Sleep prevention ---
    const sleepModeSelect = document.getElementById('setting-sleep-mode');
    if (sleepModeSelect) sleepModeSelect.addEventListener('change', () => {
      const timeoutRow = document.getElementById('sleep-timeout-row');
      if (timeoutRow) timeoutRow.hidden = (sleepModeSelect.value !== 'user-activity');
    });

    // --- Keybar editor ---
    const addBtn = document.getElementById('keybar-add-btn');
    const resetBtn = document.getElementById('keybar-reset-btn');
    const addForm = document.getElementById('keybar-add-form');
    const addConfirm = document.getElementById('keybar-add-confirm');
    const addCancel = document.getElementById('keybar-add-cancel');
    const presetSelect = document.getElementById('keybar-preset-select');
    const newLabelInput = document.getElementById('keybar-new-label');
    const newSendInput = document.getElementById('keybar-new-send');
    const newModifierCheck = document.getElementById('keybar-new-modifier');
    const newModKeySelect = document.getElementById('keybar-new-modkey');
    const newTypeSelect = document.getElementById('keybar-new-type');
    const singleFields = document.getElementById('keybar-single-fields');
    const stackFields = document.getElementById('keybar-stack-fields');
    const stackPreset = document.getElementById('keybar-stack-preset');
    const stackItemLabel = document.getElementById('keybar-stack-item-label');
    const stackItemSend = document.getElementById('keybar-stack-item-send');
    const stackAddItemBtn = document.getElementById('keybar-stack-add-item');

    // Type toggle: Single / Stack
    if (newTypeSelect) newTypeSelect.addEventListener('change', () => {
      const isStack = newTypeSelect.value === 'stack';
      if (singleFields) singleFields.hidden = isStack;
      if (stackFields) stackFields.hidden = !isStack;
    });

    // Stack item preset selection
    if (stackPreset) stackPreset.addEventListener('change', () => {
      const val = stackPreset.value;
      if (val) {
        const opt = stackPreset.selectedOptions[0];
        stackItemLabel.value = opt.dataset.label || '';
        stackItemSend.value = val;
      } else {
        stackItemSend.value = '';
      }
    });

    // Add item to stack
    if (stackAddItemBtn) stackAddItemBtn.addEventListener('click', () => {
      const label = stackItemLabel.value.trim();
      const send = stackItemSend.value;
      if (!label) { stackItemLabel.focus(); return; }
      const selectedOpt = stackPreset?.selectedOptions[0];
      if (selectedOpt && selectedOpt.dataset.btnType === 'action') {
        editingStackItems.push({
          label, send: '', type: 'action',
          action: selectedOpt.dataset.btnAction,
          display: selectedOpt.textContent,
        });
      } else {
        if (!send) { stackItemSend.focus(); return; }
        editingStackItems.push({ label, send, display: label });
      }
      stackItemLabel.value = '';
      stackItemSend.value = '';
      if (stackPreset) stackPreset.value = '';
      renderStackItems();
      stackItemLabel.focus();
    });

    function showAddForm(target) {
      addTarget = target;
      addForm.hidden = false;
      if (newTypeSelect) newTypeSelect.value = 'single';
      if (singleFields) singleFields.hidden = false;
      if (stackFields) stackFields.hidden = true;
      newLabelInput.value = '';
      newSendInput.value = '';
      presetSelect.value = '';
      newModifierCheck.checked = false;
      newModKeySelect.hidden = true;
      editingStackItems = [];
      renderStackItems();
      newLabelInput.focus();
    }

    if (addBtn) addBtn.addEventListener('click', () => showAddForm('primary'));

    if (resetBtn) resetBtn.addEventListener('click', () => {
      editingKeybarButtons = Keybar.getDefaultKeys();
      renderKeybarList();
    });

    // --- Secondary keybar editor ---
    const secondaryAddBtn = document.getElementById('keybar-secondary-add-btn');
    const secondaryResetBtn = document.getElementById('keybar-secondary-reset-btn');

    if (secondaryAddBtn) secondaryAddBtn.addEventListener('click', () => showAddForm('secondary'));

    if (secondaryResetBtn) secondaryResetBtn.addEventListener('click', () => {
      editingKeybarSecondaryButtons = Keybar.getDefaultSecondaryKeys();
      renderKeybarSecondaryList();
    });

    if (presetSelect) presetSelect.addEventListener('change', () => {
      const val = presetSelect.value;
      if (val) {
        const opt = presetSelect.selectedOptions[0];
        if (opt.dataset.btnType === 'stack') {
          // スタック: label/send は不要
          newLabelInput.value = '';
          newSendInput.value = '';
        } else {
          newLabelInput.value = opt.dataset.label || '';
          newSendInput.value = val;
        }
      } else {
        newSendInput.value = '';
      }
    });

    if (newModifierCheck) newModifierCheck.addEventListener('change', () => {
      newModKeySelect.hidden = !newModifierCheck.checked;
    });

    if (addConfirm) addConfirm.addEventListener('click', () => {
      const targetArray = addTarget === 'secondary'
        ? editingKeybarSecondaryButtons : editingKeybarButtons;
      const renderFn = addTarget === 'secondary'
        ? renderKeybarSecondaryList : renderKeybarList;
      const isStack = newTypeSelect && newTypeSelect.value === 'stack';

      if (isStack) {
        if (editingStackItems.length < 2) {
          if (typeof Toast !== 'undefined') Toast.error('Stack needs at least 2 items');
          return;
        }
        targetArray.push({
          type: 'stack',
          items: editingStackItems.map(i => ({ ...i })),
          selected: 0,
        });
        renderFn();
        addForm.hidden = true;
        return;
      }

      const selectedOpt = presetSelect.selectedOptions[0];
      if (selectedOpt && selectedOpt.dataset.btnType === 'stack') {
        const items = JSON.parse(selectedOpt.dataset.stackItems);
        targetArray.push({
          type: 'stack',
          items: items,
          selected: 0,
        });
        renderFn();
        addForm.hidden = true;
        return;
      }

      const label = newLabelInput.value.trim();
      if (!label) {
        newLabelInput.focus();
        return;
      }

      if (selectedOpt && selectedOpt.dataset.btnType === 'action') {
        targetArray.push({
          label,
          send: '',
          type: 'action',
          action: selectedOpt.dataset.btnAction,
        });
        renderFn();
        addForm.hidden = true;
        return;
      }

      if (newModifierCheck.checked) {
        targetArray.push({
          label,
          send: '',
          type: 'modifier',
          mod_key: newModKeySelect.value,
        });
      } else {
        const sendRaw = newSendInput.value;
        if (!sendRaw) {
          newSendInput.focus();
          return;
        }
        targetArray.push({
          label,
          send: sendRaw,
        });
      }

      renderFn();
      addForm.hidden = true;
    });

    if (addCancel) addCancel.addEventListener('click', () => {
      addForm.hidden = true;
    });

    // --- Snippet editor ---
    const snippetAddBtn = document.getElementById('snippet-add-btn');
    const snippetAddForm = document.getElementById('snippet-add-form');
    const snippetAddConfirm = document.getElementById('snippet-add-confirm');
    const snippetAddCancel = document.getElementById('snippet-add-cancel');
    const snippetNewLabel = document.getElementById('snippet-new-label');
    const snippetNewCommand = document.getElementById('snippet-new-command');
    const snippetNewAutorun = document.getElementById('snippet-new-autorun');

    if (snippetAddBtn) snippetAddBtn.addEventListener('click', () => {
      snippetAddForm.hidden = false;
      snippetNewLabel.value = '';
      snippetNewCommand.value = '';
      snippetNewAutorun.checked = false;
      snippetNewLabel.focus();
    });

    if (snippetAddConfirm) snippetAddConfirm.addEventListener('click', () => {
      const label = snippetNewLabel.value.trim();
      const command = snippetNewCommand.value;
      if (!label) { snippetNewLabel.focus(); return; }
      if (!command.trim()) { snippetNewCommand.focus(); return; }

      editingSnippets.push({
        label: label,
        command: command,
        auto_run: snippetNewAutorun.checked,
      });
      renderSnippetList();
      snippetAddForm.hidden = true;
    });

    if (snippetAddCancel) snippetAddCancel.addEventListener('click', () => {
      snippetAddForm.hidden = true;
    });
  }

  return { load, save, apply, get, getAll, bindUI, openModal };
})();
