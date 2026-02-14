// Den - Toast 通知 & 確認ダイアログ
// eslint-disable-next-line no-unused-vars
const Toast = (() => {
  let container = null;
  let confirmModal = null;

  function ensureInit() {
    if (container) return;

    // Toast container
    container = document.createElement('div');
    container.id = 'toast-container';
    document.body.appendChild(container);

    // Confirm dialog
    confirmModal = document.createElement('div');
    confirmModal.id = 'confirm-modal';
    confirmModal.className = 'modal';
    confirmModal.hidden = true;
    confirmModal.innerHTML =
      '<div class="modal-content confirm-dialog">' +
        '<p id="confirm-message"></p>' +
        '<div class="modal-actions">' +
          '<button id="confirm-cancel" class="modal-btn">Cancel</button>' +
          '<button id="confirm-ok" class="modal-btn primary">OK</button>' +
        '</div>' +
      '</div>';
    document.body.appendChild(confirmModal);
  }

  function show(message, type, duration) {
    ensureInit();
    if (!type) type = 'info';
    if (!duration) duration = 3000;

    const toast = document.createElement('div');
    toast.className = 'toast toast-' + type;
    toast.textContent = message;
    container.appendChild(toast);

    // Trigger slide-in
    requestAnimationFrame(() => {
      requestAnimationFrame(() => toast.classList.add('show'));
    });

    // Auto dismiss
    setTimeout(() => {
      toast.classList.remove('show');
      const onEnd = () => toast.remove();
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
      function onKey(e) {
        if (e.key === 'Escape') cleanup(false);
        if (e.key === 'Enter') cleanup(true);
      }

      okBtn.addEventListener('click', onOk);
      cancelBtn.addEventListener('click', onCancel);
      confirmModal.addEventListener('click', onBackdrop);
      document.addEventListener('keydown', onKey);

      okBtn.focus();
    });
  }

  return { show, success, error, info, warn, confirm };
})();
