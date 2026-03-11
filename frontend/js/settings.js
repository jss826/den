/* global DenDragList, DenKeyPresets, Keybar, DenTerminal, FloatTerminal, DenSnippet, Toast */
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

  // unescapeSend は keybar.js の executeNormalKey 内で実行時に適用される。
  // 設定保存時にはエスケープ形式のまま保持する。

  /**
   * サーバーから設定を読み込み、current に格納する。
   * @returns {Promise<Object>} 読み込んだ設定オブジェクト
   */
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
  let titleCtx = { tab: 'terminal', session: 'default', oscDisplay: '', remoteHost: '' };

  function isWindowsPath(s) {
    return /^[A-Za-z]:[/\\]/.test(s);
  }

  function parseOscTitle(oscTitle) {
    if (!oscTitle || isWindowsPath(oscTitle)) return { display: '', remoteHost: '' };
    const hostMatch = oscTitle.match(/@([^:\s/\\]+)/);
    return { display: oscTitle, remoteHost: hostMatch ? hostMatch[1] : '' };
  }

  function updateDocumentTitle() {
    const serverHost = current.hostname || '';
    const remote = titleCtx.remoteHost;
    const showRemote = remote && remote !== serverHost;
    const hostPart = showRemote ? `${remote} via ${serverHost}` : serverHost;
    const base = hostPart ? `Den @ ${hostPart}` : 'Den';
    const parts = [];
    if (titleCtx.tab === 'filer') {
      parts.push('Files');
    } else {
      if (titleCtx.oscDisplay) parts.push(titleCtx.oscDisplay);
      if (titleCtx.session) parts.push(titleCtx.session);
    }
    parts.push(base);
    document.title = parts.join(' - ');
  }

  function setTitleTab(tab, session) {
    titleCtx.tab = tab;
    if (session != null) titleCtx.session = session;
    updateDocumentTitle();
  }

  function setOscTitle(title) {
    const parsed = parseOscTitle(title);
    titleCtx.oscDisplay = parsed.display;
    titleCtx.remoteHost = parsed.remoteHost;
    updateDocumentTitle();
  }

  /**
   * 現在の設定をDOMに反映する（フォントサイズ・テーマ）。
   */
  function apply() {
    document.documentElement.style.setProperty('--den-font-size', current.font_size + 'px');
    applyTheme();
    updateDocumentTitle();
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

  /**
   * 指定キーの設定値を取得する。
   * @param {string} key - 設定キー名
   * @returns {*} 設定値
   */
  function get(key) {
    return current[key];
  }

  /**
   * 全設定のシャローコピーを返す。
   * @returns {Object} 設定オブジェクトのコピー
   */
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
      if (!key) return;
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

    // Delegate drag & drop via shared module
    DenDragList.init(list, {
      itemSelector: '.keybar-btn-item',
      removeSelector: '.keybar-btn-remove',
      getState: () => keybarBtnListState[listId],
    });
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

    // Delegate drag & drop via shared module
    DenDragList.init(list, {
      itemSelector: '.snippet-item',
      removeSelector: '.snippet-item-remove',
      getState: () => ({ array: editingSnippets, render: renderSnippetList }),
    });
  }

  /**
   * 設定モーダルを開く。現在の設定値をフォームに反映する。
   */
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

    DenKeyPresets.setupAddForm(
      document.getElementById('keybar-preset-select'),
      document.getElementById('keybar-stack-preset'),
    );

    // Add form を閉じた状態にリセット
    addTarget = 'primary';
    const addForm = document.getElementById('keybar-add-form');
    if (addForm) addForm.hidden = true;

    // スニペット設定の初期化
    editingSnippets = current.snippets ? current.snippets.map(s => ({ ...s })) : [];
    renderSnippetList();
    const snippetAddForm = document.getElementById('snippet-add-form');
    if (snippetAddForm) snippetAddForm.hidden = true;

    // Peers settings
    const peerNameInput = document.getElementById('setting-peer-name');
    if (peerNameInput) peerNameInput.value = current.peer_name || '';
    const peerInviteDisplay = document.getElementById('peer-invite-display');
    if (peerInviteDisplay) peerInviteDisplay.hidden = true;
    const peerJoinForm = document.getElementById('peer-join-form');
    if (peerJoinForm) peerJoinForm.hidden = true;
    latestVersion = null; // refetch on each modal open
    loadPeerList();

    const verText = document.getElementById('settings-version-text');
    if (verText && current.version) verText.textContent = 'Den v' + current.version;
    // Reset update UI state
    const updateStatus = document.getElementById('update-status');
    const updateApplyBtn = document.getElementById('update-apply-btn');
    if (updateStatus) { updateStatus.hidden = true; updateStatus.textContent = ''; }
    if (updateApplyBtn) updateApplyBtn.hidden = true;

    modal.hidden = false;
  }

  // スタックビルダーの一時アイテム配列
  let editingStackItems = [];

  function renderStackItemsUI() {
    DenKeyPresets.renderStackItems(
      document.getElementById('keybar-stack-items'),
      editingStackItems,
      (idx) => { editingStackItems.splice(idx, 1); renderStackItemsUI(); },
    );
  }

  function closeModal() {
    document.getElementById('settings-modal').hidden = true;
  }

  /**
   * 設定 UI のイベントリスナーを全てバインドする。
   * DOMContentLoaded 後に一度だけ呼び出す。
   */
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

      const peerNameEl = document.getElementById('setting-peer-name');
      const peerName = peerNameEl ? (peerNameEl.value.trim() || null) : null;

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
        peer_name: peerName,
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
      renderStackItemsUI();
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
      renderStackItemsUI();
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

    // --- Update ---
    const updateCheckBtn = document.getElementById('update-check-btn');
    const updateApplyBtn = document.getElementById('update-apply-btn');
    const updateStatus = document.getElementById('update-status');

    if (updateCheckBtn) updateCheckBtn.addEventListener('click', async () => {
      updateCheckBtn.disabled = true;
      updateCheckBtn.textContent = 'Checking...';
      updateStatus.hidden = true;
      updateApplyBtn.hidden = true;
      try {
        const resp = await fetch('/api/system/version', { credentials: 'same-origin' });
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        const info = await resp.json();
        if (info.update_available && info.latest) {
          updateStatus.textContent = 'v' + info.latest + ' available';
          updateStatus.hidden = false;
          updateStatus.className = 'update-status update-available';
          updateApplyBtn.hidden = false;
        } else if (info.latest) {
          updateStatus.textContent = 'Up to date';
          updateStatus.hidden = false;
          updateStatus.className = 'update-status update-current';
        } else {
          updateStatus.textContent = 'Could not check';
          updateStatus.hidden = false;
          updateStatus.className = 'update-status update-error';
        }
      } catch (e) {
        updateStatus.textContent = 'Check failed';
        updateStatus.hidden = false;
        updateStatus.className = 'update-status update-error';
        console.warn('Update check failed:', e);
      } finally {
        updateCheckBtn.disabled = false;
        updateCheckBtn.textContent = 'Check for Updates';
      }
    });

    if (updateApplyBtn) updateApplyBtn.addEventListener('click', async () => {
      const ok = await Toast.confirm('Download and install update? Den will restart.');
      if (!ok) return;
      updateApplyBtn.disabled = true;
      updateApplyBtn.textContent = 'Updating...';
      updateStatus.textContent = 'Downloading...';
      updateStatus.className = 'update-status';
      try {
        const resp = await fetch('/api/system/update', {
          method: 'POST',
          credentials: 'same-origin',
        });
        if (!resp.ok) {
          const body = await resp.json().catch(() => ({}));
          throw new Error(body.error || `HTTP ${resp.status}`);
        }
        updateStatus.textContent = 'Restarting...';
        // Server will restart; wait for reconnection
        setTimeout(() => { location.reload(); }, 3000);
      } catch (e) {
        updateStatus.textContent = 'Update failed: ' + e.message;
        updateStatus.className = 'update-status update-error';
        updateApplyBtn.disabled = false;
        updateApplyBtn.textContent = 'Update Now';
        console.warn('Update failed:', e);
      }
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

    // --- Peer management ---
    bindPeerUI();
  }

  // --- Peer management functions ---

  let peerInviteTimer = null;
  let latestVersion = null;

  async function fetchLatestVersion() {
    try {
      const resp = await fetch('/api/system/version', { credentials: 'same-origin' });
      if (!resp.ok) return null;
      const data = await resp.json();
      return data.latest || null;
    } catch (e) {
      return null;
    }
  }

  function isOlderVersion(peerVersion, latest) {
    if (!peerVersion || !latest) return false;
    const parse = v => v.replace(/^v/, '').split('-')[0].split('.').map(Number);
    const p = parse(peerVersion);
    const l = parse(latest);
    for (let i = 0; i < Math.max(p.length, l.length); i++) {
      const pv = p[i] || 0;
      const lv = l[i] || 0;
      if (pv < lv) return true;
      if (pv > lv) return false;
    }
    return false;
  }

  let peerUpdateInProgress = false;

  async function waitForPeerRestart(peerName, maxWaitMs) {
    const deadline = Date.now() + (maxWaitMs || 30000);
    let delay = 2000;
    while (Date.now() < deadline) {
      await new Promise(r => setTimeout(r, delay));
      try {
        const r = await fetch(`/api/peers/${encodeURIComponent(peerName)}/system/version`, {
          credentials: 'same-origin',
        });
        if (r.ok) return true;
      } catch (e) { /* peer still restarting */ }
      delay = Math.min(delay * 1.5, 5000);
    }
    return false;
  }

  async function updatePeer(peerName, btn) {
    if (peerUpdateInProgress) return;
    peerUpdateInProgress = true;
    if (btn) {
      btn.disabled = true;
      btn.textContent = 'Updating...';
    }
    try {
      const resp = await fetch(`/api/peers/${encodeURIComponent(peerName)}/system/update`, {
        method: 'POST',
        credentials: 'same-origin',
      });
      if (!resp.ok) {
        const text = await resp.text().catch(() => '');
        throw new Error(text || `HTTP ${resp.status}`);
      }
      if (btn) btn.textContent = 'Restarting...';
      const ok = await waitForPeerRestart(peerName, 30000);
      if (ok) {
        Toast.success(`${peerName} updated successfully`);
      } else {
        Toast.info(`${peerName} update sent — peer still restarting`);
      }
      loadPeerList();
    } catch (e) {
      Toast.error(`Failed to update ${peerName}: ${e.message}`);
      if (btn) {
        btn.disabled = false;
        btn.textContent = 'Update';
      }
    } finally {
      peerUpdateInProgress = false;
    }
  }

  async function loadPeerList() {
    const list = document.getElementById('peer-list');
    if (!list) return;
    list.innerHTML = '';

    // Fetch latest version once per load
    if (latestVersion == null) {
      latestVersion = await fetchLatestVersion();
    }

    try {
      const resp = await fetch('/api/peers', { credentials: 'same-origin' });
      if (!resp.ok) return;
      const peers = await resp.json();
      if (peers.length === 0) {
        list.innerHTML = '<div class="peer-empty">No peers registered</div>';
        const btn = document.getElementById('peer-update-all-btn');
        if (btn) btn.hidden = true;
        return;
      }

      let outdatedCount = 0;
      for (const peer of peers) {
        const row = document.createElement('div');
        row.className = 'peer-row';
        const statusClass = peer.status === 'connected' ? 'peer-status-connected'
          : peer.status === 'connecting' ? 'peer-status-connecting'
          : 'peer-status-disconnected';
        const statusLabel = peer.status === 'connected' ? 'Connected'
          : peer.status === 'connecting' ? 'Connecting'
          : 'Disconnected';
        const versionText = peer.version ? ` v${escHtml(peer.version)}` : '';
        const latencyText = peer.latency_ms != null ? ` ${peer.latency_ms}ms` : '';
        const outdated = peer.status === 'connected' && isOlderVersion(peer.version, latestVersion);
        if (outdated) outdatedCount++;

        let updateHtml = '';
        if (peer.status === 'connected' && peer.version) {
          if (outdated) {
            updateHtml = `<button class="peer-update-btn modal-btn primary" data-peer="${escHtml(peer.name)}">Update</button>`;
          } else {
            updateHtml = '<span class="peer-uptodate">Up to date</span>';
          }
        }

        const scopeLabel = peer.scope === 'readonly' ? 'Read' : 'Admin';
        const scopeClass = peer.scope === 'readonly' ? 'peer-scope-readonly' : 'peer-scope-admin';
        const scopeTitle = peer.scope === 'readonly' ? 'Read-only access (click to change)' : 'Full access (click to change)';

        row.innerHTML = `
          <span class="peer-status ${statusClass}" title="${statusLabel}"></span>
          <span class="peer-info">
            <strong>${escHtml(peer.name)}</strong>
            <small>${escHtml(peer.url)}${versionText}${latencyText}</small>
          </span>
          <button class="peer-scope-btn ${scopeClass}" data-peer="${escHtml(peer.name)}" data-scope="${peer.scope}" title="${scopeTitle}">${scopeLabel}</button>
          ${updateHtml}
          <button class="peer-delete-btn modal-btn" data-peer="${escHtml(peer.name)}" title="Remove peer">×</button>
        `;
        list.appendChild(row);
      }

      // Update All button visibility
      const updateAllBtn = document.getElementById('peer-update-all-btn');
      if (updateAllBtn) {
        updateAllBtn.hidden = outdatedCount === 0;
      }

      // Bind scope toggle buttons
      list.querySelectorAll('.peer-scope-btn').forEach(btn => {
        btn.addEventListener('click', async () => {
          const name = btn.dataset.peer;
          const newScope = btn.dataset.scope === 'admin' ? 'readonly' : 'admin';
          try {
            const resp = await fetch(`/api/peers/${encodeURIComponent(name)}/scope`, {
              method: 'PUT',
              credentials: 'same-origin',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({ scope: newScope }),
            });
            if (resp.ok) loadPeerList();
            else Toast.error('Failed to update scope');
          } catch (e) {
            Toast.error('Failed to update scope');
          }
        });
      });

      // Bind update buttons
      list.querySelectorAll('.peer-update-btn').forEach(btn => {
        btn.addEventListener('click', () => updatePeer(btn.dataset.peer, btn));
      });

      list.querySelectorAll('.peer-delete-btn').forEach(btn => {
        btn.addEventListener('click', async () => {
          const name = btn.dataset.peer;
          if (!confirm(`Remove peer "${name}"?`)) return;
          try {
            await fetch(`/api/peers/${encodeURIComponent(name)}`, {
              method: 'DELETE',
              credentials: 'same-origin',
            });
            loadPeerList();
          } catch (e) {
            Toast.error('Failed to remove peer');
          }
        });
      });
    } catch (e) {
      list.innerHTML = '<div class="peer-empty">Failed to load peers</div>';
    }
  }

  function escHtml(s) {
    const d = document.createElement('div');
    d.textContent = s;
    return d.innerHTML;
  }

  function bindPeerUI() {
    const inviteBtn = document.getElementById('peer-invite-btn');
    const joinBtn = document.getElementById('peer-join-btn');
    const joinForm = document.getElementById('peer-join-form');
    const joinConfirm = document.getElementById('peer-join-confirm');
    const joinCancel = document.getElementById('peer-join-cancel');
    const inviteCopy = document.getElementById('peer-invite-copy');

    if (inviteBtn) inviteBtn.addEventListener('click', async () => {
      try {
        const resp = await fetch('/api/peers/invite', {
          method: 'POST',
          credentials: 'same-origin',
        });
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        const data = await resp.json();
        const display = document.getElementById('peer-invite-display');
        const codeEl = document.getElementById('peer-invite-code');
        const ttlEl = document.getElementById('peer-invite-ttl');
        if (display && codeEl && ttlEl) {
          codeEl.textContent = data.code;
          display.hidden = false;
          // TTL countdown
          if (peerInviteTimer) clearInterval(peerInviteTimer);
          let remaining = data.expires_in_secs;
          ttlEl.textContent = `${remaining}s`;
          peerInviteTimer = setInterval(() => {
            remaining--;
            if (remaining <= 0) {
              clearInterval(peerInviteTimer);
              peerInviteTimer = null;
              display.hidden = true;
            } else {
              ttlEl.textContent = `${remaining}s`;
            }
          }, 1000);
        }
      } catch (e) {
        Toast.error('Failed to generate invite code');
      }
    });

    if (inviteCopy) inviteCopy.addEventListener('click', () => {
      const code = document.getElementById('peer-invite-code');
      if (code) navigator.clipboard.writeText(code.textContent);
    });

    if (joinBtn) joinBtn.addEventListener('click', () => {
      if (joinForm) {
        joinForm.hidden = !joinForm.hidden;
        if (!joinForm.hidden) {
          document.getElementById('peer-join-url').value = '';
          document.getElementById('peer-join-code').value = '';
          document.getElementById('peer-join-url').focus();
        }
      }
    });

    if (joinConfirm) joinConfirm.addEventListener('click', async () => {
      const url = document.getElementById('peer-join-url').value.trim();
      const code = document.getElementById('peer-join-code').value.trim();
      if (!url || !code) return;
      joinConfirm.disabled = true;
      try {
        const resp = await fetch('/api/peers/join', {
          method: 'POST',
          credentials: 'same-origin',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ code, peer_url: url }),
        });
        if (!resp.ok) {
          const status = resp.status;
          if (status === 403) Toast.error('Invalid or expired invite code');
          else if (status === 502) Toast.error('Could not connect to peer');
          else Toast.error(`Failed to join: HTTP ${status}`);
          return;
        }
        const data = await resp.json();
        Toast.success(`Paired with ${data.peer_name}`);
        joinForm.hidden = true;
        loadPeerList();
      } catch (e) {
        Toast.error('Failed to join peer');
      } finally {
        joinConfirm.disabled = false;
      }
    });

    if (joinCancel) joinCancel.addEventListener('click', () => {
      if (joinForm) joinForm.hidden = true;
    });

    const updateAllBtn = document.getElementById('peer-update-all-btn');
    if (updateAllBtn) updateAllBtn.addEventListener('click', async () => {
      if (peerUpdateInProgress) return;
      if (!confirm('Update all outdated peers?')) return;
      peerUpdateInProgress = true;
      updateAllBtn.disabled = true;
      updateAllBtn.textContent = 'Updating...';
      try {
        const resp = await fetch('/api/peers', { credentials: 'same-origin' });
        if (!resp.ok) throw new Error('Failed to fetch peers');
        const peers = await resp.json();
        const outdated = peers.filter(p =>
          p.status === 'connected' && isOlderVersion(p.version, latestVersion)
        );
        for (const peer of outdated) {
          updateAllBtn.textContent = `Updating ${peer.name}...`;
          try {
            const r = await fetch(`/api/peers/${encodeURIComponent(peer.name)}/system/update`, {
              method: 'POST',
              credentials: 'same-origin',
            });
            if (!r.ok) throw new Error(`HTTP ${r.status}`);
            const ok = await waitForPeerRestart(peer.name, 30000);
            if (ok) {
              Toast.success(`${peer.name} updated`);
            } else {
              Toast.info(`${peer.name} update sent — peer still restarting`);
            }
          } catch (e) {
            Toast.error(`Failed to update ${peer.name}`);
          }
        }
        const localResp = await fetch('/api/system/version', { credentials: 'same-origin' });
        if (localResp.ok) {
          const ver = await localResp.json();
          if (ver.update_available) {
            Toast.info('Local update available — use "Update Now" below');
          }
        }
        loadPeerList();
      } catch (e) {
        Toast.error('Update All failed');
      } finally {
        peerUpdateInProgress = false;
        updateAllBtn.disabled = false;
        updateAllBtn.textContent = 'Update All';
      }
    });
  }

  return { load, save, apply, get, getAll, bindUI, openModal, setTitleTab, setOscTitle };
})();
