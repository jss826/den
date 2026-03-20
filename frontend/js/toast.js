// Den - Toast 通知 & 確認ダイアログ & プロンプトダイアログ
// eslint-disable-next-line no-unused-vars
const Toast = (() => {
  let container = null;
  let confirmModal = null;
  let promptModal = null;
  let chooseModal = null;

  /** フォーカストラップ: Tab/Shift+Tab でダイアログ内要素を循環 */
  function trapFocus(modalEl, origHandler) {
    return function(e) {
      if (e.key === 'Tab') {
        const focusable = modalEl.querySelectorAll('button, input, textarea, select');
        if (focusable.length === 0) return;
        const first = focusable[0], last = focusable[focusable.length - 1];
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault(); last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault(); first.focus();
        }
        return;
      }
      origHandler(e);
    };
  }

  function ensureInit() {
    if (container) return;

    // Toast container
    container = document.createElement('div');
    container.id = 'toast-container';
    container.setAttribute('aria-live', 'polite');
    container.setAttribute('role', 'status');
    document.body.appendChild(container);

    // Confirm dialog
    confirmModal = document.createElement('div');
    confirmModal.id = 'confirm-modal';
    confirmModal.className = 'modal';
    confirmModal.hidden = true;
    confirmModal.setAttribute('role', 'dialog');
    confirmModal.setAttribute('aria-modal', 'true');
    confirmModal.setAttribute('aria-label', 'Confirm action');
    confirmModal.innerHTML =
      '<div class="modal-content confirm-dialog">' +
        '<p id="confirm-message"></p>' +
        '<div class="modal-actions">' +
          '<button id="confirm-cancel" class="modal-btn">Cancel</button>' +
          '<button id="confirm-ok" class="modal-btn primary">OK</button>' +
        '</div>' +
      '</div>';
    document.body.appendChild(confirmModal);

    // Prompt dialog
    promptModal = document.createElement('div');
    promptModal.id = 'prompt-modal';
    promptModal.className = 'modal';
    promptModal.hidden = true;
    promptModal.setAttribute('role', 'dialog');
    promptModal.setAttribute('aria-modal', 'true');
    promptModal.setAttribute('aria-label', 'Input');
    promptModal.innerHTML =
      '<div class="modal-content prompt-dialog">' +
        '<p id="prompt-message"></p>' +
        '<input type="text" id="prompt-input" class="settings-input" />' +
        '<div class="modal-actions">' +
          '<button id="prompt-cancel" class="modal-btn">Cancel</button>' +
          '<button id="prompt-ok" class="modal-btn primary">OK</button>' +
        '</div>' +
      '</div>';
    document.body.appendChild(promptModal);

    // Choose dialog (3-button: e.g. Save / Discard / Cancel)
    chooseModal = document.createElement('div');
    chooseModal.id = 'choose-modal';
    chooseModal.className = 'modal';
    chooseModal.hidden = true;
    chooseModal.setAttribute('role', 'dialog');
    chooseModal.setAttribute('aria-modal', 'true');
    chooseModal.setAttribute('aria-label', 'Choose action');
    chooseModal.innerHTML =
      '<div class="modal-content confirm-dialog">' +
        '<p id="choose-message"></p>' +
        '<div class="modal-actions">' +
          '<button id="choose-cancel" class="modal-btn">Cancel</button>' +
          '<button id="choose-secondary" class="modal-btn">Discard</button>' +
          '<button id="choose-primary" class="modal-btn primary">Save</button>' +
        '</div>' +
      '</div>';
    document.body.appendChild(chooseModal);
  }

  function show(message, type = 'info', duration = 3000, opts) {
    ensureInit();

    const toast = document.createElement('div');
    toast.className = 'toast toast-' + type;
    toast.textContent = message;
    if (opts && opts.onClick) {
      toast.style.cursor = 'pointer';
      toast.addEventListener('click', opts.onClick, { once: true });
    }
    container.appendChild(toast);

    // Trigger slide-in
    requestAnimationFrame(() => {
      requestAnimationFrame(() => toast.classList.add('show'));
    });

    // Auto dismiss
    setTimeout(() => {
      toast.classList.remove('show');
      let removed = false;
      const onEnd = () => { if (removed) return; removed = true; toast.remove(); };
      toast.addEventListener('transitionend', onEnd, { once: true });
      // Fallback removal if transitionend doesn't fire
      setTimeout(onEnd, 400);
    }, duration);
  }

  function success(message) { show(message, 'success', 3000); }
  function error(message)   { show(message, 'error', 4000); }
  function info(message)    { show(message, 'info', 3000); }
  function warn(message)    { show(message, 'warn', 3500); }

  /**
   * Custom confirm dialog — returns Promise<boolean>
   */
  function confirm(message) {
    ensureInit();
    return new Promise((resolve) => {
      const msgEl = confirmModal.querySelector('#confirm-message');
      msgEl.textContent = message;
      confirmModal.hidden = false;

      const okBtn = confirmModal.querySelector('#confirm-ok');
      const cancelBtn = confirmModal.querySelector('#confirm-cancel');

      function cleanup(result) {
        confirmModal.hidden = true;
        okBtn.removeEventListener('click', onOk);
        cancelBtn.removeEventListener('click', onCancel);
        confirmModal.removeEventListener('click', onBackdrop);
        document.removeEventListener('keydown', onKey);
        resolve(result);
      }

      function onOk() { cleanup(true); }
      function onCancel() { cleanup(false); }
      function onBackdrop(e) { if (e.target === confirmModal) cleanup(false); }
      function onKeyBase(e) {
        if (e.key === 'Escape') cleanup(false);
        if (e.key === 'Enter') cleanup(true);
      }
      const onKey = trapFocus(confirmModal, onKeyBase);

      okBtn.addEventListener('click', onOk);
      cancelBtn.addEventListener('click', onCancel);
      confirmModal.addEventListener('click', onBackdrop);
      document.addEventListener('keydown', onKey);

      okBtn.focus();
    });
  }

  /**
   * Custom prompt dialog — returns Promise<string|null> (Cancel=null)
   */
  function prompt(message, defaultValue) {
    ensureInit();
    return new Promise((resolve) => {
      const msgEl = promptModal.querySelector('#prompt-message');
      msgEl.textContent = message;
      const input = promptModal.querySelector('#prompt-input');
      input.value = defaultValue || '';
      promptModal.hidden = false;

      const okBtn = promptModal.querySelector('#prompt-ok');
      const cancelBtn = promptModal.querySelector('#prompt-cancel');

      function cleanup(result) {
        promptModal.hidden = true;
        okBtn.removeEventListener('click', onOk);
        cancelBtn.removeEventListener('click', onCancel);
        promptModal.removeEventListener('click', onBackdrop);
        document.removeEventListener('keydown', onKey);
        resolve(result);
      }

      function onOk() { cleanup(input.value); }
      function onCancel() { cleanup(null); }
      function onBackdrop(e) { if (e.target === promptModal) cleanup(null); }
      function onKeyBase(e) {
        if (e.key === 'Escape') cleanup(null);
        if (e.key === 'Enter') cleanup(input.value);
      }
      const onKey = trapFocus(promptModal, onKeyBase);

      okBtn.addEventListener('click', onOk);
      cancelBtn.addEventListener('click', onCancel);
      promptModal.addEventListener('click', onBackdrop);
      document.addEventListener('keydown', onKey);

      input.focus();
      input.select();
    });
  }

  /**
   * 3-choice dialog — returns Promise<'primary'|'secondary'|null>
   * primary = first action (e.g. Save), secondary = second action (e.g. Discard), null = Cancel
   */
  function choose(message, { primary = 'Save', secondary = 'Discard', cancel = 'Cancel' } = {}) {
    ensureInit();
    return new Promise((resolve) => {
      chooseModal.querySelector('#choose-message').textContent = message;
      chooseModal.querySelector('#choose-primary').textContent = primary;
      chooseModal.querySelector('#choose-secondary').textContent = secondary;
      chooseModal.querySelector('#choose-cancel').textContent = cancel;
      chooseModal.hidden = false;

      const primaryBtn = chooseModal.querySelector('#choose-primary');
      const secondaryBtn = chooseModal.querySelector('#choose-secondary');
      const cancelBtn = chooseModal.querySelector('#choose-cancel');

      function cleanup(result) {
        chooseModal.hidden = true;
        primaryBtn.removeEventListener('click', onPrimary);
        secondaryBtn.removeEventListener('click', onSecondary);
        cancelBtn.removeEventListener('click', onCancel);
        chooseModal.removeEventListener('click', onBackdrop);
        document.removeEventListener('keydown', onKey);
        resolve(result);
      }

      function onPrimary() { cleanup('primary'); }
      function onSecondary() { cleanup('secondary'); }
      function onCancel() { cleanup(null); }
      function onBackdrop(e) { if (e.target === chooseModal) cleanup(null); }
      function onKeyBase(e) {
        if (e.key === 'Escape') cleanup(null);
      }
      const onKey = trapFocus(chooseModal, onKeyBase);

      primaryBtn.addEventListener('click', onPrimary);
      secondaryBtn.addEventListener('click', onSecondary);
      cancelBtn.addEventListener('click', onCancel);
      chooseModal.addEventListener('click', onBackdrop);
      document.addEventListener('keydown', onKey);

      primaryBtn.focus();
    });
  }

  return { show, success, error, info, warn, confirm, prompt, choose };
})();
