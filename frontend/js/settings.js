// Den - 設定管理モジュール
const DenSettings = (() => {
  let current = {
    font_size: 14,
    theme: 'dark',
    terminal_scrollback: 1000,
    keybar_buttons: null,
    ssh_agent_forwarding: false,
  };

  // キーバー設定で使用する一時配列
  let editingKeybarButtons = null;

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
  ];

  // エスケープ文字列をリテラルに変換
  function unescapeSend(str) {
    return str
      .replace(/\\x([0-9a-fA-F]{2})/g, (_, hex) => String.fromCharCode(parseInt(hex, 16)))
      .replace(/\\t/g, '\t')
      .replace(/\\n/g, '\n')
      .replace(/\\r/g, '\r')
      .replace(/\\\\/g, '\\');
  }

  async function load() {
    try {
      const resp = await fetch('/api/settings', {
        headers: { 'Authorization': `Bearer ${Auth.getToken()}` },
      });
      if (resp.ok) {
        current = await resp.json();
      }
    } catch (e) {
      console.warn('Failed to load settings:', e);
    }
    return current;
  }

  async function save(updates) {
    const previous = { ...current };
    Object.assign(current, updates);
    try {
      const resp = await fetch('/api/settings', {
        method: 'PUT',
        headers: {
          'Content-Type': 'application/json',
          'Authorization': `Bearer ${Auth.getToken()}`,
        },
        body: JSON.stringify(current),
      });
      if (!resp.ok) {
        throw new Error(`HTTP ${resp.status}`);
      }
      return true;
    } catch (e) {
      Object.assign(current, previous);
      if (typeof Toast !== 'undefined' && Toast.error) {
        Toast.error('Failed to save settings');
      }
      console.warn('Failed to save settings:', e);
      return false;
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

  function renderKeybarList() {
    const list = document.getElementById('keybar-btn-list');
    if (!list) return;
    list.innerHTML = '';

    editingKeybarButtons.forEach((key, idx) => {
      const item = document.createElement('div');
      item.className = 'keybar-btn-item';
      if (key.type === 'modifier' || key.btn_type === 'modifier') {
        item.classList.add('modifier');
      }
      if (key.type === 'action' || key.btn_type === 'action') {
        item.classList.add('action');
      }
      item.setAttribute('draggable', 'true');
      item.dataset.index = idx;

      const labelSpan = document.createElement('span');
      labelSpan.textContent = key.label;
      item.appendChild(labelSpan);

      const removeBtn = document.createElement('button');
      removeBtn.className = 'keybar-btn-remove';
      removeBtn.textContent = '\u00d7';
      removeBtn.type = 'button';
      removeBtn.setAttribute('data-tooltip', 'Remove');
      removeBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        editingKeybarButtons.splice(idx, 1);
        renderKeybarList();
      });
      item.appendChild(removeBtn);

      // Desktop drag & drop
      item.addEventListener('dragstart', (e) => {
        e.dataTransfer.effectAllowed = 'move';
        e.dataTransfer.setData('text/plain', String(idx));
        item.classList.add('dragging');
      });
      item.addEventListener('dragend', () => {
        item.classList.remove('dragging');
        list.querySelectorAll('.drag-over').forEach(el => el.classList.remove('drag-over'));
      });
      item.addEventListener('dragover', (e) => {
        e.preventDefault();
        e.dataTransfer.dropEffect = 'move';
        item.classList.add('drag-over');
      });
      item.addEventListener('dragleave', () => {
        item.classList.remove('drag-over');
      });
      item.addEventListener('drop', (e) => {
        e.preventDefault();
        item.classList.remove('drag-over');
        const fromIdx = parseInt(e.dataTransfer.getData('text/plain'), 10);
        const toIdx = idx;
        if (fromIdx !== toIdx) {
          const moved = editingKeybarButtons.splice(fromIdx, 1)[0];
          editingKeybarButtons.splice(toIdx, 0, moved);
          renderKeybarList();
        }
      });

      // Touch drag & drop
      let touchStartIdx = null;
      let touchClone = null;
      let touchCurrentOverIdx = null;

      item.addEventListener('touchstart', (e) => {
        if (e.target.classList.contains('keybar-btn-remove')) return;
        touchStartIdx = idx;
        const touch = e.touches[0];
        // Long press to initiate drag
        item._touchTimer = setTimeout(() => {
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

      item.addEventListener('touchmove', (e) => {
        if (!touchClone && !item._touchTimer) return;
        if (!touchClone) return; // timer hasn't fired yet
        e.preventDefault();
        const touch = e.touches[0];
        touchClone.style.left = (touch.clientX - 20) + 'px';
        touchClone.style.top = (touch.clientY - 20) + 'px';

        // Find element under touch
        const overEl = document.elementFromPoint(touch.clientX, touch.clientY);
        const overItem = overEl ? overEl.closest('.keybar-btn-item') : null;
        list.querySelectorAll('.drag-over').forEach(el => el.classList.remove('drag-over'));
        if (overItem && overItem.dataset.index !== undefined) {
          overItem.classList.add('drag-over');
          touchCurrentOverIdx = parseInt(overItem.dataset.index, 10);
        } else {
          touchCurrentOverIdx = null;
        }
      }, { passive: false });

      item.addEventListener('touchend', () => {
        clearTimeout(item._touchTimer);
        item.classList.remove('dragging');
        list.querySelectorAll('.drag-over').forEach(el => el.classList.remove('drag-over'));
        if (touchClone) {
          touchClone.remove();
          touchClone = null;
          if (touchCurrentOverIdx !== null && touchStartIdx !== touchCurrentOverIdx) {
            const moved = editingKeybarButtons.splice(touchStartIdx, 1)[0];
            editingKeybarButtons.splice(touchCurrentOverIdx, 0, moved);
            renderKeybarList();
          }
        }
        touchStartIdx = null;
        touchCurrentOverIdx = null;
      });

      item.addEventListener('touchcancel', () => {
        clearTimeout(item._touchTimer);
        item.classList.remove('dragging');
        list.querySelectorAll('.drag-over').forEach(el => el.classList.remove('drag-over'));
        if (touchClone) { touchClone.remove(); touchClone = null; }
        touchStartIdx = null;
        touchCurrentOverIdx = null;
      });

      list.appendChild(item);
    });
  }

  function getEditingButtons() {
    // 保存用: send 内のリテラル文字はそのまま保持
    return editingKeybarButtons.map(k => ({ ...k }));
  }

  function openModal() {
    const modal = document.getElementById('settings-modal');
    document.getElementById('setting-font-size').value = current.font_size;
    document.getElementById('setting-scrollback').value = current.terminal_scrollback;
    const themeSelect = document.getElementById('setting-theme');
    if (themeSelect) themeSelect.value = current.theme || 'dark';

    const agentFwdCheck = document.getElementById('setting-ssh-agent-fwd');
    if (agentFwdCheck) agentFwdCheck.checked = !!current.ssh_agent_forwarding;

    // キーバー設定の初期化
    if (current.keybar_buttons && current.keybar_buttons.length > 0) {
      editingKeybarButtons = current.keybar_buttons.map(k => ({ ...k }));
    } else {
      editingKeybarButtons = Keybar.getDefaultKeys();
    }
    renderKeybarList();
    setupAddForm();

    // Add form を閉じた状態にリセット
    const addForm = document.getElementById('keybar-add-form');
    if (addForm) addForm.hidden = true;

    modal.hidden = false;
  }

  function setupAddForm() {
    const presetSelect = document.getElementById('keybar-preset-select');
    if (!presetSelect) return;
    // プリセット option を生成（初回のみ）
    if (presetSelect.options.length <= 1) {
      KEY_PRESETS.forEach(p => {
        const opt = document.createElement('option');
        opt.value = p.type === 'action' ? '__action:' + p.action : p.send;
        opt.textContent = p.display;
        opt.dataset.label = p.label;
        if (p.type) opt.dataset.btnType = p.type;
        if (p.action) opt.dataset.btnAction = p.action;
        presetSelect.appendChild(opt);
      });
    }
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

      // キーバーボタン: 保存用に send をリテラルに変換
      const keybarButtons = getEditingButtons();

      const agentFwdCheck = document.getElementById('setting-ssh-agent-fwd');
      const sshAgentFwd = agentFwdCheck ? agentFwdCheck.checked : false;

      const ok = await save({
        font_size: Math.max(8, Math.min(32, fontSize)),
        terminal_scrollback: Math.max(100, Math.min(50000, scrollback)),
        theme: theme,
        keybar_buttons: keybarButtons,
        ssh_agent_forwarding: sshAgentFwd,
      });
      if (!ok) return;
      apply();

      // キーバーを即時反映
      Keybar.reload(keybarButtons);

      closeModal();
    });

    const modal = document.getElementById('settings-modal');
    if (modal) modal.addEventListener('click', (e) => {
      if (e.target === modal) closeModal();
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

    if (addBtn) addBtn.addEventListener('click', () => {
      addForm.hidden = false;
      newLabelInput.value = '';
      newSendInput.value = '';
      presetSelect.value = '';
      newModifierCheck.checked = false;
      newModKeySelect.hidden = true;
      newLabelInput.focus();
    });

    if (resetBtn) resetBtn.addEventListener('click', () => {
      editingKeybarButtons = Keybar.getDefaultKeys();
      renderKeybarList();
    });

    if (presetSelect) presetSelect.addEventListener('change', () => {
      const val = presetSelect.value;
      if (val) {
        const opt = presetSelect.selectedOptions[0];
        newLabelInput.value = opt.dataset.label || '';
        newSendInput.value = val;
      } else {
        newSendInput.value = '';
      }
    });

    if (newModifierCheck) newModifierCheck.addEventListener('change', () => {
      newModKeySelect.hidden = !newModifierCheck.checked;
    });

    if (addConfirm) addConfirm.addEventListener('click', () => {
      const label = newLabelInput.value.trim();
      if (!label) {
        newLabelInput.focus();
        return;
      }

      // アクションプリセット（Copy/Paste）
      const selectedOpt = presetSelect.selectedOptions[0];
      if (selectedOpt && selectedOpt.dataset.btnType === 'action') {
        editingKeybarButtons.push({
          label,
          send: '',
          type: 'action',
          action: selectedOpt.dataset.btnAction,
        });
        renderKeybarList();
        addForm.hidden = true;
        return;
      }

      if (newModifierCheck.checked) {
        editingKeybarButtons.push({
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
        const sendValue = unescapeSend(sendRaw);
        editingKeybarButtons.push({
          label,
          send: sendValue,
        });
      }

      renderKeybarList();
      addForm.hidden = true;
    });

    if (addCancel) addCancel.addEventListener('click', () => {
      addForm.hidden = true;
    });
  }

  return { load, save, apply, get, getAll, bindUI, openModal };
})();
